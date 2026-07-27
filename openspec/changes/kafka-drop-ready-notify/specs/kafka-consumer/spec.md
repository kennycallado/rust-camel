# Spec Delta: kafka-consumer readiness

## REMOVED Requirement: Kafka partition-assignment test-synchronization hook

The Kafka consumer SHALL NOT expose a `ready_signal()` accessor, a
`ReadyContext` rdkafka context, or an internal `Arc<tokio::sync::Notify>`
fired from `post_rebalance`. Test synchronization for startup readiness SHALL
use the unified `ConsumerContext::startup_signal()` / `StartupSignal` surface
provided by `camel-component-api` (per rc-gu5n).

### Scenario: ready_signal accessor is gone

- **GIVEN** the `camel-component-kafka` crate is compiled
- **WHEN** a caller attempts to reference `KafkaConsumer::ready_signal`
- **THEN** the reference does not resolve (compile error), confirming the API
  surface is removed

### Scenario: ReadyContext struct is gone

- **GIVEN** the `camel-component-kafka` crate source
- **WHEN** a maintainer greps for `ReadyContext`
- **THEN** no match is returned — the struct, its `ClientContext`/
  `RdConsumerContext` impls, and the `ReadyStreamConsumer` type alias are
  deleted

### Scenario: Notify import is gone

- **GIVEN** the `camel-component-kafka` crate source
- **WHEN** a maintainer greps for `tokio::sync::Notify`
- **THEN** no match is returned in the kafka component (the `mpsc` import
  remains)

## MODIFIED Requirement: Kafka consumer startup readiness

The Kafka consumer SHALL continue to declare
`startup_mode() -> ConsumerStartupMode::Explicit` and SHALL call
`ctx.mark_ready()` exactly once, eagerly, immediately after `subscribe()`
succeeds inside `run_consumer_loop`. Readiness SHALL NOT be gated on the first
partition assignment (rdkafka `post_rebalance`).

### Scenario: eager mark_ready is preserved

- **GIVEN** a Kafka consumer whose `start()` has been invoked with a valid
  `ConsumerContext`
- **WHEN** the spawned `run_consumer_loop` successfully calls `subscribe()`
- **THEN** `ctx.mark_ready()` fires before the poll loop begins, resolving the
  controller's `StartupReceiver` with `Ok`, regardless of whether a partition
  assignment has arrived

### Scenario: default rdkafka context is used

- **GIVEN** the rewritten consumer creation in `run_consumer_loop`
- **WHEN** `client_cfg.create()` constructs the consumer
- **THEN** the consumer uses rdkafka's `DefaultConsumerContext` (no custom
  `post_rebalance` behaviour), and the poll loop drives group coordination as
  before

### Scenario: existing startup deadlock guards remain documented

- **GIVEN** the "Startup readiness" comment block in `run_consumer_loop`
- **WHEN** a maintainer reads it
- **THEN** the liveness (broker-drop) and ordering (self-producing graph)
  rationale for eager `mark_ready()` is still present, with no dangling
  reference to the removed `ready` Notify or `ready_signal()`

### Scenario: second ReadyContext comment is cleaned up

- **GIVEN** the poll-loop comment near the rebalance protocol note (formerly
  referencing `ReadyContext::post_rebalance` and `ready.notify_waiters()`)
- **WHEN** a maintainer reads it
- **THEN** the comment no longer names `ReadyContext` or the removed `Notify`;
  the accurate "recv() drives the rebalance protocol automatically" note remains

## MODIFIED Requirement: Kafka consumer crate compiles and passes gates

The `camel-component-kafka` crate SHALL compile and pass the project's full
quality gate suite with no warnings, after the removal.

### Scenario: build and lint clean

- **GIVEN** the post-removal source tree
- **WHEN** `cargo build -p camel-component-kafka`,
  `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`, and
  `cargo fmt --check -p camel-component-kafka` are run
- **THEN** all three succeed with zero warnings

### Scenario: lib tests pass without the removed test

- **GIVEN** the post-removal source tree with
  `test_ready_signal_returns_shared_notify_handle` deleted
- **WHEN** `cargo test -p camel-component-kafka --lib` is run
- **THEN** all remaining tests pass, and no test references the removed
  `ready_signal()` accessor
