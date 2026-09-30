# Tasks: redissweep (mission 313, LIGHT)

## 1. Single-source the CamelRedis.Value header literal (rc-158g3)

- [x] 1.1 In `crates/camel-test/tests/support/mod.rs`, add an UNGATED
  top-level constant (before the `#[cfg(feature = "integration-tests")]`
  gated infra submodule declarations):
  `/// Header name carrying the value payload for redis key/value commands.`
  `pub const REDIS_VALUE_HEADER: &str = "CamelRedis.Value";`
  Then swap every `"CamelRedis.Value"` literal in
  `tests/redis_test.rs` (5 `set_header` sites: ~51, 109, 168, 226, 839 —
  add `REDIS_VALUE_HEADER` to the existing `use support::redis::shared_redis;`
  import neighborhood as `use support::REDIS_VALUE_HEADER;`),
  `tests/redis_sentinel_test.rs` (~393), and
  `tests/component_emission_test.rs` (~790 — prefer a fully-qualified
  `support::REDIS_VALUE_HEADER` at the use site or a cfg-local `use` so the
  ungated compile leg never sees an unused import). Do NOT touch the two
  comment mentions (redis_test.rs ~760, redis_sentinel_test.rs ~472) or
  non-camel-test crates. Verify: `grep -rn '"CamelRedis.Value"'
  crates/camel-test/tests/` returns only the constant definition; then
  `cargo check -p camel-test --tests` (ungated) AND
  `cargo check -p camel-test --features integration-tests --tests` both
  exit 0; `cargo fmt --check`;
  `cargo clippy -p camel-test --features integration-tests --all-targets -- -D warnings`
  exits 0.

## 2. Compile-gate standing rule in the mission template policy (rc-9daxr)

- [x] 2.1 In `.opencode/fleet/orders/_template-policy.md`, add a short
  standing-rule section: mission orders whose diff touches
  `crates/camel-test/tests/**` — or any `#[cfg(feature =
  "integration-tests")]`-gated surface — MUST include the gate
  `cargo check -p camel-test --features integration-tests --tests` in the
  mission battery (cite the 2026-09-24 CI incident: push d090ffc6 broke at
  redis_sentinel_test.rs:425 because no local battery compiled that
  feature-gated surface; fixed by 39a7056e/234b99e7). Match the doc's
  existing section style (heading + rationale + rule). Markdown-only diff.

  DONE 2026-09-30 — section `## COMPILE-GATE FOR integration-tests
  SURFACES (2026-09-24 CI incident)` applied to the LIVE fleet doc in the
  main checkout. Disposition: `.opencode/fleet/` is gitignored by design
  (.gitignore:62; no fleet file tracked in any ref), so the edit carries
  NO commit — a worker's `git add -f` attempt was reverted (would violate
  the AGENTS.md staging rule). The live untracked file is the artifact;
  evidence recorded in inbox/redissweep-parked.json.
