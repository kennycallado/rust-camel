//! Loopback TLS harness for the forced-webpki-fallback handshake tests
//! (castrict task 1.3). The whole module is `#[cfg(test)]`-gated at its
//! declaration site; every item is `pub(crate)` so `mod tests` can
//! `use crate::tls_harness::*;`.
//!
//! Why not `camel_component_api::test_support::tls::gen_server_cert()`:
//! that helper discards the CA key and params after self-signing, so no
//! client identity (mTLS leaf) can be signed afterwards. rcgen 0.14
//! builds leaf signers from a RETAINED `Issuer::from_params(&ca_params,
//! &ca_key)` (the 3-arg `signed_by(key, ca_cert, ca_key)` is gone) — a
//! local harness that keeps the CA material alive is deliberate.

use std::io::Cursor;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

use crate::config::HttpConfig;

/// Bound for every accept / TLS handshake / read / write the harness
/// performs — a broken harness must fail the test, never hang it.
pub(crate) const HARNESS_IO_TIMEOUT: Duration = Duration::from_secs(10);
/// The harness serves at most this many connections before retiring.
pub(crate) const HARNESS_MAX_CONNECTIONS: usize = 4;
/// Upper bound on buffered request bytes before the reply is written
/// anyway (header-end search cap).
pub(crate) const HARNESS_READ_CAP: usize = 8 * 1024;
/// Verbatim reply written once a request's header-end is seen.
pub(crate) const HARNESS_OK_REPLY: &str =
    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

/// Ephemeral test CA. `ca_params`/`ca_key` are RETAINED: rcgen 0.14
/// builds leaf `Issuer`s from them, which is what lets this harness
/// sign both server leaves and client identities from the same CA.
pub(crate) struct TestCa {
    pub(crate) ca_params: rcgen::CertificateParams,
    pub(crate) ca_key: rcgen::KeyPair,
    pub(crate) ca_pem: String,
}

/// Self-signed CA ("camel-http test ca", unconstrained, keyCertSign).
/// Callers write `ca_pem` to a tempdir and configure it as
/// `tls.ca_cert_path` and/or the server's client-auth CA.
pub(crate) fn gen_test_ca() -> TestCa {
    let ca_key = rcgen::KeyPair::generate().expect("ca keygen"); // allow-unwrap(test)
    let mut ca_params = rcgen::CertificateParams::default();
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "camel-http test ca");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .key_usages
        .push(rcgen::KeyUsagePurpose::KeyCertSign);
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca self-sign"); // allow-unwrap(test)
    TestCa {
        ca_pem: ca_cert.pem(),
        ca_params,
        ca_key,
    }
}

/// Server leaf signed by `ca` with SAN `san_ip` (the loopback harness
/// passes 127.0.0.1 so reqwest hostname verification passes on the IP
/// SAN). Returns `(cert_pem, key_pem)`.
pub(crate) fn gen_leaf_signed_by_ca(ca: &TestCa, san_ip: IpAddr) -> (String, String) {
    let leaf_key = rcgen::KeyPair::generate().expect("leaf keygen"); // allow-unwrap(test)
    let mut leaf_params = rcgen::CertificateParams::default();
    leaf_params.subject_alt_names = vec![rcgen::SanType::IpAddress(san_ip)];
    let issuer = rcgen::Issuer::from_params(&ca.ca_params, &ca.ca_key);
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &issuer)
        .expect("leaf sign"); // allow-unwrap(test)
    (leaf_cert.pem(), leaf_key.serialize_pem())
}

/// Client identity (CN "camel-http test client") signed by `ca` — no IP
/// SAN needed; the server verifies it against the CA. Returns
/// `(cert_pem, key_pem)`.
pub(crate) fn gen_client_identity(ca: &TestCa) -> (String, String) {
    let id_key = rcgen::KeyPair::generate().expect("identity keygen"); // allow-unwrap(test)
    let mut id_params = rcgen::CertificateParams::default();
    id_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "camel-http test client");
    let issuer = rcgen::Issuer::from_params(&ca.ca_params, &ca.ca_key);
    let id_cert = id_params
        .signed_by(&id_key, &issuer)
        .expect("identity sign"); // allow-unwrap(test)
    (id_cert.pem(), id_key.serialize_pem())
}

/// Self-signed (NOT CA-signed) server leaf with SAN `127.0.0.1`. The
/// signer is a fresh keypair unrelated to any CA a test configures, so
/// verifying clients must reject the handshake — the parity-scenario
/// counterpart to [`gen_leaf_signed_by_ca`]. Returns
/// `(cert_pem, key_pem)`.
pub(crate) fn gen_self_signed_server_cert() -> (String, String) {
    let server_key = rcgen::KeyPair::generate().expect("server keygen"); // allow-unwrap(test)
    let mut server_params = rcgen::CertificateParams::default();
    server_params.subject_alt_names =
        vec![rcgen::SanType::IpAddress(IpAddr::V4([127, 0, 0, 1].into()))];
    let server_cert = server_params
        .self_signed(&server_key)
        .expect("self-signed server cert"); // allow-unwrap(test)
    (server_cert.pem(), server_key.serialize_pem())
}

/// Loopback TLS server on 127.0.0.1:0. With `client_ca_pem: Some(_)`
/// the server REQUIRES client certificates verified against that CA
/// (mTLS scenarios); otherwise `with_no_client_auth()`. Serves at most
/// [`HARNESS_MAX_CONNECTIONS`] connections, each fully bounded: accept,
/// TLS handshake, request-header read (header-end search under a
/// [`HARNESS_READ_CAP`] buffer), reply write. No sleeps anywhere.
/// Cleanup is the caller's `handle.abort()`.
pub(crate) async fn spawn_tls_server(
    server_cert_pem: &str,
    server_key_pem: &str,
    client_ca_pem: Option<&str>,
) -> (SocketAddr, JoinHandle<()>) {
    let chain: Vec<_> = rustls_pemfile::certs(&mut Cursor::new(server_cert_pem.as_bytes()))
        .collect::<Result<_, _>>()
        .expect("server cert pem parses"); // allow-unwrap(test)
    let key = rustls_pemfile::private_key(&mut Cursor::new(server_key_pem.as_bytes()))
        .expect("server key pem parses") // allow-unwrap(test)
        .expect("server key pem carries a private key section"); // allow-unwrap(test)

    // Mirror the client side's provider resolution
    // (webpki_root_client_config): process-default provider when the
    // host installed one, else the aws-lc-rs default — this crate
    // enables BOTH rustls crypto features, so the feature-derived
    // auto-detection in ServerConfig::builder() cannot resolve one.
    let provider: Arc<rustls::crypto::CryptoProvider> =
        rustls::crypto::CryptoProvider::get_default()
            .cloned()
            .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
    let versions_builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        // Stock rustls providers (ring, aws-lc-rs) always support the
        // safe default TLS versions.
        .expect("stock rustls provider supports the safe default protocol versions"); // allow-unwrap(test)

    let server_config = match client_ca_pem {
        None => versions_builder
            .with_no_client_auth()
            .with_single_cert(chain, key),
        Some(ca_pem) => {
            let client_ca: Vec<_> = rustls_pemfile::certs(&mut Cursor::new(ca_pem.as_bytes()))
                .collect::<Result<_, _>>()
                .expect("client CA pem parses"); // allow-unwrap(test)
            let mut roots = rustls::RootCertStore::empty();
            roots.add_parsable_certificates(client_ca);
            let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                Arc::new(roots),
                provider,
            )
            .build()
            .expect("client cert verifier builds"); // allow-unwrap(test)
            versions_builder
                .with_client_cert_verifier(verifier)
                .with_single_cert(chain, key)
        }
    }
    .expect("server TLS config builds"); // allow-unwrap(test)

    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener binds"); // allow-unwrap(test)
    let addr = listener
        .local_addr()
        .expect("loopback listener yields a local addr"); // allow-unwrap(test)

    let server = tokio::spawn(async move {
        for _ in 0..HARNESS_MAX_CONNECTIONS {
            // Bound the accept: a quiet harness retires instead of
            // parking the slot forever.
            let tcp = match tokio::time::timeout(HARNESS_IO_TIMEOUT, listener.accept()).await {
                Ok(Ok((stream, _))) => stream,
                _ => break,
            };
            // Bound the handshake: a client that cannot complete it
            // (the failure scenarios) releases the slot quickly.
            let mut tls_stream =
                match tokio::time::timeout(HARNESS_IO_TIMEOUT, acceptor.accept(tcp)).await {
                    Ok(Ok(stream)) => stream,
                    _ => continue,
                };
            // Read the request up to its header-end (\r\n\r\n), bounded
            // by the buffer cap and the IO timeout.
            let mut buf = vec![0u8; HARNESS_READ_CAP];
            let mut seen = 0usize;
            loop {
                if seen == buf.len() || buf[..seen].ends_with(b"\r\n\r\n") {
                    break;
                }
                match tokio::time::timeout(HARNESS_IO_TIMEOUT, tls_stream.read(&mut buf[seen..]))
                    .await
                {
                    Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                    Ok(Ok(n)) => seen += n,
                }
            }
            let _ = tokio::time::timeout(
                HARNESS_IO_TIMEOUT,
                tls_stream.write_all(HARNESS_OK_REPLY.as_bytes()),
            )
            .await;
            let _ = tokio::time::timeout(HARNESS_IO_TIMEOUT, tls_stream.shutdown()).await;
        }
    });

    (addr, server)
}

/// Runs `body` under the forced CA-less-platform env window: an
/// existing-but-empty `SSL_CERT_FILE` plus an existing-but-empty
/// `SSL_CERT_DIR` make rustls-native-certs load zero roots — the exact
/// condition routing `build_client` through the webpki fallback.
/// Holds the CA-store mutex for the whole window and restores the
/// previous env even when `body` panics; assertions belong AFTER
/// this helper returns.
pub(crate) fn forced_fallback_env<R>(body: impl FnOnce() -> R) -> R {
    let _ca_guard = crate::lock_ca_store_test_mutex();
    let empty_ca_dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let empty_ca_file = empty_ca_dir.path().join("empty-ca-bundle.pem");
    std::fs::write(&empty_ca_file, b"").expect("write empty CA file"); // allow-unwrap(test)

    // Safety: test-only mutation of process env vars. Previous values
    // are captured and restored before the caller observes the result
    // (and on unwind below); the mutex serializes the other
    // env-window tests in this process.
    let (prev_file, prev_dir) = unsafe {
        let prev = (
            std::env::var_os("SSL_CERT_FILE"),
            std::env::var_os("SSL_CERT_DIR"),
        );
        std::env::set_var("SSL_CERT_FILE", &empty_ca_file);
        std::env::set_var("SSL_CERT_DIR", empty_ca_dir.path());
        prev
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));

    // Safety: restoring the previously-captured values.
    unsafe {
        match prev_file {
            Some(v) => std::env::set_var("SSL_CERT_FILE", v),
            None => std::env::remove_var("SSL_CERT_FILE"),
        }
        match prev_dir {
            Some(v) => std::env::set_var("SSL_CERT_DIR", v),
            None => std::env::remove_var("SSL_CERT_DIR"),
        }
    }

    result.unwrap_or_else(|payload| std::panic::resume_unwind(payload))
}

/// Runs `body` with the `FORCE_WEBPKI_FALLBACK` test seam armed: on
/// this thread, `build_client` skips the platform-verifier primary and
/// enters the webpki fallback directly
/// ([`crate::FallbackTrigger::Forced`]). This exercises the fallback
/// path hermetically on hosts where a valid configured CA rescues the
/// primary verifier (e_glm adjudication, rc-hl9cn) — no env window is
/// needed, because the primary is skipped rather than starved. The
/// flag is ALWAYS reset, even when `body` panics: the Drop guard below
/// runs during unwinding, mirroring [`forced_fallback_env`]'s
/// restore-on-panic discipline. Assertions belong AFTER this helper
/// returns.
pub(crate) fn force_webpki_fallback<R>(body: impl FnOnce() -> R) -> R {
    /// Resets the seam flag on scope exit — normal or panicking — so a
    /// failing test cannot leak forced-fallback mode into siblings.
    struct ResetFlag;
    impl Drop for ResetFlag {
        fn drop(&mut self) {
            crate::FORCE_WEBPKI_FALLBACK.with(|c| c.set(false));
        }
    }

    let already_armed = crate::FORCE_WEBPKI_FALLBACK.with(|c| c.replace(true));
    assert!(
        !already_armed,
        "force_webpki_fallback must not nest — the seam flag was already armed"
    );
    let _reset = ResetFlag;
    body()
}

/// Runs `body` with BOTH fallback seams armed: on this thread,
/// `build_client` skips the platform-verifier primary
/// (`FORCE_WEBPKI_FALLBACK`) AND the webpki fallback's successful
/// preconfigured-backend build is treated as a rebuild failure
/// (`FORCE_FALLBACK_REBUILD_FAIL`), hermetically routing the call
/// through [`crate::webpki_fallback_client`]'s shared second-error
/// terminal. That terminal is unreachable through real triggers on
/// reqwest 0.13.4, so the seam is the only way to exercise its
/// strict-fail-closed and emergency-degrade outcomes. BOTH flags are
/// ALWAYS reset, even when `body` panics: the Drop guard below runs
/// during unwinding, mirroring [`force_webpki_fallback`]'s
/// restore-on-panic discipline. Assertions belong AFTER this helper
/// returns.
pub(crate) fn force_webpki_fallback_rebuild_failure<R>(body: impl FnOnce() -> R) -> R {
    /// Resets BOTH seam flags on scope exit — normal or panicking — so
    /// a failing test cannot leak forced-fallback mode into siblings.
    struct ResetBoth;
    impl Drop for ResetBoth {
        fn drop(&mut self) {
            crate::FORCE_WEBPKI_FALLBACK.with(|c| c.set(false));
            crate::FORCE_FALLBACK_REBUILD_FAIL.with(|c| c.set(false));
        }
    }

    let rebuild_already_armed = crate::FORCE_FALLBACK_REBUILD_FAIL.with(std::cell::Cell::get);
    let fallback_already_armed = crate::FORCE_WEBPKI_FALLBACK.with(std::cell::Cell::get);
    assert!(
        !rebuild_already_armed && !fallback_already_armed,
        "force_webpki_fallback_rebuild_failure must not nest — a seam flag was already armed"
    );
    crate::FORCE_FALLBACK_REBUILD_FAIL.with(|c| c.set(true));
    crate::FORCE_WEBPKI_FALLBACK.with(|c| c.set(true));
    let _reset = ResetBoth;
    body()
}

/// `reqwest::Client::new()` panics when the TLS backend cannot
/// initialize — including when another test's forced CA-less env
/// window ([`forced_fallback_env`]) happens to have emptied the
/// process-wide native-root probe paths (SSL_CERT_FILE/
/// SSL_CERT_DIR) at that instant. Every plain test-client build
/// therefore serializes against the CA-store mutex, closing the
/// env-window race mechanically: all plain test client builds in lib.rs
/// `mod tests` and in `client_cache`'s tests are routed through this
/// helper, so windows and plain builds can no longer overlap.
pub(crate) fn plain_http_test_client() -> reqwest::Client {
    let _ca_guard = crate::lock_ca_store_test_mutex();
    reqwest::Client::new()
}

/// Build a client through the FORCE_WEBPKI_FALLBACK seam and assert
/// the fallback path was actually taken (counter delta == 1) — the
/// positive path control for seam-based tests (rc-hl9cn). The count
/// capture and the build run on the calling thread (the seam flag is
/// thread-local), and `force_webpki_fallback`'s Drop guard resets the
/// flag even when the build panics.
pub(crate) fn build_under_seam(config: &HttpConfig) -> reqwest::Client {
    let fallbacks_before = crate::build_client_fallback_count();
    let client = force_webpki_fallback(|| {
        crate::build_client(config, None).expect("client must build") // allow-unwrap(test)
    });
    let fallbacks_taken = crate::build_client_fallback_count() - fallbacks_before;
    assert_eq!(
        fallbacks_taken, 1,
        "the force seam must route the build through the webpki fallback"
    );
    client
}
