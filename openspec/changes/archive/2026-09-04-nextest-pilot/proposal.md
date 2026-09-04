# Proposal: nextest-pilot

## Why

Two production CI incidents (rc-y24l: 4h47m runner burn from an unbounded
test wait; rc-u3aw: macos-only pooled-connection race) exposed two gaps in
the ubuntu Rust library-test job: a single hanging test burns the whole job, and a flaky
test (fail-then-pass) is invisible — it merges red noise or wastes reruns.
The adjudicated flaky-tests strategy (epic rc-99d5, bd rc-mhsn; e_opus
advisory + e_gpt counter-review) rules: pilot `cargo-nextest` on the
container-free ubuntu Rust library-test job only, with per-test timeouts and
fail-on-flaky detection.

## What Changes

- Add `.config/nextest.toml` with a `ci` profile: `retries = 1`
  (diagnostic, not tolerance), `flaky-result = "fail"` (a retry-pass
  FAILS the gating job), `slow-timeout = { period = "30s",
  terminate-after = 3 }` (effective ceiling ~90s, per nextest semantics
  period x terminate-after), `failure-output = "immediate-final"`,
  `fail-fast = false`.
- `ci.yml` Unit Tests (ubuntu-latest): install cargo-nextest via the
  pinned-SHA `taiki-e/install-action` convention
  (`tool: cargo-nextest@0.9.143` — pinned so the two-week measurement is
  reproducible); replace
  `cargo test --workspace --lib` with
  `cargo nextest run --workspace --lib --profile ci`.
- Explicit scope guards: testcontainers, K3s, bridge binaries, Full Tests,
  and the macOS legs stay on `cargo test` (process-per-test would destroy
  process-local shared-container fixtures — e_gpt cost ruling).
- No doctest job added: the Rust library-test scope runs `--lib` only today; nextest's
  doctest omission changes nothing.

## Affected Crates

None (CI configuration + new tool config file only).

## Acceptance Criteria

- Pilot job green on main with identical test selection: nextest's selected
  test count equals today's `cargo test --workspace --lib` count.
- A hung library test terminates at the ~90s ceiling with an explicit
  slow-timeout report instead of burning `timeout-minutes: 90`.
- A retry-passing test is reported FLAKY and fails the job.
- Baseline metrics (wall time, process count, failures) recorded in bd
  rc-mhsn for the 2-week rollout decision (exit criteria of the pilot).

## Risk Budget

Low. Reversible by reverting one line in ci.yml. Residual risks: hidden
test-selection differences (mitigated by the count-parity check) and
runtime overhead of process-per-test on the lib tier (measured, not
assumed — first runs compared against the cargo baseline).

Reference: bd rc-mhsn (parent epic rc-99d5).
