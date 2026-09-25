# Proposal: cidegen

## Why

Run-2 (20260915T093128Z) ratio `rust-camel-lib/node-fastify` m3
publishes `point == ci_hi == 4.401240447859682` (exact float
equality): the 95% interval asserts literally zero uncertainty above
the point. Root cause (verified against the record's
`round_values`): the paired percentile bootstrap resamples
median-of-5 round means with ONE index vector per resample; when the
round supplying both cells' medians (round 2) is also the max-ratio
round, the TOP of the resampled-ratio support IS the point estimate,
and >= 2.5% of resamples land exactly on it — the 97.5th percentile
collapses onto the point, bit-identically. The bootstrap math is
correct; the silent publication is the defect. bd rc-km1ep.

## What Changes

- `bench-loadgen` `aggregate-ratios` (benchmarks/harness/loadgen/src/
  ratios.rs): every ratio report gains a `degenerate` boolean — true
  iff `ci_lo == point || ci_hi == point` (exact f64 equality on the
  PUBLISHED values). Emitted in the `--json` line (sorted-key
  position after `ci_lo`) and as a ` DEGENERATE` suffix on the human
  line (mirrors the existing ` UNPAIRED` pattern). Module docs
  explain the mechanism and flag semantics.
- `benchmarks/harness/summarize.py`: `_ratio_row` recomputes
  `degenerate` from the FINAL (post-mirror) published values —
  inversion preserves exact equality, so the flag is always
  consistent with what `run.json` shows, including legacy binary
  rows that lack the field. `emit_summary`'s Ratios table gains a
  `degenerate` column (`true` / `-`).
- `benchmarks/records/SCHEMA.md`: `ratios` field table gains the
  `degenerate` row plus one paragraph on mechanism and consumer
  guidance (flagged interval = point estimate without a one-sided
  confidence claim; n=5 discrete support). Additive field — no
  schema_version bump (SCHEMA.md forward-compatibility clause).

## Impact

- Affected code: `benchmarks/harness/loadgen/src/ratios.rs` (+ its
  tests), `benchmarks/harness/summarize.py`,
  `benchmarks/harness/test_summarize.py`,
  `benchmarks/records/SCHEMA.md`.
- Regression pin: the exact run-2 m3 per-round means (from the
  record's `round_values`) reproduce `point == ci_hi` exactly under
  seed 0 / 2000 resamples and MUST yield `degenerate: true`.
- Scan of both era-2 records: the definition flags exactly 1/8
  run-2 rows (the known-bad one) and 0/7 run-1 rows — narrow-but-
  nonzero intervals (run-1 upper gaps 1.3-2.6%) do NOT flag.
- No spec delta: benchmarks tooling, no `openspec/specs/` change.
