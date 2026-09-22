# Tasks: rootorder

## camel-component-http

### Task 1.1: Custom-first fallback root store + order-asserting union test

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. In `fallback_root_store` (currently lines 2469-2529): replace the
   `let mut store = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };`
   initialization with `let mut store = rustls::RootCertStore::empty();`
   and a closure-or-helper `mozilla_only()` that returns
   `rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() }`.
2. Rewrite ALL FOUR early returns — the no-CA `else` arm AND the
   three non-strict degrade arms (unreadable-file, zero-PEM,
   zero-roots-accepted) — to `return Ok(mozilla_only());`. With the
   new empty-store init, a missed rewrite (especially the no-CA arm)
   would return an EMPTY store and break every handshake. Strict
   error arms are UNCHANGED (same
   `CamelError::EndpointCreationFailed` messages).
3. After the existing `let (added, _ignored) = store.add_parsable_certificates(certs);`
   success path (added > 0), append the Mozilla anchors:
   `store.roots.extend_from_slice(webpki_roots::TLS_SERVER_ROOTS);`
   then `Ok(store)`.
4. Update the `fallback_root_store` doc comment (lines 2463-2468):
   state the order contract — accepted custom anchors first, then
   the bundled Mozilla anchors, parity with the primary
   `add_root_certificate` path (rustls-platform-verifier 0.7.0
   `src/verification/others.rs:61-93` adds extra roots before
   native certs); degrade paths are Mozilla-only.
5. Strengthen `test_fallback_root_store_union_custom_and_mozilla`
   (line 14301): keep the count assertion; read the CA PEM back
   from `ca_path`, parse the single `CertificateDer` via
   `rustls_pemfile::certs`, rebuild the expected anchor with a
   scratch `rustls::RootCertStore::empty()` +
   `add_parsable_certificates(vec![der])` (same construction the
   production store uses), then assert (a)
   `store.roots.first() == Some(&expected_anchor)` — custom anchor
   at index 0, (b) `store.roots[1..] == *webpki_roots::TLS_SERVER_ROOTS`
   (TrustAnchor derives PartialEq/Eq, pki-types 1.15.1 lib.rs:509) —
   exact Mozilla tail, (c) the mission's explicit order assertion:
   `position` of first anchor equal to `expected_anchor` <
   `position` of first anchor contained in
   `webpki_roots::TLS_SERVER_ROOTS`.
6. Add the test doc comment citing BOTH semantics, per the mission:
   order is hygiene-only for verification — rustls 0.23.45
   `src/webpki/verify.rs:245-263` passes `&roots.roots` in store
   order to webpki, and rustls-webpki 0.103.15
   `src/verify_cert.rs:66-73` + `loop_while_non_fatal_error`
   (`:757-774`) returns on the FIRST anchor that validates and
   continues past non-fatal misses, so accept/reject cannot depend
   on order — but custom-first is pinned for parity with the
   primary path (rpv 0.7.0 `verification/others.rs:81-93`).

**Tests:** (executable spec — name, arrange, act, assert)
- `test_fallback_root_store_union_custom_and_mozilla` (strengthened,
  existing name): arrange `fallback_test_ca()` one-root CA → act
  `fallback_root_store(Some(&ca_path), true)` → assert store len ==
  `TLS_SERVER_ROOTS.len() + 1`, custom anchor at index 0 equals the
  expected anchor, tail `[1..]` equals `TLS_SERVER_ROOTS`, and
  first-custom position < first-Mozilla position.
  command: `cargo test -p camel-component-http --lib test_fallback_root_store_union_custom_and_mozilla`
  expected: fails on current code (custom anchor is LAST, tail
  mismatch), passes after the reorder.
- `test_fallback_root_store_no_ca_is_mozilla_only` (existing,
  regression): arrange no CA → act `fallback_root_store(None, true)`
  → assert len == `TLS_SERVER_ROOTS.len()` and
  `store.roots == *webpki_roots::TLS_SERVER_ROOTS` (strengthen with
  the exact-equality assert; on current and new code the whole store
  IS the Mozilla bundle).
  command: `cargo test -p camel-component-http --lib test_fallback_root_store_no_ca_is_mozilla_only`
  expected: passes before and after (degrade path unchanged).
- Existing strict fail-closed and degrade tests
  (`test_fallback_root_store_strict_unreadable_ca_fails_closed`,
  `test_fallback_root_store_strict_zero_pem_sections_fails_closed`,
  `test_fallback_root_store_strict_zero_roots_accepted_fails_closed`,
  `test_fallback_root_store_nonstrict_bad_ca_degrades_to_mozilla`)
  and the forced-fallback handshake suite: rerun via the full
  `cargo test -p camel-component-http --lib fallback` filter —
  all green, no source edits to them beyond what step 2 requires.

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0 (full crate
  suite — covers all 15 scenarios of the MODIFIED requirement,
  including the 14 unchanged scenarios via their existing tests).
- `cargo fmt --check --all` exits 0 (run from the crate dir).
- `cargo clippy -p camel-component-http --all-features -- -D warnings`
  exits 0 (matches the CI workspace gate's coverage for this crate,
  including the `otel` feature).
- Diff touches only `crates/components/camel-http/src/lib.rs`, only
  `fallback_root_store` + its doc + the two union/no-CA tests +
  the new test doc comment.
- No new `unwrap()`/`expect()` outside `// allow-unwrap(test)`
  annotated test lines (project lint-unwrap convention).

- [x] 1.1
