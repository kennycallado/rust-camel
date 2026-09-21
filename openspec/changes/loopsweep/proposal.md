# Proposal: loopsweep

## Why

`xtask lint-unbounded-wait` (mission 201, bd rc-3lx2, ADR-0069 §13.2 R1)
flags 548 unbounded waits in test bodies, ratcheted at 548. The bd rc-w32w7
subset — await-carrying `loop {}` sites with no deadline — is **37 sites**
in the landed seed inventory at 9ad4290f. Every one of these loops can
park a test forever when readiness never arrives: a hung Redis failover,
a broker that never acks, a spawned task that stalls. ADR-0069 R1 makes
the deadline normative — this change burns the loop class down.

**41→37 reconciliation (auditable, per-difference disposition).** The
41 (bd title, mission-201 park report L49, bd rc-3lx2 comment 17:48:14)
is count-only — no per-site list survives; it was produced by mission
201's **intermediate detector build** (17:48) on base 1cf940d3. The
landed detector (e4b3842e) produced the durable per-site inventory
whose loop class is exactly 37. Difference disposition:
1. **Tree drift: none.** Every main commit 1cf940d3→9ad4290f moves zero
   `loop {` lines (0268c590, the only loop-moving commit in the day's
   window, is −5/+5 internal and predates 1cf940d3). Verified by diffing
   each commit.
2. **Detector drift: 4 findings.** (a) −1 verifiable: the final
   detector's nested-loop subsumption collapses the camel-redis
   executor.rs pair (:880 outer + :889 inner) into one finding. (b) −3
   not reconstructable: the 17:48 build is throwaway (park report L35
   records the same practice for the lexical prototype), so its
   classification deltas have no durable artifact to diff against.
3. **Set of record: the landed 37** — the only per-site-durable loop
   list, and the set the landed lint counts against 548 in CI. After
   this change the loop class is zero under the normative (final)
   detector; the superseded intermediate's 3 unreconstructable extras
   have no surviving sites to act on.

## What Changes

- Convert 32 of the 37 flagged loop sites to deadline-bounded shapes
  (exact per-site assignment in design.md):
  - **P1 enclosing timeout (23 sites)** — readiness/sleep-poll loops
    and wait-for-completion-marker loops, including the 14 that already
    track a wall-clock `Instant` deadline whose assert is unreachable
    when the awaited call itself parks; the whole loop is wrapped in
    `tokio::time::timeout(budget, async { loop { .. } })` with a loud
    `.expect()` — timeout expiry means the awaited progress never
    arrived, which is a failure.
  - **P3 bounded-retry wrap (9 sites)** — attempt-bounded retry loops
    (policy `max_attempts`) keep their logic and gain an enclosing
    overall budget, converting policy trust into an enforced ceiling.
- Escape marker `// allow-test-wait: <reason>` for the **5 spawned
  accept-loop test servers** (loadgen cli_runtime ×2, camel-test
  component_emission + http, camel-redis executor stub): the server must
  run until teardown, so an internal deadline is semantically wrong —
  the ADR-0069 R1 service-loop exception (test spawns it, owns readiness
  assertions, teardown bounded by test-runtime drop). Each marker
  carries that justification.
- Lower `scripts/xtask/ratchet-unbounded-wait.max` from 548 to
  **511 = 548 − 37** (converted and marked sites both leave the count;
  no double subtraction) and remove the converted/marked entries from
  the embedded inventory.
- Excluded: the 511 non-loop findings (recv/spawn-handle/connect awaits)
  — separate burn-down missions; the lint itself is NOT modified.

## Acceptance criteria

- `xtask lint-unbounded-wait` reports OK at ceiling **511**; zero
  unconverted loop-class sites remain.
- Every converted loop (P1/P3) fails loudly (panic with context)
  instead of hanging when progress never arrives — for P1 sites
  timeout expiry means the awaited readiness or completion marker
  never arrived, which is a failure.
- All previously-passing locally-runnable tests still pass; infra-bound
  suites compile and are marked integration-verification-deferred-to-CI.
- Each of the 5 `allow-test-wait` markers carries a non-empty reason
  referencing the ADR-0069 R1 service-loop exception, and its owning
  test bounds readiness assertions and teardown.

## Risk budget

Test-only diff; no runtime API or behavior change is in bounds. Acceptable:
wider wall-clock budgets (generosity over flakiness), minor test-body
restructuring. Out of bounds: touching the lint detector, raising the
ratchet, weakening assertions to make tests pass.

Bd: rc-w32w7
