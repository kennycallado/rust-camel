# Tasks: bimodal

## 1. Python: `median_isolated` flag in the summarize path

**Files**

- `benchmarks/harness/summarize.py`
- `benchmarks/harness/test_summarize.py`
- `benchmarks/records/SCHEMA.md`

**Steps**

1. `summarize.py`: add module constant
   `_ISOLATED_MEDIAN_REL_GAP = 0.20` near the top statistics
   helpers, with a comment stating the margin evidence (era-2
   within-sample neighbor gaps <= ~8%; weakest between-modes flag
   gap 26.3%; bd rc-o9rwn).
2. Add helper `_median_isolated(values)` next to `round_values`:
   guard `len(values) >= 3` and `median > 0`; sort; median =
   `statistics.median`; `lo` = largest value strictly below the
   median, `hi` = smallest strictly above (for even n these are the
   middle pair — the median is their mean); return True iff both
   exist AND `(median - lo) / median > _ISOLATED_MEDIAN_REL_GAP` AND
   `(hi - median) / median > _ISOLATED_MEDIAN_REL_GAP`. Docstring:
   mechanism + "true marker, not a mode detector" honesty note.
3. All three measured-cell dict sites — `_m1_cell`, the generic
   per-run cell builder (m3/m4), and the m2 merged builder — gain
   `"median_isolated": _median_isolated(values_or_vals)` placed
   directly after `"median"`.
4. `emit_summary` Measured table: add a `median_isolated` column
   (`true` when flagged, `-` otherwise — mirror the Ratios
   `degenerate` column, `.get` so legacy/attempted shapes never
   crash). Immediately after the Measured table, ONLY if any
   measured cell of the record is flagged, append a footnote block:
   a `median_isolated: true` median falls in a sparse gap of its own
   round sample (small-n multimodal rounds) — it represents no
   cluster; consult `round_values` or re-run; see SCHEMA.md
   "`median_isolated`".
5. `SCHEMA.md` `cells` field table: add the `median_isolated` row
   (boolean; True when the median's nearest strictly-lower and
   strictly-upper sample neighbors are both >20% of the median
   away, n >= 3). One paragraph after the ATTEMPTED-cell closure:
   mechanism (small-n rounds can be bimodal — two tight tick-rate
   clusters; the order-statistics median can land in the empty gap
   between them, as run-1 t2-realistic-eip m2 node-native
   `[6673, 6883, 9337, 16501, 16581]`, median 9337 between the
   ~6.8us and ~16.5us modes), consumer guidance (a flagged median
   is a gap artifact, not a center — cite per-round values or
   re-run; run-2's tight values are the citable ones there), the
   one-sided-gap non-flag case (median AT a mode is representative
   of that mode), and additive-field/no-version-bump. Reference
   bd rc-o9rwn.
6. Tests in `test_summarize.py` (match the file's existing unittest
   style; pin the exact record vectors as module constants or
   inline):
   - unit tests for `_median_isolated` (vectors below);
   - cell-dict emission: an m2-shaped fixture with the run-1 vector
     produces `"median_isolated": true` in the cell dict (and the
     m1 / generic sites get coverage via their existing fixture
     paths if practical — at minimum direct helper coverage);
   - Measured-table emission: flagged cell renders `true` in the
     `median_isolated` column, non-flagged render `-`, footnote
     present when any flagged, absent when none.
7. Do NOT modify anything under `benchmarks/records/2026*/`
   (sealed). Do NOT touch `benchmarks/harness/loadgen/` (Rust) —
   `degenerate` (cidegen) already covers the ratios surface.

**Tests**

- Name: `test_median_isolated_run1_regression`
  - Arrange: values `[6673, 6883, 9337, 16501, 16581]`.
  - Act: `_median_isolated(values)`.
  - Assert: True (left gap 2454/9337 = 26.3% > 20%, right gap
    7164/9337 = 76.7% > 20%).
- Name: `test_median_not_isolated_run2_tight`
  - Arrange: `[16521, 16721, 16701, 16701, 16260]`.
  - Act/Assert: False (max neighbor gap ~1.1%, right neighbor
    equal).
- Name: `test_median_not_isolated_one_sided_gap`
  - Arrange: `[91792, 92152, 99626, 311553, 327792]`.
  - Act/Assert: False (left gap 7.5% — median sits at the low mode).
- Name: `test_median_not_isolated_gradual_spread`
  - Arrange: `[7094, 9818, 10870, 11261, 16852]`.
  - Act/Assert: False (gaps 9.7% / 3.6%).
- Name: `test_median_isolated_edge_guards`
  - Arrange/Act/Assert: n < 3 (`[100, 100000]`) → False; all-equal
    (`[5.0] * 5`) → False; even-n straddle (`[6700, 6900, 16400,
    16600]`, median 11650, both gaps > 41%) → True.
- Name: `test_measured_table_isolated_column`
  - Arrange: summary fixture with one flagged + one unflagged
    measured cell.
  - Act: `emit_summary` (or the existing summary-rendering test
    harness path).
  - Assert: table line for the flagged cell contains `| true |`,
    unflagged contains `| - |`; footnote block present exactly
    once; a second fixture with no flagged cells emits no footnote.

**Acceptance**

- `_median_isolated` flag on all three measured-cell dict sites;
  Measured-table column + conditional footnote in `emit_summary`.
- All new tests green; the FULL existing `test_summarize.py` and
  `test_publish.py` suites pass (publish gate unaffected — additive
  non-latency field on MEASURED cells only).
- SCHEMA.md row + paragraph landed.
- Zero changes under `benchmarks/records/2026*/` and
  `benchmarks/harness/loadgen/`.

- [x] 1.1 implement + test the `median_isolated` flag end to end
