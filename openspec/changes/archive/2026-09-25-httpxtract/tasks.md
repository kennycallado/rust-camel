# Tasks: httpxtract (mission 272, LIGHT)

Execution order is fixed: 1 → 2 → 3 (order 272: harness out first, then
TLS cluster, then client builder). Each task ends green (compile + tests
+ fmt + clippy) before the next starts. One commit per task, each with
its Bd: footer.

## 1. Extract lib.rs test harness to sibling file (rc-ax1vl)

- [x] 1.1 Move the inline `#[cfg(test)] mod tests { ... }` block
  (lib.rs lines 4498–15760, ~11.2k lines) to sibling
  `src/lib_tests.rs` following the house `#[path]` pattern
  (camel-redis `pubsub_tests.rs`, mission 270 rc-h7304): header doc
  comment stating what the module is, contents dedented one level,
  `use super::*;` / `use crate::...` imports preserved verbatim, and
  `#[cfg(test)] #[path = "lib_tests.rs"] mod tests;` left at the
  original declaration site in lib.rs. The `#[cfg(test)]` statics
  `REGISTRY_TEST_MUTEX`, `lock_registry_test_mutex`,
  `CA_STORE_TEST_MUTEX`, `lock_ca_store_test_mutex` (lib.rs 4187–4224)
  STAY in lib.rs — they are crate-root items other modules reach via
  `crate::`. Pure move, zero behavior. lib.rs lands ~4.5k lines —
  record the exact final count. Run `cargo test -p camel-http` (all
  green, test count identical to pre-move), `cargo fmt --check`,
  `cargo clippy -p camel-http --all-targets -- -D warnings`,
  `cargo xtask lint-single-source`. Commit
  `refactor(http): extract test harness to lib_tests.rs` with
  `Bd: rc-ax1vl`.

## 2. Extract TLS fallback cluster to sibling module (rc-on44c)

- [x] 2.1 Move the TLS fallback cluster from lib.rs (~lines 2290–3057:
  the `#[cfg(test)]` FORCE_WEBPKI_FALLBACK / FORCE_FALLBACK_REBUILD_FAIL
  seams, `client_builder`, `webpki_root_client_config`,
  `fallback_root_store`, `mozilla_only`, `NoVerifyServerCertVerifier`,
  `fallback_client_config`, `emergency_webpki_client`,
  `fallback_rebuild_failed`, `webpki_fallback_client`, `build_client`
  with its `#[cfg(test)]` counters, `strict_tls_error`,
  `client_or_emergency`) to sibling `src/tls.rs`. Items referenced from
  outside the moved block get `pub(crate)` in `tls.rs` plus a crate-root
  re-export (`pub(crate) use tls::{...}` in lib.rs) so every existing
  `crate::build_client`-style path (ssrf.rs imports, client_cache.rs
  tests, tls_harness.rs seam access, the big test module) compiles
  untouched. No item becomes `pub` — the public surface stays
  byte-identical (`tests/pub_surface.rs` + cargo doc prove it).
  Preserve doc comments and intra-doc links.
- [x] 2.2 Move the TLS test cluster — the tests inside `mod tests`
  (in `lib_tests.rs` after task 1) that exercise the forced-webpki
  fallback via `tls_harness` (tls_handshake / tls_parity family) — to
  sibling `src/tls_tests.rs` wired from `tls.rs` as
  `#[cfg(test)] #[path = "tls_tests.rs"] mod tests;`, same house
  pattern. These tests use `crate::tls_harness::*` and the registry
  readiness helpers — keep imports compiling; helpers reachable via
  `crate::` stay put.
- [x] 2.3 Backfill `crates/components/camel-http/CONTEXT.md`
  log-level appendix: cross-check every log site that moved into
  `tls.rs` against the appendix; add the missing pre-castrict
  rc-3j4mq/F2-7 sites (fallback-entry warn on entering the webpki
  fallback, system-roots warns, client-certificate-NOT-used warns,
  fallback-rebuild-failed error) in the appendix's existing row style
  with `// log-policy:` classification. Update line references that the
  move invalidated.
- [x] 2.4 Gates for the whole task: `cargo test -p camel-http`,
  `cargo fmt --check`,
  `cargo clippy -p camel-http --all-targets -- -D warnings`,
  `cargo test -p camel-http --test pub_surface` (if a separate target —
  otherwise the suite covers it),
  `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-http --no-deps`,
  `cargo xtask lint-single-source`. Commit
  `refactor(http): extract TLS fallback cluster to tls.rs` with
  `Bd: rc-on44c`.

## 3. Last bare test client → shared builder (rc-i4yjl)

- [x] 3.1 In `src/lib_tests.rs` (after task 1), the fixture of
  `test_create_consumer_rejects_oversized_max_inflight` builds
  `client: reqwest::Client::new()`. Replace with
  `client: plain_http_test_client()` — the shared builder in
  `tls_harness.rs` that serializes against `CA_STORE_TEST_MUTEX`,
  exactly as its sibling fixture `https_consumer_without_tls_cert_errors`
  already does. Verify zero `reqwest::Client::new()` remains in
  camel-http test code outside `plain_http_test_client` itself
  (`grep -rn "Client::new()" crates/components/camel-http/`).
  Run `cargo test -p camel-http`, `cargo fmt --check`,
  `cargo clippy -p camel-http --all-targets -- -D warnings`.
  Commit `test(http): route last bare test client through shared
  builder` with `Bd: rc-i4yjl`.
