## ADDED Requirements

### Requirement: Bounded detached-task admission with caller back-pressure

The WireTap EIP SHALL admit tap tasks through a bounded concurrency gate
whose permits are acquired BEFORE a tap task is detached. The bound is the
maximum number of DETACHED (concurrently-running) tap tasks; the default
bound SHALL be exactly 20, set in `WireTapConfig::default()`. When no
permit is available, the tap SHALL execute inline on the calling future
(the `CallerRuns` policy) so that the main route is back-pressured. Under
saturation with a single sequential caller, total concurrent execution MAY
transiently reach `bound + 1` (the inline task), but the detached count
SHALL NEVER exceed the bound.

#### Scenario: detached count never exceeds the configured bound

- **GIVEN** a WireTap configured with `max_concurrent = 2` and a tap that
  sleeps 50ms per call, with the detached-task liveness counter exposed for
  testing
- **WHEN** the caller fires several exchanges in rapid succession
- **THEN** the observed peak detached-task count is at most 2

#### Scenario: caller is back-pressured when the bound is saturated

- **GIVEN** a WireTap configured with `max_concurrent = 1`, a tap that
  blocks on a `tokio::sync::Notify` (deterministic, not wall-clock), and
  one tap already holding the sole permit (blocked on the Notify)
- **WHEN** the caller invokes `call()` a second time and polls its future
  without notifying the tap
- **THEN** the second call's future stays `Pending` (it is running the tap
  inline on the caller, engaging `CallerRuns` back-pressure); once the
  Notify is signaled, the second call completes. This is
  scheduler-independent — the leaky spawn-then-acquire version would
  resolve the second call immediately regardless of the Notify.

### Requirement: Self-reaping tap tasks with bounded retained state

The WireTap EIP SHALL run admitted taps as detached tasks that release
their admission permit on completion. The system SHALL NOT retain
completed tap task entries in a grow-only structure. A liveness tracker
SHALL expose the in-flight detached count, and that count SHALL return to
zero once queued taps complete, across repeated bursts.

#### Scenario: in-flight count drains to zero across repeated bursts

- **GIVEN** a WireTap with its in-flight liveness tracker exposed for
  testing
- **WHEN** the caller fires a burst of 1000 fast tap tasks, waits for the
  tracker to drain, then fires a second burst of 1000 fast tap tasks
- **THEN** the tracker returns to 0 after each burst within a bounded
  timeout (on the leaky version the count is monotonically non-decreasing
  and the second-burst poll times out)

### Requirement: Graceful-drain-then-abort teardown

The WireTap EIP SHALL expose its in-flight tap set to the runtime shutdown
drain via the `StepLifecycle` trait (ADR-0022). Admission and tracker
closure SHALL be atomic with respect to `call()`: an `Arc::Mutex` admission
guard MUST span "check admission open → register the task in the tracker"
so that `shutdown` cannot observe an empty closed tracker while a
concurrent `call()` is between its admission check and its task
registration. On `shutdown`, the WireTap SHALL execute this sequence under
that guard: (1) acquire the admission guard, mark admission closed, and
close the liveness tracker — so any `call()` that observes open admission
also registers its task before the tracker closes; (2) await
`tokio::time::timeout(shutdown_grace, tracker.wait())` to drain taps that
complete naturally; (3) on timeout, cancel the private `CancellationToken`
which every detached tap SHALL select on so it aborts promptly; (4) await
tracker completion. `shutdown` SHALL be idempotent. Calls attempted after
admission closure SHALL be rejected (the tap dropped, logged at `warn!`).
The default `shutdown_grace` SHALL be exactly 5 seconds; `shutdown_grace`
of zero SHALL mean "skip the drain wait and cancel immediately" (still
idempotent).

#### Scenario: fast tap drains, slow tap aborts after grace

- **GIVEN** a WireTap with one fast tap (10ms) and one slow tap (10s) in
  flight, a `WireTapLifecycle` handle, and `shutdown_grace = 200ms`
- **WHEN** `shutdown` is invoked
- **THEN** the fast tap completes (drained, not aborted), the slow tap does
  not complete (aborted after the grace period via the cancellation token),
  and `shutdown` returns within approximately the grace duration

#### Scenario: shutdown is idempotent

- **GIVEN** a `WireTapLifecycle` handle whose `shutdown` has already been
  invoked
- **WHEN** `shutdown` is invoked a second time
- **THEN** the second call returns without error and without re-aborting
  already-aborted taps

#### Scenario: calls after shutdown are rejected

- **GIVEN** a WireTap whose `shutdown` has begun (admission closed)
- **WHEN** `call()` is invoked
- **THEN** the tap is not spawned, the call returns the original exchange
  immediately, and a `warn!` log records the dropped tap

### Requirement: Tap error isolation under fire-and-forget

The WireTap main route SHALL never block on or fail because of the tap
endpoint. `WireTapService::poll_ready` SHALL always return
`Ready(Ok(()))`. Tap endpoint readiness SHALL be checked inside the tap
task (detached or inline). All tap readiness errors and processing errors
SHALL be logged at `warn!` (category handler-owned per ADR-0012) and
suppressed so the main exchange proceeds unchanged. This applies to both
the detached path and the inline `CallerRuns` path.

#### Scenario: tap readiness error is suppressed

- **GIVEN** a WireTap whose tap endpoint `poll_ready` returns `Err`
- **WHEN** the caller invokes `call()` and awaits the result
- **THEN** the main exchange is returned unchanged (Ok), and the readiness
  error is logged at `warn!` rather than propagated

#### Scenario: tap processing error is suppressed

- **GIVEN** a WireTap whose tap endpoint `call` returns `Err`
- **WHEN** the caller invokes `call()` and awaits the result
- **THEN** the main exchange is returned unchanged (Ok), and the processing
  error is logged at `warn!`

#### Scenario: cancellation while a tap is pending in readiness

- **GIVEN** a WireTap with an in-flight tap whose endpoint `poll_ready` is
  pending, and a `shutdown` in progress that has fired the cancellation
  token
- **WHEN** the cancellation propagates
- **THEN** the pending tap aborts cleanly without panicking or propagating
  an error to the main route

### Requirement: Per-request clone-drop isolation

Because the route pipeline clones the `BoxProcessor` per request and drops
the clone once the immediate-return `call()` future resolves, the WireTap
SHARED state (admission semaphore, liveness tracker, lifecycle handle)
SHALL be shared across clones via `Arc`. Dropping a per-request clone
SHALL NOT abort admitted tap tasks, close admission, or cancel the
lifecycle token. Aborts SHALL occur only via `StepLifecycle::shutdown` or
last-reference drop of the canonical service.

#### Scenario: taps survive per-request clone drops (rc-vq91 regression)

- **GIVEN** a canonical WireTap service held for the route lifetime and a
  slow tap (150ms)
- **WHEN** 3 request cycles each clone the service, call it, and drop the
  clone
- **THEN** all 3 slow taps complete despite the per-request clone drops

### Requirement: Composite lifecycle handle composition

When a WireTap step carries both a WireTap lifecycle handle and an
endpoint lifecycle handle, the compiler SHALL compose them into a single
`StepLifecycle` via a `CompositeStepLifecycle`. The composite SHALL be
constructed with children in the order `[endpoint, WireTap]` so that
`start()` runs forward (endpoint first) and `shutdown()` runs reverse
(WireTap first, so taps drain before the endpoint is torn down). On a
child `start` failure, the composite SHALL roll back already-started
children in reverse order. `shutdown` SHALL be best-effort: every child is
attempted even if an earlier child errors, and errors are aggregated. The
`CompositeStepLifecycle` type SHALL live in camel-core (runtime sequencing
concern), not camel-api (contract-only boundary). The composed handle
SHALL satisfy the existing `StepLifecycle` contract so the route drain
treats it transparently.

#### Scenario: both WireTap and endpoint handles drain on shutdown

- **GIVEN** a compiled WireTap step whose endpoint exposes a
  `StepLifecycle` handle and whose WireTap exposes its own handle, composed
  via `CompositeStepLifecycle`
- **WHEN** the route drain invokes `shutdown` on the composed handle
- **THEN** the WireTap handle shuts down first (draining in-flight taps),
  followed by the endpoint handle, both observable via their respective
  effects

#### Scenario: start failure rolls back already-started children

- **GIVEN** a `CompositeStepLifecycle` whose first child (`endpoint`)
  starts successfully but whose second child fails to start
- **WHEN** `start` is invoked on the composite
- **THEN** the already-started endpoint child is shut down (rolled back)
  and the composite returns the error

### Requirement: Stable public API preserved

`WireTapService::new`, `WireTapService::with_config`, `WireTapLayer::new`,
and `WireTapLayer::bounded` SHALL retain their existing signatures (they
are documented stable public exports per `camel-processor/CONTEXT.md`).
The lifecycle handle SHALL be exposed via an additive accessor (e.g.
`WireTapService::lifecycle() -> Arc<dyn StepLifecycle>`) rather than a
constructor parameter. `WireTapConfig` SHALL gain a `shutdown_grace:
Duration` field with a `Default` of 5 seconds; this minor additive field
change SHALL be documented.

#### Scenario: existing constructor call sites compile unchanged

- **GIVEN** code that constructs a `WireTapService` via `::new` or
  `::with_config` with the pre-change argument shape
- **WHEN** compiled against the new crate version
- **THEN** the construction compiles without modification (the lifecycle
  handle is obtained separately via the additive accessor where needed)

### Requirement: Configuration validation

`WireTapConfig` SHALL validate its fields when consumed to build a
service. `WireTapConfig::bounded(0)` SHALL panic at the constructor
(function-level assert). `WireTapService::with_config` SHALL assert
`max_concurrent > 0` when set, panicking with a fail-closed message (a
zero bound is a programmer error, not a runtime condition). Plain struct
literals are field assignment and are NOT validated at assignment time;
validation occurs at service construction. `shutdown_grace` is a
`std::time::Duration` (unsigned by type — negative values cannot be
expressed); `shutdown_grace` of zero SHALL mean "skip the drain wait,
cancel immediately".

#### Scenario: zero bound panics at service construction

- **GIVEN** a caller that invokes `WireTapConfig::bounded(0)` OR builds a
  `WireTapService::with_config` with `max_concurrent = Some(0)`
- **WHEN** the constructor runs
- **THEN** it panics with a message naming the invalid bound, before any
  service is built

#### Scenario: zero grace means immediate cancel on shutdown

- **GIVEN** a WireTap configured with `shutdown_grace = Duration::ZERO`
  and one slow tap in flight
- **WHEN** `shutdown` is invoked
- **THEN** the slow tap is cancelled immediately (no drain wait), and
  `shutdown` returns without waiting for the grace period

### Requirement: Camel divergence documentation

The system SHALL document the following divergences from Apache Camel as
tracked content in `crates/camel-processor/CONTEXT.md`, per ADR-0046: (a)
the flat-semaphore collapse — Camel's two-tier `maxPoolSize` +
`maxQueueSize` model collapsed to a single flat concurrency cap with
`CallerRuns` at the bound, citing Camel's own virtual-thread executor
(which the threading-model doc describes as exactly this semaphore-based
flat cap) as the sanctioning rationale; (b) `CallerRuns` can transiently
exceed the semaphore capacity by one inline task; (c) teardown happens at
route-level shutdown, not CamelContext-level shutdown; (d) Camel's
thread-pool/executor profiles are not exposed. Each entry SHALL name the
divergence, the forcing rationale, and the observable consequence.

#### Scenario: divergences recorded in CONTEXT.md

- **GIVEN** the WireTap was modeled on Camel's thread-pool semantics with
  deliberate divergences
- **WHEN** a contributor reads the WireTap section of
  `crates/camel-processor/CONTEXT.md`
- **THEN** they find entries for the flat-semaphore collapse, the
  CallerRuns transient-exceed, the route-level teardown, and the absent
  pool-profile knobs, each with rationale and consequence
