# RabbitMQ

The RabbitMQ component produces to and consumes from RabbitMQ brokers over native AMQP 0-9-1. One crate covers both directions. The Consumer subscribes to a queue and submits one Exchange per delivery. The Producer publishes the Exchange body to an exchange with a routing key.

The component speaks AMQP 0-9-1 in Rust through `lapin` 4.12. It does not use a Java bridge. Reconnection runs through the shared `NetworkRetryPolicy`, the same policy the JMS and MQTT components use.

The `amqp:` scheme is not this component. It is reserved for a future AMQP 1.0 component.

## Example

The rabbitmq-example wires a timer-driven producer and a log consumer against a local RabbitMQ fixture:

```rust,ignore
{{#include ../../../examples/rabbitmq-example/src/main.rs:rabbitmq-consumer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: rabbitmq-consumer
    from: "rabbitmq:default?queue=demo&autoDeclare=true"
    steps:
      - to: "log:info?showHeaders=true"
```

The consumer uses the default exchange and the `demo` queue. `autoDeclare=true` declares the queue at start. See the auto-declare divergence below.

</details>

```rust,ignore
{{#include ../../../examples/rabbitmq-example/src/main.rs:rabbitmq-producer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: rabbitmq-producer
    from: "timer:tick?period=1000"
    steps:
      - set_body: '{"event":"order","source":"rust-camel"}'
      - to: "rabbitmq:default?queue=demo"
```

The producer publishes to the default exchange with the routing key `demo`, which falls back to the queue name. The example reads the broker URL from `RABBITMQ_URL`.

</details>

## URI

```text
rabbitmq:<exchange>?queue=<queue>[&routingKey=<key>][&broker=<name>][&<option>=<value>...]
```

- `<exchange>` is the target exchange. An empty path or the literal `default` selects the broker's default exchange. The default exchange routes by queue name, so `rabbitmq:default?queue=orders` publishes to `orders`.
- A consumer requires `queue`.
- A producer target is `routingKey` when set, otherwise `queue`. A URI must carry at least one of the two.
- `broker` selects a named broker. When exactly one broker is configured the parameter is optional. When several are configured the URI must name one.

```text
# Consumer
rabbitmq:orders?queue=orders

# Producer with an explicit routing key
rabbitmq:events?queue=orders&routingKey=orders.created

# Named broker
rabbitmq:orders?queue=orders&broker=secondary
```

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

`queueArguments` is a JSON object of `string -> string`. Each value becomes a long-string `FieldTable` entry, for example `queueArguments={"x-dead-letter-exchange":"dlx"}` (URL-encoded in the URI). A non-object or a non-string value fails closed.

This table is the frozen P4 parity set. The metadata parity test `metadata_parity_p4_frozen` asserts that the descriptor exposes exactly these names.

## Broker configuration

Brokers are declared in `Camel.toml`. The component creates one connection manager per named broker.

```toml
[components.rabbitmq.brokers.main]
url = "amqp://localhost:5672"
username = "guest"
password = "${env:RABBITMQ_PASSWORD}"
vhost = "/"
```

Broker keys are exactly `url`, `username`, `password`, and `vhost`. The last three are optional and override whatever the URL carries. Values support the platform `${env:...}` interpolation. An unknown key fails at boot, including a `tls` section (see the TLS note).

An optional reconnect policy overrides the per-component default:

```toml
[components.rabbitmq]
reconnect = { max_attempts = 0, initial_delay_ms = 5000, multiplier = 2.0, max_delay_ms = 30000 }
```

The default policy retries without limit with exponential backoff from 5 s to 30 s.

Credentials never render in a diagnostic. The password wrapper renders as `<redacted>`, and a broker URL renders through the canonical URL redactor, which masks URL user information. This follows ADR-0051 and ADR-0076.

## Consumer

`rabbitmq:orders?queue=orders` subscribes to the `orders` queue. The Consumer submits one Exchange per delivery. The Exchange body carries the message payload. The default exchange routes a publish to the queue whose name equals the routing key.

The `concurrentConsumers` parameter spawns N parallel channel and consumer engines on the same queue. `prefetch` bounds the unacknowledged deliveries per engine. A consumer start waits up to 60 s for the connection and then runs one topology RPC bounded by 10 s.

The Consumer acknowledges a delivery only after the route completes. This differs from the JMS component, whose `AUTO_ACKNOWLEDGE` bridge acknowledges on handoff.

## Producer

`rabbitmq:orders?queue=orders` sends the Exchange body to the target. The `persistent` parameter sets the delivery mode. A successful send returns the Exchange unchanged. A send failure returns `Err`; the route `ErrorHandler` owns the operational signal.

The Producer uses publisher confirms. `confirmTimeout` bounds one confirm wait. A disconnected producer fails its publish within 2 s. When `mandatory=true` an unroutable publish becomes an exchange failure instead of a silent drop.

## Delivery semantics

A consumer acknowledges a delivery only after the route completes.

- A successful route acknowledges the delivery.
- A failed route rejects without requeue by default, so a dead-letter exchange receives the message. `requeueOnFailure=true` requeues instead.

A dead-letter exchange is a queue argument, not a URI option. Declare it with the active declare:

```text
rabbitmq:default?queue=orders&autoDeclare=true&queueArguments={"x-dead-letter-exchange":"dlx"}
```

WARNING: requeueing on failure without a broker-side delivery limit (a queue `x-delivery-limit`, or a dead-letter exchange) forms a hot loop. The broker redelivers the same message immediately and the failing route requeues it forever.

A route transport or lifecycle loss (`ChannelClosed`) abandons the delivery with no disposition. The consumer closes only its own channel, so the broker redelivers the still-unacked message. The component is at-least-once. A consumer must tolerate duplicates.

Each engine stamps the connection generation of the channel it consumes on. A delivery whose connection has been replaced is dropped with no ack or nack, so the broker redelivers it after reconnect.

## Auto-declare divergence

`autoDeclare` defaults to `false`. A consumer start then passively checks that the queue exists and fails fast when it does not. The Camel `spring-rabbitmq` consumer defaults to `true`. This component requires an explicit opt-in because an active declare mutates broker topology.

With `autoDeclare=true` the consumer start declares the exchange, the queue, and the binding. The exchange is always declared durable, and `durableQueue` governs the queue only. For the default exchange (empty path) the exchange declare and the bind are skipped, because the reserved default exchange cannot be declared or bound. A conflicting declare fails the route start.

## Request/reply

An InOut exchange uses RabbitMQ direct reply-to. The producer and the reply consumer share one confirm-enabled channel, as direct reply-to requires. The request carries `replyTo=amq.rabbitmq.reply-to` and a fresh UUID `correlationId`. A reply resolves the matching correlation. `replyTimeout` (default 30000 ms) bounds the wait. On timeout or cancellation the correlation entry is removed, so a late reply is dropped and no entry leaks.

The InOut path is programmatic. There is no URI pattern flag for it. A caller builds an InOut exchange and calls the producer:

```rust,ignore
use camel_api::{Body, Exchange, Message};
use camel_component_api::ProducerContext;
use tower::Service;

let mut producer = endpoint.create_producer(rt, &ProducerContext::default())?;
let request = Exchange::new_in_out(Message::new(Body::Text("world".to_string())));
let reply = producer.call(request).await?;
// reply.output carries the replier's body and headers
```

A replier is an ordinary consumer route. When the route completes `Ok` and the original request carried both `replyTo` and `correlationId`, the consumer selects the route's OUT message when present, otherwise the IN message, and takes the body and headers from that same selected message. It publishes the selection to `replyTo` before it acknowledges. A reply is transient (delivery mode 1) and best effort. A plain channel carries no publisher confirm, so a lost reply surfaces as a requester timeout. Requests are confirmed.

When the shared body converter cannot materialize an OUT body, reply preparation returns no intent and the requester times out. There is no new consumer-side signal. A failed reply publish is an infrastructure side effect, not a business route failure: the component records the ADR-0012 b-prime signal `b-prime:rabbitmq:reply-publish`, warns, and leaves the route's disposition unchanged.

## Headers and properties

Reserved basic-property names (`contentType`, `contentEncoding`, `priority`, `messageId`, `correlationId`, `replyTo`, `expiration`, `timestamp`) map onto AMQP basic properties in both directions. Every other header travels as a free-form AMQP header, written as a long string. Inbound deliveries also carry `rabbitmq.redelivered`. AMQP timestamps cross the wire as Unix seconds and surface as epoch milliseconds.

## TLS (ring)

The component selects lapin with exactly the `tokio`, `rustls--ring`, and `rustls-native-certs` features. `rustls--ring` selects rustls' ring provider, and `rustls-native-certs` supplies the system root store, so `amqps://` works against a system-trusted broker.

A custom `tls` section in a broker entry is not implemented. The boot config rejects it and names `tls` and the supported parameters, so the section never parses and is never silently ignored. A per-broker certificate override is tracked in bd `rc-l8ohw` as future work.

This note describes the component's own selection. It does not claim that the raw dependency tree has no `aws-lc-rs`. Shared workspace dependencies make `aws-lc-rs` reachable outside this component's gate. That reachability is tracked in bd `rc-2bofp`.

## Testing

Unit tests run without a broker. The integration tier needs Docker and activates only when `RABBITMQ_ITEST` is set:

```bash
RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq
```

When `RABBITMQ_ITEST` is unset the tier prints a skip notice and exits 0. When it is set and Docker is unavailable the tier panics `infra-unavailable`. It never skips silently. The fixture starts one `rabbitmq:3.13-alpine` container per test and removes it on drop. The tier follows ADR-0069.

The example uses the same image. Start a local fixture on loopback with explicit demo credentials:

```bash
docker run -d --rm --name rmq-example \
  -p 127.0.0.1:5672:5672 \
  -e RABBITMQ_DEFAULT_USER=rmq \
  -e RABBITMQ_DEFAULT_PASS=rmq \
  rabbitmq:3.13-alpine
```

Then run the example. It reads `RABBITMQ_URL`, defaulting to `amqp://rmq:rmq@127.0.0.1:5672/%2f`:

```bash
cargo run -p rabbitmq-example
```

The `rmq`/`rmq` credentials belong to this ephemeral local fixture. They are not a production secret. Override `RABBITMQ_URL` for any other broker.

## Limitations

- AMQP 1.0 is not supported. The `amqp:` scheme is reserved for a future component.
- Streams, transactions, batching, and a manual ack mode are not supported.
- A per-broker TLS certificate override is not supported (`rc-l8ohw`).
- The component is at-least-once. A redelivery can duplicate a message.

**Reference**: [RabbitMQ crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-rabbitmq/CONTEXT.md). Example source: [`examples/rabbitmq-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/rabbitmq-example).
