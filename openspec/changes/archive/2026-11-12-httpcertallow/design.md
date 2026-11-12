# Design: httpcertallow

## Approach

New module `tls_allowlist.rs` in camel-http owns three things: the parsed
allowlist, its validation, and the wrapper verifier.

**Config shape (canonical, YAML only).** `TlsConfig` gains
`expired_allowlist: Vec<ExpiredCertAllowEntry>` (`#[serde(default)]`), each
entry `{ host, port, fingerprint }`. `fingerprint` MUST be
`sha256:` + exactly 64 hex chars (upper case accepted, normalized). `host`
MUST be a bare DNS name or IP literal — no wildcard `*`, no leading dot, no
whitespace, normalized lowercase; `port` is `u16`. `HttpConfig::validate()`
runs at endpoint creation: empty list = no allowlist (default unchanged);
malformed entry, duplicate entry (same normalized host+port+fingerprint), or
a non-empty allowlist combined with `insecure`, `verify_peer=false`, or
`enabled=false` → `CamelError::Config` (precedent: the proxy_url-vs-SSRF
conflict in validate()). No URI parameter form: one canonical shape, no
duplicate parsers (ADR-0032 — never from exchange headers).

**Enforcement dimension.** rustls' `ServerCertVerifier::verify_server_cert`
receives only `ServerName` — no port (verified against rustls 0.23.46
sources, pre-flight e_gpt). Enforcement is therefore **host + fingerprint**;
`port` is declarative (config clarity, dedup). This is stated in the spec
and docs: the fingerprint pins the exact end-entity key, so a copy of the
pinned certificate served on a sibling port is the same trust the operator
already pinned.

**Verifier integration.** When the allowlist is active and verification is
on, the client is ALWAYS built via
`reqwest::ClientBuilder::tls_backend_preconfigured(rustls::ClientConfig)`:
reqwest 0.13.5 exposes no custom-verifier hook on the platform-verifier
primary path, so there is no way to wrap it. The rustls config carries:

- roots = bundled Mozilla anchors UNION configured `tls.ca_cert_path`
  (reuse `fallback_root_store`, strict semantics preserved);
- mTLS identity via the existing `fallback_client_config` assembly,
  extended with an allowlist parameter;
- verifier = `ExpiredCertAllowVerifier` wrapping
  `rustls::client::WebPkiServerVerifier`, installed with
  `dangerous().with_custom_certificate_verifier`.

Platform OS trust settings do not apply on this path (documented; corporate
MITM roots need `tls.ca_cert_path`). The webpki fallback path carries the
same wrapper; `build_client` short-circuits to the allowlist build BEFORE
attempting the platform verifier (no "platform CA store unavailable" warn —
this is a deliberate backend choice, logged once at INFO). The emergency
degrade client (TLS stack broken process-wide) has no wrapper: expired
certificates fail there — fails safe.

**Wrapper logic.** `verify_server_cert`: call the inner webpki verifier.
`Ok` → accept. `Err(InvalidCertificate(Expired | ExpiredContext { .. }))`
→ (1) re-verify hostname INDEPENDENTLY via
`rustls_webpki::EndEntityCert::verify_is_valid_for_subject_name(server_name)`
— webpki checks chain/validity before hostname, so an expired cert with a
wrong hostname surfaces Expired first and the re-check is what keeps
hostname verification unweakened; (2) match `sha256(end_entity_der)` and
lowercased DNS/IP `ServerName` against the entry set (exact match only); on
both passing, emit the rate-limited WARN and accept, else propagate the
ORIGINAL error. All other errors — `#[non_exhaustive]` catch-all — propagate
untouched: unknown issuer, bad signature, revoked, hostname mismatch,
NotValidYet. Signature verification and `supported_verify_schemes` delegate
to the inner verifier.

**Logging.** Acceptance: one WARN per `(host, fingerprint)` per injected
interval (default 60 s; `Mutex<HashMap>` inside the verifier, bounded by the
allowlist size) with redacted host (`camel_api::redact::redact_host` —
`host` is a sensitive field name under `lint-log-redaction`) and the
`sha256:<hex>` fingerprint, no secret material. No new
rejection logging (rejections surface through the existing connect-error
class). Expired INTERMEDIATE with the pinned leaf presented → also Expired
→ tolerated; documented semantics, not an accident.

**Error taxonomy.** No new error type. Expired-but-unlisted propagates the
inner verifier error through reqwest's existing TLS/connect error class.

**Cache interaction.** The allowlist is parsed from component
`HttpConfig.tls` and baked into the `reqwest::Client` at component
construction (`build_client`); the SSRF `PinnedClientCache` builds its
DNS-pinned clients from the same config, so they carry the same allowlist.
A different component instance builds its own client from its own config —
no cross-endpoint leak by construction.

## Affected crates

- `camel-component-http`: `config.rs` (field + validation), new
  `tls_allowlist.rs`, `tls.rs` (build path wiring), `lib.rs` (module wiring,
  re-exports), `Cargo.toml` (`sha2`, `rustls-webpki` direct deps — both
  already in Cargo.lock via rustls 0.23.46 / existing workspace member).
- `docs/src/components/http.md` TLS section + changelog entry.

## Architecture boundaries

Components-only change. No Runtime, DSL, Services, Languages, or Functions
surface touched; `HttpConfig` is component-level config and the verifier is
internal to camel-http (not re-exported publicly beyond `pub(crate)`).

## Docs content (binding)

`docs/src/components/http.md` TLS section SHALL: (a) recommend
`tls.expired_allowlist` over `tls.insecure`/`verify_peer=false` for the
expired-backend-cert case and state the allowlist's own risk (pinned cert
must still be the certificate the operator trusts; host+port+fingerprint is
scoped, `insecure` is endpoint-wide); (b) state the platform-trust
divergence — the allowlist path uses the bundled Mozilla roots plus
`tls.ca_cert_path` and excludes OS trust settings; (c) state the
expired-intermediate tolerance (expiry errors anywhere on the presented
path are tolerated while the pinned leaf is presented). A changelog entry
SHOULD accompany it.

## Tests

Executable, reusing `tls_harness.rs` (loopback 127.0.0.1:0 — no
`free_port()` TOCTOU, no sleeps; `lint-no-free-port`, `lint-test-sleep`).
Expired certificates via rcgen 0.14 `not_before`/`not_after`
(`date_time_ymd(2024,1,1)`/`(2024,1,2)` — always in the past, deterministic).
Cases map 1:1 to the spec scenarios: expired+listed accepted; unlisted host,
wrong fingerprint, hostname mismatch (DNS-SAN-only leaf reached over IP),
unknown-issuer on a listed host → rejected; valid certs unaffected;
malformed/duplicate/conflict configs → endpoint creation error; allowlist
unset → current behavior; rate-limiter suppression without sleeping (1 h
injected interval).
