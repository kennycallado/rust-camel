# Design: castrict

## Approach

One crate (`camel-http`), one seam: the fallback. The fix builds the
fallback rustls `ClientConfig` FROM configured material when strict
mode demands it, and makes the inability to do so a typed error
instead of a warn.

1. **`fallback_client_config(tls: &TlsConfig) ->
   Result<Option<rustls::ClientConfig>, CamelError>`** (new fn, private;
   callers resolve `config.tls` presence themselves): returns
   `Ok(None)` when `tls.enabled` is false, or when no material is
   configured AND verification is enabled (the plain
   `webpki_root_client_config()` suffices — today's behavior).
   Otherwise it builds a config from the configured material/settings.
   Material handling is ITEM-WISE, mirroring the primary path's
   independent F2-7 load sites (e_gpt round-4 fix 2):
   - **Roots** via an inspectable seam —
     `fn fallback_root_store(custom_ca: Option<&str>, strict: bool) ->
     Result<rustls::RootCertStore, CamelError>`: start from the
     Mozilla webpki anchors; custom CA configured → read + parse PEM
     certs (`rustls_pemfile`), add via
     `RootCertStore::add_parsable_certificates` — TRUST-UNION parity
     with the primary path (`add_root_certificate` merges custom
     roots into the platform set; the Mozilla store is the CA-less
     stand-in). Read failure, zero parseable CERTIFICATE sections, or
     `added == 0`: STRICT → `Err` (fail closed); NON-STRICT → warn +
     Mozilla-only store (permissive F2-7 item downgrade, unchanged).
     The seam is unit-testable: with a one-root custom CA the store
     must contain the bundled Mozilla anchors PLUS the custom root
     (union discrimination, e_gpt re-bless fix 2).
    - **Identity**: mTLS pair configured → parse cert chain + key
     (`rustls_pemfile::private_key`), `with_client_auth_cert`;
     unparseable/absent key or a rustls rejection at client-auth
     configuration (PEM-valid section, no usable key material):
     STRICT → `Err`;
     NON-STRICT → warn + `with_no_client_auth` (identity item
     downgrades alone — a valid CA still rides along, matching
     primary-path independence).
     Cert↔key signature mismatch is NOT detectable at config build
     (rustls, like reqwest's `Identity`, does not verify it) — it
     surfaces at handshake, identical to the primary path (documented
     parity, not a regression).
   - **Verification**: disabled (`tls.insecure || !tls.verify_peer`)
     → install a no-verify `ServerCertVerifier` (rustls
     `client::danger`) REGARDLESS of material outcomes — an item
     failure must never silently re-enable verification (e_gpt
     round-4 fix 2). Mirrors the primary path's
     `danger_accept_invalid_certs`.
   - Provider resolution mirrors `webpki_root_client_config`
     (process-default, else aws-lc-rs default).
   `Err` therefore surfaces ONLY under strict (typed fail-closed
   conflict); non-strict returns `Ok(Some(config))` with whatever
   loaded plus warns for what did not.
2. **`build_client` → `Result<reqwest::Client, CamelError>`**: primary
   build Ok → Ok. Primary failed → fallback path: platform warn
   (unchanged); `fallback_client_config` Ok(Some) → preconfigured
   backend IS that config (material/settings carried; under strict an
   info-level note replaces the old "material not carried" warn);
   Ok(None) → today's material-free webpki config (no material was
   configured, so no material warn applies); Err(e) → STRICT ONLY:
   return `Err` — the typed conflict (non-strict never errors; item
   failures already degraded inside the builder).
   The inner second-build terminal (unreachable on reqwest 0.13.4)
   stays as-is for the Ok branches.
3. **Error folding at constructors** (`HttpComponent`,
   `HttpsComponent` × `new`/`with_config`): on `Err`, store a
   placeholder client built from the plain webpki config
   (no-fallible-stage preconfigured build; never issues a request
   because `create_endpoint` fails first via the folded
   `strict_tls_error: strict_err.or(build_err)` field). rc-3j4mq
   never-panic contract preserved: BASE `new()` uses default config
   (no TLS) → cannot take the Err branch.
4. **Pinned path**: `PinnedClientCache::get_or_build` build closure and
   return type become `Result`; the producer call site maps the error
   into the request error path (fail closed at request time on
   material drift; unreachable when construction already folded the
   error, since no endpoint exists then).
5. **`tls.strict` semantics unchanged** (rc-ayrwk: fail closed on
   material load). No new config-combination rejection is introduced;
   the insecure flag keeps its primary-path meaning and gains fallback
   parity via the danger verifier (design point 1).

Errors are `CamelError::EndpointCreationFailed` with messages
prefix-distinct enough for assertions (`tls.strict:`,
`tls.strict/webpki-fallback:`).

## Affected crates

- `camel-http`: `src/lib.rs` (TLS fallback seam, constructors,
  producer pinned call site), `src/client_cache.rs` (fallible
  `get_or_build`), `src/ssrf.rs` (redirect-hop sibling of the producer
  pinned call site), `Cargo.toml` (dev-dep `rcgen` for handshake tests).
  No other crates; no public API outside camel-http's `pub` items is
  touched (`build_client` is `pub(crate)`).

## Architecture boundaries

Components layer only. No Runtime/DSL/Services/Languages/Functions
changes; no config schema change (`TlsConfig` fields unchanged —
behavior of existing `strict` flag is corrected, per bd rc-hl9cn).
Follows existing decisions: rc-3j4mq (fallback-only, never on healthy
platform), rc-ayrwk (strict = fail closed, eager validation), F2-7
(non-strict permissive degrade stays), ADR-0051 (no credential paths
in logs — error messages carry paths only via the existing
strict-error pattern, which already does; keep consistent).

## Alternatives considered

- **Fail closed always on strict + CA-less platform** (never carry
  material): simplest, but breaks strict users on Termux entirely when
  carrying material works fine — the material does not depend on the
  platform store. Rejected; carry first, fail only when carrying fails.
- **Exclusive custom-CA roots (no Mozilla merge) in strict fallback**:
  invented "operator-pinned exclusive trust" semantics the primary
  path does not have — `add_root_certificate` MERGES into platform
  roots, so exclusive fallback roots would flip trust posture the
  other way (public endpoints would stop working under a
  private-CA-configured component). Rejected (e_gpt spec-bless
  finding 1); merge for parity.
- **Reject `strict + insecure` as a config contradiction**: redefines
  the existing `strict` flag (rc-ayrwk: fail closed on material load)
  beyond its documented meaning — scope creep (e_gpt spec-bless
  finding 2). Rejected in favor of fallback parity: the danger
  verifier honors insecure exactly as the primary path does.
- **`native-tls` backend for the strict fallback**: second TLS stack
  in the dependency tree for one component; workspace is rustls-only
  (reqwest `rustls` feature). Rejected.
- **Eager-only detection (build strict config at construction, skip
  fallback later)**: cannot know at construction whether the primary
  build will fail (platform state at build time); duplicates logic in
  the pinned path anyway. Rejected.

Single-phase change — one coherent slice, no milestone grouping.
