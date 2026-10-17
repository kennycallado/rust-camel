# rabbitmq-component Specification (delta)

## ADDED Requirements

### Requirement: URI scheme and broker resolution

The component SHALL register scheme `rabbitmq:` with URI shape
`rabbitmq:<exchange>?queue=<queue>&routingKey=<key>`, where exchange
`default` (or empty) means the broker's default exchange. Brokers SHALL
be declared in `[components.rabbitmq.brokers]` in Camel.toml and
resolved by the jms rules: explicit `?broker=` must exist; omitted with
exactly one declared broker selects it; omitted with several declared
brokers is an error naming the `?broker=` option; omitted with none is
an error naming the Camel.toml section. The `amqp:` scheme SHALL remain
unregistered (reserved for a future AMQP 1.0 component).

#### Scenario: default exchange publish target

- **GIVEN** URI `rabbitmq:default?queue=orders`
- **WHEN** the endpoint resolves
- **THEN** the target exchange is the empty-string default exchange and
  the routing key is `orders`

#### Scenario: single broker resolves implicitly

- **GIVEN** one broker `main` declared in `[components.rabbitmq.brokers]`
  and URI `rabbitmq:orders?queue=q1`
- **WHEN** the endpoint resolves
- **THEN** broker `main` is selected without an explicit `?broker=`

#### Scenario: ambiguous broker selection errors

- **GIVEN** two declared brokers and URI without `?broker=`
- **WHEN** the endpoint resolves
- **THEN** resolution fails with an error that names `?broker=` and the
  number of declared brokers

### Requirement: Connection management

The component SHALL maintain one `RabbitConnectionManager` per resolved
broker owning a lapin `Connection` created with
`default-features = false` plus exactly the lapin features `tokio`,
`rustls--ring`, and `rustls-native-certs`, so the rustls provider that
the component selects is ring. The dependencies that
`camel-component-rabbitmq` selects SHALL NOT add a new aws-lc edge: the
component SHALL NOT enable the bare lapin `rustls` feature or
`rustls--aws_lc_rs`, and no feature selected through the lapin chain
(`lapin`, `amq-protocol`, `amq-protocol-tcp`, `tcp-stream`,
`rustls-connector`) SHALL enable the rustls `aws_lc_rs`, `aws-lc-rs`,
or `default` features. aws-lc-rs that was already reachable through
shared workspace dependencies (`camel-component-api` and its
`camel-auth`, `reqwest`, `jsonwebtoken`, and workspace-`rustls`
default features) is outside this requirement and is tracked in bd
`rc-2bofp`. The whole-graph check `cargo tree -p
camel-component-rabbitmq -i aws-lc-rs` is therefore not a gate.
lapin's built-in recovery SHALL stay
disabled: reconnection is driven by the project `NetworkRetryPolicy`
(unlimited attempts, 5 s initial delay, x2 multiplier, 30 s cap). Each
connection SHALL carry a monotonically increasing generation counter
used for stale delivery-tag suppression.

#### Scenario: retry policy owns reconnect

- **GIVEN** a started consumer whose broker connection drops
- **WHEN** the manager detects the failure
- **THEN** reconnection follows the configured `NetworkRetryPolicy`
  delays and a new generation counter is issued

#### Scenario: component-selected TLS stack adds no aws-lc edge

- **GIVEN** the workspace after adding the component
- **WHEN** `cargo tree -p camel-component-rabbitmq -e features -i lapin`
  runs
- **THEN** lapin is selected with exactly the features `tokio`,
  `rustls--ring`, and `rustls-native-certs`
- **AND WHEN** `cargo tree -p camel-component-rabbitmq -e features -i
  aws-lc-rs` runs
- **THEN** no path through a `lapin`, `amq-protocol`,
  `amq-protocol-tcp`, `tcp-stream`, or `rustls-connector` feature
  selects the rustls `aws_lc_rs`, `aws-lc-rs`, or `default` feature: no
  component-selected enabling edge turns on aws-lc. The features that do
  enable aws-lc come from shared workspace dependencies
  (`camel-component-api`, `camel-auth`), which are outside this gate
  and tracked in bd `rc-2bofp`. The printed paths still pass through the
  lapin chain, because Cargo unifies features: the one `rustls` package
  that lapin's ring/std features select also carries features that the
  shared dependencies enable. Those display paths are not enabling edges.

### Requirement: Consumer start readiness and failure

At consumer start the component SHALL first establish the broker
connection under the `NetworkRetryPolicy` and SHALL NOT report the
consumer ready until connected; the route stays not-ready while initial
connect retries. Once connected, the passive queue check governs
(topology requirement). A producer publishing while disconnected SHALL
fail the exchange with a bounded error (a reconnect-wait bounded by a
cap, never an unbounded hang).

#### Scenario: consumer not ready until connected

- **GIVEN** a broker that is unreachable at consumer start
- **WHEN** the consumer starts and the route readiness is queried
- **THEN** readiness is not-ready and no route processing begins; when
  the broker becomes reachable, connection succeeds and the consumer
  becomes ready

#### Scenario: publish while disconnected fails bounded

- **GIVEN** a started producer whose broker connection is down
- **WHEN** an exchange is sent
- **THEN** the send fails with an error within the bounded
  reconnect-wait cap instead of hanging

### Requirement: Producer publish mapping

The producer SHALL map the exchange body to a byte payload and set
delivery mode 2 (persistent) when `persistent=true` (the default) and 1
otherwise, plus `contentType` when configured or present as a header.
Publish SHALL target the endpoint's exchange with the routing key (the
queue name when the routing key is absent).

#### Scenario: persistent default publish

- **GIVEN** a route sending through `rabbitmq:orders?queue=q1` with
  default options
- **WHEN** the producer publishes
- **THEN** the message carries delivery mode 2 and routing key `q1`

### Requirement: Header and property mapping

Inbound deliveries SHALL map AMQP basic properties to camel headers —
`contentType`, `contentEncoding`, `priority`, `messageId`,
`correlationId`, `replyTo`, `expiration`, `timestamp` — plus the broker
redelivery flag as `rabbitmq.redelivered`. Outbound publishes SHALL map
camel headers back onto the same basic-property set, and free-form
headers without a reserved property meaning SHALL travel as AMQP
headers verbatim.

#### Scenario: redelivered flag maps to header

- **GIVEN** a queue where a message was delivered before and nacked with
  requeue
- **WHEN** it is consumed again
- **THEN** the exchange carries header `rabbitmq.redelivered=true`

#### Scenario: free-form header round trips

- **GIVEN** a producer message with header `x-custom=abc`
- **WHEN** it is published and consumed through a queue
- **THEN** the consumed exchange carries header `x-custom=abc`

### Requirement: Consumer acknowledges only after route completion

The consumer SHALL deliver each message into the route pipeline and
`basic_ack` the delivery tag only after the route completes successfully
(kafka/mqtt contract). Prefetch (`prefetch`, default 10) SHALL bound
in-flight deliveries per channel and `concurrentConsumers` (default 1)
SHALL run that many channel+consumer loops with explicit readiness
before route activation.

#### Scenario: ack follows route completion

- **GIVEN** a docker-backed broker, a queue with one message, and a
  route that blocks until released
- **WHEN** the consumer processes the message and the route has not yet
  returned
- **THEN** the message remains unacknowledged on the broker; when the
  route returns Ok the message is acked

#### Scenario: concurrent consumers register on the queue

- **GIVEN** a consumer endpoint with `concurrentConsumers=3` and
  `prefetch=5`
- **WHEN** the consumer starts against a docker-backed broker
- **THEN** the queue reports three consumers and each channel holds at
  most five unacked deliveries

### Requirement: Failure disposition defaults to reject without requeue

On route failure the consumer SHALL `basic_nack` with requeue=false
(the Camel `rejectAndDontRequeue` parity; a DLX takes poison messages).
`requeueOnFailure=true` SHALL opt into requeue=true, and its
documentation SHALL warn about hot requeue loops without a broker-side
delivery limit.

#### Scenario: failed route rejects to DLX

- **GIVEN** a queue with a dead-letter exchange and a route that always
  fails, default options
- **WHEN** one message is consumed
- **THEN** the message is nacked without requeue and appears on the
  dead-letter queue

#### Scenario: requeue opt-in

- **GIVEN** the same setup with `requeueOnFailure=true`
- **WHEN** one message is consumed and the route fails
- **THEN** the message is redelivered with `rabbitmq.redelivered=true`

#### Scenario: redelivery after mid-flight stop

- **GIVEN** a consumed message whose route is still running when the
  consumer stops without acking
- **WHEN** the consumer restarts
- **THEN** the broker redelivers the message and it is processed again

### Requirement: Reconnect safety for delivery tags

Ack/nack operations SHALL be applied only to delivery tags whose
connection generation matches the current channel generation; stale tags
(pre-reconnect) SHALL be dropped without error. Dropping a stale tag
means the broker redelivers that message after reconnect — the
component is at-least-once and consumers SHALL tolerate duplicates.

#### Scenario: stale tag after broker restart is dropped

- **GIVEN** an in-flight message when the broker connection drops and
  reconnects
- **WHEN** the route later completes for the pre-restart delivery
- **THEN** no ack/nack is sent on the new channel for the stale tag and
  no protocol error closes the channel

### Requirement: Topology checks at consumer start

Consumer start SHALL passively declare (queue-passive check) the
configured queue and fail fast when it does not exist. With
`autoDeclare=true` (default false — documented divergence from the
Camel spring-rabbitmq consumer default), start SHALL actively declare
exchange, queue, and binding using `exchangeType` (default direct),
`durableQueue` (default true), and `queueArguments` (x-args map). A
406 PRECONDITION_FAILED from a conflicting active declare SHALL fail
route start with the broker's message.

#### Scenario: missing queue fails fast

- **GIVEN** a broker without queue `nope` and default options
- **WHEN** the consumer starts
- **THEN** start fails with an error naming the missing queue

#### Scenario: conflicting declare fails route start

- **GIVEN** queue `q` existing as non-durable and `autoDeclare=true`
  with `durableQueue=true`
- **WHEN** the consumer starts
- **THEN** start fails carrying the broker's 406 precondition error

#### Scenario: autoDeclare creates topology

- **GIVEN** a broker without queue `fresh` and `autoDeclare=true` with
  defaults
- **WHEN** the consumer starts
- **THEN** durable queue `fresh` exists, bound to the endpoint exchange
  by the routing key, and consumption begins

### Requirement: Publisher confirms are always enabled

The producer SHALL enable publisher confirms on its channel; every
publish SHALL wait for the broker confirm bounded by `confirmTimeout`
(default 5 s). A confirm not received within the bound, or a nack from
the broker, SHALL fail the exchange. With `mandatory=true` (default
false), an unroutable message returned via basic.return SHALL fail the
exchange with an error naming the exchange and routing key.

#### Scenario: confirm timeout fails the exchange

- **GIVEN** a producer whose confirm wait is bounded by
  `confirmTimeout=1s` against a broker that stalls confirms
- **WHEN** a publish awaits its confirm
- **THEN** the exchange fails with a confirm-timeout error within the
  bound

#### Scenario: unroutable mandatory publish fails

- **GIVEN** a mandatory publish to a routing key with no bound queue
- **WHEN** the broker returns the message
- **THEN** the producer exchange fails with an error naming the
  exchange and routing key

### Requirement: Request/reply over direct reply-to

InOut producer exchanges SHALL publish with reply-to
`amq.rabbitmq.reply-to`, consume that pseudo-queue no-ack on the
publishing connection's reply channel (replies are not sent under
publisher confirms), match replies by `correlationId` through a
correlation table with one-shot senders, and resolve or fail the
exchange within `replyTimeout` (default 30 s). Late replies after
timeout or cancellation SHALL be dropped and the table entry removed.
The consumer side SHALL publish the out-message to the inbound
`replyTo` with the inbound `correlationId` after the route completes
successfully; a failed route SHALL send no reply.

#### Scenario: reply round trip

- **GIVEN** a request route and a replier route over the same broker
- **WHEN** an InOut exchange is sent with `rabbitmq:`
- **THEN** the producer resolves with the replier's body

#### Scenario: reply timeout fails within the bound

- **GIVEN** a replier that never answers and `replyTimeout=2s`
- **WHEN** an InOut exchange is sent
- **THEN** the producer exchange fails with a reply-timeout error
  within the bound

#### Scenario: late reply is dropped

- **GIVEN** a pending request whose replyTimeout expires
- **WHEN** the reply arrives afterwards
- **THEN** the correlation entry no longer exists, the reply is
  dropped, and no handler leaks

#### Scenario: failed route sends no reply

- **GIVEN** a replier route whose processing fails
- **WHEN** the route fails
- **THEN** no message is published to the inbound `replyTo`

### Requirement: Credential redaction

Broker passwords and URLs containing credentials SHALL NOT appear in
`Debug` output, logs, or error messages; broker config types follow
ADR-0051 redaction at diagnostic boundaries.

#### Scenario: password never traverses Debug

- **GIVEN** a resolved broker config holding a password
- **WHEN** the config is formatted with `{:?}`
- **THEN** the output contains a redaction marker, not the password

### Requirement: Health check

The component SHALL expose an async health check probing the broker
connection with a bounded timeout (5 s) and reporting Unhealthy
(`CheckResult::unhealthy`, kafka health shape) when the probe fails or
times out.

#### Scenario: broker down reports Unhealthy

- **GIVEN** a configured broker that is unreachable
- **WHEN** the health check runs
- **THEN** it returns an Unhealthy result within the probe timeout

### Requirement: Metrics emission

The component SHALL emit `camel_component_operations_total{component,
operation, outcome}` per the component-metrics-emission spec:
`operation="publish"` (success/failure) from the producer and
`operation="consume"` (success/failure) from the consumer, with the
standard component label set.

#### Scenario: publish outcome counted

- **GIVEN** a producer that published one message successfully and one
  that failed at the broker (for example a publish to a missing
  exchange, which closes the channel with a 404)
- **WHEN** metrics are scraped
- **THEN** `camel_component_operations_total{component="rabbitmq",
  operation="publish", outcome="success"}` is 1 and
  `outcome="failure"` is 1

#### Scenario: consume outcome counted

- **GIVEN** a consumer whose route succeeded for one message and failed
  for another
- **WHEN** metrics are scraped
- **THEN** `camel_component_operations_total{component="rabbitmq",
  operation="consume", outcome="success"}` is 1 and
  `outcome="failure"` is 1

### Requirement: Option-phase alignment

Every URI option exposed in the component metadata descriptor (and
therefore schema-check output) SHALL be parsed AND behaviorally used by
the code landed in the same phase. The metadata parity unit test SHALL
assert the descriptor option set equals the implemented set for the
landed phases (a const list per phase), and each listed option SHALL
have at least one behavior test that fails when the option is ignored.

#### Scenario: metadata parity holds at every phase exit

- **GIVEN** the component metadata descriptor at any phase exit
- **WHEN** the parity unit test runs
- **THEN** the descriptor's option names equal the phase const list
  exactly — no extra parsed-but-unused options, no implemented options
  missing from metadata

### Requirement: Docker integration tier activation

Integration tests requiring a real broker SHALL share one fixture that
runs `rabbitmq` under docker with a mapped AMQP port, probes readiness
with a real AMQP handshake, and tears the container down. Two binary
activation rules: with `RABBITMQ_ITEST` unset, broker-dependent tests
SHALL NOT run and SHALL print an explicit notice naming
`RABBITMQ_ITEST=1`; with `RABBITMQ_ITEST=1` and docker unavailable, the
tests SHALL panic with an `infra-unavailable` message naming the
requirement (no silent skip).

#### Scenario: fixture round trip

- **GIVEN** `RABBITMQ_ITEST=1` and docker available
- **WHEN** the integration suite runs
- **THEN** the fixture broker is reachable, tests run against it, and
  the container is removed afterwards

#### Scenario: unset gate prints notice and does not run

- **GIVEN** `RABBITMQ_ITEST` unset
- **WHEN** the integration suite runs
- **THEN** broker-dependent tests do not execute and the output carries
  a notice naming `RABBITMQ_ITEST=1`

#### Scenario: gated tier without docker panics

- **GIVEN** `RABBITMQ_ITEST=1` and docker unavailable
- **WHEN** the integration suite runs
- **THEN** the broker-dependent tests panic with an infra-unavailable
  message naming docker
