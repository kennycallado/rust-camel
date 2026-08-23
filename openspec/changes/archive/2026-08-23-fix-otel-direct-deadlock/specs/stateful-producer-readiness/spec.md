# stateful-producer-readiness Delta — fix-otel-direct-deadlock

## ADDED Requirements

### Requirement: Producers do not carry semaphore permits across the poll_ready/call boundary

The Direct, Kafka, JMS, CXF, WASM, OpenSearch, and gRPC producers SHALL acquire their concurrency
semaphore permit inside `call()`'s future, immediately before dispatching the
exchange. Their `poll_ready` SHALL NOT acquire a semaphore permit; it SHALL
retain each producer's non-semaphore readiness behavior as specified in the
scenarios below.

#### Scenario: DirectProducer keeps registry fail-fast in poll_ready

- **GIVEN** a `DirectProducer` for a name with no consumer ever registered and
  `fail_if_no_consumers` unset (default)
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Ready(Err(EndpointCreationFailed(_)))`
- **WHEN** `poll_ready()` is called with a live consumer registered and
  `call(exchange)` is then invoked
- **THEN** `poll_ready` SHALL have returned `Poll::Ready(Ok(()))` without
  acquiring a permit, and the permit SHALL be acquired inside the call future
  and released when it completes

#### Scenario: DirectProducer closed-channel error is preserved

- **GIVEN** a registry entry for the producer's name whose sender is closed
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Ready(Err(EndpointCreationFailed(_)))`

#### Scenario: KafkaProducer keeps stopped-state error in poll_ready

- **GIVEN** a `KafkaProducer` whose `stopped` flag is set
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Ready(Err(ProcessorError(_)))` with the
  stopped-state message
- **AND** with `stopped` unset, `poll_ready()` SHALL return
  `Poll::Ready(Ok(()))` without acquiring a permit; the permit SHALL be
  acquired inside the call future

#### Scenario: JmsProducer readiness is unconditional

- **GIVEN** a built `JmsProducer`
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Ready(Ok(()))` unconditionally, deferring
  permit acquisition and delivery to `call()`

#### Scenario: OpenSearchProducer readiness is unconditional

- **GIVEN** a built `OpenSearchProducer`
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Ready(Ok(()))` unconditionally, deferring
  permit acquisition and the request to `call()`
- **WHEN** the semaphore is closed and `call()` is invoked
- **THEN** the call future SHALL surface the acquisition failure as an error

#### Scenario: GrpcProducer readiness is unconditional

- **GIVEN** a built `GrpcProducer`
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Ready(Ok(()))` unconditionally, deferring
  permit acquisition and the request to `call()`
- **WHEN** the semaphore is closed and `call()` is invoked
- **THEN** the call future SHALL surface the acquisition failure as an error

#### Scenario: CxfProducer keeps bridge-state readiness in poll_ready

- **GIVEN** a `CxfProducer` with a bridge slot in state `Ready`
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Ready(Ok(()))` without acquiring a permit
- **GIVEN** a bridge slot in state `Starting` or `Restarting`
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Pending` (waking on the next bridge state
  change)
- **GIVEN** a bridge slot in state `Degraded(reason)` or `Stopped`
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Ready(Err(ProcessorError(_)))` carrying the
  degraded reason or stopped message
- **AND** the semaphore permit SHALL be acquired inside the call future

#### Scenario: WasmProducer keeps init-failed error in poll_ready

- **GIVEN** a `WasmProducer` whose `init_failed` flag is set
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Ready(Err(ProcessorError(_)))` with the
  initialization-failure message
- **AND** with `init_failed` unset, `poll_ready()` SHALL return
  `Poll::Ready(Ok(()))` without acquiring a permit; the permit SHALL be
  acquired inside the call future

#### Scenario: Bounded concurrency survives the acquisition move per producer

- **GIVEN** any of the seven producers with ALL of its semaphore permits held
  externally
- **WHEN** a `call()` future is polled
- **THEN** it SHALL remain pending on the semaphore until the permits are
  released — proving acquisition happens inside `call`, before dispatch
- **AND GIVEN** the Direct producer with a consumer that receives the exchange
  but does not yet reply
- **WHEN** a first `call()` is in flight and a second `call()` is driven
- **THEN** the second SHALL remain pending on the semaphore until the first
  completes and releases its permit (permit held for the full dispatch
  duration)
