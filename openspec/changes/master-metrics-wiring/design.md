# Design: master-metrics-wiring

## Approach

Single-crate wiring change. `MetricsCollector` already flows
`component.rs → endpoint.rs → MasterConsumer → ReconcileContext`;
only emission call sites are missing. All emission uses the generic
`record_counter(name, value, labels)` trait method (default no-op,
implemented by `OtelMetrics` and `PrometheusMetrics`) — no trait or
backend change.

Two metrics, two emission rules:

**Leadership transitions — observed state edges only.** The
supervision loop dispatches `reconcile_event` from three sites: the
initial watch snapshot, watch-channel deliveries, and a synthetic
retry-tick re-dispatch of `StartedLeading` used after transient
construction failures or delegate exit (`supervision.rs`, retry_tick
arm). Transition counters must count leadership-state transitions,
not dispatches, so emission lives at the two OBSERVED sites in
`supervision.rs` (initial snapshot and `events.changed()`), NOT
inside `reconcile_event`, gated on the leading-state edge. The loop
already computes the edge at the `events.changed()` site
(`was_leading` vs `is_leading`); the initial site starts from
`is_leading = false`, so a snapshot already leading is one
`false → true` edge. A helper in `leadership.rs` —

`emit_leadership_transition(metrics: &Arc<dyn MetricsCollector>, lock: &str, route_id: &str, event: &str)`

— is called immediately BEFORE the `reconcile_event` call (the
transition logs live inside `reconcile_event`), with `event` =
`acquired` for a `false → true` edge and `lost` for a `true → false`
edge. Repeated identical watch deliveries are not transitions and
emit nothing; synthetic re-dispatches emit nothing.

**Delegate lifecycle — attempt-scoped, uniform schema.** Emitted
inside `reconcile_event` and `stop_delegate` because each successful
construction attempt and each real drain is one observation.
Every observation of `master_delegate_lifecycle_total` carries the
SAME label keys — `lock`, `route_id`, `event`, `reason` — where
`reason` is `none` for `started`/`stopped` and
`transient`/`permanent` for `create_error`. This is mandatory: the
Prometheus collector matches label keys against the first observation
of a name and silently drops later arity/key drift
(`camel-prometheus/src/metrics.rs`, `keys_match` guard). Sites:

1. `reconcile_event`, `StartedLeading` — after the state becomes
   `DelegateState::Active`: `event=started`, `reason=none`.
2. The two construction-error arms (endpoint create, consumer
   create) — before the swallow/propagate branch:
   `event=create_error`, `reason=transient|permanent` per the
   existing `is_retryable_camel_error` classification. Each failed
   attempt emits once; retries accumulate honestly.
3. `stop_delegate` — when the state matched `Active` and the drain
   completed: `event=stopped`, `reason=none`. The `Inactive` no-op
   path emits nothing.

`stop_delegate` widens to
`stop_delegate(state, drain_timeout, lock_name, route_id, metrics)`;
call sites: two in `leadership.rs` (fields from `ctx`), two in
`supervision.rs` (task-scope variables), plus the existing
`leadership.rs` module tests updated with dummy values.

The two placeholder `tracing::info!` lines ("metrics emission
placeholder") are deleted; the real `info!` transition logs remain.
Stale `TODO(MST-001)` comments in `consumer.rs` and `endpoint.rs` are
deleted.

Naming follows repo convention: snake_case `_total` counters, short
lowercase label values (cf. `tls_reloads_total`,
`template_reloads_total` in `runtime_bus.rs`).

## Affected crates

- `camel-master`: emission helper + sites in `leadership.rs`;
  observed-event emission + `stop_delegate` call updates in
  `supervision.rs`; `stop_delegate` signature widening; comment
  removal in `consumer.rs`, `endpoint.rs`; recording-mock and
  error-injecting tests. No public API change (all touched items are
  `pub(crate)`).

## Architecture boundaries

Components → Services dependency direction respected: `camel-master`
(Components layer) consumes the `camel-api` `MetricsCollector` port;
no service-layer crate is touched, no new dependency. Counters are
strictly observational: leadership state machine, epoch fencing
(ADR-0035), retry classification, and drain semantics are unchanged.
Hexagonal boundary test untouched (no port or adapter changes).

## Phases

Single-phase: one coherent slice (emission sites + signature
widening + tests + comment cleanup) with no milestone value in
splitting.

## Test strategy

Reuse the crate-internal module-level harness in `src/tests.rs`
(`ErrorDelegateComponent`, `ErrorDelegateEndpoint`,
`SuccessDelegateConsumer`, `FakeLeadershipService`,
`FakePlatformService`) and add a `RecordingMetricsCollector`
(Mutex-Vec of `(name, value, labels)` tuples — established pattern,
cf. `cache_eip.rs`), plus whatever additional error-injecting
endpoint/consumer variants the four construction-failure cases need
(endpoint-stage vs consumer-stage failures, transient vs permanent).
Unit-drive `reconcile_event` and `stop_delegate` directly (they are
`pub(crate)` and the harness is in-crate); drive supervision-level
edge/synthetic emission through `MasterConsumer::start` with
`FakeLeadershipService` delivering scripted events, asserting exact
emitted tuples: acquired/lost on state edges only, repeated identical
deliveries and retry re-dispatches add nothing beyond `create_error`
observations, started/stopped/inactive-silence, and the four
construction-failure cases. Synchronization follows the existing
harness idioms: bounded `tokio::time::timeout` wrappers, exchange
receipt or call-counter polling for success paths, and
`leadership_task.is_finished()` polling with short (5 ms) sleep
steps for failure/exhaustion paths (the pattern used by the existing
failure tests). Retries advance on the 200 ms `DELEGATE_RETRY_INTERVAL`
tick, so test budgets scale with `max_attempts`.
