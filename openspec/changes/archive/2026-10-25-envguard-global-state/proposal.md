# Proposal: envguard-global-state

## Why

Tests in one process share `std::env` and statics: `set_var`/`remove_var` in one test leak into
concurrently running siblings. This is the adjudicated `global-state` flake class (epic rc-99d5,
bd rc-zkfub) with three observed incidents: rc-yzpb / rc-nqv7 (camel-config `from_file` read
ambient `CAMEL_PROFILE` set by a sibling test under a *different* lock scope) and rc-lq4bc
(log-capture race under load). nextest (pilot rc-mhsn) isolates only the lib tier; the class
needs a structural in-process fix. An e_glm pre-flight (2026-10-25) returned GO-WITH-CONDITIONS;
an e_gpt spec-bless R1 was REJECTED with six findings, all incorporated here (distinct
constructor/chaining names, per-site reason markers, seed-only-decreases ratchet, full
`::global()` disposition audit, documented soundness preconditions, named hostile-ambient proof).

Today three crates hand-roll their own env locks (`camel-config::config::env_lock`,
`camel-bench` ENV_LOCK, `camel-cli` TRUSTSTORE_ENV_LOCK) — duplication the ticket forbids
("no ad-hoc set_var plus restore duplication").

## What Changes

- **One `EnvGuard` RAII type** in `camel_component_api::test_support` (existing harness home,
  `test-support` feature): `acquire()` (lock-only, for reader tests), constructors
  `set(k,v)` / `remove(k)` returning `Self`, and chainable `and_set` / `and_remove` on one
  guard (multi-key on ONE guard; no method-name collision, no self-deadlock), undo log restored
  in reverse on drop while the env lock is still held, `PoisonError::into_inner` so one
  panicking test does not cascade. Replaces the per-crate locks. Soundness preconditions for
  the `unsafe` mutation are documented in the module.
- **Cluster migration** of env-mutating callsites (rg inventory: 182 sites / 44 files on base
  789c84faa), high-risk clusters first: camel-config ambient `CAMEL_PROFILE` reads,
  log-capture, `CAMEL_*` / `RUST_LOG` / `XDG_*` / `HOME` mutations. Migrate-as-touched allowed
  for the remainder; residual raw sites carry an adjacent per-site reason marker
  (`env-guard-exception:`) and are seeded into the ratchet ledger.
- **`::global()` disposition audit**: every test-scope `::global()` callsite from the inventory
  gets a recorded disposition — migrated to per-test state, `#[cfg(test)]`-scoped AND
  reset/guarded between tests, or excluded as a production static (recorded, not redesigned).
- **`no-env-mutation-unguarded` xtask lint**: bans raw `env::set_var|remove_var` tokens in
  test-scope files unless the site carries an adjacent `env-guard-exception: <reason>` marker
  (`EnvGuard::set` does not match the token); a per-file seeded ledger enforces the monotone
  ratchet — counts may never exceed the seed and seed entries only decrease; wired into
  `.github/workflows/ci.yml` next to lint-no-free-port and the AGENTS.md `## QUALITY GATES` block.
- One new test proving the formerly ambient-dependent camel-config profile-loading cluster
  (rc-yzpb) passes under a hostile ambient `CAMEL_PROFILE`, retaining the original
  deterministic assertion.

**Excluded**: production behavior changes, public API outside test support, nextest expansion,
version bump, process-per-test runner changes.

## Acceptance criteria

- Single `EnvGuard` in `camel_component_api::test_support`; per-crate ad-hoc locks removed.
- High-risk clusters migrated; every residual raw site marked with an adjacent reason;
  `::global()` dispositions recorded for the full test-scope inventory.
- Lint landed, seeded, green in CI; gate registered in AGENTS.md; seed only decreases.
- Touched suites green + `cargo test --workspace --lib`; fmt/clippy clean on touched crates;
  `lint-unbounded-wait` (ceiling 0), `lint-test-sleep`, `lint-no-free-port` (17),
  `lint-no-raw-http-test-server` (0) unchanged; no sleeps added.
- Hostile-ambient test present and green.

## Risk budget

- In bounds: test-only refactors, dev-dependency additions (camel-config → camel-component-api,
  no cycle: component-api does not depend on camel-config), test-support API (feature-gated).
- Out of bounds: redesigning production statics, changing test assertions/expectations,
  unsafe outside the single guarded EnvGuard module.

Affected crates: camel-component-api (test_support), camel-config, camel-dsl, camel-http,
camel-bench, camel-cli, camel-jms, camel-test, camel-core (dev-dep consumers), scripts/xtask.

Bd: rc-zkfub
