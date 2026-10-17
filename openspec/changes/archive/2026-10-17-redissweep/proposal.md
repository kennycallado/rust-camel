# Proposal: redissweep — redis zone mechanical sweep (mission 313)

## Why

Sweep epic rc-62mdr (e_opus backlog ruling 2026-09-25) collects mechanical
P3 leftovers in the redis zone. Two are actionable at HEAD; one (rc-safez)
turned out to be already fixed upstream and is closed zero-code-change.
One lease, one worktree, one pass (LIGHT protocol, order:
`.opencode/fleet/orders/313-redissweep-mission.md`).

## What Changes

1. **rc-158g3 (code dedup)** — `crates/camel-test/tests/`: the
   `"CamelRedis.Value"` header-name literal is repeated across the redis
   suites — `redis_test.rs` (5 `set_header` sites: ~51, 109, 168, 226, 839),
   `redis_sentinel_test.rs` (~393), and `component_emission_test.rs` (~790,
   RouteBuilder seed site). Extract one shared constant in the camel-test
   test-support module (`tests/support/mod.rs`, UNGATED top-level — the
   per-component submodules are `#[cfg(feature = "integration-tests")]`
   gated, so a const in `support/redis.rs` would be unreachable from
   ungated binaries) and swap every site. No behavior change: same string,
   same call shapes.
2. **rc-9daxr (docs)** — `.opencode/fleet/orders/_template-policy.md`:
   the 2026-09-24 CI incident (redis_sentinel_test.rs:425 broke on push
   d090ffc6 — the file only compiles under `--features integration-tests`,
   which no local mission battery built) stays recurring until the
   compile-gate is a standing template rule. Add it: mission orders
   touching `crates/camel-test/tests/**` (or any
   `#[cfg(feature = "integration-tests")]` surface) must include
   `cargo check -p camel-test --features integration-tests --tests`.
3. **rc-safez (closed, zero change)** — needless_borrows at
   redis_test.rs:325 was fixed at head by 75141c9d (2026-09-26 22:44,
   tracked as rc-cf1su); rc-safez was a duplicate parallel filing. Leg
   `cargo clippy -p camel-test --features integration-tests --all-targets
   -- -D warnings` verified exit 0 at HEAD in this worktree. bd closed.

## Impact

- Affected: `camel-test` (integration-test files + `tests/support/mod.rs`),
  one fleet policy doc. No production behavior change, no spec deltas
  (`skip_specs: true` — no canonical specs touched).
- Gates: fmt · clippy (4 legs + the camel-test all-targets leg) ·
  `cargo check -p camel-test --tests` (ungated leg — import hygiene must
  hold on BOTH feature states) · camel-component-redis tests (baseline 608)
  · camel-redis-repo tests (baseline 61) — both under the systemd-run
  mandate (MemoryMax=12G, -j4) · lint-single-source clean ·
  lint-unbounded-wait ratchet 296 unmoved · schema untouched.
- Protocol: LIGHT — r_glm holistic over the commit set, e_glm pre-park,
  park with buzzer (no merge).
