# Tasks: multicast-panic-outcome

Single-phase change (see design.md `## Phases`): two tasks, one crate.
Worker instructions live in the full block above each checkbox.

## 1. Parallel-multicast panic-to-Failed mapping

### Task 1.1: catch_unwind wiring + core accounting tests

Implement the index-preserving panic mapping in `parallel_multicast` and
the three tests that pin the core accounting semantics (partial-success
counting, zero-success guard, highest-index last-error selection).

**Dispatch:** No prerequisite. Budget: 30 minutes.

**Files:**
- `crates/camel-processor/src/multicast_segment.rs` (modified)
- `crates/camel-processor/CONTEXT.md` (modified)

**Steps:**
1. In `crates/camel-processor/src/multicast_segment.rs`, add the imports
   `std::panic::AssertUnwindSafe` and `futures::FutureExt` (the `futures`
   crate is already a workspace dependency of `camel-processor`; verify in
   `crates/camel-processor/Cargo.toml` and add it there only if absent).
2. In `parallel_multicast`, locate the spawned-task body where the branch
   outcome future is built (the `let outcome = async` block that awaits
   `branch.run(ex)` and records Stop state), then wrapped by the
   per-branch timeout arm (`tokio::time::timeout`).
3. Wrap that outcome future with
   `AssertUnwindSafe(outcome).catch_unwind()` BEFORE the timeout arm, and
   map both unwind results to representative failures exactly as
   `design.md §Chosen approach` specifies:
   - timeout arm: `Ok(Ok(o)) => o`;
     `Ok(Err(panic_payload)) =>` build
     `Failed(ProcessorError(format!("multicast branch {idx} panicked")))`,
     then `std::mem::forget(panic_payload)`, then return the failure;
     `Err(_elapsed) =>` the existing timed-out failure unchanged.
   - no-timeout arm: `Ok(o) => o`; `Err(panic_payload) =>` same
     construct-error-then-forget-payload mapping.
4. Add a comment at the mapping citing the segment-outcome-composition
   spec's panicked-branch classification clause (bd rc-f88o) and the
   ADR-0058 zero-success invariant, and explaining the
   `mem::forget`: a payload with a panicking `Drop` would otherwise cause
   a second panic outside `catch_unwind`, recreating the dropped-
   `JoinError` defect. Reference the recipient_list precedent
   (`recipient_list.rs` panic arm).
5. Leave the drain loop `while let Some(res) = set.join_next().await` and
   its `if let Ok((idx, Some(o)))` arm unchanged: cancellation
   `JoinError`s (not panics) must keep being excluded from accounting.
6. In `crates/camel-processor/CONTEXT.md`, update the `MulticastSegment`
   row of the "Structural EIP Segments" table: append to its description
   one clause recording that a parallel-mode branch panic maps to a
   representative `Failed(ProcessorError)` counted in the partial-success
   accounting (spec: segment-outcome-composition, bd rc-f88o).
7. In the `#[cfg(test)] mod tests` module of `multicast_segment.rs`, add
   a helper:
   `fn panicking_body(msg: &str) -> OutcomeSegment` returning a cloneable
   `OutcomePipeline` impl whose `run` returns a future that panics with
   `msg` when polled (e.g. `Box::pin(async move { panic!("{}", msg) })` —
   the panic must occur while the future is polled inside the spawned
   task, not at `run()` call time).
8. Add the three tests listed under Tests below, following the existing
   rc-b41j test style in that module (`MulticastSegment { .. }` literal,
   `tagged_completed_body`, `always_failed_body`, `OutcomePipeline::run`,
   `#[tokio::test]`, inline `Exchange::new(Message::new("inbound"))`).

**Tests** (all in `mod tests` of `multicast_segment.rs`; per-test
fail-before expectations stated individually — not all of them fail
pre-fix):
1. name: `multicast_parallel_panic_branch_counted_as_failed_partial_success`
   - setup: `MulticastSegment` with `parallel: true`,
     `stop_on_exception: false`, branches
     `[tagged_completed_body("b0", 10ms), always_failed_body("errA"), panicking_body("boom")]`,
     aggregator joining bodies with `|`. Install a minimal test-local
     `tracing` subscriber. Implement all required `tracing::Subscriber`
     methods: return `true` from `enabled`, return a fixed nonzero
     `tracing::span::Id` from `new_span`, make `record`,
     `record_follows_from`, `enter`, and `exit` no-ops, and use `event` to
     record the `failed_branches` and `branch_count` fields of warn-level
     events into shared state via `tracing::field::Visit`. Install it with
     `let _guard = tracing::subscriber::set_default(subscriber);` before
     running the segment. This uses only the existing `tracing`
     dependency. The plain `#[tokio::test]` current-thread flavor keeps
     the thread-local default dispatch valid across the await. The warn at
     the end of `parallel_multicast` is emitted from the test's own task,
     not from a spawned branch.
   - action: `OutcomePipeline::run(&mut seg, Exchange::new(Message::new("inbound"))).await`.
   - assert: `Completed` with body exactly `"b0"` (only the success is
     aggregated; the Failed branch AND the panicked branch are both
     discarded from aggregation); NOT `Failed`, NOT `Stopped`. AND the
     captured partial-success warn reports `failed_branches = 2` and
     `branch_count = 3` (the panicked branch counts as a discarded
     failure).
   - command: `cargo test -p camel-processor --lib multicast_parallel_panic_branch_counted_as_failed_partial_success`
   - expected: FAILS before implementation on the captured diagnostic —
     pre-fix the panicked branch is a dropped `JoinError`, so the warn
     reports `failed_branches = 1` (the `errA` slot only). The
     `Completed("b0")` outcome itself holds both before and after
     (regression pin for the partial-success invariant once the
     synthetic `Failed` slot exists).
2. name: `multicast_parallel_all_branches_panicked_returns_failed`
   - setup: `parallel: true`, `stop_on_exception: false`, branches
     `[panicking_body("boom-0"), panicking_body("boom-1")]`.
   - action: run the segment.
   - assert: `Failed` whose error is `ProcessorError` containing
     `"multicast branch 1 panicked"` (highest branch index, per the
     parallel last-error determinism rule); NOT `Completed` (ADR-0058:
     no laundering), NOT `Stopped`.
   - command: `cargo test -p camel-processor --lib multicast_parallel_all_branches_panicked_returns_failed`
   - expected: FAILS before implementation — the pre-fix code returns
     `Completed(aggregator([]))` because both JoinErrors are dropped and
     no last_error exists.
3. name: `multicast_parallel_mixed_failed_and_panicked_reports_panic_as_last_error`
   - setup: `parallel: true`, `stop_on_exception: false`, branches
     `[always_failed_body("errA"), panicking_body("boom")]`.
   - action: run the segment.
   - assert: `Failed` whose error message contains
     `"multicast branch 1 panicked"` and does NOT contain `"errA"` (the
     panicked higher-index branch supplies the representative error,
     proving the panicked branch entered the results accounting at all —
     pre-fix the error would be `errA` from branch 0 alone).
   - command: `cargo test -p camel-processor --lib multicast_parallel_mixed_failed_and_panicked_reports_panic_as_last_error`
   - expected: FAILS before implementation (pre-fix last_error is
     `errA`).

**Acceptance:**
- `cargo test -p camel-processor --lib` passes (new tests AND the
  existing rc-b41j scenario suite — sequential mode, partial success,
  Stopped-wins, timeout — unchanged).
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-processor` exits 0.
- `cargo xtask lint-unwrap` reports no new `unwrap()` in the diff.
- The drain loop's cancellation handling is byte-identical to pre-change.

- [x] 1.1

### Task 1.2: precedence and payload-safety tests

Three additional tests pinning Stop precedence over panics, the
timeout-window panic hole, and the panicking-Drop payload safety. No
production code changes expected in this task; if a test exposes a real
defect in the Task 1.1 implementation, fix the implementation in
`parallel_multicast` (same file) rather than weakening the test.

**Dispatch:** Prerequisite: Task 1.1. Independent worker budget after that
task lands: 20 minutes.

**Files:**
- `crates/camel-processor/src/multicast_segment.rs` (modified — test
  module only, unless a Task 1.1 defect surfaces)

**Steps:**
1. Reuse the `panicking_body` helper introduced in Task 1.1.
2. Add the three tests listed under Tests below, same module and style
   as Task 1.1.
3. For the payload-safety test, add a dedicated helper `fn
   panicking_drop_bomb_body() -> OutcomeSegment` that defines (inside the
   helper) a `struct PanicDropBomb;` whose `impl Drop` calls `panic!`,
   and returns a body whose future panics via
   `std::panic::panic_any(PanicDropBomb)` when polled.

**Tests** (all in `mod tests` of `multicast_segment.rs`; per-test
fail-before expectations stated individually — not all of them fail
pre-fix):
1. name: `multicast_stopped_branch_wins_over_panicked`
   - setup: `parallel: true`, `stop_on_exception: false`, branches
     `[always_stopped_body(), panicking_body("boom")]` (helper
     `always_stopped_body` already exists in the module).
   - action: run the segment on `Exchange::new(Message::new("inbound"))`.
   - assert: `Stopped` (the Stop-winner scan still finds the stopped
     branch's index; the panicked branch's synthetic `Failed` does not
     corrupt Stop precedence per ADR-0025 §3); NOT `Failed`, NOT
     `Completed`.
   - command: `cargo test -p camel-processor --lib multicast_stopped_branch_wins_over_panicked`
   - expected: regression pin — PASSES both before the fix (the panicked
     branch is a dropped JoinError; results = the Stopped slot alone)
     and after (the synthetic `Failed` slot coexists with Stop
     precedence). Its value is pinning the invariant against the new
     synthetic slot, not failing first.
2. name: `multicast_branch_panicking_inside_timeout_window_maps_to_failed`
   - setup: `parallel: true`, `stop_on_exception: false`,
     `timeout: Some(Duration::from_secs(10))` (generous — the panic must
     win the race), branches `[panicking_body("boom")]`.
   - action: run the segment.
   - assert: `Failed` whose error contains `"multicast branch 0
     panicked"` and does NOT contain `"timed out"` (the `Ok(Err(payload))`
     arm of the timeout wrapper, not the elapsed arm); NOT `Completed`.
   - command: `cargo test -p camel-processor --lib multicast_branch_panicking_inside_timeout_window_maps_to_failed`
   - expected: FAILS before implementation (pre-fix the task aborts into
     a dropped JoinError; zero outcomes with no last_error can launder
     to `Completed`).
3. name: `multicast_panicking_drop_payload_maps_once_without_double_panic`
   - setup: `parallel: true`, `stop_on_exception: false`, branches
     `[panicking_drop_bomb_body()]`.
   - action: run the segment.
   - assert: `Failed` whose error contains `"multicast branch 0
     panicked"`; the outcome is produced exactly once from the catch arm
     (the `mem::forget` prevents the payload's panicking `Drop` from
     causing a second panic — if it fired, the branch would vanish into
     a JoinError and the outcome would not be the representative
     failure); NOT `Completed`.
   - command: `cargo test -p camel-processor --lib multicast_panicking_drop_payload_maps_once_without_double_panic`
   - expected: fails against an implementation that drops the payload
     instead of forgetting it (double panic → dropped JoinError).

**Acceptance:**
- `cargo test -p camel-processor --lib` passes with all six new tests
  plus the existing suite.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-processor` exits 0.

- [x] 1.2
