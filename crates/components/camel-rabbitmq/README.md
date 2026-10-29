# camel-component-rabbitmq

RabbitMQ component for [rust-camel](https://github.com/kennycallado/rust-camel), speaking
AMQP 0-9-1 over the `rabbitmq:` URI scheme.

The component uses `lapin` 4.12 with lapin's built-in auto-recovery disabled. Reconnection runs
through the shared `NetworkRetryPolicy`, the same policy the JMS and MQTT components use. The
`amqp:` scheme is not this component; it is reserved for a future AMQP 1.0 component.

## URI format

```text
rabbitmq:<exchange>?queue=<queue>[&routingKey=<key>][&broker=<name>][&<option>=<value>...]
```

- `<exchange>` is the target exchange. An empty path or the literal `default` selects the broker's
  default exchange. The default exchange routes by queue name, so `rabbitmq:default?queue=orders`
  publishes to `orders`.
- A consumer requires `queue`.
- A producer target is `routingKey` when set, otherwise `queue`. A URI must carry at least one of
  the two.
- `broker` selects a named broker. When exactly one broker is configured the parameter is optional.
  When several are configured the URI must name one.

```text
# Consumer
rabbitmq:orders?queue=orders

# Producer with an explicit routing key
rabbitmq:events?queue=orders&routingKey=orders.created

# Named broker
rabbitmq:orders?queue=orders&broker=secondary
```

## Camel.toml configuration

```toml
[components.rabbitmq.brokers.main]
url = "amqp://localhost:5672"
username = "guest"
password = "${env:RABBITMQ_PASSWORD}"
vhost = "/"
```

Broker keys are exactly `url`, `username`, `password`, `vhost`, and an optional `tls` section (see
[TLS](#tls-amqps)). `username`, `password`, and `vhost` are optional and override whatever the URL carries. Values
support the platform `${env:...}` interpolation. An unknown key fails at boot.

An optional reconnect policy overrides the per-component default:

```toml
[components.rabbitmq]
reconnect = { max_attempts = 0, initial_delay_ms = 5000, multiplier = 2.0, max_delay_ms = 30000 }
```

The default policy retries without limit with exponential backoff from 5 s to 30 s.

## Credential redaction

A broker password never renders in a diagnostic. The password wrapper renders as `<redacted>`, and a
broker URL renders through the canonical URL redactor, which masks URL user information. This follows
ADR-0051 and ADR-0076.

## URI options

The set below is the frozen P4 parity set. The metadata parity test `metadata_parity_p4_frozen`
asserts that the descriptor exposes exactly these names, derived from the `P1_OPTIONS`,
`P2_OPTIONS`, `P3_OPTIONS`, and `P4_OPTIONS` constants.

| Parameter | Default | Description |
|-----------|---------|-------------|
| `broker` | (none) | Named broker from `[components.rabbitmq.brokers]`; optional when exactly one is configured |
| `queue` | (none) | Queue to consume from; required for a consumer |
| `routingKey` | (none) | Publish routing key; falls back to `queue` |
| `persistent` | `true` | Delivery mode 2 (`true`) or 1 (`false`) |
| `contentType` | (none) | Content type; overrides the exchange `contentType` header |
| `requeueOnFailure` | `false` | Requeue a failed route's delivery instead of rejecting it (see the warning below) |
| `prefetch` | `10` | Maximum unacknowledged deliveries per channel (`basic.qos`); greater than 0 |
| `concurrentConsumers` | `1` | Channel and consumer engines; prefetch applies per engine; greater than 0 |
| `confirmTimeout` | `5000` | Publisher-confirm wait bound in milliseconds |
| `replyTimeout` | `30000` | InOut direct reply-to wait bound in milliseconds |
| `mandatory` | `false` | Return an unroutable publish as an exchange failure instead of a silent drop |
| `autoDeclare` | `false` | Actively declare the exchange, queue, and binding at consumer start |
| `exchangeType` | `direct` | Exchange kind for the active declare |
| `durableQueue` | `true` | Durability of the actively declared queue |
| `queueArguments` | (none) | JSON object of string values, URL-encoded, for the actively declared queue |

`queueArguments` is a JSON object of `string -> string`. Each value becomes a long-string
`FieldTable` entry, for example `queueArguments={"x-dead-letter-exchange":"dlx"}` (URL-encoded in
the URI). A non-object or a non-string value fails closed.

## Delivery semantics

A consumer acknowledges a delivery only after the route completes.

- A successful route acknowledges the delivery.
- A failed route rejects without requeue by default, so a dead-letter exchange receives the
  message. `requeueOnFailure=true` requeues instead.

WARNING: requeueing on failure without a broker-side delivery limit (a queue `x-delivery-limit`, or
a dead-letter exchange) forms a hot loop. The broker redelivers the same message immediately and the
failing route requeues it forever.

A route transport or lifecycle loss (`ChannelClosed`) abandons the delivery with no disposition.
The consumer closes only its own channel, so the broker redelivers the still-unacked message.

The component is at-least-once. A broker restart, a mid-flight stop, or a dropped disposition makes
the broker redeliver, so a consumer must tolerate duplicates.

Each engine stamps the connection generation of the channel it consumes on. A delivery whose
connection has been replaced is dropped with no ack or nack, so the broker redelivers it after
reconnect. A local cancellation cancels only this consumer's engines and preserves the shared
connection and the sibling engines on it.

## Auto-declare divergence

`autoDeclare` defaults to `false`. A consumer start then passively checks that the queue exists and
fails fast when it does not. The Camel `spring-rabbitmq` consumer defaults to `true`. This component
requires an explicit opt-in because an active declare mutates broker topology.

With `autoDeclare=true` the consumer start declares the exchange, the queue, and the binding. The
exchange is always declared durable, and `durableQueue` governs the queue only. For the default
exchange (empty path) the exchange declare and the bind are skipped, because the reserved default
exchange cannot be declared or bound. A conflicting declare fails the route start.

## Request/reply

An InOut exchange uses RabbitMQ direct reply-to. The producer and the reply consumer share one
confirm-enabled channel, as direct reply-to requires. The request carries
`replyTo=amq.rabbitmq.reply-to` and a fresh UUID `correlationId`. A reply resolves the matching
correlation. `replyTimeout` (default 30000 ms) bounds the wait. On timeout or cancellation the
correlation entry is removed, so a late reply is dropped and no entry leaks.

The InOut path is programmatic. A caller builds an InOut exchange and calls the producer:

```rust
use camel_api::{Body, Exchange, Message};
use camel_component_api::ProducerContext;
use tower::Service;

let mut producer = endpoint.create_producer(rt, &ProducerContext::default())?;
let request = Exchange::new_in_out(Message::new(Body::Text("world".to_string())));
let reply = producer.call(request).await?;
// reply.output carries the replier's body and headers
```

A replier is an ordinary consumer route. When the route completes `Ok` and the original request
carried both `replyTo` and `correlationId`, the consumer selects the route's OUT message when present,
otherwise the IN message, and takes the body and headers from that same selected message. It
publishes the selection to `replyTo` before it acknowledges. A reply is transient (delivery mode 1)
and best effort. A plain channel carries no publisher confirm, so a lost reply surfaces as a
requester timeout. Requests are confirmed.

When `mandatory=true` the InOut request serializes on the shared reply-state publish lane, because
lapin pops a mandatory `basic.return` in FIFO order per confirmation with no per-publish
correlation. InOnly mandatory publishes use a per-call dedicated channel for the same reason.

A failed reply publish is an infrastructure side effect, not a business route failure. The component
records the ADR-0012 b-prime signal `b-prime:rabbitmq:reply-publish`, warns, and leaves the route's
disposition unchanged. A successful route is still acknowledged.

## Headers and properties

Reserved basic-property names (`contentType`, `contentEncoding`, `priority`, `messageId`,
`correlationId`, `replyTo`, `expiration`, `timestamp`) map onto AMQP basic properties in both
directions. Every other header travels as a free-form AMQP header, written as a long string. Inbound
deliveries also carry `rabbitmq.redelivered`. AMQP timestamps cross the wire as Unix seconds and
surface as epoch milliseconds.

## TLS (amqps://)

`amqps://` selects TLS from the URL scheme alone. An `amqps://` broker never falls back to
plaintext. Without a `tls` section, lapin performs the handshake against the system native roots.

A custom CA adds a trust anchor:

```toml
[components.rabbitmq.brokers.secure]
url = "amqps://broker.internal:5671"

[components.rabbitmq.brokers.secure.tls]
ca_cert = "/etc/rabbitmq/tls/ca.pem"
```

The trust policy is additive. The component trusts the system native roots AND the certificates in
`ca_cert`; it does not replace the native store.

TLS SNI and certificate verification always use the host from the broker URL. There is no override.
`tls.server_name` is rejected at boot, because lapin 4.12.0 derives both values from the URL host
and exposes no override:

```text
rabbitmq broker '<name>': tls.server_name is unsupported by lapin 4.12.0; TLS SNI and certificate verification use the broker URL host; remove tls.server_name.
```

Boot validation is fail-closed for every configured broker, and each error names the broker only.
It rejects `tls.*` on an `amqp://` URL, a missing or unreadable `tls.ca_cert`, a file with no PEM
certificate, a malformed PEM frame or body, a certificate that does not parse as DER, and an
unknown `tls.*` key such as `skip_verify`.

TLS provides server authentication only. mTLS and client certificates are not supported, and the
client `identity` stays unset.

The component selects lapin with the `rustls--ring` and `rustls-native-certs` features.
`rustls--ring` enables only `rustls-connector/rustls--ring`, so the connector's `crypto_provider`
fallback selects the ring provider. A process-level `CryptoProvider::install_default` call takes
precedence over that fallback. Any crate in a full CLI build can install the default, so this note
does not prove the runtime provider beyond that chain and the TLS handshake tests. Process-level
provider selection is tracked in bd `rc-2bofp`.

This note describes the component's own selection. It does not claim that the raw dependency tree
has no `aws-lc-rs`. Shared workspace dependencies make `aws-lc-rs` reachable outside this
component's gate. That reachability is tracked in bd `rc-2bofp`.

## Bounds

Each bound is local to one operation. `confirmTimeout` bounds one confirm, and `replyTimeout` bounds
one reply wait. Neither implies a whole-call deadline. A disconnected producer fails its publish
within 2 s. A consumer start waits up to 60 s for the connection and then runs one topology RPC
bounded by 10 s.

## Testing

Unit tests run without a broker. The integration tier needs Docker and activates only when
`RABBITMQ_ITEST` is set:

```bash
RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq
```

When `RABBITMQ_ITEST` is unset the tier prints a skip notice and exits 0. When it is set and Docker
is unavailable the tier panics `infra-unavailable`. It never skips silently. The fixture starts one
`rabbitmq:3.13-alpine` container per test and removes it on drop. The tier follows ADR-0069.

The TLS handshake tests run without a broker, against a local TLS server with static test
certificates. The TLS end-to-end tier activates only when `RABBITMQ_TLS_ITEST` is set, and reads
`RABBITMQ_TLS_URL` and `RABBITMQ_TLS_CA`. An optional `RABBITMQ_TLS_MISMATCH_URL` (the same broker reached through a host name that is not in its certificate) enables the hostname-mismatch rejection case.

## Divergences

| Aspect | This component | Kafka / MQTT |
|--------|----------------|--------------|
| Crash and redelivery | The consumer engine owns channel-local recovery. A healthy connection re-opens its subscription; a dead origin is reported to the manager, and the retry policy owns reconnection. A route transport loss terminates the engine and closes its channel. | The consumer does not pin route health on task crash; the runtime owns route failure. |
| Unmaterializable InOut body | When the shared body converter cannot materialize an OUT body, reply preparation returns no intent and the requester times out. There is no new consumer-side signal. | Not applicable. |

## Installation

```toml
[dependencies]
camel-component-rabbitmq = "*"
```

The component ships in the regular CLI build under the `rabbitmq` cargo feature.
