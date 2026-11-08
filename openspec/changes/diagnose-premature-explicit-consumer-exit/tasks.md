# Tasks: diagnose-premature-explicit-consumer-exit

## Runtime and Component SPI

### Task 1.1: Diagnose successful Explicit completion without a background handle

**Files:**
- `crates/camel-core/src/lifecycle/adapters/consumer_management.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/consumer_exit_tests.rs` (new)
- `crates/components/camel-component-api/src/consumer.rs` (modified)
- `crates/components/camel-component-api/CONTEXT.md` (modified)

**Steps:**
1. Add a `#[cfg(test)]` path module named `consumer_exit_tests` in consumer_management.rs, backed by the new sibling file. Keep new fixtures and tests out of the existing large inline test module.
2. In consumer_exit_tests.rs introduce a small test Consumer that can signal readiness, cancel its context, and return a completing background handle. Track `stop()` calls and captured warning fields. Add a tracing Layer recording actual warning events from the consumer_management target, with a field Visitor to inspect message, route_id, and readiness_fallback. Use the established bare-global-registry initialization pattern, scoped subscriber, current-thread runtimes, and bounded joins. Give fixture options named fields and clear enum parameters for distinct modes.
3. Write the five tests below first. Run the `explicit_consumer_no_handle_warns` subset and confirm both already-ready and missing-readiness expectations fail on the unchanged production code. Do not count compilation errors as the red phase.
4. In only the Explicit branch after `background_task_handle()`, replace the existing conditional fallback warning with a no-handle block: retain `mark_ready()` and record its return value as `readiness_fallback`; when the context is not cancelled, emit one warning with route_id and that field, stating that stop follows immediately and suggesting keeping start alive or returning a background handle for consumers intended to remain active. Leave all monitoring, failure publication, stop calls, and outcome accounting unchanged.
5. Extend Consumer shutdown/background-handle rustdoc and Component SPI CONTEXT.md: readiness does not extend lifetime; no handle means stop follows start completion. Long-lived consumers must remain in start until cancellation or return a handle. Finite completion remains valid and is only diagnosed before cancellation. Do not alter API signatures.
6. Run the regression suite and existing consumer_management tests. Run the full camel-core library suite and applicable project quality gates, recording results honestly. Do not fix unrelated gate failures inside this task.

**Tests:**
- `explicit_consumer_no_handle_warns_after_ready`: arrange a ready Explicit fixture with no handle and an uncancelled context; act through spawn_consumer_task for initial start and resume; assert bounded successful startup/join, exactly one warning naming the route with readiness_fallback=false and the cleanup/lifetime advice, warning observable before stop, and exactly one stop. Command: `cargo test -p camel-core --lib explicit_consumer_no_handle_warns_after_ready`; expected red before step 4, green after.
- `explicit_consumer_no_handle_warns_with_readiness_fallback`: arrange the same fixture without mark_ready; act for initial start and resume; assert startup resolves successfully via fallback, one warning with readiness_fallback=true, and one stop. Command: `cargo test -p camel-core --lib explicit_consumer_no_handle_warns_with_readiness_fallback`; expected red before step 4, green after.
- `cancelled_explicit_consumer_no_handle_is_silent`: arrange an Explicit fixture that cancels its context before returning, with and without mark_ready; act for initial start and resume; assert startup still resolves, cleanup runs once, and zero no-handle completion warnings are dispatched. Command: `cargo test -p camel-core --lib cancelled_explicit_consumer_no_handle_is_silent`; expected green after step 4, including the readiness-fallback shutdown case.
- `explicit_consumer_background_handle_is_silent`: arrange a ready Explicit fixture returning a completing background handle before cancellation; act for initial start and resume; assert startup/join succeed, cleanup runs once, and zero no-handle completion warnings. Command: `cargo test -p camel-core --lib explicit_consumer_background_handle_is_silent`; expected green before and after.
- `immediate_consumer_no_handle_is_silent`: arrange an Immediate fixture returning Ok without a handle or cancellation; act for initial start and resume; assert startup/join succeed, cleanup runs once, and zero no-handle completion warnings. Command: `cargo test -p camel-core --lib immediate_consumer_no_handle_is_silent`; expected green before and after.
- Existing `outer_task_watcher_silent_on_normal_completion`, `outer_task_watcher_silent_on_normal_completion_with_bg`, `outer_task_watcher_silent_on_cancel`, and `outer_task_watcher_failroute_on_panic_in_stop` remain green under `cargo test -p camel-core --lib consumer_management`, confirming the new diagnostic does not alter route-failure publication.

**Acceptance:**
- The red/green regression commands produce the expected outcomes.
- `cargo test -p camel-core --lib` passes, including the new warning-field/cancellation coverage and existing supervision tests.
- `cargo fmt --check --all` and relevant clippy checks pass; other required gate results are recorded with any pre-existing failures or explicit N/A reasons.
- The delta requirement and all five scenarios in specs/consumer-activation/spec.md are covered by the tests above. No new public signatures, dependencies, or lifecycle-state transitions appear in the diff.

- [x] 1.1
