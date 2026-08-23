# JMS

The JMS component publishes to and consumes from JMS brokers through a Java bridge process. It supports ActiveMQ Classic and ActiveMQ Artemis. One crate covers both directions. The Consumer subscribes to a destination and submits one Exchange per message. The Producer publishes the Exchange body to a destination.

The component does not implement JMS in Rust. It delegates protocol work to a native Java bridge binary over gRPC. The bridge is downloaded once and cached.

## Schemes

Three schemes share one bridge pool:

| Scheme | Shorthand | Locks broker type |
| --- | --- | --- |
| `jms` | rejected (ambiguous) | no |
| `activemq` | `activemq:orders` → queue | yes |
| `artemis` | `artemis:orders` → queue | yes |

The `jms:` scheme requires an explicit destination type. `jms:orders` returns an error. Use `jms:queue:orders` or `jms:topic:orders`.

The `activemq:` and `artemis:` schemes set the broker type at the URI level. They override any `broker_type` declared in the broker config. Use `jms:` when you want the broker type to come from configuration.

## Example

The jms-example wires a timer-driven producer and a log consumer against a testcontainers broker:

```rust,ignore
{{#include ../../../examples/jms-example/src/main.rs:jms-producer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: jms-producer
    from: "timer:tick?period=3000"
    steps:
      - set_body: '{"event":"order","source":"rust-camel"}'
      - to: "activemq:queue:orders"
```

The Rust example starts an ActiveMQ Classic container through testcontainers. The broker URL is read from the container port. Substitute your real broker address in production.

</details>

```rust,ignore
{{#include ../../../examples/jms-example/src/main.rs:jms-consumer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: jms-consumer
    from: "activemq:orders"
    steps:
      - to: "log:info?showHeaders=true"
```

The consumer uses the `activemq:orders` shorthand. It is equivalent to `activemq:queue:orders`. The destination type defaults to queue for the broker-specific schemes.

</details>

## URI

```text
jms:queue:<name>[?param=value&...]
jms:topic:<name>[?param=value&...]
activemq:queue:<name>[?param=value&...]
activemq:topic:<name>[?param=value&...]
artemis:queue:<name>[?param=value&...]
artemis:topic:<name>[?param=value&...]
activemq:<name>      # shorthand, defaults to queue
artemis:<name>       # shorthand, defaults to queue
```

| Parameter | Default | Description |
| --- | --- | --- |
| `broker` | configured `default_broker` | Named broker from the `[components.jms.brokers]` table |
| `acknowledgementMode` | `Auto` | `Auto`, `Client`, `DupsOk`, or `Transacted` |
| `messageSelector` | — | SQL-92 selector expression for filtering inbound messages |
| `concurrentConsumers` | `1` | Number of parallel consumer tasks for this endpoint |
| `transactionMode` | `None` | `None` or `Session` (Session is not yet implemented) |
| `timeToLive` | — | Message time-to-live in milliseconds |
| `priority` | — | Message priority 0-9 (9 is highest) |
| `persistentDelivery` | `true` | PERSISTENT or NON_PERSISTENT delivery mode |
| `mapJmsHeaders` | `true` | Map JMS headers and properties to Exchange headers |
| `exchangePattern` | `InOnly` | `InOnly` or `InOut` (InOut is not yet implemented) |

Credentials and the broker URL are not URI parameters. They live in the `[components.jms.brokers.<name>]` table.

## Broker configuration

Brokers are declared in `Camel.toml`. The component creates one Java bridge process per broker. The bridge pool admits at most `max_bridges` (default 8) bridges concurrently.

```toml
[default.components.jms]
default_broker = "main"

[default.components.jms.brokers.main]
broker_url  = "tcp://localhost:61616"
broker_type = "activemq"   # "activemq" | "artemis"
username    = "admin"      # optional
password    = "admin"      # optional
```

The bridge binary downloads on first use. It comes from a configured release URL and is cached at `~/.cache/rust-camel/jms-bridge/`. The download is SHA256-verified. Set `CAMEL_JMS_BRIDGE_BINARY_PATH` to point at a local build for development.

## Consumer

`activemq:queue:orders` subscribes to a destination. The Consumer submits one Exchange per inbound JMS message. The Exchange body carries the message payload. With `mapJmsHeaders=true` (the default), the headers carry `JMSMessageID`, `JMSCorrelationID`, `JMSTimestamp`, `JMSDestination`, and `JMSPriority`.

The body is typed from the JMS `content_type`. `text/*` becomes `Body::Text`. `application/json` becomes `Body::Json` when valid JSON. Binary content becomes `Body::Bytes`. The consumer pre-flights the bridge slot before starting. A missing or degraded bridge fails fast with `JMS bridge not available`.

The `concurrentConsumers` parameter spawns N parallel consumer tasks on the same destination. Each task subscribes independently and submits Exchanges into the shared route pipeline. `messageSelector` filters messages at the broker with a SQL-92 expression.

## Producer

`activemq:queue:orders` sends the Exchange body to a destination. The content type is inferred from the body. `Body::Text` sends `text/plain`. `Body::Json` sends `application/json`. `Body::Xml` sends `text/xml`. An explicit `Content-Type` header wins over inference.

The Producer uses a semaphore for backpressure. The default concurrency limit is 128 in-flight sends. When the limit is reached, `poll_ready` still returns ready. The `call` future waits on the semaphore. ADR-0024 classifies a closed semaphore as `ConsumerStopping`.

A successful send returns the Exchange unchanged. The `JMSMessageID` header carries the broker-assigned message ID. A send failure on a gRPC transport error refreshes the channel. The original send is not retried. A retry on a non-idempotent write would cause duplicates. The caller decides whether to retry.

## Java bridge

The component does not speak JMS. A native Java bridge process handles the JMS protocol and the TCP connection. The component talks to the bridge over gRPC. The bridge binary is a `jlink` image. The host does not need a Java runtime.

The bridge pool assigns one bridge per broker. The first send or consume starts the bridge. The bridge's ephemeral gRPC port comes from its stdout. A health monitor pings the bridge every `healthCheckIntervalMs` (default 5s). A failed health check moves the slot to `Degraded`, then `Restarting`. Restarts use exponential backoff capped at 120 seconds. After 10 failed restart attempts the slot stays `Degraded`.

The Consumer observes bridge state through a watch channel. A pending bridge returns `Pending` from `poll_ready`. A degraded bridge returns `Err` with the reason. The producer waits for the bridge to become `Ready` before sending.

## Trust boundary

Per ADR-0032, incoming JMS headers, bodies, correlation IDs, and destinations enter `exchange.input` without validation. The route is responsible for validation when data crosses into a control action, a resource decision, or an executable sink.

Credentials cross the gRPC boundary in plain text for the username and through a `Redacted` wrapper for the password. `BrokerConfig` redacts the password in `Debug` output. `BridgeSlot` omits its `credentials` field. `redact_url` strips user information from URLs before logging. The audit command `rg '#\[derive.*Debug' crates/components/camel-jms/src/` checks that no type holding a password derives `Debug`.

## Error handling

ADR-0007 governs Consumer shutdown. `JmsConsumer::stop` cancels the `CancellationToken` and waits up to 5 seconds for the consumer tasks to finish. Tasks that do not exit in that window are aborted. An in-flight `ConsumerContext::send` completes before the loop checks cancellation. The Consumer does not restart itself. Route supervision owns restart.

ADR-0012 classifies log sites as outside-contract or system-broken. A consumer-side `ctx.send` failure increments the `b-prime:jms:consumer-send` metric and logs at `error!`. A bridge restart that exhausts the attempt cap logs at `error!` and leaves the slot in `Degraded`.

## Limitations

- The bridge uses `AUTO_ACKNOWLEDGE`. Messages are acknowledged on delivery, not after processing. A failed route cannot request redelivery.
- Durable topic subscribers are not supported.
- IBM MQ is not supported.
- The `InOut` exchange pattern and `Session` transaction mode log a warning and fall back to the default.

**Reference**: [JMS crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-jms/CONTEXT.md). Example source: [`examples/jms-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/jms-example).
