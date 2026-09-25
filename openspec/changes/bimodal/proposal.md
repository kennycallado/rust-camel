# Proposal: bimodal

## Why

Run-1 (20260903T084658Z) `t2-realistic-eip` m2 `node-native`
`round_values` is `[6673, 6883, 9337, 16501, 16581]` — two tight
modes (~6.8us, ~16.5us) with the published median 9337 sitting in
the gap, representing NEITHER mode. A small-n median over multimodal
round data is an unstable center: run-2 (20260915T093128Z) is tight
at the high mode (`[16260..16721]`, median 16701). The summarizer
publishes the median with no warning, so a between-modes artifact
masquerades as a central tendency. bd rc-o9rwn (AUDM2 mission 255).

Distinct from cidegen's `degenerate` (5a17ad42): that flags a
bootstrap-interval collapse in `ratios`; this is WITHIN-cell round
structure surfacing at the `cells` level.

## What Changes

Chosen candidate (order option b, the true-marker flag): a boolean
computed from order statistics only — no clustering, no mode
estimation, no record-format redesign.

- `benchmarks/harness/summarize.py`: new helper
  `_median_isolated(values)` — True iff n >= 3 AND the median's
  nearest strictly-lower and strictly-upper neighbors are BOTH more
  than 20% away from the median (relative to the median). For odd n
  these are the median's sort neighbors; for even n the middle pair
  (the median is their mean). All three measured-cell dict sites
  (m1, the generic per-run site, the m2 merged site) gain
  `"median_isolated": bool` next to `median`. `emit_summary`'s
  Measured table gains a `median_isolated` column (`true` / `-`,
  mirroring the Ratios `degenerate` column), plus a short footnote
  under the Measured table ONLY when at least one cell flags.
- `benchmarks/harness/test_summarize.py`: regression pins on the
  exact record vectors (below) plus edge guards (n < 3, all-equal,
  one-sided gap, even-n straddle) and Measured-table emission tests.
- `benchmarks/records/SCHEMA.md`: `cells` field table gains the
  `median_isolated` row plus one paragraph — mechanism, threshold
  margin, and consumer guidance (a flagged median is a gap artifact,
  not a cluster center; cite per-round values or re-run). Additive
  field — no `schema_version` bump (SCHEMA.md forward-compatibility
  clause).

Sealed records are NOT touched: `benchmarks/records/2026*/` stay
byte-identical; the flag applies to summaries published from now on.

## Impact

- Affected code: `benchmarks/harness/summarize.py`,
  `benchmarks/harness/test_summarize.py`,
  `benchmarks/records/SCHEMA.md`. No Rust changes.
- Regression pins (exact record values):
  - run-1 node-native m2 `[6673, 6883, 9337, 16501, 16581]` →
    `median_isolated: true` (gaps 26.3% / 76.7% — both > 20%).
  - run-2 node-native m2 `[16521, 16721, 16701, 16701, 16260]` →
    `false` (max within-sample neighbor gap ~1.1%).
  - run-1 rust-camel-cli m2 `[91792, 92152, 99626, 311553, 327792]`
    → `false` (median sits AT the low mode; only the right gap is
    large — one-sided, not between modes).
  - run-1 node-fastify m2 `[7094, 9818, 10870, 11261, 16852]` →
    `false` (gradual spread, nearest gaps 9.7% / 3.6%).
- Threshold margin: the largest both-sided neighbor gap among
  non-flagged era-2 cells is 9.5%; the weakest flagging gap in
  evidence is 26.3% — 20% sits in the empty gulf between them.
- The definition is a true marker (conservative): it flags only
  medians isolated on BOTH sides; gradual multimodal spreads
  (fastify) do not flag. Honest narrow, not a mode detector.
- m4 restriction (r_glm round-1 finding): m4 `delta_distribution` is
  a distribution series, not per-round values — the sealed cell
  `http-server/node-fastify` `[0, 0, 4, 8, 332]` false-flagged (its
  median 4 sits inside the dominant low cluster; the relative gaps
  are quantization noise). `median_isolated` is per-round metrics
  only and is always false for m4.
- No spec delta: benchmarks tooling, no `openspec/specs/` change.
