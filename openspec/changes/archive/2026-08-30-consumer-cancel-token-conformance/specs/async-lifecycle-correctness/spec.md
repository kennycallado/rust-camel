## ADDED Requirements

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
