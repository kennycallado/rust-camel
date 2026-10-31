# Mission 473 benchmark record — rhai choice batching grouped shapes + gate (bd rc-1y2qs)

- run_id: 20261031T205833Z (criterion evidence snapshot; NOT an era-2 harness run)
- git branch: feature/473-rhai-choice-batching
- git_commit: 47f0c058dc356848a1dfed25fe6255be05c1bfdc (bench change committed on top of this)
- machine: mission worktree target/, bench profile, TMPDIR=/home/shared/tmp, sccache disabled
  (`RUSTC_WRAPPER=`)

## What this record is

Raw Criterion median artifacts for the grouped choice shapes and the sequential controls of
the `camel-processor` choice bench, copied from `target/criterion/` of the mission worktree at
`/home/shared/rust-camel-worktrees/473-rhai-choice-batching`. This is a mission-level
Criterion record, NOT a `benchmarks/harness` era-2 run: it has no contenders/cells roster,
so per `SCHEMA.md` it MUST NOT carry a fabricated `run.json`, and `index.json` (only harness
runs) is intentionally NOT updated.

The `choice-pre` baseline for the sequential controls is the saved baseline from record
`20261031T191644Z` (`criterion/choice_lang-<shape>-before-estimates.json` below). The new
`rhai_group_*` shapes have no `choice-pre` baseline by design (they did not exist), so only
their fresh `-after` estimates are recorded.

## Exact commands (in /home/shared/rust-camel-worktrees/473-rhai-choice-batching, TMPDIR=/home/shared/tmp)

```
cargo fmt -p camel-processor
cargo clippy -p camel-processor --all-targets -- -D warnings
cargo bench -p camel-processor --bench choice -- "choice_lang/rhai_group"
cargo bench -p camel-processor --bench choice -- '^choice_lang/(rhai_1_match$|rhai_10_|mixed_10_no_match$)' --baseline choice-pre
```

Criterion estimates are stored in nanoseconds; the table shows the raw median point estimate
(ns) and the converted value (µs).

## Results — grouped shapes (fresh run, no baseline)

| shape | median (ns) | median (µs) | 95% CI (ns) |
|---|---|---|---|
| rhai_group_1_match | 9,730.4 | 9.730 | [9,701.5, 9,850.2] |
| rhai_group_10_match_first | 11,169.6 | 11.170 | [11,122.6, 11,244.9] |
| rhai_group_10_match_middle | 53,234.9 | 53.235 | [51,075.9, 53,718.9] |
| rhai_group_10_match_last | 83,385.7 | 83.386 | [81,665.4, 87,112.8] |
| rhai_group_10_no_match | 79,667.1 | 79.667 | [79,103.2, 82,199.7] |

## Results — sequential controls vs `choice-pre`

| shape | pre median (ns) | post median (ns) | pre median (µs) | post median (µs) | delta (criterion) |
|---|---|---|---|---|---|
| rhai_1_match | 15,459.8 | 9,820.8 | 15.460 | 9.821 | -36.263% (p=0.00, improved) |
| rhai_10_match_first | 12,461.9 | 9,974.8 | 12.462 | 9.975 | -26.352% (p=0.00, improved) |
| rhai_10_match_middle | 84,343.1 | 57,416.5 | 84.343 | 57.417 | -45.505% (p=0.00, improved) |
| rhai_10_match_last | 110,889.1 | 90,939.7 | 110.889 | 90.940 | -21.081% (p=0.00, improved) |
| rhai_10_no_match | 112,629.1 | 85,798.7 | 112.629 | 85.799 | -29.506% (p=0.00, improved) |
| mixed_10_no_match | 50,977.1 | 44,824.0 | 50.977 | 44.824 | -11.321% (p=0.00, improved) |

## Gate

Gate: **PASS**.

- Every grouped 10-clause shape median is <= 250 µs:
  - rhai_group_10_match_first: 11.170 µs — PASS
  - rhai_group_10_match_middle: 53.235 µs — PASS
  - rhai_group_10_match_last: 83.386 µs — PASS
  - rhai_group_10_no_match (all-clauses-evaluated coverage point): 79.667 µs — PASS
  - control rhai_group_1_match: 9.730 µs — PASS
- Match-last (83.386 µs) is the highest observed grouped median; no-match is the required
  all-clauses-evaluated coverage point, not the measured maximum. Both pass comfortably.
- Sequential controls show no regression against `choice-pre`: every control improved
  (-11.3% to -45.5%, p=0.00). The improvement is partly the t2a conversion de-duplication
  and partly unquantified host effects (see Reading), not host noise alone.

## Files

- `criterion/choice_lang-<grouped-shape>-after-estimates.json` — grouped medians (this run).
- `criterion/choice_lang-<sequential-shape>-before-estimates.json` — saved `choice-pre`
  baseline (record `20261031T191644Z`).
- `criterion/choice_lang-<sequential-shape>-after-estimates.json` — control medians (this run).

## Reading

- The sequential no-match control improved from the pre-change baseline 112.629 µs to
  85.799 µs, a ~24% total reduction on the sequential path. Part of this is a genuine
  production eval-path optimization, not host noise: t2a (commit a11f78c06) de-duplicated
  the body-to-dynamic conversion, which previously ran twice on this path
  (`body_to_dynamic` plus `make_scope`'s internal conversion) and now runs once via the
  shared snapshot helper. Host effects on the remaining difference are unquantified (no
  A/B host measurement was made).
- Grouped no-match (79.667 µs) is ~7.1% faster than the current sequential no-match control
  (85.799 µs). This is the batching-specific gain (one shared snapshot instead of one per
  clause), distinct from the de-duplicated conversion fix. It must not be compared against
  the pre-change baseline to derive a single isolated "batching is N% faster" number.
- Match position still scales with the number of script evaluations: first 11.2 µs, middle
  53.2 µs, last 83.4 µs, so per-clause evaluation remains the dominant cost after batching.
  Match-last (83.386 µs) is the highest observed grouped median; no-match (79.667 µs) is the
  required all-clauses-evaluated coverage point (the conservative gate shape). Both pass the
  250 µs gate comfortably.
