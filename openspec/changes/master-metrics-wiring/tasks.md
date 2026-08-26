# Tasks: master-metrics-wiring

## camel-master

### Task 1.1: Wire all metric emission sites and widen stop_delegate

**Files:**
- `crates/components/camel-master/src/leadership.rs` (modified)
- `crates/components/camel-master/src/supervision.rs` (modified)
- `crates/components/camel-master/src/consumer.rs` (modified)
- `crates/components/camel-master/src/endpoint.rs` (modified)

**Steps:**
1. In `leadership.rs`, add `pub(crate) fn emit_leadership_transition(metrics: &Arc<dyn MetricsCollector>, lock: &str, route_id: &str, event: &str)` that calls `metrics.record_counter("master_leadership_transitions_total", 1.0, &[("lock", lock), ("route_id", route_id), ("event", event)])`.
2. In `leadership.rs`, add `pub(crate) fn emit_lifecycle(metrics: &Arc<dyn MetricsCollector>, lock: &str, route_id: &str, event: &str, reason: &str)` that calls `metrics.record_counter("master_delegate_lifecycle_total", 1.0, &[("lock", lock), ("route_id", route_id), ("event", event), ("reason", reason)])`. All lifecycle emission goes through this helper so the uniform label schema is structural.
3. In `leadership.rs` `reconcile_event` `StartedLeading` arm: delete the `TODO(MST-001)` comment and the placeholder `tracing::info!("metrics emission placeholder: leadership acquired")` line.
4. In the same arm, at each of the two construction-error branches (endpoint `create_endpoint` error, endpoint `create_consumer` error), insert before the `is_retryable_camel_error` branch: compute `let is_retryable = is_retryable_camel_error(&err);` then `emit_lifecycle(ctx.metrics, ctx.lock_name, ctx.route_id.as_str(), "create_error", if is_retryable { "transient" } else { "permanent" });` and reuse `is_retryable` in the existing branch decision (the check is a pure match on `&err` — hoisting is behavior-preserving).
5. In the same arm, immediately after `*state = DelegateState::Active { .. }`: `emit_lifecycle(ctx.metrics, ctx.lock_name, ctx.route_id.as_str(), "started", "none");`.
6. In `reconcile_event` `StoppedLeading` arm: delete the `TODO(MST-001)` comment and the placeholder `tracing::info!("metrics emission placeholder: leadership lost")` line.
7. Widen `stop_delegate` to `pub(crate) async fn stop_delegate(state: &mut DelegateState, drain_timeout: Duration, lock_name: &str, route_id: &str, metrics: &Arc<dyn MetricsCollector>)`. Inside the branch that matched `DelegateState::Active` and completed the drain: `emit_lifecycle(metrics, lock_name, route_id, "stopped", "none");`. The `Inactive` path emits nothing.
8. Update the two `stop_delegate` call sites inside `leadership.rs` (`reconcile_event` arms) to pass `ctx.lock_name`, `ctx.route_id.as_str()`, `ctx.metrics`.
9. Update the two `stop_delegate` call sites in `supervision.rs` (retry-tick dead-handle cleanup and post-loop final cleanup) to pass `rctx.lock_name`, `rctx.route_id.as_str()`, and `rctx.metrics` (the task-scope `route_id` is moved into `ReconcileContext`, so read it back through `rctx`).
10. In `supervision.rs`, at the initial-snapshot site: after `is_leading` is computed from the snapshot event and before `reconcile_event` is called, when the snapshot establishes a `false → true` leading edge, call `emit_leadership_transition(rctx.metrics, rctx.lock_name, rctx.route_id.as_str(), "acquired")` (`rctx` is built before the initial-snapshot block, so the task-scope `route_id` is already moved into it).
11. In `supervision.rs`, at the `events.changed()` site: after `was_leading`/`is_leading` are computed and before `reconcile_event` is called, call `emit_leadership_transition` with `"acquired"` when `!was_leading && is_leading` and `"lost"` when `was_leading && !is_leading`, using `rctx.metrics`, `rctx.lock_name`, `rctx.route_id.as_str()`; no emission when the state did not change.
12. Delete the `TODO(MST-001)` comment block in `consumer.rs` (lines around 13-14) and in `endpoint.rs` (line around 18).
13. Update the existing `stop_delegate` drain tests in the `leadership.rs` test module (around lines 461 and 513) to pass the three new arguments (any `&str` lock/route values and `Arc::new(NoOpMetrics)`).

**Tests:** (existing tests must stay green — new tests come in 1.2/1.3)
- `stop_delegate_drains_bridge_on_delegate_error` (existing, updated): signature update only → still passes.
- `stop_delegate_drains_bridge_on_delegate_timeout` (existing, updated): signature update only → still passes.
- `command`: `cargo test -p camel-master` — **expected**: all existing tests pass before 1.2/1.3 add new ones.

**Acceptance:**
- `rg -n "TODO\(MST-001\)|metrics emission placeholder" crates/components/camel-master/src/` returns zero hits.
- `cargo test -p camel-master` exits 0 (existing tests with widened signatures).
- `cargo fmt --check --all` and `cargo clippy -p camel-master --all-targets -- -D warnings` exit 0.

- [x] 1.1

### Task 1.2: Recording collector harness + delegate lifecycle metric tests

**Files:**
- `crates/components/camel-master/src/tests.rs` (modified)

**Steps:**
1. Add a module-level `struct RecordingMetricsCollector { events: Mutex<Vec<(String, f64, Vec<(String, String)>)>> }` implementing `MetricsCollector`: the five classic methods as no-ops; `record_counter` pushes `(name.to_string(), value, labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())`. Add a helper `fn counters_named(&self, name: &str) -> Vec<(f64, Vec<(String, String)>)>` (or an equivalent filtered accessor) and a `Clone`-able `Arc` wrapper pattern matching the existing test style.
2. Add a mandatory sibling builder `fn build_error_delegate_master_with_metrics(platform_service: Arc<dyn PlatformService>, create_endpoint_calls: Arc<AtomicUsize>, create_consumer_calls: Arc<AtomicUsize>, endpoint_error: Option<CamelError>, consumer_error_after: usize, consumer_error: Option<CamelError>, max_attempts: u32, metrics: Arc<dyn MetricsCollector>) -> MasterConsumer` that passes `metrics` into `MasterConsumer::new`. Rework the existing `build_error_delegate_master` to delegate to it with `Arc::new(NoOpMetrics)` so existing tests compile unchanged. Recording-collector access is always through this builder — no ad-hoc equivalent.
3. Write the tests listed below using `FakePlatformService`/`FakeLeadershipService` (event-watch mechanism as in existing tests), `ErrorDelegateComponent` with configured `endpoint_error`/`consumer_error_after`/`consumer_error`, and the recording collector. Add a `fn transient_io_error() -> CamelError` helper returning `CamelError::Io("boom".to_string())` (`CamelError::Io` carries a `String`; transient per `is_retryable_camel_error` matching `Io(_)` in `camel-component-api/src/network_retry.rs`); use `CamelError::ProcessorError("permanent".to_string())` (no `[TRANSIENT]` marker) / `CamelError::EndpointCreationFailed("permanent".to_string())` for permanent ones.

4. Synchronization mechanics for all tests below: wrap each await in `tokio::time::timeout` with a generous bound (≥ 5 s); for "run to success" await the delegate's first exchange (SuccessDelegateConsumer sends `"ok"` through the pipeline sender) or poll `create_consumer_calls`/`create_endpoint_calls` counters; for "await task failure" and "await budget-exhaustion shutdown" poll `master.leadership_task` completion via `is_finished()` using the existing 5 ms poll-sleep pattern (tests.rs ~792-805); for "let the loop process it" await counter/watch confirmation before asserting. Retries advance on the 200 ms `DELEGATE_RETRY_INTERVAL` tick — budgets must accommodate `max_attempts × 200 ms`.
5. Every lifecycle assertion below checks the COMPLETE tuple: name `master_delegate_lifecycle_total`, value `1.0`, and labels exactly `[("lock", lock), ("route_id", route), ("event", <event>), ("reason", <reason>)]` — partial-label matches are not acceptable. Transition assertions check name, `1.0`, and labels exactly `[("lock", lock), ("route_id", route), ("event", <event>)]`.

**Tests:**
- `lifecycle_started_emitted_on_acquisition`: a delegate component whose endpoint and consumer construct successfully, leadership snapshot already leading → run `master.start(ctx)`, await the delegate's first exchange via the pipeline sender → assert `counters_named("master_delegate_lifecycle_total")` equals exactly one observation `1.0` with labels `[("lock", lock), ("route_id", route), ("event", "started"), ("reason", "none")]`.
- `lifecycle_stopped_emitted_after_active_drain`: same setup, then deliver `StoppedLeading` on the watch and await the drain (bounded timeout) → assert exactly two complete lifecycle tuples: the `started` one and one `1.0` with labels exactly `[("lock", lock), ("route_id", route), ("event", "stopped"), ("reason", "none")]`.
- `inactive_stop_emits_nothing`: leadership snapshot never leading → `master.start(ctx)`, then cancel and `master.stop()` → assert `counters_named("master_delegate_lifecycle_total")` is empty.
- `create_error_endpoint_transient`: `endpoint_error = Some(transient_io_error())`, `max_attempts = 1` → start, await budget-exhaustion shutdown → assert exactly two complete tuples `1.0` with labels exactly `[("lock", lock), ("route_id", route), ("event", "create_error"), ("reason", "transient")]` (the initial-snapshot attempt is not budget-gated; `should_retry(0)` is `true` with `max_attempts = 1`, so one retry follows the initial attempt).
- `create_error_endpoint_permanent`: `endpoint_error = Some(CamelError::EndpointCreationFailed("permanent".into()))` → start, await task failure → assert exactly one complete tuple `1.0` with labels exactly `[("lock", lock), ("route_id", route), ("event", "create_error"), ("reason", "permanent")]`.
- `create_error_consumer_transient`: `consumer_error_after = 2`, `consumer_error = Some(transient_io_error())`, `max_attempts = 3` → start and run to success → assert exactly three complete `master_delegate_lifecycle_total` tuples of value `1.0`, in order: two with labels exactly `[("lock", lock), ("route_id", route), ("event", "create_error"), ("reason", "transient")]` then one with labels exactly `[("lock", lock), ("route_id", route), ("event", "started"), ("reason", "none")]`.
- `create_error_consumer_permanent`: `consumer_error_after = 1`, `consumer_error = Some(CamelError::ProcessorError("permanent".into()))` → start, await task failure → assert exactly one complete tuple `1.0` with labels exactly `[("lock", lock), ("route_id", route), ("event", "create_error"), ("reason", "permanent")]`.
- `retry_accumulation_one_transition_n_create_errors`: `consumer_error_after = 3`, `consumer_error = Some(transient_io_error())`, `max_attempts = 4` → start and run to success → assert exactly one complete `master_leadership_transitions_total` tuple (value `1.0`, labels exactly `[("lock", lock), ("route_id", route), ("event", "acquired")]`) and exactly three complete `master_delegate_lifecycle_total` tuples of value `1.0` with labels exactly `[("lock", lock), ("route_id", route), ("event", "create_error"), ("reason", "transient")]`.
- `command`: `cargo test -p camel-master` — **expected**: all listed tests pass; they fail before Task 1.1's emission sites exist.

**Acceptance:**
- All eight tests above pass under `cargo test -p camel-master`.
- `cargo fmt --check --all` and `cargo clippy -p camel-master --all-targets -- -D warnings` exit 0.

- [x] 1.2

### Task 1.3: Leadership transition edge-semantics tests

**Files:**
- `crates/components/camel-master/src/tests.rs` (modified)

**Steps:**
1. Using the same harness from 1.2, write the tests below. Drive the leadership watch through `FakeLeadershipService` (initial snapshot value plus subsequent deliveries, matching how existing supervision tests script events). Filter with `counters_named("master_leadership_transitions_total")` and assert exact tuples.

**Tests:**
- `transition_acquired_on_initial_snapshot`: leadership snapshot already leading, delegate constructs successfully → assert exactly one complete `master_leadership_transitions_total` tuple: value `1.0`, labels exactly `[("lock", lock), ("route_id", route), ("event", "acquired")]`.
- `transition_lost_on_leading_edge`: start leading, then deliver `StoppedLeading` and await the drain (bounded timeout) → assert exactly two complete `master_leadership_transitions_total` tuples of value `1.0`: one with `("event", "acquired")`, one with `("event", "lost")`, each carrying the exact labels `[("lock", lock), ("route_id", route), ("event", "acquired")]` and `[("lock", lock), ("route_id", route), ("event", "lost")]` respectively.
- `repeated_identical_delivery_does_not_reemit`: two phases in one test. Phase A: start leading, then deliver `StartedLeading` again (true→true, no edge) and let the loop process it → the count of complete `master_leadership_transitions_total` tuples with labels exactly `[("lock", lock), ("route_id", route), ("event", "acquired")]` is still exactly one. Phase B: from not-leading (fresh consumer or after a `lost` edge), deliver `StoppedLeading` twice in a row (false→false, no edge) → the count of complete tuples with labels exactly `[("lock", lock), ("route_id", route), ("event", "lost")]` is unchanged (exactly the one from the preceding edge, or zero if never led).
- `synthetic_retry_does_not_reemit_transition`: `consumer_error_after = 3` with transient `Io` error and `max_attempts = 4` (the retry tick re-dispatches synthetic `StartedLeading` three times) → run to success → assert the total count of `master_leadership_transitions_total` tuples with `("event", "acquired")` is exactly one.
- `transition_counted_despite_permanent_endpoint_failure`: `endpoint_error = Some(CamelError::EndpointCreationFailed("permanent".into()))` → start, await task failure → assert exactly one complete `master_leadership_transitions_total` tuple of value `1.0` with labels exactly `[("lock", lock), ("route_id", route), ("event", "acquired")]` was recorded before the failure.
- `command`: `cargo test -p camel-master` — **expected**: all listed tests pass.

**Acceptance:**
- All five tests above pass under `cargo test -p camel-master`.
- `cargo fmt --check --all` and `cargo clippy -p camel-master --all-targets -- -D warnings` exit 0.

- [x] 1.3
