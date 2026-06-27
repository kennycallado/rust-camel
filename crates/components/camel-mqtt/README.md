# camel-component-mqtt

> MQTT 3.1.1 component for [rust-camel](https://github.com/kennycallado/rust-camel), built on rumqttc-v4-next.

Provides consumer (event-driven subscriber) and producer (Tower `Service<Exchange>`) endpoints
for MQTT brokers. Uses JMS-style named broker configuration — credentials live in `Camel.toml`,
never in the URI.

## Overview

The MQTT component implements the MQTT 3.1.1 protocol via `rumqttc::AsyncClient`. It supports:

- **Consumer**: background task that subscribes to one or more MQTT topic filters and feeds received
  messages into a route pipeline. Wildcard subscriptions (`+`, `#`) are supported.
- **Producer**: a Tower [`Service`] that publishes message bodies to a broker topic. Topic, QoS, and
  retain flag can be overridden per-exchange via outbound headers.
- **Named broker config**: brokers are configured in `Camel.toml` under `[components.mqtt.brokers.<name>]`.
  URIs reference brokers by name (not by host:port inline), keeping credentials out of URLs.
- **Manual ack**: with `ackMode=manual`, QoS 1/2 messages are acknowledged only after the downstream
  pipeline succeeds. Requires `cleanSession=false` (enforced at config validation).
- **TLS**: `mqtts://` scheme with optional CA certificate, gated behind the `tls` feature (rustls).

## URI Format

```
mqtt://<broker_name>[/<topic>][?query]
mqtts://<broker_name>[/<topic>][?query]    # requires the `tls` feature
```

- `<broker_name>` — must match a key in `[components.mqtt.brokers]` (from `Camel.toml` or programmatic
  config). This is **not** a host:port — it is a logical name that resolves to a `MqttBrokerConfig`.
- `/<topic>` — a single path segment becomes the consumer subscription filter. If the topic contains
  no wildcards (`+` or `#`) it also serves as the producer's default publish topic.
- When no path topic is given (e.g. `mqtt://my-broker?topics=...`) subscriptions are specified via the
  `topics` query parameter.

**Examples:**

```
# Consumer: subscribe to sensors/#
mqtt://my-broker/sensors/#

# Consumer: multiple explicit topics
mqtt://my-broker?topics=sensors/temp,sensors/humidity

# Producer: publish to sensors/temp
mqtt://my-broker/sensors/temp
```

## URI Query Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `qos` | `1` | Quality of Service: `0` (AtMostOnce), `1` (AtLeastOnce), `2` (ExactlyOnce). Invalid values produce an error. |
| `ackMode` | `auto` | Acknowledgement mode: `auto` (ack on delivery), `manual` (ack only after pipeline success). |
| `cleanSession` | `true` | Start with a clean session. **Must be `false`** when using `ackMode=manual` with QoS 1 or 2. |
| `retain` | `false` | Retain published messages on the broker. |
| `keepAliveSecs` | `60` | MQTT keep-alive interval in seconds. |
| `maxPayloadBytes` | `262144` | Maximum incoming payload size (256 KB). Larger payloads raise `StreamLimitExceeded`. |
| `clientId` | — | Override the auto-generated client ID (see client_id generation below). |
| `topics` | path topic | Comma-separated MQTT topic filters. Supports repeated `topics=` keys and `%2C` for literal commas within a filter. When present, replaces the path-based subscription. |

When a parameter value is invalid (e.g. `qos=9`, `ackMode=fast`), the endpoint creation fails with a
descriptive `CamelError::Config`.

## Headers

### Inbound (set by the consumer on each exchange)

| Header | Type | Description |
|--------|------|-------------|
| `CamelMqttTopic` | `String` | Topic the message was received on. |
| `CamelMqttQos` | `String` | QoS level of the received message (`"0"`, `"1"`, or `"2"`). |
| `CamelMqttRetained` | `String` | Whether the message was retained (`"true"`/`"false"`). |
| `CamelMqttDuplicate` | `String` | Whether this is a duplicate delivery (`"true"`/`"false"`). |
| `CamelMqttPacketId` | `String` | MQTT packet ID. Only present for QoS 1 or 2. |
| `CamelMqttClientId` | `String` | Client ID of the subscribing connection. |

### Outbound (producer; override URI / config defaults)

| Header | Valid Values | Description |
|--------|-------------|-------------|
| `CamelMqttTopic` | Any non-wildcard topic string | Override the publish topic. Must not contain `+` or `#`. |
| `CamelMqttQos` | `"0"`, `"1"`, `"2"` | Override the publish QoS. Invalid values produce an error. |
| `CamelMqttRetain` | `"true"`, `"false"` | Override the retain flag. |

## Camel.toml Configuration

Broker credentials and connection parameters are configured under `[components.mqtt]` — never in the URI:

```toml
[default.components.mqtt]
client_id_prefix = "camel"

[default.components.mqtt.brokers.my-broker]
url = "mqtt://mqtt.example.com:1883"
username = "app-user"
password = "app-secret"

[default.components.mqtt.reconnect]
enabled = true
max_attempts = 10
initial_delay_ms = 1000
multiplier = 2.0
max_delay_ms = 30000
jitter_factor = 0.1
```

Multiple brokers are supported — add as many `[default.components.mqtt.brokers.<name>]` entries
as needed.

### client_id Generation

When no `clientId` URI parameter is given, the client ID is generated as:

```
{prefix}-{route_id}-{hash6}
```

Where:
- `prefix` defaults to `camel` (configurable via `client_id_prefix`).
- `route_id` is the Camel route ID.
- `hash6` is the first 6 hex characters of a SHA-256 hash of the endpoint's identity.

The total is truncated to ≤ 23 bytes (MQTT 3.1.1 portable maximum). The hash input incorporates
the **full endpoint URI** for producers (distinct topics get distinct IDs), and the broker name +
subscription list for consumers.

## TLS

TLS is supported via the `mqtts://` scheme and requires the `tls` feature:

```toml
[dependencies]
camel-component-mqtt = { features = ["tls"] }
```

```toml
[default.components.mqtt.brokers.my-broker]
url = "mqtts://mqtt.example.com:8883"
tls_ca_cert = "/etc/ssl/ca.pem"
```

mTLS (client certificate authentication) is **not yet supported** — see Known Limitations.

## Quick Start

### Add the Dependency

```bash
cargo add camel-component-mqtt       # base crate
cargo add camel-component-mqtt --features tls  # if using mqtts://
```

Register the component after configuring `Camel.toml`:

```rust
use camel_component_mqtt::MqttComponent;

// Register after configuring Camel.toml
ctx.register_component(MqttComponent::new());
```

### Programmatic (no Camel.toml)

```rust
use std::collections::HashMap;
use camel_component_mqtt::{MqttComponent, config::{MqttConfig, MqttBrokerConfig}};

let mut cfg = MqttConfig::default();
let mut brokers = HashMap::new();
brokers.insert("local".into(), MqttBrokerConfig {
    url: "mqtt://localhost:1883".into(),
    username: None,
    password: None,
    tls_ca_cert: None,
});
cfg.brokers = brokers;

let mqtt = MqttComponent::with_config(cfg).expect("valid MQTT config");

ctx.register_component(mqtt);

// Consumer: subscribe to sensors/#
let route = RouteBuilder::from("mqtt://local/sensors/#")
    .to("log:info")
    .build()?;

// Producer: publish every 3 seconds
let producer = RouteBuilder::from("timer:tick?period=3000")
    .set_body("hello".to_string())
    .to("mqtt://local/sensors/temp")
    .build()?;
```

## Running the Example

A runnable example is provided at `examples/mqtt-example/`:

```bash
cargo run -p mqtt-example
```

Requires Docker (starts a Mosquitto broker container). Demonstrates:
- Producer: timer → MQTT publish on `sensors/temp`
- Consumer: MQTT subscribe to `sensors/#` → log

## Manual Ack & cleanSession

With `ackMode=manual` and QoS 1 or 2, the broker must be able to redeliver unacknowledged messages
if the connection drops. This requires a **persistent session** — configuration rejects
`ackMode=manual` + QoS 1/2 + `cleanSession=true` with an explicit error.

Use `ackMode=manual&cleanSession=false` on the consumer URI when you need to guarantee
at-least-once delivery through downstream processing failure.

## CLI Usage

Enable the MQTT component in `camel-cli` via Cargo features:

```bash
cargo install camel-cli --features mqtt       # plain MQTT
cargo install camel-cli --features mqtt-tls   # MQTT + TLS
```

When enabled, `mqtt:` and `mqtts:` URIs are automatically available through Camel.toml broker config.

## Known Limitations

- **MQTT 3.1.1 only** — MQTT 5.0 support is deferred to a future release.
- **One connection per endpoint** — each Consumer and each Producer opens its own TCP connection
  to the broker. A shared-connection pool (`connectionMode=sharedSession`) is planned for v2.
- **mTLS not supported** — client certificate authentication requires `rumqttc::TlsConfiguration`
  client_auth fields, which are deferred to v2.
- **Broker connection limits** — in v1, the number of concurrent connections is the number of
  route endpoints. Users with connection-limited brokers should account for this.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-mqtt = "0.20"
```

To enable TLS support:

```toml
[dependencies]
camel-component-mqtt = { version = "0.20", features = ["tls"] }
```

See the [rust-camel repository](https://github.com/kennycallado/rust-camel) for source, examples,
and integration tests.
