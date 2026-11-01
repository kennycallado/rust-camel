# Tasks: do-try-error-clear

## camel-processor

### Task 1.1: Clear the residual error on Handled/Continued and pin it with unit tests

**Files:**
- `crates/camel-processor/src/do_try_segment.rs` (modified)
- `crates/camel-processor/CONTEXT.md` (modified)

**Steps:**
1. Test-only step. Inside `mod tests` in `crates/camel-processor/src/do_try_segment.rs`,
   next to `MutateThenStop` (~line 329, i.e. inside the `#[cfg(test)] mod tests`
   block — NOT in production code), add:
   `struct CheckThenComplete { check: Arc<dyn Fn(&Exchange) + Send + Sync> }`
   whose `OutcomePipeline::run` calls `self.check(&exchange)` then returns
   `PipelineOutcome::Completed(exchange)`, plus
   `fn seg_check_then_complete(check: impl Fn(&Exchange) + Send + Sync + 'static) -> camel_api::OutcomeSegment`
   wrapping it in `camel_api::OutcomeSegment::new(Box::new(...))`, mirroring
   `seg_stop_with`.
2. Test-only step. At the top of `mod tests`, add one `use` per line
   (ADR-0091, no nested braces): `use camel_api::exchange::PROPERTY_EXCEPTION_MESSAGE;`
   and `use camel_api::Value;`.
3. Test-only step. Write the three new tests and the two strengthenings listed
   below. Run `cargo test -p camel-processor --lib do_try_segment` and capture
   the RED output (see the per-test "Expected before" lines).
4. Production step. Change the catch-body match arm pattern from
   `PipelineOutcome::Completed(next) =>` to
   `PipelineOutcome::Completed(mut next) =>` (~line 197) so the exchange can be
   mutated. `next` is bound immutably today.
5. Production step. In the `ExceptionDisposition::Handled` arm, replace
   `current_ex = next;` with `next.handle_error(); current_ex = next;`.
6. Production step. In the `ExceptionDisposition::Continued` arm, replace
   `current_ex = next;` with `next.clear_error(); current_ex = next;`.
7. Leave the `_` (Propagate/fail-closed) arm, `PipelineOutcome::Failed`,
   `PipelineOutcome::Stopped`, both predicate-error returns, and the
   `catch_matched == false` return byte-for-byte unchanged.
8. Re-run `cargo test -p camel-processor --lib do_try_segment`; all tests
   green (GREEN capture).
9. In `crates/camel-processor/CONTEXT.md`, in the "Structural EIP Segments"
   table, extend the `DoTrySegment` row description to: "Error-handling block —
   runs body; on error, matches `catch` clauses. A `Handled` or `Continued`
   catch clears the exchange error before `finally` runs; `Handled` also sets
   `CamelExceptionHandled`. Stop from body or catch propagates." Change no
   other line of that file.
10. Commit the task (worker contract: one commit per task) so the fix is at
    `HEAD`. Task 1.2's RED procedure depends on this commit: it reverts the two
    disposition lines in the working tree and restores them with
    `git checkout --`, which only recovers the fix if the fix is committed.

**Tests:** (executable spec — name, arrange, act, assert)
- `handled_catch_clears_error_and_marks_handled`: arrange a `DoTrySegment`
  with `try_body = seg_fail(CamelError::ProcessorError("boom".into()))`, one
  `CatchClauseSegment { matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
  on_when: None, body: seg_check_then_complete(|ex| { assert!(ex.has_error(),
  "catch body must see the try error"); assert!(matches!(
  ex.properties.get(PROPERTY_EXCEPTION_MESSAGE), Some(Value::String(s)) if
  s.contains("boom")), "catch body must see the derived exception message, got {:?}",
  ex.properties.get(PROPERTY_EXCEPTION_MESSAGE)); }), disposition:
  ExceptionDisposition::Handled }`, `finally: None` → act
  `seg.run(Exchange::default()).await` → assert the outcome matches
  `PipelineOutcome::Completed(ex)` with `!ex.has_error()`,
  `ex.properties.get(PROPERTY_EXCEPTION_HANDLED) == Some(&Value::Bool(true))`,
  and `!ex.properties.contains_key(PROPERTY_EXCEPTION_MESSAGE)`. The derived
  message is `CamelError::ProcessorError("boom").to_string()` — assert with
  `contains("boom")` rather than an exact string so the test does not depend on
  the `Display` prefix.
  Command: `cargo test -p camel-processor --lib do_try_segment::tests::handled_catch_clears_error_and_marks_handled`.
  Expected before step 5: FAIL on the post-block `!ex.has_error()` assertion.
  After step 5: PASS.
- `continued_catch_clears_error_without_handled_marker`: same arrangement, but
  seed the input exchange with `PROPERTY_EXCEPTION_HANDLED = Value::Bool(true)`
  (the segment removes a stale marker on entry, so the marker found after the
  run can only come from the disposition arm) and use `disposition:
  ExceptionDisposition::Continued` → act `seg.run(ex)` → assert `Completed(ex)`
  with `!ex.has_error()`, `!ex.properties.contains_key(PROPERTY_EXCEPTION_HANDLED)`,
  and `ex.properties.get(PROPERTY_EXCEPTION_MESSAGE).is_none()`.
  Command: `cargo test -p camel-processor --lib do_try_segment::tests::continued_catch_clears_error_without_handled_marker`.
  Expected before step 6: FAIL on `!ex.has_error()`. After step 6: PASS.
- `finally_after_handled_catch_sees_no_error`: arrange
  `try_body = seg_fail(ProcessorError("boom"))`, catch `ByVariant(["ProcessorError"])`
  with `body: seg_complete()` and `disposition: Handled`, and
  `finally: Some(FinallyClauseSegment { on_when: None,
  body: seg_check_then_complete(move |ex| { finally_calls.fetch_add(1,
  Ordering::SeqCst); assert!(!ex.has_error(),
  "finally must observe a clean exchange after a handled catch");
  assert!(ex.properties.get(PROPERTY_EXCEPTION_MESSAGE).is_none(),
  "derived exception properties must be cleared"); }) })`, where
  `finally_calls = Arc::new(AtomicU32::new(0))` and the closure captures a
  clone → act `seg.run(Exchange::default()).await` → assert `Completed(ex)`
  with `!ex.has_error()` **and** `finally_calls.load(Ordering::SeqCst) == 1`
  (the spec's "finally body runs exactly once" THEN clause).
  Command: `cargo test -p camel-processor --lib do_try_segment::tests::finally_after_handled_catch_sees_no_error`.
  Expected before step 5: FAIL inside the finally check.
  After step 5: PASS.
- `catch_by_variant_handled_returns_ok` (existing, strengthen): change the
  trailing `assert!(matches!(result, PipelineOutcome::Completed(_)))` to a
  `Completed(ex)` binding that also asserts `!ex.has_error()` and
  `ex.properties.get(PROPERTY_EXCEPTION_HANDLED) == Some(&Value::Bool(true))`.
  Keep the failure-message style already used in the file.
- `nested_do_try_inner_catch_does_not_leak_to_outer` (existing, strengthen):
  bind `Completed(ex)` and add `assert!(!ex.has_error(), "inner handled error
  must not leak to the outer exchange")`. Do not add any assertion about
  `CamelExceptionHandled` here.

Unchanged regression guards (run them, do not edit them). They own the
requirement's "unchanged paths" clause and the `Propagate` scenario:
- `catch_by_variant_propagate_runs_side_effects_and_rethrows` — owns the spec
  scenario "Propagate catch keeps the error"; it already asserts
  `PipelineOutcome::Failed(CamelError::ProcessorError(_))`.
- `catch_body_failure_runs_finally_once_then_returns_catch_err` — owns the
  catch-body-failure exit.
- `no_clause_matches_runs_finally_and_propagates_original` — owns the
  no-match exit.
- `stop_inside_try_skips_catch_and_finally` and
  `stop_inside_catch_skips_finally` — own the Stop exits.

**Acceptance:**
- `cargo test -p camel-processor --lib do_try_segment` passes (exit 0), with
  the RED output for the three new tests captured in the worker report.
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.
- `git -C <worktree> diff HEAD~1 -- crates/camel-processor/src/do_try_segment.rs`
  shows production changes confined to the match-arm binding and the two
  disposition arms (the rest of the diff is test-only, inside `mod tests`).
- No new `unwrap()` outside `mod tests` (`cargo xtask lint-unwrap` exits 0).
- The `DoTrySegment` row in `crates/camel-processor/CONTEXT.md` states the
  clear-on-Handled/Continued contract.
- The task is committed and `git -C <worktree> status --short` lists no file
  under `crates/` (the conductor's `tasks.md` checkbox may be the only
  pending change).

- [x] 1.1

## camel-test

### Task 1.2: Pin the Unit-of-Work contract with an end-to-end handled doTry route

**Files:**
- `crates/camel-test/tests/do_try_test.rs` (modified)

**Steps:**
1. Add a Rust builder failure-injection route `direct:failing-step` (route id
   `failing-step`) whose single `.process_fn(BoxProcessor::from_fn(|_ex: Exchange|
   Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })))` always
   throws, mirroring the existing `do_try_yaml_on_when_predicate_filters_catch_e2e`
   test.
2. Add the UoW route through `parse_yaml` (the builder has no `on_complete`
   hook), with a handled `doTry`:
   ```yaml
   routes:
     - id: "do-try-uow"
       from: "direct:uow"
       on_complete: "mock:ok"
       on_failure: "mock:fail"
       steps:
         - do_try:
             steps:
               - to: "direct:failing-step"
             catch:
               - exception: ["ProcessorError"]
                 disposition: handled
                 steps:
                   - set_body: "caught"
   ```
3. Build the harness with `.with_direct().with_mock()`, add both routes,
   `h.start().await`, then send with the existing no-sleep helper
   `send_when_registered(&h, "direct:uow", Exchange::default()).await` (do NOT
   add `tokio::time::sleep`, do NOT use `send_to_direct` — `lint-test-sleep`
   hygiene; the UoW hooks are awaited inline by `ExchangeUoWLayer::call`, so
   the `direct:` producer's `oneshot` that `send_when_registered` awaits
   returns only after both hooks ran).
4. Assert the send succeeded (`expect` on the `Result` with a message naming
   the route), then `h.mock().get_endpoint("ok").unwrap().await_exchanges(1,
   Duration::from_secs(5)).await` (notification-based, no sleep), then
   `assert_exchange_count(1)` on `ok` and `assert_exchange_count(0)` on `fail`.
5. Precondition: Task 1.1's fix is committed at `HEAD`
   (`git -C <worktree> log --oneline -1` names it, and
   `git -C <worktree> status --short` lists no file under `crates/`). If it is
   not committed, STOP and report — do not continue.
6. Verify RED before accepting GREEN: temporarily edit the two disposition arms
   in `crates/camel-processor/src/do_try_segment.rs` back to
   `current_ex = next;` without `handle_error()`/`clear_error()`; run
   `cargo test -p camel-test --test do_try_test do_try_handled_catch_fires_on_complete_not_on_failure`;
   capture the failure (pre-fix the `on_failure` hook fires: either the 5 s
   `await_exchanges` on `mock:ok` times out, or `mock:fail` holds 1 exchange);
   then restore the committed fix with
   `git -C <worktree> checkout -- crates/camel-processor/src/do_try_segment.rs`.
7. Re-run the test; it passes. Confirm `git -C <worktree> status --short` lists
   only `crates/camel-test/tests/do_try_test.rs` (plus the conductor's
   `tasks.md` checkbox edit, if any).
8. `h.stop().await` at the end, matching the other tests in the file.

**Tests:** (executable spec — name, arrange, act, assert)
- `do_try_handled_catch_fires_on_complete_not_on_failure`: arrange the two
  routes above on a `direct:` + `mock:` harness → act
  `send_when_registered(&h, "direct:uow", Exchange::default())` → assert the
  send is `Ok`, `mock:ok` received exactly 1 exchange, `mock:fail` received
  exactly 0. Command:
  `cargo test -p camel-test --test do_try_test do_try_handled_catch_fires_on_complete_not_on_failure`.
  Expected before the Task 1.1 fix: FAIL (`mock:ok` never reaches 1, so the
  `await_exchanges` deadline elapses; `mock:fail` holds the 1 exchange).
  After the fix: PASS.

**Acceptance:**
- `cargo test -p camel-test --test do_try_test` passes (exit 0) — the whole
  file, not only the new test.
- The new test contains no `tokio::time::sleep`.
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-test --all-targets -- -D warnings` exits 0.
- `git -C <worktree> status --short` lists only
  `crates/camel-test/tests/do_try_test.rs` for this task (plus
  `openspec/changes/do-try-error-clear/tasks.md` when the conductor ticks the
  checkbox).
- RED output captured for the new test (with the fix temporarily reverted) and
  GREEN output after restoration.

- [x] 1.2
