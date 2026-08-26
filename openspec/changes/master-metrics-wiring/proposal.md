# Proposal: master-metrics-wiring

## Why

bd rc-tpv4 (MST-001): the `camel-master` component stores a
`MetricsCollector` in `MasterConsumer` and `ReconcileContext` but
never calls it. Leadership transitions and delegate lifecycle churn
are unmeasured: an operator running master-elected routes cannot tell
how often leadership flips or how often delegates start, stop, or
fail to construct. Four `TODO(MST-001)` markers document the gap
(`consumer.rs`, `endpoint.rs`, two sites in `leadership.rs`),
including placeholder `info!` lines that stand in for real emission.

## What Changes

Wire the already-plumbed `MetricsCollector` through the leadership
supervision path in `crates/components/camel-master`:

- Emit `master_leadership_transitions_total` (labels `lock`,
  `route_id`, `event=acquired|lost`) exactly once per observed
  leadership-state TRANSITION — not-leading→leading emits `acquired`,
  leading→not-leading emits `lost`. The initial watch snapshot
  establishes the first state (a snapshot already leading is one
  `acquired` transition); repeated identical watch deliveries and
  synthetic `StartedLeading` re-dispatches from the delegate retry
  tick emit nothing, so retries never recount an acquisition.
- Emit `master_delegate_lifecycle_total` with a UNIFORM label schema
  on every observation (`lock`, `route_id`, `event`, `reason`):
  `event=started` with `reason=none` after a delegate consumer spawns
  and the state becomes `Active`; `event=stopped` with `reason=none`
  when `stop_delegate` drains an `Active` delegate (Inactive no-op
  emits nothing); `event=create_error` with
  `reason=transient|permanent` at the endpoint and consumer
  construction arms, matching the existing retry classification.
  Uniform keys are mandatory: the Prometheus collector drops
  observations whose label keys drift from the first observation of a
  metric name.
- `stop_delegate` gains `lock_name`, `route_id`, and `metrics`
  parameters (call sites: two in `leadership.rs`, two in
  `supervision.rs`, plus existing tests).
- Emit through the existing generic `record_counter` trait method —
  no `MetricsCollector` trait change, no backend change.
- Delete the four `TODO(MST-001)` comments and the two placeholder
  `info!` lines.
- Deterministic tests using the module-level mock harness in
  `src/tests.rs` (`ErrorDelegateComponent`, `ErrorDelegateEndpoint`,
  `SuccessDelegateConsumer`, `FakeLeadershipService`,
  `FakePlatformService`) plus a recording `MetricsCollector`
  (established pattern, cf. `cache_eip.rs` tests), including
  error-injecting delegate mocks for the four construction-failure
  cases and a retry-accumulation case.

Excluded: metric backends (`OtelMetrics`/`PrometheusMetrics` already
implement `record_counter`); leadership-tenure histograms; any
behavioral change to fencing, epochs, retry classification, or
supervision control flow; k8s Lease metadata (rc-j94g).

## Acceptance criteria

- Every observed leadership-state transition emits the leadership
  counter exactly once, labeled with lock and route, immediately
  before `reconcile_event` runs; repeated identical deliveries and
  retry-tick re-dispatches emit nothing.
- Delegate start, Active drain, and each construction-error
  classification emit the lifecycle counter with identical label
  keys; stopping an already `Inactive` delegate emits nothing.
- One acquisition followed by repeated failed construction attempts
  records exactly one `acquired` transition and one `create_error`
  per attempt.
- No `TODO(MST-001)` marker or placeholder emission line remains.
- `cargo test -p camel-master` green including new metric assertions;
  full quality gates green.

## Risk budget

Emission is synchronous, performs no awaited operation, and cannot
alter control flow through a returned error; leadership semantics,
retry classification, and epoch fencing stay byte-identical. Label
cardinality is bounded by lock count, route count, and closed
`event`/`reason` value sets — acceptable. Any change beyond emission
call sites, the `stop_delegate` signature widening, comment removal,
and tests (trait edits, new dependencies, unbounded label values) is
out of bounds.
