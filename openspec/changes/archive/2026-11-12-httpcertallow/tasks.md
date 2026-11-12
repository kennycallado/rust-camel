# Tasks: httpcertallow

Single-phase change (design.md has no `## Phases` section). Tasks are
ordered; each ends committable. Worktree root for all paths:
`/home/shared/rust-camel-worktrees/httpcertallow`.

## camel-component-http config surface

### Task 1.1: `tls.expired_allowlist` config field, parsing, and fail-closed validation

**Files:**
- `crates/components/camel-http/Cargo.toml` (modified)
- `crates/components/camel-http/src/config.rs` (modified)

**Steps:**
1. In `crates/components/camel-http/Cargo.toml` `[dependencies]`, add:
   `sha2.workspace = true`, `hex.workspace = true`, and
   `rustls-webpki = "0.103"` (the lock already carries 0.103.16 via rustls
   0.23.46; do NOT add it to the workspace `[workspace.dependencies]`).
2. In `config.rs`, extend `TlsConfig` with
   `#[serde(default)] pub expired_allowlist: Vec<ExpiredCertAllowEntry>`
   (keep field order last so existing serde structs stay append-compatible).
   Update BOTH manual impls: `Default for TlsConfig` adds `expired_allowlist: Vec::new()`,
   and the manual `Debug` impl adds
   `.field("expired_allowlist", &self.expired_allowlist)` (fingerprints and
   host names are public certificate material, not secrets — visible is
   correct; the redacted path fields stay redacted).
3. Define in `config.rs`:
   ```rust
   /// One pinned-expiry allowlist entry: an exact DNS name or IP literal,
   /// a declarative port, and the SHA-256 hex of the pinned end-entity DER.
   #[derive(Clone, PartialEq, serde::Deserialize)]
   pub struct ExpiredCertAllowEntry {
       pub host: String,
       pub port: u16,
       pub fingerprint: String,
   }
   ```
   with a manual `Debug` that prints host, port, and fingerprint verbatim
   (public material), following the existing manual-Debug pattern in this file.
4. Add `pub(crate) fn validate_expired_allowlist(tls: &TlsConfig) -> Result<(), CamelError>`
   in `config.rs`, called from `HttpConfig::validate()` AFTER the existing
   checks. Rules, each producing a distinct `CamelError::Config(String)`
   message that names the offending entry index (0-based):
   a. `host` trimmed must be non-empty, contain no whitespace, no `*`, no
      `/`, and not start with `.`; normalize to lowercase for comparison.
      (DNS vs IP is decided at parse time by `std::net::IpAddr::from_str`
      inside the allowlist module — validation here is textual.)
   b. `fingerprint` must start with `sha256:`, and the remainder must be
      exactly 64 ASCII hex chars (accept upper case; do not mutate the
      stored String — normalization happens in the allowlist module).
   b2. `port` must be non-zero (1–65535; the `u16` type already rejects
      >65535 at deserialization, port 0 is rejected here as `Config`).
   c. Duplicate entries (same normalized lowercase host + same port + same
      lowercased fingerprint) → `Config` error.
   d. Conflicts: when `expired_allowlist` is non-empty AND
      (`tls.insecure` is true OR `tls.verify_peer` is false) → `Config`
      error naming both fields; when non-empty AND `tls.enabled` is false →
      `Config` error. An empty list short-circuits: no checks beyond
      iteration (no conflicts raised for an empty list).
5. Call the new validator from `HttpConfig::validate()` so endpoint
   creation (`HttpComponent::create_endpoint` → `self.config.validate()`,
   lib.rs) fails closed on any malformed entry, duplicate, or conflict.
6. Style gates for new code: no `unwrap()` outside tests (`cargo xtask
   lint-unwrap`), no glob imports, one module per `use`, qualify function
   calls with their module where the file mixes multiple modules.

**Tests:** (in `config.rs` `mod tests`, follow existing `TlsConfig` test
style; command for all: `cargo test -p camel-component-http --lib config::`)
- `expired_allowlist_parses_from_yaml`: one valid entry
  `{host: "Backend.Internal", port: 8443, fingerprint: "sha256:<64
  upper-case hex>"}` deserializes into `TlsConfig` through the same serde
  derive the YAML loader uses (existing config tests call
  `toml::from_str` for this — same derive, follow that pattern);
  `validate()` returns Ok (upper-case fingerprint and mixed-case host are
  accepted, normalization is internal).
- `expired_allowlist_rejects_missing_sha256_prefix` → validate Err containing
  "sha256:".
- `expired_allowlist_rejects_short_fingerprint` (63 hex chars) → Err.
- `expired_allowlist_rejects_non_hex_fingerprint` → Err.
- `expired_allowlist_rejects_wildcard_host` (`*.backend.internal`) → Err
  containing "wildcard".
- `expired_allowlist_rejects_empty_host` → Err.
- `expired_allowlist_rejects_host_with_whitespace` (`backend .internal`) →
  Err.
- `expired_allowlist_rejects_out_of_range_port_at_deserialization`: port
  `70000` fails AT DESERIALIZATION (u16 type), not in validate() — assert
  the deserialize call is Err (endpoint creation fails either way; the
  failure just surfaces one layer earlier).
- `expired_allowlist_rejects_zero_port`: port `0` passes deserialization
  but `validate()` → Err (rule b2).
- `expired_allowlist_rejects_duplicate_entries` (two identical after
  normalization) → Err containing "duplicate".
- `expired_allowlist_conflicts_with_insecure` → Err containing both
  "expired_allowlist" and "insecure".
- `expired_allowlist_conflicts_with_verify_peer_false` → Err.
- `expired_allowlist_conflicts_with_tls_disabled` (enabled=false) → Err.
- `empty_expired_allowlist_is_valid_and_conflict_free`: `[]` +
  `insecure=true` → validate Ok (empty list short-circuits; insecure alone
  remains legal as today).
- `tls_config_debug_shows_allowlist`: Debug output contains the host and
  fingerprint (public material) and still redacts `client_key_path`.

**Acceptance:**
- `cargo test -p camel-component-http --lib config::` exits 0 with all new
  tests green.
- `cargo fmt --check -p camel-component-http` exits 0.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-unwrap` exits 0.

- [x] 1.1

## camel-component-http allowlist module

### Task 1.2: `tls_allowlist` module — parsed allowlist, rate-limited warn, wrapper verifier (hermetic unit tests)

**Files:**
- `crates/components/camel-http/src/tls_allowlist.rs` (new)
- `crates/components/camel-http/src/lib.rs` (modified — module declaration
  only)

**Steps:**
1. In `lib.rs`, add `pub(crate) mod tls_allowlist;` next to `mod tls;`
   (keep it `pub(crate)`, not `pub` — internal verifier, design.md
   Architecture boundaries).
2. Create `tls_allowlist.rs` with:
   a. `pub(crate) enum AllowHost { Dns(String), Ip(std::net::IpAddr) }` —
      parsed from the entry host string: `IpAddr` literal if parseable,
      else lowercased DNS name.
   b. `pub(crate) struct AllowKey { host: AllowHost, fingerprint: [u8; 32] }`
   c. `pub(crate) struct ExpiredCertAllowlist` holding the entries
      (`Vec<AllowKey>`; small N, linear scan) with:
      - `pub(crate) fn parse(entries: &[crate::config::ExpiredCertAllowEntry])
        -> Result<Self, CamelError>` — re-runs the same textual rules as
        `config::validate_expired_allowlist` (defense in depth: validate()
        already rejects at endpoint creation; parse must not panic on
        anything that slipped through), hex-decoding the fingerprint with
        `hex::decode_to_slice` into `[u8; 32]`.
      - `pub(crate) fn matches(&self, server_name: &rustls::pki_types::ServerName<'_>,
        end_entity_der: &[u8]) -> bool` — computes
        `sha2::Sha256::digest(end_entity_der)` (use `use sha2::{Digest, Sha256};`,
        digest compared as slice), maps `server_name` to the same
        `AllowHost` form (`ServerName::DnsName` → lowercased
        `String` from the ascii `rustls::pki_types::ServerName::to_str()`
        — verify the accessor exists on the pinned rustls version, else use
        `as_name()`/pattern match on `DnsName(DnsNameRef)`; IP variant →
        `AllowHost::Ip`), and returns true iff BOTH host and fingerprint
        match an entry. Exact match only — no suffix, no wildcard logic.
   d. `pub(crate) const WARN_INTERVAL: std::time::Duration =
        Duration::from_secs(60)` and
      `pub(crate) struct WarnRateLimiter { interval: Duration, seen:
        std::sync::Mutex<std::collections::HashMap<RateKey,
        std::time::Instant>> }` where
      `RateKey` is a fully owned `(String, [u8; 32])` (lowercased host
      string or IP string, fingerprint bytes) implementing `Eq` + `Hash`.
      Constructor `with_interval(Duration)` for tests; `Default` uses
      `WARN_INTERVAL`. Method
      `pub(crate) fn allows(&mut self, key: &RateKey) -> bool` — inserts
      and returns true on first sighting, false while within `interval`.
      NOTE: with a single-shot HashSet keyed only by (host, fingerprint)
      the interval window is the client lifetime — implement it as
      `HashMap<RateKey, std::time::Instant>` with
      `elapsed >= interval` eviction/re-allow so the injected-interval test
      is meaningful.
   e. `pub(crate) struct ExpiredCertAllowVerifier { inner:
        std::sync::Arc<dyn rustls::client::danger::ServerCertVerifier>,
        allowlist: std::sync::Arc<ExpiredCertAllowlist>, limiter:
        std::sync::Mutex<WarnRateLimiter> }` with a constructor taking the
        three inputs (test interval injectable through `WarnRateLimiter::with_interval`).
      Implement `rustls::client::danger::ServerCertVerifier`:
      - `verify_server_cert`: call `self.inner.verify_server_cert` with the
        exact arguments the wrapper received;
        on `Ok` return it. On `Err(rustls::Error::InvalidCertificate(ref ce))`
        where `ce` matches
        `rustls::error::CertificateError::Expired { .. }` OR
        `rustls::error::CertificateError::ExpiredContext { .. }`
        (NOTE the path: `rustls::error::CertificateError`, NOT
        `rustls::pki_types` — verified against rustls 0.23.46
        error.rs:374, `#[non_exhaustive]`; the variants
        `Expired`, `ExpiredContext { time, not_after }`,
        `NotValidYet`/`NotValidYetContext`, and the issuer variants all
        live there; rustls maps webpki `CertExpired` → `ExpiredContext`):
        FIRST independently re-verify hostname via
        `rustls_webpki::EndEntityCert::try_from(end_entity.as_ref())` then
        `.verify_is_valid_for_subject_name(server_name)` — on re-check
        failure return the ORIGINAL error unchanged. Then, if
        `self.allowlist.matches(server_name, end_entity.as_ref())`: lock the
        limiter, and on `allows(&rate_key)` emit
        `tracing::warn!(host = %camel_api::redact::redact_host(&host_display),
        fingerprint = %hex_fp,
        "accepted expired TLS certificate — allowlist entry matched")`
        — the `host` field name is sensitive to `cargo xtask
        lint-log-redaction`, so it MUST go through `redact_host`
        (precedent: lib.rs:1239; verify the exact fn path
        `camel_api::redact::redact_host` in the pinned camel-api before
        use; no secret material otherwise) and return
        `Ok(rustls::client::danger::ServerCertVerified::assertion())`;
        otherwise return the ORIGINAL error unchanged. ALL other errors
        (the `#[non_exhaustive]` catch-all, including
        `NotValidYet`/`NotValidYetContext`, unknown issuer, revocation)
        return the original error unchanged — match via a catch-all arm.
      - `verify_tls12_signature`, `verify_tls13_signature`,
        `supported_verify_schemes`: delegate to `self.inner` verbatim.
   f. Crate-doc comment on the module: enforcement is host + fingerprint;
      port is declarative (rustls verifier API carries no port); expired
      INTERMEDIATE with the pinned leaf presented is tolerated; the
      re-verified hostname keeps hostname verification independent of the
      webpki check order (chain/validity run before hostname).
3. Doc comments on every `pub(crate)` item (crate is
   `lint-missing-docs-ratchet`-gated); no intra-doc links to private items.

**Tests:** (in a `#[cfg(test)] mod tests` at the bottom of
`tls_allowlist.rs`; rcgen is a dev-dep; command:
`cargo test -p camel-component-http --lib tls_allowlist::`)
Helper (shared, `#[cfg(test)]`): `gen_test_leaf(dns_san: Option<&str>,
ip_san: Option<std::net::IpAddr>, not_before: rcgen::date_time_ymd(2024,1,1),
not_after: rcgen::date_time_ymd(2024,1,2))` building `rcgen::CertificateParams`
with the given SANs and self-signed with a freshly generated
`rcgen::KeyPair` (call `self_signed` and expect success as in the existing
harness helpers),
returning `(rcgen::Certificate, String /* pem */, String /* key pem */)`.
Derive fingerprint via `sha2` over `cert.der().as_ref()`, hex-encode with
`hex`.
- `allowlist_matches_ip_server_name_and_fingerprint`: expired leaf with
  IP SAN 127.0.0.1; allowlist entry host "127.0.0.1" + its fingerprint;
  `matches(&ServerName::try_from("127.0.0.1").unwrap(test), der)` → true;
  same call with a one-byte-different fingerprint → false.
- `allowlist_matches_dns_name_case_insensitively`: expired leaf with DNS
  SAN `backend.internal`; entry host `BACKEND.Internal`; ServerName
  "backend.internal" → true; ServerName "sub.backend.internal" → false
  (exact host, no subdomain).
- `rate_limiter_suppresses_within_window`: `WarnRateLimiter::with_interval(
  Duration::from_secs(3600))`; first `allows` → true, second same key →
  false, different key → true (no sleeps — the 1 h window cannot elapse).
- `rate_limiter_reallows_after_interval`: interval `Duration::ZERO`; same
  key twice → true, true.
- `wrapper_accepts_expired_listed_cert`: stub inner verifier (local test
  type implementing `ServerCertVerifier`, returning
  `Err(rustls::Error::InvalidCertificate(CertificateError::ExpiredContext {
  .. }))` with the variant's real fields filled from the pinned
  rustls/pki-types source) + expired IP-SAN leaf + matching allowlist +
  limiter interval 1 h; calling the wrapper's `verify_server_cert` →
  `Ok(ServerCertVerified::assertion())`. This test also owns the spec
  scenario "Expired intermediate with pinned leaf is tolerated":
  `ExpiredContext` is exactly the variant rustls emits for an expired
  certificate on the presented path — pre-flight verified rustls 0.23.46
  maps webpki `CertExpired` (chain-level expiry, leaf OR intermediate) to
  `ExpiredContext` (`webpki/mod.rs:66-69`) — so tolerating it pins that
  semantics end-to-end.
- `wrapper_rejects_expired_cert_when_hostname_mismatches`: stub as above;
  leaf has ONLY DNS SAN `backend.internal`; server_name is IP
  `127.0.0.1`; allowlist entry pins "127.0.0.1" + that fingerprint → Err
  equal to the inner error (hostname re-check fired).
- `wrapper_rejects_expired_cert_with_wrong_fingerprint`: listed host,
  unlisted fingerprint → Err.
- `wrapper_propagates_not_valid_yet`: stub returns
  `NotValidYetContext` → Err unchanged (catch-all arm, not tolerated).
- `wrapper_propagates_unknown_issuer`: stub returns
  `CertificateError::UnknownIssuer` (or the variant the version carries —
  verify) → Err unchanged.

**Acceptance:**
- `cargo test -p camel-component-http --lib tls_allowlist::` exits 0.
- `cargo fmt --check -p camel-component-http` exits 0.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-http --no-deps`
  exits 0.

- [x] 1.2

## camel-component-http client build path

### Task 1.3: Build-path wiring — allowlist forces the preconfigured backend; integration tests through the loopback TLS harness

**Files:**
- `crates/components/camel-http/src/tls.rs` (modified)
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/src/tls_tests.rs` (modified — call-site
  arity only)
- `crates/components/camel-http/src/tls_allowlist_tests.rs` (new)
- `crates/components/camel-http/src/lib.rs` (modified — test module decl)

**Steps:**
1. In `tls.rs`:
   a. Add `pub(crate) fn allowlist_active(config: &HttpConfig) -> bool` —
      true iff `config.tls` is Some, `tls.enabled`, verification is on
      (`!tls.insecure && tls.verify_peer`), and
      `!tls.expired_allowlist.is_empty()`. (Conflicts are already
      rejected at validate(); this predicate is the build-path gate.)
   b. Extend `fallback_client_config(tls)` to
      `fallback_client_config(tls, allowlist: Option<&std::sync::Arc<crate::tls_allowlist::ExpiredCertAllowlist>>)`:
      when `allowlist` is Some (and verification is on), build the rustls
      config with `fallback_root_store(tls.ca_cert_path.as_deref(),
      tls.strict)?` roots and install
      `.dangerous().with_custom_certificate_verifier(std::sync::Arc::new(
      crate::tls_allowlist::ExpiredCertAllowVerifier::new(
      std::sync::Arc::new(rustls::client::WebPkiServerVerifier::builder(
      std::sync::Arc::new(store)).build().expect("webpki server verifier
      builds from a valid root store"), // allow-unwrap: valid store
      allowlist.clone(), limiter)))` instead of the plain
      `with_root_certificates(store)` arm. Treat a Some allowlist like
      material for the `Ok(None)` early-return (never return `Ok(None)`
      when an allowlist must be installed). Preserve ALL existing
      insecure/mTLS/strict arms byte-for-byte in behavior (the `insecure`
      arm keeps `NoVerifyServerCertVerifier`; an allowlist reaching this
      function alongside insecure cannot happen post-validate but the
      insecure branch keeps priority). Keep the existing
      `webpki_fallback_client` call sites compiling by passing the
      allowlist through from config.
      IMPORTANT: `tls_tests.rs` calls the single-arg
      `fallback_client_config(tls)` directly at ~8 sites (around lines
      237, 250, 275, 305, 323, and successors) — update EVERY call site to
      pass `None` as the new second argument (their scenarios are
      allowlist-inactive and must keep identical behavior; grep
      `fallback_client_config(` in tls_tests.rs to catch all sites).
      Inner-verifier construction note: `rustls::client::WebPkiServerVerifier::builder`
      takes `Arc<RootCertStore>`; build() returns
      `Result<Arc<WebPkiServerVerifier>, VerifierBuilderError>` — an
      empty-roots store still builds, so the expect is about programmer
      error only.
   c. In `build_client`: AFTER the existing `FORCE_WEBPKI_FALLBACK` test
      seam block (tls.rs `#[cfg(test)] if FORCE_WEBPKI_FALLBACK...` — keep
      the seam's forced-fallback semantics intact: the seam wins), and
      BEFORE the platform-verifier attempt, if
      `allowlist_active(config)` call a new
      `pub(crate) fn allowlist_client(config, resolve_override) ->
      Result<reqwest::Client, CamelError>` that: parses the allowlist once
      (`ExpiredCertAllowlist::parse` over the configured entries — Err here
      means validate() was bypassed; fail closed with
      `CamelError::EndpointCreationFailed`),
      logs ONE `tracing::info!("expired-certificate allowlist active ({} pinned
      entries) — using bundled-root verifier (platform verifier bypassed)",
      n)` (log-policy: handler-owned), resolves the rustls config via
      `fallback_client_config(tls, Some(&allowlist))` (expect Some), and
      builds via `client_builder(config, resolve_override)
      .tls_backend_preconfigured(rustls_config).build()` mapping a build
      error through the existing `fallback_rebuild_failed(config, &err)`
      terminal (strict → typed Err; non-strict → emergency client, which
      carries NO wrapper — expired certs fail there: fails safe, matches
      design.md). Do NOT route through `webpki_fallback_client` (its
      platform-CA-store warn would lie about the trigger).
   d. `webpki_fallback_client` keeps its behavior for the
      allowlist-inactive case; for a CA-less platform WITH allowlist the
      fallback re-enters the same wrapper via the extended
      `fallback_client_config` (pass the parsed allowlist through: parse
      inside both paths from `config.tls`; a parse Err inside
      `webpki_fallback_client`'s path is fail-closed the SAME way as 1.3c —
      `CamelError::EndpointCreationFailed` naming the malformed entry — so
      NO allowlist config can silently build a wrapper-free client).
   e. `client_or_emergency` and `strict_tls_error` need NO allowlist
      changes (strict material checks unchanged; allowlist validation
      lives in `HttpConfig::validate()`).
2. In `lib.rs`: add `#[cfg(test)] #[path = "tls_allowlist_tests.rs"] mod
   tls_allowlist_tests;` next to the `lib_tests.rs` block.
3. Create `tls_allowlist_tests.rs` following `tls_tests.rs` structure:
   explicit imports from the harness (`use crate::tls_harness::{gen_test_ca,
   gen_leaf_signed_by_ca, gen_expired_leaf_signed_by_ca, gen_self_signed_server_cert,
   spawn_tls_server, HARNESS_OK_REPLY};` — enumerate every name used, NO
   glob import: `lint-glob-imports` is a gate), `tracing-test` for the WARN
   capture, `tempfile` for CA/key files, `EnvGuard::acquire()` around any
   plain client build. Server certs:
   - expired+listed: `gen_test_ca()` + `gen_leaf_signed_by_ca` variant —
     EXTEND `tls_harness.rs` with `pub(crate) fn gen_expired_leaf_signed_by_ca(ca:
     &TestCa, san: rcgen::SanType) -> (String, String)` building
     `CertificateParams` with `not_before = rcgen::date_time_ymd(2024,1,1)`,
     `not_after = rcgen::date_time_ymd(2024,1,2)` (always in the past) and
     the given SAN — do NOT modify the existing fns.
   - hostname-mismatch leaf: expired leaf with ONLY
     `rcgen::SanType::DnsName(rcgen::Ia5String::try_from("backend.internal")
     .expect("dns san is ia5 string"))` — `DnsName` holds an `Ia5String`
     (rcgen 0.14.10), a plain `.into()` does NOT compile; use
     `try_into`/`Ia5String::try_from` with an expect reason, reached over IP.
   - Config assembly: `HttpConfig { tls: Some(TlsConfig { enabled: true,
     expired_allowlist: vec![entry], ..Default::default() }), allow_internal:
     true, ..Default::default() }`; entry host "127.0.0.1", port = the
     bound port (declarative), fingerprint = hex of sha256 over the leaf
     `der()` — the test computes it from the SAME `TestCa` leaf it hands
     the server.
   - Client under test: `crate::build_client(&config, None)` (the
     allowlist short-circuit fires without any seam; assert one
     `tracing::info!` capture contains "allowlist active" via
     `tracing_test` when convenient).
   - Request: `client.get(format!("https://127.0.0.1:{port}/"))` — assert
     `.send().await` Ok vs Err per scenario; for Err cases assert the
     error is a connect/TLS error (reqwest `is_connect()` or message
     contains "certificate"/"invalid"), NOT a timeout.
   - Endpoint-creation conflicts: build `HttpsComponent::with_config(cfg)`
     and `.create_endpoint("https://127.0.0.1:8443/api",
     &camel_component_api::NoOpComponentContext)` (exported at the
     camel-component-api crate root, same import `lib_tests.rs` uses);
     assert Err with the conflict message. (validate() fires inside
     create_endpoint.)
4. Style gates as in Task 1.1; no `free_port()`, no sleeps
   (`cargo xtask lint-no-free-port`, `cargo xtask lint-test-sleep`).

**Tests:** (all in `tls_allowlist_tests.rs`; command:
`cargo test -p camel-component-http --lib tls_allowlist_tests::` — tokio
tests use `#[tokio::test]` like `tls_tests.rs`)
- `expired_cert_on_listed_host_handshakes`: expired CA-signed leaf with IP
  SAN 127.0.0.1 served by `spawn_tls_server`; config pins
  (127.0.0.1, port, sha256-of-leaf) → request succeeds (HTTP 200 body
  "ok" from `HARNESS_OK_REPLY`).
- `expired_cert_on_unlisted_host_rejected`: same server; allowlist pins a
  different host (`example.internal`) → request fails with connect/TLS
  error.
- `expired_cert_with_wrong_fingerprint_rejected`: listed host, fingerprint
  of a DIFFERENT leaf → fails.
- `expired_cert_hostname_mismatch_rejected_even_when_listed`: expired
  DNS-SAN-only leaf (`backend.internal`), allowlist pins
  (127.0.0.1, port, its fingerprint) → fails (proves the wrapper's
  hostname re-check end-to-end).
- `unknown_issuer_still_fails_on_listed_host`: UNEXPIRED self-signed leaf
  (`gen_self_signed_server_cert`, unknown issuer) + allowlist pinning
  (127.0.0.1, port, its fingerprint) → fails (only expiry errors are
  tolerated).
- `cert_chained_to_unbundled_ca_fails_on_allowlist_path` (owns the
  spec scenario "Certificate rooted only in the OS trust store fails on
  the allowlist path" without mutating the machine trust store): UNEXPIRED
  leaf signed by a rcgen CA that is neither in the bundled Mozilla set nor
  configured as `tls.ca_cert_path`, plus an allowlist entry pinning that
  host, port, and the leaf's fingerprint → fails with unknown issuer —
  a chain that is valid under SOME store but absent from bundled roots +
  configured CA is rejected on the allowlist path.
- `valid_cert_unaffected_by_allowlist`: VALID CA-signed leaf
  (`gen_leaf_signed_by_ca`, IP SAN) served; CA written to tempdir and set
  as `tls.ca_cert_path`; allowlist also configured → succeeds.
- `allowlist_unset_keeps_current_behavior`: expired leaf, no allowlist →
  request fails (identical class as the unlisted case).
- `empty_allowlist_list_is_inert`: expired leaf, `expired_allowlist: []` →
  request fails exactly as unset.
- `insecure_allowlist_conflict_fails_endpoint_creation`:
  `HttpsComponent::with_config(cfg)` with `insecure=true` + one entry →
  `create_endpoint` Err containing "expired_allowlist".
- `allowlist_does_not_leak_across_components`: two components — one with
  the allowlist, one without — against the same expired-leaf server →
  allowlisted request succeeds, non-allowlisted fails.
- `acceptance_warn_is_rate_limited`: `tracing-test` capture; two
  successful requests through the SAME client → exactly ONE warn line
  containing "accepted expired TLS certificate" and the hex fingerprint.
  Non-vacuous by construction: every `HARNESS_OK_REPLY` carries
  `Connection: close` and the harness shuts the stream down, so each
  request forces a FRESH TLS handshake (no keep-alive reuse can hide the
  second warn); `HARNESS_MAX_CONNECTIONS` (4) covers both connections.

**Acceptance:**
- `cargo test -p camel-component-http --lib tls_allowlist_tests::` exits 0.
- `cargo test -p camel-component-http --lib` exits 0 (whole crate green).
- `cargo fmt --check -p camel-component-http` exits 0.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings`
  exits 0.
- `cargo xtask lint-no-free-port && cargo xtask lint-test-sleep` exit 0.

- [x] 1.3

## docs

### Task 1.4: Document `tls.expired_allowlist` in the HTTP component TLS section

**Files:**
- `docs/src/components/http.md` (modified)

**Steps:**
1. Locate the TLS section of `docs/src/components/http.md` (the
  `tls.insecure` documentation around line 77).
2. Add a subsection `### Expired-certificate allowlist (per-host pinned)`
  documenting:
   - the YAML shape: `tls.expired_allowlist: [{host, port, fingerprint}]`
     with `fingerprint: "sha256:<64 hex chars>"`; exact host (no
     wildcard/subdomain), normalized case; duplicates rejected.
   - enforcement semantics: ONLY expiry errors are tolerated; hostname is
     independently re-verified; every other error still fails on a listed
     host; `port` is declarative (validation/dedup) — the TLS verifier
     layer has no port, so enforcement is host + fingerprint.
   - the platform-trust divergence: the allowlist path uses the bundled
     Mozilla roots plus `tls.ca_cert_path` and does NOT consult OS trust
     settings; operators with OS-store-issued chains must add the CA via
     `tls.ca_cert_path`.
   - the expired-intermediate tolerance (documented semantics).
   - the rate-limited WARN per accepted certificate (host + fingerprint).
   - a recommendation: prefer the allowlist over `tls.insecure` /
     `verify_peer=false` for the single-expired-backend case, and state
     the allowlist's own risk (pins trust to that specific certificate —
     only pin certificates the operator controls or has verified).
   - the conflicts that fail endpoint creation (`insecure`,
     `verify_peer=false`, `enabled=false`, malformed/duplicate entries).
3. Language-policy: English prose; identifiers verbatim; ASD-STE100-ish
   terse sentences (match the surrounding docs register).

**Tests:** (non-Rust task; verification is build + grep)
- `docs build`: `cd docs && mdbook build` exits 0 (no broken links
  introduced; `mdbook test` is the CI gate for code fences — run
  `cd docs && mdbook test` if the section contains fenced code).
- `grep -n "expired_allowlist" docs/src/components/http.md` shows the new
  subsection.

**Acceptance:**
- `cd docs && mdbook build` exits 0.
- The subsection covers all six content bullets from Steps 2.

- [x] 1.4
