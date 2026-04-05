# camel-component-jms

JMS component for [rust-camel](https://github.com/kennycallado/rust-camel). Supports **ActiveMQ Classic** and **ActiveMQ Artemis**.

## URI Format

```
jms:queue:name[?param=value&...]
jms:topic:name[?param=value&...]
```

## Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `brokerUrl` | `tcp://localhost:61616` | Broker connection URL |
| `brokerType` | `activemq` | `activemq` or `artemis` |
| `username` | — | Broker username |
| `password` | — | Broker password |

## Quick Start

```rust
use camel_component_jms::JmsComponent;

ctx.register_component(JmsComponent::default());

// Consumer
let route = RouteBuilder::from("jms:queue:orders?brokerUrl=tcp://localhost:61616")
    .to("log:info")
    .build()?;

// Producer
let route = RouteBuilder::from("timer:tick?period=1000")
    .set_body("hello".to_string())
    .to("jms:queue:events?brokerUrl=tcp://localhost:61616")
    .build()?;
```

## Artemis

```rust
.to("jms:queue:orders?brokerUrl=tcp://artemis:61616&brokerType=artemis")
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
