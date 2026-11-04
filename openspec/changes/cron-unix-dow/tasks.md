# Tasks: cron-unix-dow

Single-phase change (no `## Phase N` grouping — see design.md).

## camel-component-cron

### Task 1.1: Translate Unix numeric day-of-week to cron-crate ordinals

**Files:**
- `crates/components/camel-cron/src/lib.rs` (modified)

**Steps:**
1. Add a private free function `fn translate_unix_dow(dow_field: &str) -> Result<String, CamelError>` in `crates/components/camel-cron/src/lib.rs` implementing design.md §Algorithm: split on `,`; per token split off an optional `/step` suffix (step must parse as `u32`, must be > 0); classify the pre-step part: `*` passes through with its step; a part containing no ASCII digits passes through with its step (named forms, including `MON-FRI/2`); a numeric part of form `a` or `a-b` (each endpoint parsed as `u32`, required in `0..=7`, `a <= b` required): a bare value with NO step yields the single value `[a]`; only the step form `a/s` expands from `a` up to the field maximum 7; `a-b` expands to `a..=b`; `a-b/s` expands to `a..=b` taking every `s`-th value. Map each expanded value `v` to `v % 7 + 1`, dedupe preserving first occurrence, and emit a comma-joined list of ordinals.
2. Reject with `CamelError::EndpointCreationFailed` and a message containing the literal `day-of-week` plus the offending token: a value outside `0..=7` (message also names the allowed range `0-7`), a descending range `a > b`, a zero step, and a token mixing digits and letters in its endpoints (message also contains `ambiguous`).
3. Change `CronConfig::normalized_cron_expression` to return `Result<String, CamelError>`: split `self.schedule` on whitespace, translate field index 4 with `translate_unix_dow`, reassemble as `"0 "` + fields 0..3 joined + `" "` + translated dow. Propagate any error verbatim.
4. Update the two call sites to propagate: in `validate()`, replace the current `self.normalized_cron_expression().parse::<cron::Schedule>()` chain with `let normalized = self.normalized_cron_expression()?;` followed by `normalized.parse::<cron::Schedule>()` keeping the existing `invalid cron expression` mapping for parse errors; in `CronConsumer::start`, use `expression: config.normalized_cron_expression()?`.
5. Fix the pre-existing `test_rejects_invalid_cron`: after this change the dow field `99` is rejected by translation before crate parse, so the message no longer contains `invalid cron`. Change its URI to `schedule=99%2099%2099%2099%20*` (invalidity stays in minute/hour/dom/month, which reach the crate parse unchanged) and keep the existing `invalid cron` assertion unchanged.
6. In the `test_valid_expressions` test, replace the stale comment `// 3. "0 9 * * 1"        → Mondays at 09:00` with `// 3. "0 9 * * 1"        → Mondays at 09:00 (Unix dow 1 = Monday, translated to crate ordinal 2)` and add `"0%209%20*%20*%20MON"` to the loop's expression list.

**Tests:** (all in `mod tests` of `crates/components/camel-cron/src/lib.rs`; command: `cargo test -p camel-component-cron --lib`)
- `test_translate_single_values`: setup none → act: call `translate_unix_dow` on `"0"`, `"1"`, `"6"`, `"7"` → assert exact outputs `"1"`, `"2"`, `"7"`, `"1"` respectively.
- `test_translate_ranges`: act on `"1-5"`, `"6-7"`, `"0-7"` → assert exact outputs `"2,3,4,5,6"`, `"7,1"`, `"1,2,3,4,5,6,7"`.
- `test_translate_steps`: act on `"1-5/2"`, `"1/2"`, `"*/2"`, `"MON-FRI/2"` → assert exact outputs `"2,4,6"`, `"2,4,6,1"`, `"*/2"`, `"MON-FRI/2"` (`"1/2"` expands source values `1,3,5,7` to ordinals `2,4,6,1`; order preserved, Sunday last).
- `test_translate_dedupes_sunday`: act on `"0,7"` → assert exact output `"1"` (single ordinal, deduplicated).
- `test_translate_mixed_list_keeps_both`: act on `"1,FRI"` → assert exact output `"2,FRI"`.
- `test_rejects_dow_8`: setup `CronConfig::from_uri("cron:test?schedule=0%202%20*%20*%208")` → act: `unwrap_err()` → assert the message contains `day-of-week` and `0-7`.
- `test_rejects_descending_dow_range`: setup `"0%202%20*%20*%206-0"` → act: `unwrap_err()` → assert message contains `day-of-week`.
- `test_rejects_zero_dow_step`: setup `"0%202%20*%20*%201%2F0"` (URL-encoded `/`) → act: `unwrap_err()` → assert message contains `day-of-week`.
- `test_rejects_mixed_dow_endpoints`: setup `"0%202%20*%20*%201-FRI"` → act: `unwrap_err()` → assert message contains `day-of-week` and `ambiguous`.
- `test_normalized_expression_shape`: setup `CronConfig::from_uri("cron:t?schedule=0 2 * * 1").unwrap()` → act: `normalized_cron_expression().unwrap()` → assert exact `"0 0 2 * * 2"`. (This test pins the design.md §Algorithm reassembly format — an internal contract, not a blessed spec scenario; note that in a `//` comment on the test.)

**Acceptance:**
- `cargo test -p camel-component-cron --lib` passes, including all pre-existing tests.
- `cargo fmt --check` clean on the crate.
- `cargo clippy -p camel-component-cron --all-targets -- -D warnings` exits 0.
- No `unwrap()`/`expect()` outside `mod tests` (enforced by `cargo xtask lint-unwrap`).

- [x] 1.1

### Task 1.2: Calendar regression tests asserting actual next-fire instants

**Files:**
- `crates/components/camel-cron/src/lib.rs` (modified — `mod tests` only)

**Steps:**
1. Add a test helper in `mod tests`: `fn next_fires(schedule: &str, count: usize) -> Vec<chrono::DateTime<chrono::Utc>>` that builds `CronConfig::from_uri(&format!("cron:t?schedule={}", schedule))` (spaces are fine in `from_uri` — existing tests already pass raw spaces), calls `normalized_cron_expression().unwrap()`, parses `cron::Schedule`, and collects `schedule.after(&base).take(count)` where `base` is `chrono::Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap()` (fixed UTC anchor). The iterator yields `DateTime<Utc>` directly; no conversion needed.
2. Add a second helper `fn assert_utc(instant: chrono::DateTime<Utc>, y: i32, mo: u32, d: u32, h: u32, mi: u32)` that asserts the exact UTC date and time of one fire.
3. Write the regression tests listed below. Each asserts a COMPLETE ordered fire sequence from the anchor (per blessed spec `specs/cron-schedule-semantics/spec.md`), never parse success alone.
4. Run the full crate test suite and fix only test-code issues; if a failure reveals a translation defect, STOP and report `test-design-gap: <what>` instead of patching Task 1.1's algorithm beyond the blessed spec.

**Tests:** (command: `cargo test -p camel-component-cron --lib`; anchor base `2026-11-02T12:00:00Z` = Monday noon UTC)
- `test_bd_repro_numeric_monday_fires_monday`: `next_fires("0 2 * * 1", 1)` → assert single fire `2026-11-09T02:00:00Z`.
- `test_zero_and_seven_both_fire_sundays`: `next_fires("0 2 * * 0", 2)` and `next_fires("0 2 * * 7", 2)` → assert both sequences equal [`2026-11-08T02:00:00Z`, `2026-11-15T02:00:00Z`].
- `test_six_fires_saturday`: `next_fires("0 2 * * 6", 1)` → assert `2026-11-07T02:00:00Z`.
- `test_full_range_fires_every_day_once`: `next_fires("0 2 * * 0-7", 7)` → assert exactly `2026-11-03..2026-11-09` at 02:00 UTC, one per consecutive day.
- `test_work_week_range_fires_monday_to_friday`: `next_fires("0 9 * * 1-5", 5)` → assert [`2026-11-03T09:00Z`, `2026-11-04T09:00Z`, `2026-11-05T09:00Z`, `2026-11-06T09:00Z`, `2026-11-09T09:00Z`].
- `test_wrapped_range_fires_weekend`: `next_fires("0 2 * * 6-7", 3)` → assert [`2026-11-07T02:00Z`, `2026-11-08T02:00Z`, `2026-11-14T02:00Z`].
- `test_stepped_range_fires_mon_wed_fri`: `next_fires("0 2 * * 1-5/2", 4)` → assert [`2026-11-04T02:00Z`, `2026-11-06T02:00Z`, `2026-11-09T02:00Z`, `2026-11-11T02:00Z`].
- `test_single_value_step_includes_sunday_via_seven`: `next_fires("0 2 * * 1/2", 3)` → assert [`2026-11-04T02:00Z`, `2026-11-06T02:00Z`, `2026-11-08T02:00Z`].
- `test_star_step_fires_unix_stepped_set`: `next_fires("0 2 * * */2", 4)` → assert [`2026-11-03T02:00Z`, `2026-11-05T02:00Z`, `2026-11-07T02:00Z`, `2026-11-08T02:00Z`].
- `test_mon_fires_monday`: `next_fires("0 2 * * MON", 1)` → assert `2026-11-09T02:00:00Z`.
- `test_mon_fri_range_fires_weekdays`: `next_fires("0 2 * * MON-FRI", 5)` → assert the same five instants as `test_work_week_range_fires_monday_to_friday` but at 02:00 UTC.
- `test_mon_fri_step_stays_valid`: `next_fires("0 2 * * MON-FRI/2", 3)` → assert [`2026-11-04T02:00Z`, `2026-11-06T02:00Z`, `2026-11-09T02:00Z`].
- `test_mixed_list_fires_both_days`: `next_fires("0 2 * * 1,FRI", 3)` → assert [`2026-11-06T02:00Z`, `2026-11-09T02:00Z`, `2026-11-13T02:00Z`].
- `test_duplicate_sunday_collapses`: `next_fires("0 2 * * 0,7", 2)` → assert [`2026-11-08T02:00Z`, `2026-11-15T02:00Z`].

**Acceptance:**
- All new tests pass under `cargo test -p camel-component-cron --lib`.
- Each calendar assertion checks exact UTC instants (weekday via date, not `Weekday` enum alone).
- `cargo fmt --check` and `cargo clippy -p camel-component-cron --all-targets -- -D warnings` stay clean.

- [x] 1.2

## docs

### Task 1.3: Document Unix day-of-week semantics (crate authority + component page)

**Files:**
- `crates/components/camel-cron/CONTEXT.md` (modified)
- `docs/src/components/cron.md` (modified)

**Steps:**
1. In `crates/components/camel-cron/CONTEXT.md`, add a `## Day-of-week semantics` section after `## URI`: state the Unix contract (`0` and `7` = Sunday, `1` = Monday, `6` = Saturday), the translation rule applied at config normalization (numeric values map to crate ordinals via `v % 7 + 1`; ranges, lists, and steps expand over the source range before mapping; `*`/steps and day names pass through), the rejected forms (value outside `0-7`, descending range, zero step, mixed numeric/name endpoints — all fail endpoint creation), and the scope note that restricted DOM + DOW intersect (crate behavior) where Unix cron ORs them. STE rules: short sentences, active voice, no em-dashes.
2. In `docs/src/components/cron.md`, extend the `## Schedule format` section with one paragraph: the day-of-week field uses Unix numbering (`0` and `7` are Sunday, `1` is Monday); numeric ranges, lists, and steps keep that meaning; day names (`MON`, `SUN`) work unchanged; values outside `0-7` are rejected at endpoint creation. End the paragraph with a two-source citation to the crate authority: "(See `crates/components/camel-cron/CONTEXT.md`.)" Tag any new fenced block `text`.
3. Do not change include directives, YAML example blocks, or any other section.

**Tests:** (documentation — no Rust tests; commands from the WORKTREE ROOT, not from `docs/`: `mdbook build docs && mdbook test docs`)
- `mdbook build docs`: exits 0; every include directive and link resolves.
- `mdbook test docs`: exits 0; all fenced Rust blocks compile.

**Acceptance:**
- Both `mdbook` commands exit 0 when run from the worktree root.
- `cargo xtask lint-context-citations` exits 0 (the new CONTEXT.md section is linted; if the lint rejects a backticked pseudo-expression such as the mapping formula, rewrite it in plain prose instead of backticks).
- `cargo xtask lint-single-source` exits 0.
- The dow paragraph in `docs/src/components/cron.md` matches the CONTEXT.md authority word for word on the mapping rule (the formula appears in CONTEXT.md, not in the user-facing page).
- No changes outside the two listed files.

- [x] 1.3
