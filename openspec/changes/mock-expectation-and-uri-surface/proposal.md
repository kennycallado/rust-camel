# Proposal: mock-expectation-and-uri-surface

## Why

`to: mock:x` from YAML routes is a black hole: `MockUriConfig` is a
metadata-only placeholder (`skip_impl`, `lib.rs:96`) with zero real URI
parameters, and count expectations live outside `assert_satisfied`
(manual `assert_exchange_count` only). A complex camel-cli/YAML demo
currently cannot express mock intent declaratively nor assert counts in
one call. Additionally, `exchange(idx)` deadlocks on current-thread
tokio runtimes (`block_in_place`, `lib.rs:422`), and `assert_satisfied`
can only panic — a future `camel test` CLI needs a `Result`-based
assertion to produce exit codes without `catch_unwind`.

Adjudicated by e_opus (docs/reviews/2026-08-18-camel-mock-expansion-ruling.md
Q1/Q4/Q5/Q6, reaffirmed 2026-08-20 packaging consultation). This is
OpenSpec change #1 of two; the declarative testkit (change #2) stays a
separate, gated change.

## What Changes

- **Count expectations in `assert_satisfied`**: `expect_count(n)`
  (exact) and `expect_minimum_count(n)` folded into the existing
  assertion path — additive branch, not a redesign.
- **Non-panicking assertion**: `try_assert_satisfied() -> Result<(), MockAssertionError>`
  alongside the panicking variant (e_opus condition — the single most
  expensive-to-change-later item for the future testkit).
- **Real URI parameter surface** on the mock scheme (5 params, same
  pattern as controlbus): `retain`, `copy`, `failFast`,
  `expectedCount`, `anyOrder`. URI params override component-level
  `MockConfig` defaults at endpoint creation.
- **`expectedCount` inertness guarantee**: documented + spec-enforced —
  before any assertion runs it never autonomously rejects or drops
  live traffic under `camel run`; after an explicit assertion reports
  the mismatch, normal fail-fast behavior applies (enforcement belongs
  to a future `camel test`).
- **Hardening**: `exchange(idx)` detects current-thread runtimes and
  fails with a clear message instead of deadlocking.
- **Live spec update**: `mock-component-correctness` gains the new
  requirements.
- **Opportunistic file split** along natural seams only where regions
  are already edited (ruling Q6 — not a precondition).

Explicitly excluded: reply/behavior simulator mode, RuntimeQueryBus
extension, IPC, `camel test` subcommand, sidecar/`expects:` file
format, AdviceWith (all rejected/deferred by the ruling; backlog
bd rc-i2qf, rc-3bq5, rc-3kwt, rc-3g4f, rc-77my).

## Acceptance criteria

- `assert_satisfied` enforces exact + minimum count expectations.
- `try_assert_satisfied` returns `Ok`/`Err(MockAssertionError)` without
  panicking; fail-fast latch parity with the panicking variant.
- `mock:` URIs accept the 5 params; `cargo xtask schema --check` passes
  with the catalogued surface.
- A spec scenario proves `expectedCount` does not reject live producer
  traffic.
- `exchange(idx)` errors clearly (no deadlock) on current-thread
  runtimes; unchanged on multi-thread.
- Quality gates green (fmt, clippy, `lint-*` xtasks).

## Risk budget

Component-only, additive API — no producer semantics change for
existing callers, no CLI/runtime changes. Acceptable: minor breaking
edge if `exchange(idx)` messaging changes; NOT acceptable: any change
to sink identity (producer stays write-only), any enforcement of
`expectedCount` in the live producer path, any new cross-crate
dependency. bd: rc-413i.

## Affected crates

- `camel-mock` (all code changes: URI parsing, expectations, assertion
  surface, hardening, opportunistic split).
- Generated schema/catalog artifacts if `xtask schema` regenerates them
  from component metadata.
