# async-lifecycle-correctness Specification

## Purpose
TBD - created by archiving change audit-fix-async-lifecycle. Update Purpose after archive.
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

