# Proposal: httpxtract — camel-http lib.rs decomposition sweep (mission 272)

## Why

camel-http `src/lib.rs` is 15,760 lines (13.6k when rc-ax1vl was filed;
it has grown since): production server/consumer/TLS machinery, test-support
statics, and an ~11.2k-line inline `#[cfg(test)] mod tests` in one file.
Three P3 mechanicals from prior missions' review findings touch this one
file family; one lease, one worktree, one pass (LIGHT protocol, order:
`.opencode/fleet/orders/272-httpxtract-mission.md`).

## What Changes

1. **rc-ax1vl (hygiene)** — extract the inline `#[cfg(test)] mod tests`
   (lib.rs lines 4498–15760, ~11.2k lines) to sibling `src/lib_tests.rs`
   via the house `#[cfg(test)] #[path = "lib_tests.rs"] mod tests;`
   pattern (precedent: camel-redis `pubsub_tests.rs` / `topology_tests.rs`
   / `consumer_tests.rs`, mission 270 rc-h7304). Pure move, zero behavior.
   The `REGISTRY_TEST_MUTEX` / `CA_STORE_TEST_MUTEX` statics and their
   lock helpers (lib.rs 4187–4224) stay at their declaration site — they
   are crate-root `#[cfg(test)]` items outside `mod tests` today and other
   files (`tls_harness.rs`) reach them via `crate::`. lib.rs lands ~4.5k
   lines. Overlap with rc-sdwhj (ADR-0070 inline probes Tier A) is
   checked: that batch operates on the same test bodies but CONVERTS probe
   patterns; this change is a pure move and lands first, rc-sdwhj follows
   on the new file locations (noted in its bd at park time).
2. **rc-on44c (hygiene)** — extract the TLS fallback cluster from lib.rs
   (~lines 2290–3057: the cfg(test) FORCE seams, `client_builder`,
   `webpki_root_client_config`, `fallback_root_store`, `mozilla_only`,
   `NoVerifyServerCertVerifier`, `fallback_client_config`,
   `emergency_webpki_client`, `fallback_rebuild_failed`,
   `webpki_fallback_client`, `build_client` + its cfg(test) counters,
   `strict_tls_error`, `client_or_emergency`) to sibling `src/tls.rs`,
   re-exporting at the crate root (`pub(crate) use tls::{...}`) so every
   existing `crate::build_client`-style path (ssrf.rs, client_cache.rs,
   tls_harness.rs, tests) keeps compiling untouched — public surface
   byte-identical, all moved items stay crate-internal. The TLS test
   cluster inside `mod tests` (forced-fallback handshake / parity tests
   built on `tls_harness`) moves to sibling `src/tls_tests.rs` as the
   `tls` module's test module. Also backfill the CONTEXT.md log-level
   appendix with the pre-castrict rc-3j4mq/F2-7 sites still missing there
   (fallback-entry warn, system-roots warns, client-certificate-NOT-used
   warns, fallback-rebuild-failed error).
3. **rc-i4yjl (test hygiene)** — the last bare `reqwest::Client::new()`
   test site (`test_create_consumer_rejects_oversized_max_inflight`
   fixture, lib.rs ~8655, in `lib_tests.rs` after change 1) becomes
   `plain_http_test_client()` — the shared builder that serializes
   against `CA_STORE_TEST_MUTEX` (tls_harness.rs), matching its sibling
   fixture `https_consumer_without_tls_cert_errors`. Zero bare test
   client builds remain outside that helper.

## Impact

- Affected code: `crates/components/camel-http/src/lib.rs` (shrinks
  15,760 → ~4.5k), new `src/lib_tests.rs`, `src/tls.rs`,
  `src/tls_tests.rs`, `crates/components/camel-http/CONTEXT.md`.
- No behavior change, no public API change (pure moves + crate-internal
  re-exports; the unchanged `pub` set and the cargo doc gate prove it).
- Risk: mechanical (moves) + one fixture client swap. The
  lint-single-source gate guards against duplicate-source drift on pure
  moves.
