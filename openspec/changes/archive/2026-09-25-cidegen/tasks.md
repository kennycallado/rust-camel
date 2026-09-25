# Tasks: cidegen

## 1. Rust: `degenerate` flag in aggregate-ratios

**Files**

- `benchmarks/harness/loadgen/src/ratios.rs`

**Steps**

1. `RatioReport`: add `pub degenerate: bool` after `ci_hi` (before
   `method`).
2. `compute_ratio_report`: set it from the published values —
   `outcome.hi == outcome.point || outcome.lo == outcome.point`
   (exact f64 equality; use those locals, not recomputed medians).
3. `RatioJsonLine`: add `degenerate: bool` — declaration order stays
   alphabetical so the emitted line stays sorted-key
   (`ci_hi < ci_lo < degenerate < denominator`).
4. `compute_ratio` human line: append ` DEGENERATE` after
   ` UNPAIRED`-position logic (only when true).
5. Module docs: add a "Degenerate intervals" paragraph — mechanism
   (paired median-of-n bootstrap, point can coincide with the top of
   the resample support when the median round is the max-ratio
   round; >= 2.5% mass on it collapses the percentile onto the
   point), definition (exact f64 equality on either bound), meaning
   (zero-width side asserts no uncertainty — flag, don't hide).
6. Tests:
   - `ratio_degenerate_run2_regression`: two summaries with the
     EXACT run-2 m3 round values —
     A `[65984.66, 66446.82, 66165.08, 66262.92, 65573.94]`,
     B `[14994.66, 15147.02, 15033.28, 15095.8, 15001.06]`,
     rounds 5, paired, seed 0, 2000 resamples. Assert
     `report.point == 4.401240447859682`,
     `report.ci_hi == report.point` (bit-exact), and
     `report.degenerate` is true; assert the human line ends with
     ` DEGENERATE` and the JSON line contains
     `"degenerate":true`.
   - Existing synthetic non-varying test values that already yield
     `lo == point == hi` (all-constant cells) will now be
     degenerate — update expectations there (line suffix) if the
     asserts break.
   - Update `ratio_json_line_sorted_keys_golden` to the 8-field
     line with `"degenerate":false` in sorted position.
   - Add a non-degenerate assert: varying round values with real
     width → `degenerate == false`, JSON `"degenerate":false`.
   - Constant-value cells (zero-width BOTH sides) → `degenerate ==
     true` (zero information is degenerate too — assert it
     explicitly with a tiny dedicated test).

**Tests**

- Name: `ratio_degenerate_run2_regression`
  - Arrange: temp run root, two m3 summaries + uniform order file
    with the exact run-2 vectors above.
  - Act: `compute_ratio_report(pa, pb, false, 0, 2000)` + line +
    JSON line.
  - Assert: point bit-equals 4.401240447859682; ci_hi bit-equals
    point; degenerate true; human line has ` DEGENERATE` suffix;
    JSON contains `"degenerate":true`.
- Name: `ratio_json_line_sorted_keys_golden` (updated)
  - Assert: 8-field sorted-key golden string with
    `"degenerate":false`.
- Name: `ratio_constant_cells_flagged_degenerate`
  - Arrange: both cells all-constant (200.0 ×5 vs 100.0 ×5).
  - Act: compute report.
  - Assert: lo == point == hi == 2.0 and degenerate true.

**Acceptance**

- `cargo test -p bench-loadgen` green; no `expect` outside tests;
  `cargo fmt` clean; `cargo clippy -p bench-loadgen --all-targets
  -- -D warnings` green.

- [x] 1.1

## 2. Python: summarize mirrors + surfaces the flag

**Files**

- `benchmarks/harness/summarize.py`
- `benchmarks/harness/test_summarize.py`

**Steps**

1. `_ratio_row`: after the mirror branch and name overwrite, set
   `out["degenerate"] = (out["ci_lo"] == out["point"]) or
   (out["ci_hi"] == out["point"])` — recomputed from FINAL values so
   mirrored pairs and legacy rows (field absent) are both handled;
   update the docstring (inversion preserves exact equality, so the
   flag survives 1/x mirroring by construction).
2. `emit_summary` Ratios table: header becomes
   `| numerator | denominator | metric | point | ci_lo | ci_hi |
   degenerate | method |` (9 columns, separator row too); cell
   value `"true" if r.get("degenerate") else "-"`.
3. Tests in `test_summarize.py`:
   - Degenerate row (ci_hi == point float-equal) → table row shows
     `| true |` in the degenerate column.
   - Mirrored degenerate row (binary reported the pair inverted;
     `_ratio_row` mirrors values) → recomputed flag still true.
   - Legacy row lacking the key → `-` in the column, no crash.
   - Non-degenerate row → `-`.

**Tests**

- Name: `test_ratio_table_marks_degenerate`
  - Arrange: ratios rows incl. one with `ci_hi == point` exactly.
  - Act: `emit_summary`.
  - Assert: summary.md Ratios table has 9 columns; the degenerate
    row shows `true`; others show `-`.
- Name: `test_ratio_row_mirror_preserves_degenerate`
  - Arrange: raw row where numerator is the denominator cell dir
    (mirror branch taken) with `ci_lo == point` pre-mirror.
  - Act: `_ratio_row`.
  - Assert: `out["degenerate"] is True` computed from mirrored
    values.

**Acceptance**

- `python3 -m pytest test_summarize.py` (or the repo's runner —
  check how existing tests are invoked) green in
  `benchmarks/harness`.

- [x] 2.1

## 3. Docs: SCHEMA.md field + paragraph

**Files**

- `benchmarks/records/SCHEMA.md`

**Steps**

1. `ratios` field table: add row `| degenerate | boolean | True
   when ci_lo or ci_hi equals point exactly (zero-width side). |`.
2. One paragraph after the table (before the pairing rule):
   mechanism summary + consumer guidance — a flagged row's interval
   must not be read as a confident one-sided bound; the bootstrap
   support collapsed onto the point (small-n discrete medians);
   treat the point as the estimate and the OTHER side (when
   non-degenerate) as its bound. Note the field is additive (no
   version bump per Forward compatibility).

**Acceptance**

- Table renders (pipe counts consistent); paragraph ≤ 10 lines;
  English.

- [x] 3.1
