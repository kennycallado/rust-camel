# camel-component-jms

JMS component for [rust-camel](https://github.com/kennycallado/rust-camel). Supports **ActiveMQ Classic** and **ActiveMQ Artemis**.

## URI Format

Three URI schemes are supported:

```
# Generic — requires explicit destination type
jms:queue:name[?param=value&...]
jms:topic:name[?param=value&...]

# Broker-specific — destination type can be omitted (defaults to queue)
activemq:queue:name[?param=value&...]
activemq:name              # shorthand → queue
activemq:topic:name

artemis:queue:name[?param=value&...]
artemis:name               # shorthand → queue
artemis:topic:name
```

Using `activemq:` or `artemis:` as scheme locks the `broker_type` automatically — no `brokerType` query param needed.

## URI Query Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `brokerUrl` | `tcp://localhost:61616` | Broker connection URL (overrides `Camel.toml`) |
| `username` | — | Broker username (overrides `Camel.toml`) |
| `password` | — | Broker password (overrides `Camel.toml`) |

> `brokerType` is **not** a URI parameter — it is inferred from the scheme (`activemq:` → ActiveMQ Classic, `artemis:` → Artemis) or set in `Camel.toml`.

## Camel.toml Configuration

```toml
[default.components.jms]
broker_url  = "tcp://localhost:61616"
broker_type = "activemq"   # "activemq" | "artemis" — ignored when using activemq:/artemis: schemes
username    = "admin"
password    = "admin"
```

## Quick Start

```rust
use camel_component_jms::JmsComponent;

// Option A: broker config from Camel.toml
ctx.register_component(JmsComponent::default());

// Option B: programmatic config
use camel_component_jms::{BrokerType, JmsConfig};
let config = JmsConfig {
    broker_url: "tcp://localhost:61616".to_string(),
    broker_type: BrokerType::ActiveMq,
    ..Default::default()
};
ctx.register_component(JmsComponent::new(config));

// Option C: broker-specific scheme (locks broker type automatically)
ctx.register_component(JmsComponent::with_scheme("activemq"));
```

```rust
// Consumer — generic scheme
let route = RouteBuilder::from("jms:queue:orders")
    .to("log:info")
    .build()?;

// Consumer — activemq: shorthand (queue is the default)
let route = RouteBuilder::from("activemq:orders")
    .to("log:info")
    .build()?;

// Producer — override broker URL inline
let route = RouteBuilder::from("timer:tick?period=1000")
    .set_body("hello".to_string())
    .to("activemq:queue:events?brokerUrl=tcp://192.168.1.1:61616")
    .build()?;

// Producer — Artemis topic
let route = RouteBuilder::from("timer:tick?period=1000")
    .set_body("hello".to_string())
    .to("artemis:topic:notifications")
    .build()?;
```

## How It Works

The component manages a Java bridge process (jlink native binary) internally:

1. On first use, downloads the bridge binary from GitHub releases (SHA256-verified, cached at `~/.cache/rust-camel/jms-bridge/`).
2. Spawns the bridge with broker config passed via environment variables.
3. Reads the ephemeral gRPC port from stdout (`{"status":"ready","port":N}`).
4. Polls the health endpoint until ready, then routes messages over gRPC.

No Java runtime is required on the host — the bridge is a native binary.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `CAMEL_JMS_BRIDGE_BINARY_PATH` | **Development override** — absolute path to a local `jms-bridge` binary; skips download entirely |
| `CAMEL_JMS_BRIDGE_RELEASE_URL` | Override release download URL (must be `https://github.com/**`) |

## Development Setup

### Prerequisites

- Docker (daemon running)
- **NixOS:** `programs.nix-ld.enable = true` must be set in your NixOS configuration
- **Linux:** glibc ≥ 2.34 (Ubuntu 22.04+, Debian 12+, RHEL 9+)

### Building the JMS Bridge

The bridge binary is built using Docker (no Java required on your host):

```bash
# One-time build (~5–8 min first time, faster on subsequent runs)
cargo xtask build-jms-bridge

# The binary is auto-detected. No env vars needed:
cargo run -p jms-example
cargo test -p camel-test --features integration-tests --test jms_test
```

### Override

If you need to use a custom binary:

```bash
export CAMEL_JMS_BRIDGE_BINARY_PATH=/path/to/your/jms-bridge
cargo run -p jms-example
```

## Known Limitations

- IBM MQ not supported (planned for a future release)
- No durable topic subscribers yet
