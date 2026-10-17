# Proposal: httpsweep — camel-http/TLS zone mechanical sweep (mission 320)

## Why

Sweep epic rc-mnia0 (e_opus backlog ruling 2026-09-25) collects mechanical
P3 leftovers in the camel-http/TLS zone. Three items attached at dispatch;
one is actionable code at HEAD, one is already fixed upstream (zero-code
close), one already landed via httpxtract (verify-only). One lease, one
worktree, one pass (LIGHT protocol, order:
`.opencode/fleet/orders/320-httpsweep-mission.md`).

## What Changes

1. **rc-jr2g8 (code dedup)** — `crates/components/camel-http/src/tls.rs`:
   the provider-mirror + `with_safe_default_protocol_versions().expect(...)`
   preamble (comment block included) was duplicated between
   `webpki_root_client_config` and `fallback_client_config`. Extract
   `fallback_client_config_builder()` (returns the `WantsVerifier`-state
   builder) and chain both callers through it. Behavior change: none on
   the reachable paths — same provider resolution, same pinned versions,
   same expect message; one precedence shift (r_glm finding 2): in
   `fallback_client_config` the helper's documented-infallible
   `expect(...)` now evaluates before `fallback_root_store(...)?` instead
   of after, so a hypothetical non-stock provider lacking safe default
   versions AND strict-invalid CA material would panic instead of
   returning the typed error — unreachable with stock ring/aws-lc-rs.
   NOTE:
   bd cited lib.rs ~2440/~2639; b393d15b (tlsseam follow-up split) moved
   both fns to `src/tls.rs` (162/349 at base 76687d95).
2. **rc-oltht (already fixed, zero change)** — "dedup Mozilla root-store
   in webpki_root_client_config": landed as ed5417df "refactor(http): reuse
   mozilla_only() root store" (the rc-wk6yi close commit). At base,
   `webpki_root_client_config` uses `.with_root_certificates(mozilla_only())`
   (tls.rs:176); the literal `RootCertStore { roots: TLS_SERVER_ROOTS.to_vec() }`
   exists only inside `mozilla_only()` itself. Stale duplicate filing —
   bd closed zero-code-change (rc-safez precedent, redissweep).
3. **rc-i4yjl (closed, verify-only)** — bare `reqwest::Client::new()` in
   tests: landed via httpxtract squash. Verified at base: the only
   remaining `Client::new()` site is `tls_harness.rs` `plain_http_test_client`
   — the deliberate mutex-guarded fix itself, not a leak.
4. **Sweep-inventory scan** — duplicate fn names (none), TODO/FIXME (none),
   stale comments: one hit — `plain_http_test_client` doc cited
   "lib.rs `mod tests`" although the b393d15b split moved the 44 callers to
   `lib_tests.rs` (10 more in `client_cache.rs`). Comment corrected.

## Impact

- Affected: `camel-component-http` (one production refactor, one test-harness
  doc comment). No API change (helper is private), no spec deltas, no
  production behavior change.
- Battery: baseline camel-component-http 461 (sum of ALL result lines),
  post-change 461 — parity, zero failures, under the fleet-httpsweep
  containment scope.
