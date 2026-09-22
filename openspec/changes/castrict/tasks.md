# Tasks: castrict

## camel-http

### Task 1.1: Fallback rustls config builder — root-store seam, no-verify verifier, client-auth assembly

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified — new private fns near `webpki_root_client_config`, ~line 2379)
- `crates/components/camel-http/Cargo.toml` (modified — dev-dep `rcgen = { workspace = true }`)

**Steps:**
1. Add `rcgen = { workspace = true }` to `[dev-dependencies]` in `crates/components/camel-http/Cargo.toml`.
2. Add `fn fallback_root_store(custom_ca: Option<&str>, strict: bool) -> Result<rustls::RootCertStore, CamelError>` (private, next to `webpki_root_client_config`). All strict errors below are `CamelError::EndpointCreationFailed` values constructed with `format!` and the FULL literal message given; every message starts with the prefix `tls.strict/webpki-fallback: `:
   - Start from `rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() }` (the Mozilla anchors).
   - `custom_ca = None` → return the Mozilla-only store.
   - `Some(ca_path)` → `std::fs::read(ca_path)`; on `Err(e)`: strict → Err with `format!("tls.strict/webpki-fallback: configured CA certificate '{ca_path}' is unreadable: {e}")`; non-strict → `tracing::warn!` (with `// log-policy: handler-owned` comment, wording mirroring the existing F2-7 CA-unreadable warn in `client_builder`, containing the substring `falling back to bundled Mozilla roots`) and return the Mozilla-only store.
   - Parse with `rustls_pemfile::certs` (count Ok sections; same Cursor pattern as `client_builder`/`strict_tls_error`); zero parseable CERTIFICATE sections → strict Err with `format!("tls.strict/webpki-fallback: configured CA certificate '{ca_path}' has no parseable PEM CERTIFICATE section")`, non-strict warn + Mozilla-only.
   - `store.add_parsable_certificates(&certs)` → `(added, ignored)`; `added == 0` → strict Err with `format!("tls.strict/webpki-fallback: configured CA certificate '{ca_path}' was rejected by the TLS root store (0 of {} accepted)", certs.len())`, non-strict warn + Mozilla-only.
   - Otherwise return the union store (Mozilla + custom).
2. Add `#[derive(Debug)] struct NoVerifyServerCertVerifier;` implementing `rustls::client::danger::ServerCertVerifier` (the trait requires `Debug`; mirror reqwest 0.13.4's internal `NoVerifier` exactly — read it in the vendored registry sources if needed):
   - `verify_server_cert` → `Ok(rustls::client::danger::ServerCertVerified::assertion())` (accept any input, no validation; doc comment referencing `danger_accept_invalid_certs` parity).
   - `verify_tls12_signature` / `verify_tls13_signature` → `Ok(rustls::client::danger::HandshakeSignatureValid::assertion())` (NOT `DigitallySignedStruct` — that type belongs to signing, not verification).
   - `supported_verify_schemes` → EXACTLY reqwest 0.13.4's `NoVerifier` list (verify against the vendored source at `src/tls.rs` lines 703-719): `vec![SignatureScheme::RSA_PKCS1_SHA1, SignatureScheme::ECDSA_SHA1_Legacy, SignatureScheme::RSA_PKCS1_SHA256, SignatureScheme::ECDSA_NISTP256_SHA256, SignatureScheme::RSA_PKCS1_SHA384, SignatureScheme::ECDSA_NISTP384_SHA384, SignatureScheme::RSA_PKCS1_SHA512, SignatureScheme::ECDSA_NISTP521_SHA512, SignatureScheme::RSA_PSS_SHA256, SignatureScheme::RSA_PSS_SHA384, SignatureScheme::RSA_PSS_SHA512, SignatureScheme::ED25519, SignatureScheme::ED448]` — 13 schemes, SHA1-legacy and P-521 included.
4. Add `fn fallback_client_config(tls: &TlsConfig) -> Result<Option<rustls::ClientConfig>, CamelError>` (private; callers resolve `config.tls` presence):
   - `!tls.enabled` → `Ok(None)`.
   - `material = tls.ca_cert_path.is_some() || tls.client_cert_path.is_some() || tls.client_key_path.is_some()`; `verification_disabled = tls.insecure || !tls.verify_peer`; if `!material && !verification_disabled` → `Ok(None)` (plain `webpki_root_client_config()` suffices).
   - Provider resolution identical to `webpki_root_client_config` (process-default, else `rustls::crypto::aws_lc_rs::default_provider()`).
   - `let store = fallback_root_store(tls.ca_cert_path.as_deref(), tls.strict)?` — computed ALWAYS (strict material validation happens even when the danger verifier would bypass roots; `Err` propagates only under strict by construction of `fallback_root_store`).
   - Base builder: `rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().expect("stock rustls provider supports the safe default protocol versions")` with the same rationale comment as `webpki_root_client_config` and the `// allow-unwrap` marker.
   - Verification: if `verification_disabled` → `builder.dangerous().with_custom_certificate_verifier(std::sync::Arc::new(NoVerifyServerCertVerifier))`; else → `builder.with_root_certificates(store)`.
   - Client auth item-wise (all strict errors are `EndpointCreationFailed` values constructed with `format!` and the FULL literal message; all non-strict branches warn with `// log-policy: handler-owned` and a message containing `client certificate NOT used`):
     - Both `client_cert_path` + `client_key_path`: read both; read failure → strict Err with `format!("tls.strict/webpki-fallback: configured mTLS cert/key files are unreadable")`, non-strict warn + `with_no_client_auth()`. Parse cert chain (`rustls_pemfile::certs`, empty chain → strict Err with `format!("tls.strict/webpki-fallback: configured client certificate chain '{cert_path}' has no parseable PEM CERTIFICATE section")`, non-strict warn + no-auth). Key via `rustls_pemfile::private_key` — it returns `Ok(Some(key))`, `Ok(None)` (no private-key section), OR `Err(io::Error)` (malformed section); both `None` and `Err` take: strict Err with `format!("tls.strict/webpki-fallback: configured client key '{key_path}' has no parseable private key section")`, non-strict warn + no-auth. Then `with_client_auth_cert(chain, key)`; on rustls rejection `e` (PEM-valid section, no usable key material) → strict Err with `format!("tls.strict/webpki-fallback: configured mTLS identity was rejected by the TLS backend: {e}")`, non-strict warn + `with_no_client_auth()`.
     - Half-pair (XOR): strict Err with `format!("tls.strict/webpki-fallback: mTLS requires BOTH client_cert_path and client_key_path")` (mirror of `strict_tls_error`), non-strict warn + `with_no_client_auth()`.
     - Neither → `with_no_client_auth()`.
   - Return `Ok(Some(config))`. CA-item warns contain `falling back to bundled Mozilla roots` (the CA-less stand-in phrasing of the existing `falling back to system roots`).
5. Unit tests in the existing `mod tests` near the TLS test cluster (~line 13560), using `rcgen` for a real one-root CA where a valid CA is needed — follow the rcgen 0.14 API shapes in `crates/components/camel-component-api/src/test_support.rs:32-55` (retain `CertificateParams`, sign via `rcgen::Issuer::from_params` / `params.self_signed(&key)`; there is NO 3-arg `signed_by` in 0.14) — self-signed CA, `cert.pem()` written to a `tempfile::tempdir` path.

**Tests:** (all in `crates/components/camel-http/src/lib.rs` `mod tests`; command `cargo test -p camel-http --lib fallback_` — expected: fail before implementation, pass after)
- `test_fallback_root_store_union_custom_and_mozilla`: rcgen one-root CA PEM on disk → `fallback_root_store(Some(path), true)` → Ok; assert `store.roots.len() == webpki_roots::TLS_SERVER_ROOTS.len() + 1` (union, not replacement).
- `test_fallback_root_store_no_ca_is_mozilla_only`: `fallback_root_store(None, true)` → Ok; `store.roots.len() == webpki_roots::TLS_SERVER_ROOTS.len()`.
- `test_fallback_root_store_strict_unreadable_ca_fails_closed`: path to nonexistent file, strict=true → Err `EndpointCreationFailed`; message contains `tls.strict/webpki-fallback` and `unreadable`.
- `test_fallback_root_store_strict_zero_pem_sections_fails_closed`: file containing only `-----BEGIN PRIVATE KEY-----` block bytes / plain text, strict → Err; message contains `no parseable PEM CERTIFICATE`.
- `test_fallback_root_store_strict_zero_roots_accepted_fails_closed`: file containing a well-framed `-----BEGIN CERTIFICATE-----`/base64 `AAAA`/`-----END CERTIFICATE-----` block (pemfile-parseable, store-rejected), strict → Err; message contains `rejected by the TLS root store`.
- `test_fallback_root_store_nonstrict_bad_ca_degrades_to_mozilla`: garbage CA file, strict=false → Ok with `store.roots.len() == webpki_roots::TLS_SERVER_ROOTS.len()`.
- `test_fallback_client_config_disabled_tls_returns_none`: `TlsConfig { enabled: false, ..Default }` → `Ok(None)`.
- `test_fallback_client_config_no_material_verifying_returns_none`: enabled, no material, verification on → `Ok(None)`.
- `test_fallback_client_config_strict_identity_garbage_key_fails_closed`: valid rcgen client cert PEM + key file with plain-text garbage, strict → Err; message contains `no parseable private key`.
- `test_fallback_client_config_strict_rustls_rejected_key_fails_closed`: valid cert + key file holding `-----BEGIN PRIVATE KEY-----` + base64 `AAAA` + `-----END PRIVATE KEY-----` (the PEM section parses — pemfile base64-decodes and tags it PKCS8 without inner DER validation — but rustls rejects it at `with_client_auth_cert`: no usable key material), strict → Err; message contains `rejected by the TLS backend`.
- `test_fallback_client_config_half_mtls_pair_strict_fails_nonstrict_no_auth`: cert-only config → strict Err (`BOTH client_cert_path`), non-strict `Ok(Some(_))`.
- `test_no_verify_verifier_accepts_any_certificate`: construct `NoVerifyServerCertVerifier`; `verify_server_cert(&garbage CertificateDer, &[], &ServerName::try_from("localhost"), &[], rustls::pki_types::UnixTime::now())` → `Ok(ServerCertVerified::assertion())`.

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-http --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-http --lib fallback_` passes (12 tests).
- `cargo xtask lint-unwrap`, `cargo xtask lint-log-levels`, `cargo xtask lint-log-redaction`, `cargo xtask lint-secrets` exit 0 (new warns carry log-policy comments; no embedded private keys — rcgen generates at runtime).

- [x] 1.1

### Task 1.2: `build_client` becomes fallible — fallback rewiring, constructor folding, pinned-cache error propagation

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/src/client_cache.rs` (modified)
- `crates/components/camel-http/src/ssrf.rs` (modified — redirect-hop `get_or_build` call site adopts the fallible signature)

**Steps:**
1. Extract `fn emergency_webpki_client() -> reqwest::Client` from the inner terminal of `webpki_fallback_client` (the `no_proxy` + no-redirect + `tls_backend_preconfigured(webpki_root_client_config())` + `.build()` with the existing unreachable-on-reqwest-0.13.4 `expect` and `// allow-unwrap` + system-broken log stays at the former call site).
2. Rewrite `fn webpki_fallback_client(config, resolve_override, first_error) -> Result<reqwest::Client, CamelError>`:
   - Keep the `BUILD_CLIENT_FALLBACKS` counter increment and the platform-CA-store warn verbatim.
   - `match config.tls.as_ref().map(fallback_client_config)`:
     - `None | Some(Ok(None))` → existing material-free path: `client_builder(config, resolve_override).tls_backend_preconfigured(webpki_root_client_config()).build()` — on the (unreachable) second error, log system-broken and fall back to `emergency_webpki_client()`, return Ok.
     - `Some(Ok(Some(strict_or_parity_config)))` → `client_builder(config, resolve_override).tls_backend_preconfigured(strict_or_parity_config).build()` with the same second-error terminal; when `tls.strict` emit `tracing::info!` (`// log-policy: handler-owned`): `"platform CA store unavailable — webpki fallback carries configured TLS material"`. Return Ok.
     - `Some(Err(e))` → `Err(e)` (typed fail-closed conflict; strict-only by construction).
   - DELETE the old material warn block — the `tracing::warn!` whose message contains `is not carried` (superseded; non-strict failed-item warns now come from inside the builder seam).
3. `pub(crate) fn build_client(config, resolve_override) -> Result<reqwest::Client, CamelError>`: primary `client_builder(config, resolve_override).build()` Ok → `Ok(client)`; Err(first) → `webpki_fallback_client(config, resolve_override, first)`.
4. Constructors `HttpComponent::new`, `HttpComponent::with_config`, `HttpsComponent::new`, `HttpsComponent::with_config` (4 sites):
   - `let strict_err = strict_tls_error(&config);`
   - `let (client, build_err) = match build_client(&config, None) { Ok(c) => (c, None), Err(e) => (emergency_webpki_client(), Some(e)) };`
   - Field init `strict_tls_error: strict_err.or(build_err)` — endpoint creation fails with the folded error; construction never panics (rc-3j4mq preserved: BASE `new()` uses TLS-free default config and cannot take the Err branch).
5. `PinnedClientCache::get_or_build` (`src/client_cache.rs`): build closure type `impl FnOnce() -> Result<reqwest::Client, CamelError>` and return `Result<reqwest::Client, CamelError>` — swap `moka::future::Cache::get_with` for `try_get_with(key, init)` (moka 0.12.16, `future/cache.rs:1305`: `try_get_with<F, E>(&self, key, init) -> Result<V, Arc<E>>`), which preserves single-flight, does NOT cache the error, and returns `Err(Arc<CamelError>)` → map to owned via `CamelError: Clone` (`camel-api/src/error.rs:76`). Keep the build-counter increment + `built` flag inside the init closure (miss-metric semantics unchanged). Update the doc comment.
6. Producer pinned call site (`lib.rs`, the `pinned_cache.get_or_build(host.as_str(), addrs, || build_client(&http_config, Some((host.as_str(), addrs))))` inside the producer send path): the closure now returns the `build_client(&http_config, Some((host.as_str(), addrs)))` Result directly; map the returned Err into the request error path with `CamelError::ProcessorError`-equivalent propagation (`?` through the send path) — fail closed at request time; no degraded client is built.
7. Update existing in-crate call sites of `build_client` for the Result signature: the direct-construction test around line 10437 (`HttpProducer` literal) uses `.expect("client must build") // allow-unwrap(test)`; the two env-forcing tests (~13621, ~13699) bind `let _client = build_client(&HttpConfig::default(), None)` → add `.expect("client must build") // allow-unwrap(test)`; existing `client_cache.rs` tests pass Ok-returning closures.
8. Update the existing forced-fallback test `test_build_client_falls_back_on_empty_platform_ca_store` — default config still yields `Ok`, exactly one fallback; assertion unchanged apart from the Result unwrap.

**Tests:** (command `cargo test -p camel-http --lib` — the full library suite; individual filters used below are single-filter libtest invocations like `cargo test -p camel-http --lib fallback_`; expected: new tests fail before wiring, pass after)
- `test_build_client_forced_fallback_strict_store_rejected_ca_returns_typed_error`: under `lock_ca_store_test_mutex()` + empty `SSL_CERT_FILE`/`SSL_CERT_DIR` env window (existing pattern), config with `tls{enabled, strict, ca_cert_path}` pointing at the pemfile-ok/store-rejected `AAAA` CERTIFICATE fixture → `build_client(&config, None)` → `Err`; message contains `tls.strict/webpki-fallback` and `rejected by the TLS root store`; `build_client_fallback_count()` delta == 1.
- `test_build_client_forced_fallback_strict_material_rejection_classes`: same env window; four cases, each `tls{enabled, strict}` — (a) `ca_cert_path` = file with zero PEM CERTIFICATE sections (plain text) → Err containing `no parseable PEM CERTIFICATE section`; (b) valid rcgen client cert + `client_key_path` = plain-text garbage → Err containing `no parseable private key section`; (c) valid cert + `client_key_path` = file holding `-----BEGIN PRIVATE KEY-----`, base64 `AAAA`, `-----END PRIVATE KEY-----` (PEM section parses; rustls rejects at client-auth configuration) → Err containing `rejected by the TLS backend`; (d) `client_cert_path` = nonexistent path with a valid `client_key_path` → Err containing `unreadable`.
- `test_build_client_forced_fallback_nonstrict_bad_ca_still_builds`: same env window, bad CA but `strict=false` → `Ok(client)`; fallback count delta == 1 (permissive item downgrade).
- `test_http_component_with_config_folds_fallback_error_without_panic`: same env window; `HttpComponent::with_config` with strict + store-rejected CA → construction completes (no panic); `create_endpoint("http://h/p", &NoOpComponentContext)` → `Err` whose message contains `rejected by the TLS root store` (folded error surfaces; existing `NoOpComponentContext` double as used at lib.rs ~4833).
- `test_http_component_new_no_panic_on_empty_ca_store` (in env window): `HttpComponent::new()` constructs without panic under the forced-fallback env AND `create_endpoint("http://h/p", &NoOpComponentContext)` returns `Ok` — the material-free fallback serves endpoints (rc-3j4mq regression guard at component level; the spec scenario "Default construction never panics on CA-less platforms" requires serving endpoints on the material-free fallback).
- `test_pinned_get_or_build_propagates_error_without_caching` (in `client_cache.rs` tests): closure returning `Err(CamelError::Config("pinned build failed"))` → `get_or_build` returns Err; `build_count() == 1`; after `run_pending_tasks().await`, `entry_count() == 0`.

**Acceptance:**
- `cargo fmt --check --all` and `cargo clippy -p camel-http --all-targets -- -D warnings` exit 0.
- `cargo test -p camel-http --lib` passes in full (existing suite updated, no regressions).
- `cargo xtask lint-unwrap`, `cargo xtask lint-log-levels`, `cargo xtask lint-log-redaction`, `cargo xtask lint-unbounded-wait` exit 0.
- `rg -n "is not carried" crates/components/camel-http/src/lib.rs` returns no matches (old warn removed).

- [x] 1.2

### Task 1.3: Loopback TLS test harness + strict material-carried handshake scenarios

**Files:**
- `crates/components/camel-http/src/tls_harness.rs` (new — `#[cfg(test)]`-gated module: cert generation + loopback TLS server helpers; ALL its items declared `pub(crate)` so `mod tests` can `use crate::tls_harness::*;`)
- `crates/components/camel-http/src/lib.rs` (modified — `#[cfg(test)] mod tls_harness;` declaration + the handshake tests in `mod tests`)

**Steps:**
1. Create `src/tls_harness.rs` (keep lib.rs from growing further; module precedent: `client_cache.rs`). Add in `mod tests` of lib.rs: `#[cfg(test)] mod tls_harness;` — actually declare it at crate level as `#[cfg(test)] mod tls_harness;` so `mod tests` can `use crate::tls_harness::*;`.
   - rcgen 0.14 API shape — follow the in-repo precedent `crates/components/camel-component-api/src/test_support.rs:32-55` EXACTLY: rcgen 0.14 has NO 3-arg `signed_by(key, ca_cert, ca_key)`; signing requires retaining the CA's `CertificateParams` and building `rcgen::Issuer::from_params(&ca_params, &ca_key)`, then `server_params.signed_by(&server_key, &issuer)`. Helper set:
   - `fn gen_test_ca() -> TestCa { ca_params, ca_key, ca_pem: String }` — `rcgen::CertificateParams` with `IsCa::Ca(BasicConstraints::Unconstrained)`, `KeyUsagePurpose::KeyCertSign`, dist_name "camel-http test ca"; PEM string written by callers to tempdirs. RETAIN `ca_params` (the `Issuer` needs it).
   - `fn gen_leaf_signed_by_ca(ca: &TestCa, san_ip: 127.0.0.1) -> (cert_pem, key_pem)` — leaf `CertificateParams` with `subject_alt_names = vec![rcgen::SanType::IpAddress("127.0.0.1".parse())]`, fresh `KeyPair`, `signed_by(&leaf_key, &issuer)`.
   - `fn gen_client_identity(ca: &TestCa) -> (cert_pem, key_pem)` — same leaf signing (no IP SAN needed; client-auth certs are verified by the server against the CA, but include CN).
   - The existing `test_support::tls::gen_server_cert()` CANNOT be reused: it discards `ca_key`/params so no client identity can be signed — local harness is deliberate (note this in a code comment).
   - `async fn spawn_tls_server(server_cert_pem, server_key_pem, client_ca_pem: Option<&str>) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>)` — `tokio::net::TcpListener` on 127.0.0.1:0; `rustls::ServerConfig` via `rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(chain, key)` — when `client_ca_pem` is Some: build a `rustls::server::WebPkiClientVerifier` on that CA's root store and use `.with_client_cert_verifier(verifier)` then `.with_single_cert(chain, key)` (server REQUIRES client certs). Accept at most a bounded number of connections (e.g. 4), each: `tokio_rustls::TlsAcceptor.accept` under `tokio::time::timeout`, read request bytes until header-end with a bounded buffer under timeout, write `HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok`; cleanup by aborting the JoinHandle. No `sleep` anywhere; all reads/writes/accepts timeout-bounded.
   - `async fn forced_fallback_env<R>(f: impl FnOnce() -> R) -> R` — CA-store mutex + empty `SSL_CERT_FILE`/`SSL_CERT_DIR` env capture/restore (extract the duplicated pattern from the two existing tests; refactor those two tests to use it, keeping their bodies).
2. Write the handshake tests (below) in `mod tests` (`use crate::tls_harness::*;`). All use `forced_fallback_env` + `build_client(&config, None).expect("client must build") // allow-unwrap(test)`; request via `client.get(format!("https://{addr}/"))` — rcgen server certs carry IP SAN 127.0.0.1 so reqwest hostname verification passes; wrap EVERY client `.send().await` in `tokio::time::timeout` (bounded — a broken harness must fail, not hang); assert `status()`/`text()`.
3. Negative log assertions use the crate's existing `#[traced_test]` pattern (precedent near lib.rs ~12939/~13026) with `logs_contain` / negated `logs_contain` using the concrete substrings named in the test bullets below.

**Tests:** (command `cargo test -p camel-http --lib -- tls_handshake` — every test name below starts `tls_handshake_`; expected: fail before tasks 1.1/1.2 land, pass after)
- `tls_handshake_strict_custom_ca_carried`: forced fallback; `tls{enabled, strict, ca_cert_path=CA}`; server certified by CA (no client-auth requirement) → send → status 200, body "ok".
- `tls_handshake_strict_material_free_fails_same_server`: same env + server; `tls{enabled, strict}` no material → send → Err (handshake failure — Mozilla roots do not trust the rcgen CA); `#[traced_test]` asserting `!logs_contain("client certificate NOT used")` and `!logs_contain("falling back to bundled Mozilla roots")` (material-free strict fallback: no material warns — spec scenario).
- `tls_handshake_strict_mtls_identity_carried`: forced fallback; server REQUIRING client certs (client_ca = the CA); `tls{enabled, strict, ca_cert_path=CA, client_cert_path, client_key_path}` (identity signed by CA) → send → 200 "ok".
- `tls_handshake_strict_mtls_less_client_rejected`: same env + client-cert-requiring server; `tls{enabled, strict, ca_cert_path=CA}` (no identity) → send → Err (server rejects handshake).
- `tls_handshake_nonstrict_valid_ca_carried`: forced fallback; `tls{enabled, strict=false, ca_cert_path=CA}` → send → 200 "ok" (valid material carried regardless of strict); `#[traced_test]` asserting `!logs_contain("client certificate NOT used")` and `!logs_contain("falling back to bundled Mozilla roots")` (no degradation warn for loadable material — spec scenario).

**Acceptance:**
- `cargo test -p camel-http --lib -- tls_handshake` passes (5 tests).
- `cargo fmt --check --all` and `cargo clippy -p camel-http --all-targets -- -D warnings` exit 0.
- `cargo xtask lint-test-sleep`, `cargo xtask lint-unbounded-wait`, `cargo xtask lint-secrets` exit 0 (no sleeps, bounded reads, no embedded private keys).

- [x] 1.3

### Task 1.3-deviation: Forced-fallback seam (e_glm adjudication 2026-09-22, e_gpt quota-exhausted)

The blessed plan's env-window forcing cannot exercise the fallback with
VALID material on Linux: rustls-platform-verifier 0.7.0
(`verification/others.rs:61-105`) adds extra roots FIRST and hard-errors
only when the merged store is EMPTY — a valid configured CA rescues the
primary build under the empty `SSL_CERT_FILE`/`SSL_CERT_DIR` window
(conductor-verified empirically; the fallback-with-valid-material path
is real only on verifier-hard-error platforms: android/apple — the
Termux case). Since bd rc-hl9cn demands forced-fallback handshake tests
for custom CA and mTLS, the ruled amendment (Hybrid A+C):

- **A (seam)**: `enum FallbackTrigger { Platform(reqwest::Error),
  #[cfg(test)] Forced }` threaded into `webpki_fallback_client`
  (reqwest::Error has no public constructor); `#[cfg(test)] thread_local
  FORCE_WEBPKI_FALLBACK` + panic-safe Drop guard (in `tls_harness`);
  `build_client` checks the flag first and enters the fallback directly.
  Release builds identical to today. The four carried/nonstrict
  handshake tests run under the seam with
  `build_client_fallback_count()` delta == 1 as positive path control
  (the strict carried-info log becomes assertable). Task 1.4's parity
  tests (insecure/verify_peer=false) use the seam for the same reason:
  danger mode rescues the primary on every platform.
- **C (genuine)**: `tls_handshake_mtls_identity_only_genuine_fallback`
  — mTLS pair WITHOUT `ca_cert_path` + `verify_peer=false` + strict,
  under the env window with NO seam (no extra roots rescue the
  verifier → genuine primary failure): asserts delta == 1 AND the real
  `Platform` warn (`HTTP client build failed on platform TLS roots`)
  AND a 200 "ok" handshake against a client-cert-requiring server.
  Anchors the seam to real-trigger behavior.
  **Correction (2026-09-22, post-hoc)**: the 200-ok variant is
  architecturally unreachable — reqwest's `!certs_verification` branch
  constructs the NoVerifier verifier and never builds the platform
  verifier, so a verify-off config cannot genuinely fail the primary on
  ANY platform; and every verify-on genuine fallback trusts only
  Mozilla ∪ custom roots, which cannot handshake with a hermetic rcgen
  CA. C is realized as
  `tls_handshake_mtls_identity_only_genuine_fallback_anchor`: strict
  identity WITHOUT `ca_cert_path`, verification ON, under the env
  window with NO seam — genuine delta == 1, the real `Platform` warn,
  and a strict build `Ok` (identity carried, not fail-closed). The
  seam tests carry the handshake-level identity proof (all downstream
  code from `fallback_client_config` onward is shared).
- Existing outcome-contract tests, material-free test, and all 1.1/1.2
  fail-closed tests (genuinely forced via merge-breaking material)
  unchanged. Stage-4 e_opus + dual review re-verify this delta
  explicitly.

### Task 1.4: Verification parity + item-wise degradation + fail-closed end-to-end scenarios

**Files:**
- `crates/components/camel-http/src/tls_harness.rs` (modified — add `pub(crate) fn gen_self_signed_server_cert()`; module stays `#[cfg(test)]`-gated)
- `crates/components/camel-http/src/lib.rs` (modified — test module only)

**Steps:**
1. Self-signed (NOT CA-signed) server certificate helper in `crates/components/camel-http/src/tls_harness.rs`: `fn gen_self_signed_server_cert() -> (cert_pem, key_pem)` — `rcgen::generate_simple_self_signed(vec!["127.0.0.1"])` equivalent with IP SAN 127.0.0.1, signer NOT related to any configured CA.
2. Write the parity + degradation tests below, reusing `tls_harness::{spawn_tls_server, forced_fallback_env, gen_test_ca, gen_leaf_signed_by_ca}` from Task 1.3; wrap EVERY client `.send().await` in `tokio::time::timeout`.
3. Where a warn is asserted, use the existing `#[traced_test]` pattern from this crate (see the precedent near lib.rs ~12939/~13026) and assert on `logs_contain` using the concrete substrings named in the test bullets below.

**Tests:** (command `cargo test -p camel-http --lib -- tls_parity` — expected: fail before 1.1/1.2, pass after)
- `tls_parity_insecure_self_signed_succeeds`: forced fallback; self-signed server; `tls{enabled, strict=false, insecure=true}` → send → 200 "ok" (danger-verifier parity).
- `tls_parity_verify_peer_false_self_signed_succeeds`: same server; `tls{enabled, strict=false, verify_peer=false}` → send → 200 "ok".
- `tls_parity_insecure_strict_bad_material_still_fails_closed`: forced fallback; self-signed server; `tls{enabled, strict=true, insecure=true, ca_cert_path=store-rejected fixture}` → `build_client` → Err with `tls.strict/webpki-fallback` (verification-disable never masks strict material failure).
- `tls_parity_nonstrict_itemwise_valid_ca_bad_identity`: forced fallback; CA-certified server (no client-auth requirement); `tls{enabled, strict=false, ca_cert_path=CA(valid), client_cert_path=valid, client_key_path=/nonexistent/key.pem}` (UNREADABLE key path, per the spec scenario) → send → 200 "ok" (valid CA item carried) AND `#[traced_test]` asserting `logs_contain("client certificate NOT used")` (the mTLS item warn from the builder seam) and `!logs_contain("falling back to bundled Mozilla roots")` (the CA item did NOT degrade).
- `tls_parity_verifying_client_rejects_self_signed_without_insecure`: forced fallback; self-signed server; `tls{enabled, strict=true}` no material → send → Err (control proving the two parity tests pass BECAUSE of the danger verifier, not because the harness skips verification).

**Acceptance:**
- `cargo test -p camel-http --lib -- tls_parity` passes (5 tests).
- `cargo fmt --check --all` and `cargo clippy -p camel-http --all-targets -- -D warnings` exit 0.
- `cargo xtask lint-log-levels`, `cargo xtask lint-log-redaction`, `cargo xtask lint-test-sleep` exit 0.

- [x] 1.4

### Task 1.5: Docs alignment — crate CONTEXT.md TLS section + log-policy appendix

**Files:**
- `crates/components/camel-http/CONTEXT.md` (modified)

**Steps:**
1. In the `### Outbound SSRF and TLS defaults` section (~line 30): after the existing producer-TLS-material sentences, add the fallback contract: on CA-less platforms the webpki fallback carries configured CA (merged with Mozilla anchors) and mTLS material; strict material failures fail closed at endpoint creation with `tls.strict/webpki-fallback:`-prefixed `EndpointCreationFailed`; `insecure`/`verify_peer=false` keep primary-path parity in the fallback via a no-verify verifier; non-strict material failures degrade item-wise with warns (F2-7). Reference bd rc-hl9cn and rc-3j4mq.
2. In the log-policy appendix (~line 121 area): register the new handler-owned log sites (strict carry info note; seam degrade warns) alongside the existing `build_client()` entries — same `(lib.rs, fn, message)` format.
3. Grep-audit for drift: `rg -n "webpki|fallback" crates/components/camel-http/CONTEXT.md` shows the new contract paragraph; `rg -n "not carried" crates/components/camel-http/` returns no matches anywhere.

**Tests:**
- `docs_drift_audit` (manual, no Rust test): the two rg checks in step 3 produce the expected matches/emptiness; `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `cargo xtask lint-log-levels` exits 0.
- The step-3 rg checks verified manually by the worker and quoted in its report.

- [x] 1.5
