# camel-component-rabbitmq — CONTEXT

## Scope and architecture

camel-component-rabbitmq is the native AMQP 0-9-1 adapter for rust-camel. It serves the `rabbitmq:`
URI scheme with a producer and a consumer role. It uses `lapin` 4.12 with lapin's built-in
auto-recovery disabled.

The component keeps one `RabbitConnectionManager` per named broker, created once and cached for the
component's lifetime. Producers and consumers share that manager's connection. Each consumer engine
owns its own channel.

`RabbitMqBundle` registers the scheme from `[components.rabbitmq]`. The bundle binds a
registration-time lifecycle context, so a reconnect chain observes the current runtime shutdown
token and a runtime stop cancels a pending attempt.

## Key design decisions

- Generation counter. `RabbitConnectionManager` stamps a monotonically increasing generation on
  every published connection. A disposition, a channel-open failure, or a connection-level error is
  qualified by the generation it was captured on. A late failure therefore cannot demote a successor
  (the failure fence), and a stale delivery tag is dropped instead of acked on a new channel.
- No auto-recover. lapin's recovery stays off. `NetworkRetryPolicy` owns reconnection. Each
  reconnect derives a manager-local child of the current runtime shutdown token, so a runtime stop
  cancels a pending attempt and the manager stays alive for the next start.
- Confirms always on. Every producer channel is confirm-enabled once. A publish awaits the broker
  confirm, bounded by `confirmTimeout` (default 5000 ms). A confirm timeout invalidates the cached
  channel because its confirm accounting is then uncertain. A `mandatory` publish runs on its own
  dedicated channel, so a FIFO `basic.return` cannot pair with another publish's confirm.
- Direct reply-to. An InOut request and its reply consumer share one confirm-enabled channel, as
  direct reply-to requires. `CorrelationTable` maps each request UUID onto a oneshot sender.
  `ReplyState` owns the table, the mandatory publish lane, the retirement flag, and the reply loop.
  Its `Drop` retires the state without blocking a runtime thread.

## Seams

- `RabbitConnectionManager`: connection lifecycle, generation, bounded connect, and channel open.
- `DeliveryAcker`: ack and nack over one delivery tag. Production wraps `lapin::Acker`; unit tests
  use fakes.
- `CorrelationTable`: InOut request-to-reply correlation.
- `ReplyPublisher`: consumer-side reply publish. Production wraps `lapin::Channel`; unit tests use
  fakes.
- `ReplyState` and `ReplySession`: producer-side InOut session. `ReplyState::retire_sync` cancels and
  aborts the reply loop; `ReplySession` adds the shared channel.

## Delivery and disposition

A consumer acknowledges a delivery only after the route completes. A successful route acknowledges.
A failed route rejects without requeue by default, so a dead-letter exchange receives the message.
`requeueOnFailure=true` requeues instead. Requeueing without a broker-side delivery limit forms a hot
loop.

A route transport or lifecycle loss (`ChannelClosed`) abandons the delivery with no disposition. The
consumer closes only its own channel, so the broker redelivers the still-unacked message. The
component is at-least-once, and a consumer must tolerate duplicates.

A disposition is applied only while the connection generation still matches. A pre-reconnect tag is
dropped, so the broker redelivers. When a delivery stream ends, the engine inspects the origin
connection. A still-healthy origin re-opens a subscription on the same connection and generation, so
sibling engines keep their generation and their acks stay valid. Only a dead origin is reported to
the manager, which fences on the generation and lets the retry policy own reconnection.

## Divergences vs Kafka and MQTT

- Crash and redelivery. The RabbitMQ consumer engine owns channel-local recovery inside the running
  consumer. Kafka and MQTT do not pin route health on task crash; the runtime owns route failure.
- Unmaterializable InOut body. The consumer selects the route's OUT message when present, otherwise
  the IN message, and takes the body and headers from that same selected message. When the shared
  body converter cannot materialize the selected message, reply preparation returns no intent and the
  requester times out. There is no new consumer-side signal. This is a documented limitation, not an
  accepted risk.

## Credential redaction

`SecretString` implements `Debug` manually and renders `<redacted>`. `RabbitBrokerConfig` implements
`Debug` manually and renders its URL through the canonical URL redactor, which masks URL user
information, so a broker password never reaches a diagnostic. This follows ADR-0051 and ADR-0076.

## Log-level policy (ADR-0012)

The component has no `error!` site. Its one component-operations error signal is the consumer
reply-publish failure: it increments `b-prime:rabbitmq:reply-publish` through
`RuntimeObservability::metrics` before a `warn!`, and the route's own disposition is unchanged.
