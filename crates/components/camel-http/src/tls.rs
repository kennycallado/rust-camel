//! TLS backend selection and webpki fallback machinery for camel-http.
//!
//! Extracted verbatim from lib.rs (mission 272, bd rc-on44c): pure move
//! plus crate-root re-exports for the items the rest of the crate
//! references; no behavior or public-surface change.

use std::time::Duration;

use camel_component_api::CamelError;

use crate::HttpConfig;

#[cfg(test)]
thread_local! {
    static BUILD_CLIENT_CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static BUILD_CLIENT_FALLBACKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Test seam arming [`build_client`] to skip the platform-verifier
    /// primary and enter [`webpki_fallback_client`] directly (see
    /// [`FallbackTrigger::Forced`]). Always manipulated through the
    /// panic-safe [`crate::tls_harness::force_webpki_fallback`] guard.
    pub(crate) static FORCE_WEBPKI_FALLBACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Test seam arming the shared second-error terminal in
    /// [`webpki_fallback_client`]: a successful preconfigured-backend
    /// build is then treated as a rebuild failure, exercising the
    /// terminal hermetically (it is unreachable through real triggers
    /// on reqwest 0.13.4). Always manipulated through the panic-safe
    /// [`crate::tls_harness::force_webpki_fallback_rebuild_failure`]
    /// guard.
    pub(crate) static FORCE_FALLBACK_REBUILD_FAIL: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Shared builder assembly for [`build_client`]: everything except the
/// terminal `build()` — proxy, timeouts, pool, redirect policy, DNS-pin
/// override, and the permissive (warn-on-degrade) TLS material loads
/// (rc-ayrwk / audit F2-7).
fn client_builder(
    config: &HttpConfig,
    resolve_override: Option<(&str, &[std::net::SocketAddr])>,
) -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder()
        .no_proxy() // CRITICAL: env proxies bypass resolve_to_addrs
        .connect_timeout(Duration::from_millis(config.connect_timeout_ms))
        .pool_max_idle_per_host(config.pool_max_idle_per_host)
        .pool_idle_timeout(Duration::from_millis(config.pool_idle_timeout_ms));

    // Redirects are always handled manually in the producer's send path
    // so that each hop can be SSRF-validated. reqwest's built-in redirect
    // policy is sync and cannot perform async DNS resolution or SSRF checks.
    builder = builder.redirect(reqwest::redirect::Policy::none());

    if let Some((host, addrs)) = resolve_override {
        builder = builder.resolve_to_addrs(host, addrs);
    }

    if let Some(tls) = &config.tls
        && tls.enabled
    {
        if tls.insecure || !tls.verify_peer {
            // log-policy: handler-owned
            tracing::warn!("HTTP TLS verification disabled — connections are vulnerable to MitM");
            builder = builder.danger_accept_invalid_certs(true);
        }

        if let Some(ca_path) = &tls.ca_cert_path {
            // Audit 2026-08-31, F2-7: a configured CA that fails to load must
            // never degrade silently to system roots. Loud warn (config error
            // class: fail-fast would break existing deployments relying on the
            // fallback; the warning is the operator signal).
            match std::fs::read(ca_path) {
                Ok(ca_bytes) => {
                    // Under the rustls backend `Certificate::from_pem`
                    // never fails (it defers parsing), so the parse-error
                    // warn below is effectively dead and a file with zero
                    // parseable PEM CERTIFICATE sections would silently
                    // contribute no roots. Warn on that case explicitly
                    // (e_glm stage-4 finding 1).
                    let pem_sections = rustls_pemfile::certs(&mut std::io::Cursor::new(&ca_bytes))
                        .filter(|r| r.is_ok())
                        .count();
                    if pem_sections == 0 {
                        // log-policy: handler-owned
                        tracing::warn!(
                            "configured CA certificate contains no parseable PEM CERTIFICATE section — falling back to system roots"
                        );
                    }
                    match reqwest::Certificate::from_pem(&ca_bytes)
                        .or_else(|_| reqwest::Certificate::from_der(&ca_bytes))
                    {
                        Ok(ca_cert) => {
                            builder = builder.add_root_certificate(ca_cert);
                        }
                        Err(e) => {
                            // log-policy: handler-owned
                            tracing::warn!(
                                error = %e,
                                "configured CA certificate failed to parse — falling back to system roots"
                            );
                        }
                    }
                }
                Err(e) => {
                    // log-policy: handler-owned
                    tracing::warn!(
                        error = %e,
                        "configured CA certificate file unreadable — falling back to system roots"
                    );
                }
            }
        }

        // mTLS identity: BOTH files must load and parse, or the identity is
        // absent. A partial failure previously meant silently downgrading to
        // non-mTLS — now loud.
        if let (Some(cert_path), Some(key_path)) = (&tls.client_cert_path, &tls.client_key_path) {
            match (std::fs::read(cert_path), std::fs::read(key_path)) {
                (Ok(cert_bytes), Ok(key_bytes)) => {
                    let mut identity_pem = cert_bytes;
                    identity_pem.extend_from_slice(&key_bytes);
                    match reqwest::Identity::from_pem(&identity_pem) {
                        Ok(identity) => {
                            builder = builder.identity(identity);
                        }
                        Err(e) => {
                            // log-policy: handler-owned
                            tracing::warn!(
                                error = %e,
                                "configured mTLS identity failed to parse — client certificate NOT used"
                            );
                        }
                    }
                }
                (cert_r, key_r) => {
                    // log-policy: handler-owned
                    tracing::warn!(
                        cert_ok = cert_r.is_ok(),
                        key_ok = key_r.is_ok(),
                        "configured mTLS cert/key file unreadable — client certificate NOT used"
                    );
                }
            }
        }
    }

    builder
}

/// Shared start for the fallback client configs (rc-jr2g8): resolve the
/// crypto provider the way reqwest does (async_impl/client.rs) —
/// process-default provider when the host installed one (camel-cli
/// installs ring), else the aws-lc-rs default reqwest's `rustls`
/// feature falls back to — then pin the safe default protocol versions.
/// Stock rustls providers (ring, aws-lc-rs) always support the safe
/// default TLS versions; this choice is static, not input- or
/// platform-dependent.
fn fallback_client_config_builder()
-> rustls::ConfigBuilder<rustls::ClientConfig, rustls::WantsVerifier> {
    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
    rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("stock rustls provider supports the safe default protocol versions") // allow-unwrap
}

/// Bundled-root TLS config for the CA-less-platform fallback (rc-3j4mq):
/// Mozilla's root set from `webpki-roots`, precedent rc-ayy11 (camel-cli
/// redis-tls pure-rust roots).
///
/// Why a preconfigured `rustls::ClientConfig` and not a reqwest root
/// knob: reqwest 0.13 removed `tls_built_in_root_certs` and its
/// webpki-roots feature, `Certificate::from_der` needs full DER
/// certificates while webpki-roots ships pre-parsed `TrustAnchor`s, and
/// `tls_certs_merge` still routes through rustls-platform-verifier
/// (whose native-root probe hard-fails on CA-less platforms such as
/// Android/Termux, rc-3j4mq — apple targets do not fail at build time
/// at all, rc-97uah). `tls_backend_preconfigured` swaps the whole TLS backend; on
/// reqwest 0.13.4 the preconfigured path builds its connector without
/// consulting platform roots, so the empty-CA-store builder error cannot
/// recur there.
fn webpki_root_client_config() -> rustls::ClientConfig {
    fallback_client_config_builder()
        .with_root_certificates(mozilla_only())
        .with_no_client_auth()
    // No ALPN override: the workspace reqwest builds without the http2
    // feature, so the primary path negotiates plain HTTP/1.1. Sending no
    // ALPN extension yields the same HTTP/1.1 outcome here without
    // duplicating reqwest feature knowledge in this crate.
}

/// Root store for the CA-less-platform fallback that UNIONS the bundled
/// Mozilla anchors with any configured custom CA, instead of the
/// primary path's system-roots-plus-CA scheme (which cannot load on a
/// CA-less platform — that is what triggered the fallback). Order
/// contract: accepted custom anchors come FIRST, then the bundled
/// Mozilla anchors — parity with the primary `add_root_certificate`
/// path's extra-roots-before-platform-roots order (rustls-platform-
/// verifier 0.7.0 `src/verification/others.rs:61-93` adds extra roots
/// before native certs). The no-CA path and the non-strict degrade
/// paths (unreadable file, zero PEM sections, zero roots accepted)
/// return Mozilla-only stores. Strict mode fails closed on every CA
/// load/parse/reject failure; non-strict warns and degrades to
/// Mozilla-only.
fn fallback_root_store(
    custom_ca: Option<&str>,
    strict: bool,
) -> Result<rustls::RootCertStore, CamelError> {
    // Start EMPTY, not Mozilla-seeded: the custom anchors must land
    // before the Mozilla tail, so the bundle is appended only on the
    // success path (and served verbatim by `mozilla_only()` elsewhere).
    // A missed early-return rewrite would otherwise hand back an empty
    // store and break every handshake, which the degrade arms below
    // must never do.
    let mut store = rustls::RootCertStore::empty();
    let Some(ca_path) = custom_ca else {
        return Ok(mozilla_only());
    };
    let ca_bytes = match std::fs::read(ca_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            if strict {
                return Err(CamelError::EndpointCreationFailed(format!(
                    "tls.strict/webpki-fallback: configured CA certificate '{ca_path}' is unreadable: {e}"
                )));
            }
            // log-policy: handler-owned
            tracing::warn!(
                error = %e,
                "configured CA certificate file unreadable — falling back to bundled Mozilla roots"
            );
            return Ok(mozilla_only());
        }
    };
    let certs: Vec<_> = rustls_pemfile::certs(&mut std::io::Cursor::new(&ca_bytes))
        .filter_map(|r| r.ok())
        .collect();
    if certs.is_empty() {
        if strict {
            return Err(CamelError::EndpointCreationFailed(format!(
                "tls.strict/webpki-fallback: configured CA certificate '{ca_path}' has no \
                 parseable PEM CERTIFICATE section"
            )));
        }
        // log-policy: handler-owned
        tracing::warn!(
            "configured CA certificate contains no parseable PEM CERTIFICATE section — \
             falling back to bundled Mozilla roots"
        );
        return Ok(mozilla_only());
    }
    let certs_len = certs.len();
    let (added, _ignored) = store.add_parsable_certificates(certs);
    if added == 0 {
        if strict {
            return Err(CamelError::EndpointCreationFailed(format!(
                "tls.strict/webpki-fallback: configured CA certificate '{ca_path}' was \
                 rejected by the TLS root store (0 of {certs_len} accepted)"
            )));
        }
        // log-policy: handler-owned
        tracing::warn!(
            "configured CA certificate was rejected by the TLS root store — \
             falling back to bundled Mozilla roots"
        );
        return Ok(mozilla_only());
    }
    // Custom anchors first, then the bundled Mozilla anchors — the
    // same extra-roots-before-platform-roots order the primary path
    // produces (rustls-platform-verifier 0.7.0
    // `src/verification/others.rs:61-93`). Order does not affect
    // accept/reject (webpki tries anchors until one validates), so
    // this is parity hygiene, not a security boundary.
    store
        .roots
        .extend_from_slice(webpki_roots::TLS_SERVER_ROOTS);
    Ok(store)
}

/// The bundled Mozilla webpki anchors alone — the no-CA root set, the
/// target of every non-strict degrade path in [`fallback_root_store`],
/// and the root store behind [`webpki_root_client_config`].
fn mozilla_only() -> rustls::RootCertStore {
    rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    }
}

/// Server-cert verifier that accepts any certificate without validation
/// — the preconfigured-backend stand-in for reqwest's
/// `danger_accept_invalid_certs(true)` knob (that knob does not exist on
/// the `tls_backend_preconfigured` path). Mirrors reqwest 0.13.4's
/// internal `NoVerifier` exactly, scheme list included.
#[derive(Debug)]
struct NoVerifyServerCertVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifyServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // danger_accept_invalid_certs parity: no validation whatsoever.
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        // Mirrors reqwest 0.13.4 NoVerifier (src/tls.rs): SHA1-legacy and
        // P-521 included, ED448 included.
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA1,
            rustls::SignatureScheme::ECDSA_SHA1_Legacy,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}

/// Full rustls `ClientConfig` for the CA-less-platform fallback when the
/// configured TLS material must be honored: custom CA union store
/// (fallback_root_store), optional no-verify mode (insecure /
/// verify_peer=false), and mTLS client auth. Returns `Ok(None)` when the
/// plain `webpki_root_client_config()` suffices (TLS on, no material,
/// verification on) — callers resolve `config.tls` presence before
/// calling. Strict mode fails closed on every material failure;
/// non-strict warns and degrades per item.
fn fallback_client_config(
    tls: &crate::config::TlsConfig,
) -> Result<Option<rustls::ClientConfig>, CamelError> {
    if !tls.enabled {
        return Ok(None);
    }
    let material = tls.ca_cert_path.is_some()
        || tls.client_cert_path.is_some()
        || tls.client_key_path.is_some();
    let verification_disabled = tls.insecure || !tls.verify_peer;
    if !material && !verification_disabled {
        return Ok(None);
    }
    let builder = fallback_client_config_builder();
    // Computed ALWAYS: strict material validation must happen even when
    // the danger verifier would bypass the roots; `Err` propagates only
    // under strict by construction of `fallback_root_store`.
    let store = fallback_root_store(tls.ca_cert_path.as_deref(), tls.strict)?;
    let builder = if verification_disabled {
        builder
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(NoVerifyServerCertVerifier))
    } else {
        builder.with_root_certificates(store)
    };
    let config = match (&tls.client_cert_path, &tls.client_key_path) {
        (Some(cert_path), Some(key_path)) => {
            match (std::fs::read(cert_path), std::fs::read(key_path)) {
                (Ok(cert_bytes), Ok(key_bytes)) => {
                    let chain: Vec<_> =
                        rustls_pemfile::certs(&mut std::io::Cursor::new(&cert_bytes))
                            .filter_map(|r| r.ok())
                            .collect();
                    if chain.is_empty() {
                        if tls.strict {
                            return Err(CamelError::EndpointCreationFailed(format!(
                                "tls.strict/webpki-fallback: configured client certificate \
                                 chain '{cert_path}' has no parseable PEM CERTIFICATE section"
                            )));
                        }
                        // log-policy: handler-owned
                        tracing::warn!(
                            "configured client certificate chain has no parseable PEM \
                             CERTIFICATE section — client certificate NOT used"
                        );
                        builder.with_no_client_auth()
                    } else {
                        match rustls_pemfile::private_key(&mut std::io::Cursor::new(&key_bytes)) {
                            // Clone-on-attempt: rustls consumes the builder
                            // in `with_client_auth_cert`, but a rejection
                            // must fall back to `with_no_client_auth` on
                            // the SAME builder state.
                            Ok(Some(key)) => {
                                match builder.clone().with_client_auth_cert(chain, key) {
                                    Ok(config) => config,
                                    Err(e) => {
                                        if tls.strict {
                                            return Err(CamelError::EndpointCreationFailed(
                                                format!(
                                                    "tls.strict/webpki-fallback: configured mTLS \
                                             identity was rejected by the TLS backend: {e}"
                                                ),
                                            ));
                                        }
                                        // log-policy: handler-owned
                                        tracing::warn!(
                                            error = %e,
                                            "configured mTLS identity was rejected by the TLS \
                                             backend — client certificate NOT used"
                                        );
                                        builder.with_no_client_auth()
                                    }
                                }
                            }
                            // `Ok(None)` (no private-key section) and `Err`
                            // (malformed section) are the same failure for
                            // our purposes: no usable key.
                            Ok(None) | Err(_) => {
                                if tls.strict {
                                    return Err(CamelError::EndpointCreationFailed(format!(
                                        "tls.strict/webpki-fallback: configured client key \
                                         '{key_path}' has no parseable private key section"
                                    )));
                                }
                                // log-policy: handler-owned
                                tracing::warn!(
                                    "configured client key has no parseable private key \
                                     section — client certificate NOT used"
                                );
                                builder.with_no_client_auth()
                            }
                        }
                    }
                }
                (cert_r, key_r) => {
                    if tls.strict {
                        return Err(CamelError::EndpointCreationFailed(
                            "tls.strict/webpki-fallback: configured mTLS cert/key files are \
                             unreadable"
                                .to_string(),
                        ));
                    }
                    // log-policy: handler-owned
                    tracing::warn!(
                        cert_ok = cert_r.is_ok(),
                        key_ok = key_r.is_ok(),
                        "configured mTLS cert/key file unreadable — client certificate NOT used"
                    );
                    builder.with_no_client_auth()
                }
            }
        }
        // Half-configured mTLS pair (cert XOR key) — mirror of
        // strict_tls_error's half-pair rejection.
        (Some(_), None) | (None, Some(_)) => {
            if tls.strict {
                return Err(CamelError::EndpointCreationFailed(
                    "tls.strict/webpki-fallback: mTLS requires BOTH client_cert_path and \
                     client_key_path"
                        .to_string(),
                ));
            }
            // log-policy: handler-owned
            tracing::warn!(
                "mTLS requires BOTH client_cert_path and client_key_path — \
                 client certificate NOT used"
            );
            builder.with_no_client_auth()
        }
        (None, None) => builder.with_no_client_auth(),
    };
    Ok(Some(config))
}

/// Last-resort client for the (unreachable-on-reqwest-0.13.4) second
/// build error inside [`webpki_fallback_client`]: no proxy, no
/// redirects, webpki roots. Reaching the `expect` means the TLS stack
/// is broken process-wide, CA store or not — the caller logs
/// system-broken before delegating here.
fn emergency_webpki_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .tls_backend_preconfigured(webpki_root_client_config())
        .build()
        .expect("preconfigured webpki-rooted client build has no fallible stage") // allow-unwrap
}

/// Why the webpki fallback was entered: a real platform-verifier
/// build failure, or (tests only) a forced entry to exercise the
/// fallback path hermetically on hosts where a valid configured CA
/// rescues the primary verifier (e_glm adjudication, rc-hl9cn).
pub(crate) enum FallbackTrigger {
    Platform(reqwest::Error),
    #[cfg(test)]
    Forced,
}

impl std::fmt::Display for FallbackTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Same rendering as the pre-seam `error = %first_error`
            // field, so operator-facing logs are unchanged.
            FallbackTrigger::Platform(e) => std::fmt::Display::fmt(e, f),
            #[cfg(test)]
            FallbackTrigger::Forced => f.write_str("forced webpki fallback entry (test)"),
        }
    }
}

/// Shared terminal for a failed webpki fallback client rebuild —
/// `build_with_backend`'s `Err` arm and its `cfg(test)` seam twin:
/// always emits the system-broken error line, then fails closed with a
/// typed error under `tls.strict`, or degrades to the material-free
/// [`emergency_webpki_client`] otherwise.
fn fallback_rebuild_failed(
    config: &HttpConfig,
    failure: &dyn std::fmt::Display,
) -> Result<reqwest::Client, CamelError> {
    // log-policy: system-broken
    tracing::error!(
        error = %failure,
        "webpki fallback client build failed — TLS stack broken process-wide"
    );
    if config.tls.as_ref().is_some_and(|tls| tls.strict) {
        Err(CamelError::EndpointCreationFailed(format!(
            "tls.strict/webpki-fallback: webpki fallback client rebuild failed — \
             refusing material-free emergency client under tls.strict: {failure}"
        )))
    } else {
        Ok(emergency_webpki_client())
    }
}

/// Fallback path when the platform-verifier client build fails (typical
/// trigger: a platform without a system CA store at the probed paths,
/// e.g. Android/Termux — the eager `HttpComponent::new()` in the BASE
/// flavor used to panic there, rc-3j4mq). Startup must never panic on
/// CA-less platforms, so the retry swaps the TLS backend for the bundled
/// Mozilla root set — and, unlike the preconfigured backend of old, the
/// retry HONORS the configured TLS material (custom CA union, mTLS
/// identity, no-verify mode) via [`fallback_client_config`]. The
/// fallback is ONLY reachable through a failed primary build (or, under
/// `cfg(test)`, a [`FallbackTrigger::Forced`] seam entry) — it never
/// activates when native roots load.
///
/// `Err` is typed and strict-only by construction: [`fallback_client_config`]
/// fails closed only under `tls.strict`, and the shared second-error
/// rebuild terminal ([`fallback_rebuild_failed`]) fails closed the same
/// way — so a permissive config always leaves here with a built
/// (possibly item-downgraded, possibly material-free emergency) client.
pub(crate) fn webpki_fallback_client(
    config: &HttpConfig,
    resolve_override: Option<(&str, &[std::net::SocketAddr])>,
    trigger: FallbackTrigger,
) -> Result<reqwest::Client, CamelError> {
    #[cfg(test)]
    BUILD_CLIENT_FALLBACKS.with(|c| c.set(c.get() + 1));

    // log-policy: handler-owned
    tracing::warn!(
        error = %trigger,
        "HTTP client build failed on platform TLS roots — the platform CA store \
         is missing or unreadable (typical on Android/Termux). Retrying with \
         bundled Mozilla root certificates; platform trust settings do not \
         apply to this client"
    );

    // Shared second-error terminal: unreachable on reqwest 0.13.4 (the
    // preconfigured backend has no fallible stage — no platform
    // verifier, no root-store parse — unless the http3 feature is on,
    // which this workspace does not enable). Kept as an honest,
    // loudly-logged terminal instead of silently re-panicking: reaching
    // it means the TLS stack is broken process-wide, CA store or not.
    // Strict-fail-closed: under `tls.strict` the rebuild failure is a
    // typed `Err` (`fallback_rebuild_failed`); non-strict keeps the
    // material-free emergency-client degrade. The `cfg(test)` seam in
    // the `Ok` arm routes the forced-rebuild-failure twin through the
    // same terminal so both outcomes stay test-hermetic.
    let build_with_backend =
        |backend: rustls::ClientConfig| -> Result<reqwest::Client, CamelError> {
            match client_builder(config, resolve_override)
                .tls_backend_preconfigured(backend)
                .build()
            {
                Ok(client) => {
                    #[cfg(test)]
                    if FORCE_FALLBACK_REBUILD_FAIL.with(std::cell::Cell::get) {
                        return fallback_rebuild_failed(
                            config,
                            &"forced webpki fallback rebuild failure (test)",
                        );
                    }
                    Ok(client)
                }
                Err(second_error) => fallback_rebuild_failed(config, &second_error),
            }
        };

    // The strict carry note is accurate only when material is actually
    // configured: in the strict + insecure/verify_peer=false + zero-material
    // corner the fallback config exists solely for the danger verifier and
    // carries nothing.
    let material_configured = config.tls.as_ref().is_some_and(|tls| {
        tls.ca_cert_path.is_some()
            || tls.client_cert_path.is_some()
            || tls.client_key_path.is_some()
    });

    match config.tls.as_ref().map(fallback_client_config) {
        // No TLS config, TLS off, or verification-on with no material:
        // the plain webpki-rooted backend is the parity configuration.
        None | Some(Ok(None)) => build_with_backend(webpki_root_client_config()),
        Some(Ok(Some(strict_or_parity_config))) => {
            if material_configured && config.tls.as_ref().is_some_and(|tls| tls.strict) {
                // log-policy: handler-owned
                tracing::info!(
                    "platform CA store unavailable — webpki fallback carries \
                     configured TLS material"
                );
            }
            build_with_backend(strict_or_parity_config)
        }
        // Typed fail-closed conflict (strict-only by construction).
        Some(Err(e)) => Err(e),
    }
}

/// Build the shared/dns-pinned reqwest client. Primary path is the
/// platform verifier; on a platform whose CA store cannot be loaded the
/// build retries on bundled Mozilla roots honoring the configured TLS
/// material (see [`webpki_fallback_client`]) instead of panicking
/// (rc-3j4mq). `Err` is strict-only by construction: it means the
/// configured CA/mTLS material failed to load under `tls.strict`, or —
/// in the broken-TLS-stack corner — the webpki fallback client rebuild
/// failed under `tls.strict`, and the caller must fail closed rather
/// than serve with a degraded client.
pub(crate) fn build_client(
    config: &HttpConfig,
    resolve_override: Option<(&str, &[std::net::SocketAddr])>,
) -> Result<reqwest::Client, CamelError> {
    #[cfg(test)]
    BUILD_CLIENT_CALLS.with(|c| c.set(c.get() + 1));

    // Test seam (e_glm adjudication, rc-hl9cn): on hosts where a valid
    // configured CA rescues the primary verifier build
    // (rustls-platform-verifier merges extra roots first), the fallback
    // path with VALID material is unreachable through real triggers;
    // armed tests enter the fallback directly. Release builds are
    // unchanged.
    #[cfg(test)]
    if FORCE_WEBPKI_FALLBACK.with(std::cell::Cell::get) {
        return webpki_fallback_client(config, resolve_override, FallbackTrigger::Forced);
    }

    match client_builder(config, resolve_override).build() {
        Ok(client) => Ok(client),
        Err(first_error) => webpki_fallback_client(
            config,
            resolve_override,
            FallbackTrigger::Platform(first_error),
        ),
    }
}

/// Eagerly load and parse the configured TLS material when strict mode is
/// on (audit 2026-08-31 R3 / rc-ayrwk). Returns the first failure as an
/// `EndpointCreationFailed` error; `None` when the material loads, or when
/// strict mode is off (the permissive F2-7 fallback with its loud warns
/// stays the default for back-compat).
///
/// Mirrors the four load sites in [`build_client`]: CA unreadable, CA
/// unparseable, mTLS cert/key unreadable, mTLS identity unparseable.
pub(crate) fn strict_tls_error(config: &HttpConfig) -> Option<CamelError> {
    let tls = config.tls.as_ref()?;
    if !tls.enabled || !tls.strict {
        return None;
    }
    if let Some(ca_path) = &tls.ca_cert_path {
        match std::fs::read(ca_path) {
            Ok(ca_bytes) => {
                // `reqwest::Certificate::{from_pem,from_der}` defer parsing
                // under rustls, and unparseable entries are silently
                // skipped at client build — so strict validation must be
                // eager AND match what the backend actually enforces:
                // a PEM bundle with at least one parseable CERTIFICATE
                // section (rustls-pemfile). A raw-DER file is rejected
                // outright: the rustls backend never honors lone-DER
                // bytes here (they wrap unvalidated and are dropped at
                // root-store insertion), so certifying one under strict
                // would certify an unenforced config (e_glm stage-4
                // finding 1). Operators convert DER bundles to PEM.
                let pem_sections = rustls_pemfile::certs(&mut std::io::Cursor::new(&ca_bytes))
                    .filter(|r| r.is_ok())
                    .count();
                if pem_sections == 0 {
                    return Some(CamelError::EndpointCreationFailed(format!(
                        "tls.strict: configured CA certificate '{ca_path}' has no \
                         parseable PEM CERTIFICATE section (DER bundles are not \
                         enforced by the TLS backend — convert to PEM)"
                    )));
                }
            }
            Err(e) => {
                return Some(CamelError::EndpointCreationFailed(format!(
                    "tls.strict: configured CA certificate '{ca_path}' is unreadable: {e}"
                )));
            }
        }
    }
    // A half-configured mTLS pair (cert XOR key) previously degraded
    // silently to non-mTLS even under strict — reject it (e_glm stage-4
    // finding 2).
    if tls.client_cert_path.is_some() != tls.client_key_path.is_some() {
        return Some(CamelError::EndpointCreationFailed(
            "tls.strict: mTLS requires BOTH client_cert_path and client_key_path".to_string(),
        ));
    }
    if let (Some(cert_path), Some(key_path)) = (&tls.client_cert_path, &tls.client_key_path) {
        match (std::fs::read(cert_path), std::fs::read(key_path)) {
            (Ok(mut cert_bytes), Ok(key_bytes)) => {
                cert_bytes.extend_from_slice(&key_bytes);
                if reqwest::Identity::from_pem(&cert_bytes).is_err() {
                    return Some(CamelError::EndpointCreationFailed(
                        "tls.strict: configured mTLS identity failed to parse".to_string(),
                    ));
                }
            }
            _ => {
                return Some(CamelError::EndpointCreationFailed(
                    "tls.strict: configured mTLS cert/key files are unreadable".to_string(),
                ));
            }
        }
    }
    None
}

#[cfg(test)]
pub(crate) fn build_client_call_count() -> u64 {
    BUILD_CLIENT_CALLS.with(|c| c.get())
}

#[cfg(test)]
pub(crate) fn build_client_fallback_count() -> u64 {
    BUILD_CLIENT_FALLBACKS.with(|c| c.get())
}

/// Shared constructor fold: build the client, or — when the strict
/// webpki fallback fails closed — pair an emergency webpki client
/// (never issues a request; `create_endpoint` fails first via the
/// folded error) with the typed build error (rc-hl9cn).
pub(crate) fn client_or_emergency(config: &HttpConfig) -> (reqwest::Client, Option<CamelError>) {
    match build_client(config, None) {
        Ok(c) => (c, None),
        Err(e) => (emergency_webpki_client(), Some(e)),
    }
}

#[cfg(test)]
#[path = "tls_tests.rs"]
mod tests;
