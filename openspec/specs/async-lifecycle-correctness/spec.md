# async-lifecycle-correctness Specification

## Purpose
Correct shutdown and cancellation at asynchronous lifetime boundaries. The
capability pins how the health server aborts a detached task, how the function
service and master bridges drain on every exit path, and how producers and
consumers derive cancellation from the runtime token.
## Requirements
### Requirement: Health server shutdown aborts detached task

The system SHALL abort the server `JoinHandle` when graceful shutdown
exceeds the derived shutdown timeout, preventing a detached task from
holding the bound port.

#### Scenario: Handler timeout exceeds default shutdown timeout

- **GIVEN** a `HealthServer` configured with `handler_timeout` of 8s
- **WHEN** `stop()` is called and the probe drain does not complete within
  `handler_timeout + 2s` (10s)
- **THEN** the `JoinHandle` is aborted via `.abort()` and awaited to
  completion, and the status is set to `STATUS_STOPPED`

#### Scenario: Server task panics during shutdown

- **GIVEN** a running `HealthServer` whose spawned task panics during
  graceful shutdown
- **WHEN** `stop()` awaits the `JoinHandle`
- **THEN** the panic is logged at `error!` level (not silently swallowed),
  the status is set to `STATUS_STOPPED`, and `stop()` returns `Ok(())`

### Requirement: Function service shutdown drains all providers

The system SHALL attempt to shut down every provider runner in
`FunctionRuntimeService::stop()`, even when a provider returns an error,
collecting the first error for deferred return after the drain completes.

#### Scenario: Second provider fails on shutdown

- **GIVEN** a `FunctionRuntimeService` with three runners where the second
  `provider.shutdown()` returns `Err`
- **WHEN** `stop()` is called
- **THEN** all three runners are cancelled and `provider.shutdown()` is
  called for each, the first error is returned, and `status` is set to
  `ServiceStatus::Stopped` regardless of the error

### Requirement: JMS producer returns ConsumerStopping on broker stopped

The system SHALL return `CamelError::ConsumerStopping` from
`LazyJmsProducer::poll_ready` when the bridge is in `BridgeState::Stopped`,
conforming to ADR-0024 §Decision for JMS shutdown signals.

#### Scenario: Poll ready on stopped broker

- **GIVEN** a `LazyJmsProducer` whose bridge state is `BridgeState::Stopped`
- **WHEN** `poll_ready()` is called
- **THEN** the returned `Poll::Ready(Err(...))` contains
  `CamelError::ConsumerStopping` (not `ProcessorError`)

### Requirement: Master stop_delegate drains epoch bridge on all paths

The system SHALL drain the epoch-stamping bridge in `stop_delegate`
regardless of how the delegate task exits — success, error, panic,
cancellation, or timeout — preventing a detached bridge from stamping
stale exchanges after the leader yields.

#### Scenario: Delegate task errors during shutdown

- **GIVEN** a `DelegateState::Active` where the delegate task returns
  `Err(CamelError)` during `stop_delegate`
- **WHEN** `stop_delegate` is called
- **THEN** the epoch-bridge is drained (awaited within `drain_timeout`)
  before the delegate error is propagated, and no bridge `JoinHandle` is
  left detached

#### Scenario: Delegate task times out during shutdown

- **GIVEN** a `DelegateState::Active` where the delegate task does not
  finish within `drain_timeout`
- **WHEN** `stop_delegate` is called
- **THEN** the delegate handle is aborted, the epoch-bridge is drained
  within its own `drain_timeout` window (or aborted if that window
  elapses), and no `JoinHandle` is left detached

### Requirement: Auth cache uses per-key deduplication

The system SHALL deduplicate concurrent introspection and permission
evaluations on a per-key basis, preventing different tokens or permission
requests from serializing behind a single global in-flight lock.

#### Scenario: Two different tokens introspected concurrently

- **GIVEN** a `CachingTokenIntrospector` with an empty cache and a backend
  that takes 500ms per introspection
- **WHEN** two different tokens are introspected concurrently
- **THEN** both HTTP calls proceed in parallel (neither waits for the
  other), and both results are cached

#### Scenario: Same token introspected concurrently

- **GIVEN** a `CachingTokenIntrospector` with an empty cache and a backend
  that takes 500ms per introspection
- **WHEN** the same token is introspected by two concurrent callers
- **THEN** only one HTTP call is made (thundering-herd prevention
  preserved), and both callers receive the same result

#### Scenario: Permission evaluator per-key deduplication

- **GIVEN** a `CachingPermissionEvaluator` with an empty cache and an inner
  evaluator that takes 500ms per evaluation
- **WHEN** two different permission requests are evaluated concurrently
- **THEN** both evaluations proceed in parallel, and both results are cached

### Requirement: Consumer-lifetime cancellation tokens derive from the Runtime token

Route consumers in camel-kafka, camel-jms, camel-cxf,
camel-component-wasm (source side), and camel-redis SHALL acquire their
consumer-lifetime cancellation token from
`ConsumerContext::cancel_token()` in `Consumer::start`. Self-created
consumer-lifetime tokens are prohibited in these five consumers. Each
consumer poll loop SHALL observe route-stop cancellation without
waiting for the stop-timeout abort. Per-request child tokens SHALL keep
their existing semantics.

#### Scenario: Kafka loop observes route stop

- **GIVEN** a started camel-kafka consumer whose event loop selects on
  the consumer-lifetime token, with an unreachable broker
- **WHEN** the consumer context token is cancelled
- **THEN** the event loop exits and `stop()` returns within 2 seconds,
  without the stop-timeout abort

#### Scenario: Redis sessions observe route stop

- **GIVEN** a started camel-redis consumer whose pubsub and queue
  sessions listen on a child of the consumer context token
- **WHEN** the consumer context token is cancelled
- **THEN** the poll loop exits within 2 seconds, and a local `stop()`
  call does not cancel the Runtime-owned token

#### Scenario: JMS handles join on route stop

- **GIVEN** a started camel-jms consumer whose task handles wait on the
  consumer context token
- **WHEN** the consumer context token is cancelled
- **THEN** `stop()` joins the handles within 1 second

#### Scenario: CXF consumer exits on route stop

- **GIVEN** a started camel-cxf consumer driven by a mock bridge, with
  its consumer task bound to the consumer context token
- **WHEN** the consumer context token is cancelled
- **THEN** the consumer task completes within 2 seconds

#### Scenario: Wasm source host observes route stop

- **GIVEN** a started camel-component-wasm source consumer whose host
  state carries the token provided by `ConsumerContext`
- **WHEN** the consumer context token is cancelled
- **THEN** the run task completes within 3 seconds, without waiting for
  the stop-timeout abort

### Requirement: Per-dispatch cancellation stops an enqueued exchange between steps

The system SHALL provide a per-dispatch cancellation signal for the
request-reply envelope path. `ConsumerContext::send_and_wait` SHALL mint a
fresh, independent cancellation token and stamp it on the Exchange as an
enqueue-only marker; dropping the wait future before the reply SHALL cancel
exactly that dispatch, except when the consumer-lifetime token is already
cancelled — then the drop is shutdown-driven and the existing route-stop drain
grace SHALL govern the exchange, not the dispatch signal. A completed reply
SHALL disarm the signal. Each pipeline entry SHALL take (remove) the marker
before running any step, so replies, stash emissions, and later redispatches
carry no stale signal. The token SHALL be independent of the consumer-lifetime
and route-stop tokens, and the fresh token SHALL be created by the `camel-api`
dispatch helper, not by component code.

The pipeline SHALL observe the signal before each step, beside the route-stop
check, at the entries that already scope route-stop cancellation (the
concurrent envelope loop, the sequential envelope loop, and inline dispatch),
and resolve with `PipelineOutcome::Failed(CamelError::ConsumerStopping)` when a
step is prevented; an exchange with no remaining step SHALL finish with its
last step's outcome. The aggregate-split loop SHALL take (remove) the marker
without observing it, because that loop scopes no route-stop cancellation
either; its replies SHALL still carry no stale signal. A dispatch-cancelled
`ConsumerStopping` SHALL NOT be retried or absorbed by the error handler. A
non-transactional step that is already executing SHALL run to completion —
cancellation SHALL NOT abort a step mid-`.await`; it SHALL prevent the next
step. Route stop SHALL keep its existing drain-grace semantics and SHALL remain
an independent trigger.

For a `transacted` block the new signal joins the existing block-level
cancellation arm with the frozen precedence: block deadline first, then
cancellation, then body. A body cancelled before the commit cutoff SHALL skip
COMMIT and await the session's total rollback within the 5 s finalize grace; an
acknowledged rollback SHALL leave no checked-out session, and an unacknowledged
rollback SHALL take the existing detach path (`RollbackUncertain`/`Detached`)
with the cleanup uncertainty logged and the primary error standing. A body that
completes successfully SHALL consult both tokens only at the existing cutoff:
after COMMIT is handed to the driver, cancellation SHALL NOT be re-consulted
and the commit resolution SHALL stand. No new field SHALL be added to any
published public struct.

#### Scenario: cancelled dispatch stops before the next step

- **GIVEN** a started route whose first step parks until released and whose
  second step records execution, with a dispatcher waiting through
  `send_and_wait`
- **WHEN** the dispatching wait is dropped and the first step is then released
- **THEN** the first step completes, the second step never runs, the exchange
  resolves with `ConsumerStopping`, and the in-flight bookkeeping returns to
  zero

#### Scenario: cancelled dispatch releases the exchange

- **GIVEN** a `Concurrent { max: Some(1) }` route whose single step parks until
  released, and a dispatcher whose wait is dropped while the step is parked
- **WHEN** the parked step is released and a second dispatch is submitted
- **THEN** the dropped dispatch's reply channel is closed, its in-flight claim
  and its concurrency permit are released, the second dispatch completes, and
  neither reply carries a dispatch marker

#### Scenario: route stop still cancels an in-flight dispatch

- **GIVEN** a started `Concurrent` route with two steps whose first step parks,
  and a live `send_and_wait` waiter
- **WHEN** the route is stopped, the drain grace expires so the route-stop token
  fires, and the first step is then released
- **THEN** the waiter resolves with `ConsumerStopping` at the next step
  boundary, the second step never runs, and no dispatch token was involved

#### Scenario: abandoned wait during route stop drains to completion

- **GIVEN** a started Sequential route whose first step parks and whose second
  step records execution, with a live `send_and_wait` waiter and a shutdown
  budget long enough for the drain grace
- **WHEN** the route is stopped, the consumer-lifetime token is cancelled, the
  waiter is then dropped, and the parked step is released
- **THEN** the drop does not cancel the dispatch, the second step runs, the
  exchange drains to completion, and the stop returns success

#### Scenario: cancelled transacted block finalizes without leaking a session

- **GIVEN** a `transacted` block whose body is parked inside a joined step when
  the dispatching wait is dropped, with cancellation arriving before the commit
  cutoff and a session whose rollback is acknowledged
- **WHEN** the block finalizes
- **THEN** COMMIT is never issued, the bounded rollback completes within the
  5 s grace, the session reaches its finalized state, and no checked-out
  session remains

#### Scenario: uncancelled dispatch is unchanged

- **GIVEN** a started route and a dispatcher that awaits a normal
  `send_and_wait` to completion
- **WHEN** every step succeeds and the reply arrives
- **THEN** all steps run, the reply carries the unchanged exchange with no
  dispatch marker, the dispatch signal is disarmed, and no cancellation is
  observed

