# MQTT

The MQTT component publishes to and consumes from MQTT 3.1.1 brokers. The Consumer subscribes to topic filters and submits one Exchange per incoming publish. The Producer is a Tower `Service<Exchange>` that publishes the Exchange body to a topic.

The mqtt-example wires a timer-driven producer and a log consumer against a Mosquitto broker started by testcontainers:

```rust,ignore
{{#include ../../../examples/mqtt-example/src/main.rs:mqtt-producer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: mqtt-producer
    from: "timer:tick?period=3000&repeatCount=2"
    steps:
      - set_body: "hello-mqtt"
      - to: "mqtt://test/sensors/temp"
```

The example reads the broker port from a testcontainers container. Substitute your real broker name and credentials in `Camel.toml` under `[components.mqtt.brokers]`.

</details>

```rust,ignore
{{#include ../../../examples/mqtt-example/src/main.rs:mqtt-consumer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: mqtt-consumer
    from: "mqtt://test/sensors/#"
    steps:
      - to: "log:info?showHeaders=true"
```

Wildcard subscriptions (`sensors/#`) match every topic under the prefix. `+` matches a single level.

</details>

## URI

```
mqtt://<broker_name>[/<topic>][?query]
mqtts://<broker_name>[/<topic>][?query]
```

`<broker_name>` is a logical key, not a host:port. The component resolves it to a `MqttBrokerConfig` declared in `Camel.toml` under `[components.mqtt.brokers.<name>]`. The path segment becomes the default subscription filter for the Consumer and the default publish topic for the Producer. Use the `topics` query parameter for multi-filter subscriptions.

| Parameter | Default | Description |
| --- | --- | --- |
| `qos` | `1` | Quality of Service: `0` (AtMostOnce), `1` (AtLeastOnce), or `2` (ExactlyOnce) |
| `ackMode` | `auto` | `auto` acks on delivery. `manual` acks after the pipeline succeeds |
| `cleanSession` | `true` | Must be `false` when `ackMode=manual` and QoS 1 or 2 |
| `retain` | `false` | Retain published messages on the broker |
| `keepAliveSecs` | `60` | MQTT keep-alive interval in seconds |
| `maxPayloadBytes` | `262144` | Incoming payload limit (256 KB) |
| `clientId` | auto | Override the auto-generated client ID |
| `topics` | path | Comma-separated topic filters. Repeated `topics=` keys allowed |

Invalid `qos` or `ackMode` values fail endpoint creation with `CamelError::Config`.

## Consumer

`mqtt://<broker>/sensors/#` subscribes to a topic filter. The Consumer opens a TCP connection to the broker, subscribes, and feeds each incoming publish into the route as an Exchange. The Exchange body carries the MQTT payload. The headers carry `CamelMqttTopic`, `CamelMqttQos`, `CamelMqttRetained`, `CamelMqttDuplicate`, `CamelMqttClientId`, and `CamelMqttPacketId` (the last only for QoS 1 and 2).

Each route Consumer opens its own TCP connection. v1 has no shared connection pool ([ADR-0027](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0027-mqtt-component-3-1-1-per-endpoint-connections.md)). Account for the per-route connection when you size your broker.

Manual ack with QoS 1 or 2 requires `cleanSession=false`. With `cleanSession=true` the broker discards session state on reconnect and unacknowledged messages cannot be redelivered. Validation rejects the unsafe combination at endpoint creation. The ack decision uses the received packet QoS, not the subscription QoS. A subscription at QoS 1 can still receive QoS 0 messages, and those messages must never be manually acked.

## Producer

`mqtt://<broker>/sensors/temp` publishes the Exchange body to a topic. Each route endpoint creates a Tower `Service<Exchange>` Producer. Each Producer opens its own TCP connection to the broker (ADR-0027).

The body becomes the MQTT payload. The URI path sets the default publish topic and QoS. The headers `CamelMqttTopic`, `CamelMqttQos`, and `CamelMqttRetain` override the defaults for that exchange. `CamelMqttTopic` must not contain `+` or `#`.

Connection retries use the shared `NetworkRetryPolicy` with exponential backoff and jitter. Every backoff sleep is cancellation-aware and stops when the route shuts down. The driver loop logs retried connection errors at `warn!`. It has no runtime handle so it cannot call `error!` with a replacement signal.

## Configuration

Brokers live in `Camel.toml` under `[components.mqtt.brokers]`. The URI references the broker by name. Credentials stay in the config file, never in the route:

```toml
[default.components.mqtt]
client_id_prefix = "camel"

[default.components.mqtt.brokers.my-broker]
url = "mqtt://mqtt.example.com:1883"
username = "app-user"
password = "app-secret"
```

The `url` field accepts `mqtt://` (plain TCP) or `mqtts://` (TLS). `mqtts://` requires the `tls` cargo feature. The default features include TLS, so connections use rustls out of the box. To compile without TLS, disable default features:

```toml
camel-component-mqtt = { version = "0.20", default-features = false }
```

The component redacts the broker password in `Debug` output. mTLS (client certificate authentication) is not yet supported in v1.

## Connection lifecycle

Every Consumer and every Producer opens one TCP connection to the broker. The component uses no shared connection in v1. The connection carries a 10-second timeout. Unreachable brokers fail fast instead of hanging.

The auto-generated `client_id` follows the pattern `{prefix}-{route_id}-{hash6}` and truncates to 23 bytes (the MQTT 3.1.1 portable maximum). The hash input is the full endpoint URI for producers and the broker name plus subscription list for consumers. Set the `clientId` URI parameter to override.

**Reference**: [MQTT crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-mqtt/CONTEXT.md). Architecture decisions: [ADR-0027](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0027-mqtt-component-3-1-1-per-endpoint-connections.md). Example source: [`examples/mqtt-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/mqtt-example).
