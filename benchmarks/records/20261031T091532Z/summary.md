# Mission 457 benchmark record — circuit breaker lock-free closed fast path (bd rc-bnsxr)

- run_id: 20261031T091532Z (criterion evidence snapshot; NOT an era-2 harness run)
- git_commit after: 703d32802 (branch feature/457-circuit-breaker-fast-path; steps landed as
  64a4932e6 step 1, 076d9d6aa step 2 + review fixes 3a72da968, 3f2de7303, 14eb0744d, tests
  2706b635c, 294fe7484, 703d32802)
- git_commit before: b7d222b44 (bench harness commits 0d6a6e2bf, bd2ea70f3, b7d222b44; breaker
  implementation identical to base c1912e5d4)
- machine: same host for baseline and after runs (mission worktree target/, bench profile,
  sccache warm build / cold measurement identical, TMPDIR=/home/shared/tmp)

## What this record is

Raw Criterion artifacts (estimates.json medians + samples.json) for both baselines and the
after runs, copied from `target/criterion/` of the mission worktree. This is a mission-level
Criterion record, NOT a `benchmarks/harness` era-2 run: it has no contenders/cells roster, so
per `SCHEMA.md` it MUST NOT carry a fabricated `run.json`, and `index.json` (only harness
runs) is intentionally NOT updated.

## Exact commands (all in /home/shared/rust-camel-worktrees/457-circuit-breaker-fast-path, TMPDIR=/home/shared/tmp)

1. Bench-harness fixes (pre-existing breakage on base; commits 0d6a6e2bf, bd2ea70f3):
   current-thread runtime (tokio pause requires it) and real sleeps (breaker expiry uses
   `std::time::Instant`, so `tokio::time::advance` never elapsed `open_duration`).
2. Baseline before any implementation change:
   `cargo bench -p camel-processor --bench circuit_breaker -- --save-baseline cb-pre`
3. Multi-task gate microbench added (commit b7d222b44, bench-only), then baseline:
   `cargo bench -p camel-processor --bench circuit_breaker -- --save-baseline cb-multi-pre`
4. After the implementation (steps 1+2, commits through 703d32802):
   `cargo bench -p camel-processor --bench circuit_breaker -- "circuit_breaker/" --baseline cb-pre`
   `cargo bench -p camel-processor --bench circuit_breaker -- gate_multi --baseline cb-multi-pre`

## Results (median point estimates; criterion-reported change for the after runs)

| bench | before (median) | after (median) | delta |
|---|---|---|---|
| circuit_breaker/closed_passthrough | 322.0 ns | 314.5 ns | -2.34% (p=0.00, improved) |
| circuit_breaker/closed_to_open_cycle | 1,074.6 ns | 1,028.1 ns | -4.33% (p=0.00, improved) |
| circuit_breaker/half_open_recovery | 13.039 ms | 13.039 ms | +0.00% (p=0.69, no change) |
| circuit_breaker/repeated_cycles | 39.113 ms | 39.126 ms | +0.04% (p=0.10, no change) |
| gate_multi/gate_closed_success_tasks_1 | 23,340 ns | 18,202 ns | -22.02% (p=0.00, improved) |
| gate_multi/gate_closed_success_tasks_8 | 747,351 ns | 47,121 ns | -93.69% (p=0.00, improved) |
| gate_multi/gate_closed_success_tasks_16 | 1,934,596 ns | 65,904 ns | -96.59% (p=0.00, improved) |

## Reading

- The contended closed/success path (gate microbench, 8 and 16 concurrent tokio tasks) is
  15.9x / 28.6x faster: the per-exchange `Arc<Mutex<CircuitState>>` lock round-trips were the
  bottleneck; the closed path is now one Acquire load in `before_call` and a load pair in
  `after_result`, and successes never write shared state when the breaker is already clean.
- Serial (1-task) gate traffic improves 22%: the redundant `Closed { 0 }` write is gone and
  uncontended mutex lock/unlock is replaced by plain loads.
- Mixed/state-transition benches (half_open_recovery, repeated_cycles) are sleep-dominated and
  show no change beyond noise; closed_to_open_cycle (transition-heavy, no sleeps) improved
  4.3%. No regression anywhere.
- Mission expectation met: "closed fast path improves substantially; mixed/state-transition
  paths must not regress beyond noise." No threshold semantics changed.
