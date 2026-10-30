# Mission 453 benchmark record — aggregator compile-once (bd rc-p5qlx)

- run_id: 20261030T195324Z (criterion evidence snapshot; NOT an era-2 harness run)
- git_commit after: 34cfe2768 (perf commit; branch feature/453-aggregator-compile-once, base b12a77f56)
- git_commit before: b12a77f56 (baseline runs on this base, before any code change)
- machine: same host for baseline and after runs (shared worktree target/, bench profile, sccache warm build / cold measurement identical)

## What this record is

Raw Criterion artifacts (estimates.json medians + sample.json) for both
baselines and the after runs, copied from `target/criterion/` of the
mission worktree. This is a mission-level Criterion record, NOT a
`benchmarks/harness` era-2 run: it has no contenders/cells roster, so per
`SCHEMA.md` it MUST NOT carry a fabricated `run.json`, and `index.json`
(only harness runs) is intentionally NOT updated.

## Exact commands (all in /home/shared/rust-camel-worktrees/453-aggregator-compile-once, TMPDIR=/home/shared/tmp)

1. Baseline (on base b12a77f56, before any change):
   `cargo bench -p camel-processor --bench aggregator -- --save-baseline aggregator-pre`
2. Route scenario bench added (commit d75c598dc, bench-only), then baseline:
   `cargo bench -p camel-bench --bench aggregate_expr -- --save-baseline aggregator-route-pre`
3. After the implementation (commit 34cfe2768):
   `cargo test -p camel-processor`
   `cargo bench -p camel-processor --bench aggregator -- --baseline aggregator-pre`
   `cargo bench -p camel-bench --bench aggregate_expr -- --baseline aggregator-route-pre`

## Results (median point estimates)

| scenario | N | before (median) | after (median) | time delta | before thrpt | after thrpt |
|---|---|---|---|---|---|---|
| microbench-header-correlation | 10 | 13.90 µs | 13.61 µs | -2.12% | 719.3 Kelem/s | 734.9 Kelem/s |
| microbench-header-correlation | 100 | 109.12 µs | 106.35 µs | -2.54% | 916.4 Kelem/s | 940.3 Kelem/s |
| microbench-header-correlation | 1000 | 1.0370 ms | 1.0024 ms | -3.33% | 964.3 Kelem/s | 997.6 Kelem/s |
| route-expr-correlation-predicate | 10 | 34.72 µs | 21.71 µs | -37.48% | 288.0 Kelem/s | 460.7 Kelem/s |
| route-expr-correlation-predicate | 100 | 347.74 µs | 208.42 µs | -40.06% | 287.6 Kelem/s | 479.8 Kelem/s |
| route-expr-correlation-predicate | 1000 | 3.3720 ms | 2.0154 ms | -40.23% | 296.6 Kelem/s | 496.2 Kelem/s |

## Reading

- Microbench (`aggregator/messages/aggregate`, HeaderName correlation — regression guard): small
  consistent improvement (-1.8% to -3.2% time), no regression.
- Route bench (`integration/aggregate_expr`, Expression correlation + PredicateExpr completion —
  the audited per-exchange parse path): -37.6% to -40.1% time, +60% to +67% throughput.
- Criterion verdict lines: "Performance has improved." on all six comparisons (p = 0.00 < 0.05).
- No threshold tightening was applied.

## Spec check

`openspec/specs/eip-aggregator` pins divergence docs, semantic pins, sweep lifecycle, and
correlation-source lowering/validation — no requirement states per-exchange expression
compilation or late failure. Result: **no spec change**.
