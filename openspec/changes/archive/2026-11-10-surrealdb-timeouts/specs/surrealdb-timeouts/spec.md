## ADDED Requirements

### Requirement: SurrealDB SDK calls carry configurable deadlines

The `camel-component-surrealdb` component SHALL bound every SDK call it owns
with a `tokio::time::timeout` deadline: connect per attempt, signin,
namespace selection, database selection, the datasource health check,
`invalidate()` teardown, every producer statement execution, and the live
subscription. The polling consumer keeps its caller-supplied
`receive(timeout)` bound (ADR-0015). The bounds SHALL be datasource `extra`
keys holding TOML integers in milliseconds: `connect_timeout_ms` (default
10000) and `operation_timeout_ms` (default 30000), parsed at pool and
endpoint creation with no upper cap. The configured `operation_timeout_ms`
SHALL apply to signin, namespace/database selection, producer statement
execution, and the live subscription; the health check and teardown hooks
receive no configuration (their handle carries none) and SHALL use the
built-in 30000 ms default, which this change also makes finite. A timeout
SHALL NOT be classified as retryable and SHALL NOT map to a protocol/query
error variant: expiry produces an error whose text names the phase and the
bound in milliseconds (e.g. `surrealdb statement execution exceeded the
30000ms deadline`). Timeout expiry releases the caller only; server-side
orphan work is explicit in the design and is not claimed as cancelled. Live
notification delivery after the subscription is exempt (long-lived by
design; residual bd `rc-lft0b`).

#### Scenario: Defaults apply when no extras are configured

- **GIVEN** a surrealdb datasource with no `connect_timeout_ms` or
  `operation_timeout_ms` extras
- **WHEN** connect exceeds 10000 ms per attempt, or a bounded operation
  exceeds 30000 ms
- **THEN** the call ends with a timeout error naming the phase and the
  10000 ms (connect) or 30000 ms (operation) deadline — the default bounds
  the previously unbounded call

#### Scenario: Explicit values replace the defaults

- **GIVEN** a surrealdb datasource with `connect_timeout_ms = 100` and
  `operation_timeout_ms = 5000` TOML-integer extras
- **WHEN** connect, signin, namespace/database selection, a producer
  statement, or the live subscription runs past its configured deadline
- **THEN** the deadline in effect for that phase is the configured value,
  not the default

#### Scenario: Zero or invalid values are rejected naming the option

- **GIVEN** a surrealdb datasource whose `extra` map holds
  `connect_timeout_ms` as `0`, a negative number, an empty string, or a
  non-numeric value (same rule for `operation_timeout_ms`)
- **WHEN** the pool or the endpoint is created
- **THEN** creation fails with an error naming the offending extra key, and
  no connection attempt is made

#### Scenario: Stalled endpoint trips the connect bound with the named error

- **GIVEN** a `ws://` datasource whose endpoint accepts TCP and never
  answers, with `connect_timeout_ms = 100`
- **WHEN** the pool factory attempts to connect
- **THEN** the attempt ends with a non-retryable timeout error whose text
  names the connect phase and the 100 ms bound, and no second attempt runs;
  the elapsed time is at least the deadline (scheduler-aware upper guard in
  the test, not a strict wall-clock assertion)

#### Scenario: Setup-phase expiry is not retried

- **GIVEN** a connected client whose signin round trip stalls past
  `operation_timeout_ms`
- **WHEN** the setup sequence runs under its existing transaction-conflict
  retry classifier
- **THEN** the expiry error is not classified as a transaction conflict, the
  setup fails fast without burning retry attempts, and the surfaced error
  text names the signin phase and the bound

#### Scenario: Health-check and teardown expiry degrade instead of hanging

- **GIVEN** an established datasource whose server stops answering
- **WHEN** the health check runs `INFO FOR DB` past the 30000 ms built-in
  bound
- **THEN** the datasource reports `HealthStatus::Unhealthy` instead of
  hanging; the close hook's `invalidate()` past the same bound returns an
  error instead of hanging

#### Scenario: A normal query is unaffected by the statement bound

- **GIVEN** a healthy `mem://` datasource with the default bounds
- **WHEN** a producer statement executes against the embedded instance
- **THEN** the statement completes normally — the wrapper is transparent on
  the happy path

#### Scenario: Producer statement execution is bounded against a stalled server

- **GIVEN** an established surrealdb client whose server stops reading, and
  the default `operation_timeout_ms`
- **WHEN** a statement execution exceeds 30000 ms
- **THEN** the producer call ends with an error whose text names the
  statement-execution phase and the bound; the server may complete the
  orphaned work (no server-side cancellation is claimed). No new container
  test is added by this change: the existing CI harness
  (`cargo test -p camel-test --test surrealdb_test --features
  integration-tests`, Docker) proves happy-path statement behaviour only.
  The local proof is wrapper transparency on `mem://` plus the unit-covered
  elapsed mapping (`statement_elapsed_arm_trips_named_timeout` and the
  stalled-connect machinery test)
