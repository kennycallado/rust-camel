# Tasks: multicast-partial-success

## camel-processor

### Task 1.1: sequential partial-success aggregation

**Files:**
- `crates/camel-processor/src/multicast_segment.rs` (modified)

**Steps:**
1. In `sequential_multicast`, replace the post-loop block `if let Some(err) = last_error { return camel_api::PipelineOutcome::Failed(err); }` with: when `last_error.is_some()`, return `Failed(err)` ONLY if `outputs.is_empty()`; otherwise emit `tracing::warn!(failed_branches = total - outputs.len(), branch_count = total, "multicast partial success: discarding failed branch outcomes");` and fall through (derive the failed count — no accumulator variable; `Stopped` returns early so `total - outputs.len()` counts exactly the Failed branches). The function still ends with `camel_api::PipelineOutcome::Completed((seg.aggregator)(outputs))`.
2. Annotate the warn site with `// log-policy: handler-owned` (ADR-0012 table discipline; the branch failure is scoped, the route continues).
3. Rename test `multicast_sequential_stop_on_exception_false` to `multicast_sequential_partial_success_aggregates_successes` and update it: keep the same 3 branches `[counting_passing_body, counting_body(fail_at=1), counting_passing_body]` with `stop_on_exception=false`, but change the aggregator to one that sets the result body to `format!("n={}", exchanges.len())`. Assert the outcome is `Completed` with body `"n=2"` (branches 0 and 2 succeeded; branch 1's failure is discarded), keep `assert_eq!(invocations.load(Ordering::SeqCst), 3)` (all branches still execute).

**Tests:** (executable spec — name, arrange, act, assert)
- `multicast_sequential_partial_success_aggregates_successes`: 3 sequential branches [pass, fail-at-idx-1, pass], stop_on_exception=false, count-aggregator → run segment → assert `Completed` with body `n=2` and 3 invocations. Command: `cargo test -p camel-processor --lib multicast_sequential_partial_success_aggregates_successes`. Expected: fails before step 2 (current code returns `Failed`), passes after.
- `multicast_sequential_partial_success_two_branches` (new, exact delta-spec scenario): 2 sequential branches `[always_completed_body(), always_failed_body("boom")]`, stop_on_exception=false, count-aggregator (body = `n={len}`) → run segment → assert `Completed` with body `n=1` and NOT `Failed`. Command: `cargo test -p camel-processor --lib multicast_sequential_partial_success_two_branches`. Expected: fails before step 2, passes after.
- `multicast_sequential_stop_on_exception_true` (existing, unchanged): fail-fast still returns `Failed`, 2 invocations. Command: `cargo test -p camel-processor --lib multicast_sequential_stop_on_exception_true`. Expected: green before and after.
- `multicast_all_branches_failed_no_stopped_returns_failed` (existing, strengthened): zero-success stays `Failed` — strengthen the assertion to also check iteration-last error identity (`msg.contains("branch-b-failed")`; branches use `always_failed_body("branch-a-failed")` / `always_failed_body("branch-b-failed")`). Command: `cargo test -p camel-processor --lib multicast_all_branches_failed_no_stopped_returns_failed`. Expected: green before and after.
- `multicast_stopped_branch_wins_over_failed` (existing, strengthened): Stopped-wins — strengthen to assert the propagated exchange body is the stopped branch's body (mark the stopped branch's exchange body before run, e.g. inbound body "stop-body" set inside `always_stopped_body` variant, then assert `Stopped(ex)` carries it). Command: `cargo test -p camel-processor --lib multicast_stopped_branch_wins_over_failed`. Expected: green before and after.

**Acceptance:**
- `cargo test -p camel-processor --lib multicast` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `cargo xtask lint-log-levels` exits 0 (warn site annotated).
- `cargo xtask lint-unwrap` exits 0 (no new unwrap).

- [x] 1.1

### Task 1.2: parallel partial-success aggregation + doc comment

**Files:**
- `crates/camel-processor/src/multicast_segment.rs` (modified)

**Steps:**
1. In `parallel_multicast`, `stop_on_exception=false` else-arm: keep the loop that collects `last_error` from sorted `results` (no accumulator — derive the failed count after the guard point by filtering the outcome slot: `results.iter().filter(|(_, o)| matches!(o, Some(PipelineOutcome::Failed(_)))).count()`; this counts only actual `Failed` outcomes, not pre-start-gate-skipped branches whose slot is `None`).
2. Keep the shared tail (the post-if/else `filter_map` aggregation over `Completed` outcomes) as the SINGLE aggregation point, and place the guard after it: when `last_error.is_some()` and `completed.is_empty()`, return `Failed(err)`; when `last_error.is_some()` and `completed` is non-empty, emit `tracing::warn!(failed_branches, branch_count = total, "multicast partial success: discarding failed branch outcomes");` where `failed_branches` is the filter-derived count from step 1 (annotated `// log-policy: handler-owned`) and fall through to `Completed((seg.aggregator)(completed))`. The `stop_on_exception=true` arm (first-failed by lowest branch index) is untouched — `last_error` is never set there, so the guard is a no-op on that path.
3. Rewrite the `stop_on_exception` field doc comment on `MulticastSegment`: state that with `false`, a zero-success run propagates the representative error while a partial-success run aggregates the successful branches' outputs (discarded failures logged at warn); with `true`, the first `Failed` branch propagates. `Stopped` outcomes always propagate per ADR-0025 §7.
4. Rework test `multicast_parallel_stop_on_exception_false_propagates_last_error`: change branches from `[always_fail_body("err1"), always_pass_body(), always_fail_body("err2")]` to `[always_fail_body("err1"), always_fail_body("err2")]` (zero-success), keep the `Failed` + `msg.contains("err2")` (highest-index LastWins) assertion, and delete both now-unused local helpers (`always_pass_body`, `always_fail_body`) — use the module-level `always_failed_body` helper instead.
5. Rework test `multicast_parallel_timeout_stop_on_exception_false_propagates_timeout_error`: replace `FastPassBody` with a `FastFailBody` returning `Failed(ProcessorError("fast-fail"))`, set branches to `[FastFailBody, SlowBody]` (the timeout error from branch 1 is the highest-index failure, so LastWins propagates it), and assert the outcome is `Failed` whose message contains `timed out`.
6. Add test `multicast_parallel_partial_success_aggregates_successes`: 3 parallel branches `[always_completed_body(), always_failed_body("err-mid"), always_completed_body()]`, `stop_on_exception=false`, aggregator that sets result body to `format!("n={}", exchanges.len())`; assert `Completed` with body `"n=2"` (branches 0 and 2) and NOT `Failed`.

**Tests:** (executable spec — name, arrange, act, assert)
- `multicast_parallel_partial_success_aggregates_successes`: arrange per step 6 → run segment → assert `Completed` body `n=2`. Command: `cargo test -p camel-processor --lib multicast_parallel_partial_success_aggregates_successes`. Expected: fails before step 2 (current code returns `Failed`), passes after.
- `multicast_parallel_stop_on_exception_false_propagates_last_error` (reworked): zero-success 2-branch parallel → `Failed` containing `err2`. Command: `cargo test -p camel-processor --lib multicast_parallel_stop_on_exception_false_propagates_last_error`. Expected: green after rework.
- `multicast_parallel_timeout_stop_on_exception_false_propagates_timeout_error` (reworked): zero-success, `[FastFailBody, SlowBody(timeout)]` → `Failed` containing `timed out` (timeout is highest-index failure, LastWins). Command: `cargo test -p camel-processor --lib multicast_parallel_timeout_stop_on_exception_false_propagates_timeout_error`. Expected: green after rework.
- Full regression lock: `cargo test -p camel-processor --lib multicast` exits 0.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0 (full crate, catches collateral).
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `cargo fmt --check` clean on the modified file.
- `cargo xtask lint-log-levels` and `cargo xtask lint-unwrap` exit 0.

- [x] 1.2

## docs

### Task 2.1: ADR-0058 + CONTEXT.md sync

**Files:**
- `docs/adr/0058-outcome-aware-segment-composition.md` (modified)
- `crates/camel-processor/CONTEXT.md` (modified)

**Steps:**
1. In `docs/adr/0058-outcome-aware-segment-composition.md` §`Multicast outcome`: replace the two "out of scope / inconsistency tracked as bd rc-b41j" paragraphs with the reconciled rule: with `stop_on_exception=false`, zero-success reports `Failed(last_error)` (highest-branch-index representative, parallel; iteration-last, sequential) while partial success aggregates successful branch outputs only and reports `Completed` (discarded failures logged at warn). Keep the Stopped-wins sentence. Apply `ste-writing` discipline (short imperative sentences).
2. In the same ADR, §`Migration / existing-code alignment`: update the multicast paragraph to state the partial-success guard (`outputs.is_empty()` / `completed.is_empty()`) is enforced in both arms (bd rc-b41j).
3. In `crates/camel-processor/CONTEXT.md`, `MulticastSegment` row of the "Structural EIP Segments" table: extend the description with "Partial success (stopOnException=false) aggregates successful branches only; zero-success returns Failed (ADR-0058)."
4. In the same CONTEXT.md, update the "ADR-0012 log-policy sites" section: the count goes from 11 to 13 annotated sites, and the table gains two `multicast_segment.rs` rows — `sequential_multicast` partial-success discard warn and `parallel_multicast` partial-success discard warn, both category "(a) handler-owned" with the `// log-policy: handler-owned` annotation.

**Tests:**
- Non-Rust task — verification by command, not `#[test]` (each line is one shell command that must exit 0):
  - `cargo xtask lint-context-citations`
  - `[ "$(grep -c 'rc-b41j' docs/adr/0058-outcome-aware-segment-composition.md)" -ge 1 ]`
  - `! sed -n '/### Multicast outcome/,/### Governed/p' docs/adr/0058-outcome-aware-segment-composition.md | grep -qi "out of scope"` (negated grep: exits 0 iff zero matches in the isolated section)
  - `[ "$(grep -c 'aggregates successful branches only' crates/camel-processor/CONTEXT.md)" -ge 1 ]`
  - `[ "$(grep -c 'multicast_segment.rs' crates/camel-processor/CONTEXT.md)" -ge 2 ]`

**Acceptance:**
- Each of the five commands above exits 0.

- [x] 2.1
