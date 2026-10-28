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
| `broker` | `default_broker` from config | Named broker to use (must exist in the broker map) |
| `messageSelector` | — | Consumer only. Applied at the broker: non-matching messages are withheld before delivery. The expression uses provider-specific SQL-like JMS selector syntax and is not parsed or validated by rust-camel beyond requiring a non-blank value (empty `messageSelector=`/`message_selector=` and an absent option both mean no selector; whitespace-only is a URI validation error). Requires jms bridge >= `camel_jms::BRIDGE_VERSION` advertising `message-selector-v1`; with an older bridge the slot parks at startup with `bridge <version> lacks message-selector-v1; use a jms bridge >= camel_jms::BRIDGE_VERSION`. JVM and Rust unit tests prove plumbing only — selector semantics are verified by the CI-owned live-broker suite. |
| `jmsHeaderAllowlist` | empty | Producer only. Comma-separated exchange header names to forward as JMS properties. With the empty default no user header is forwarded. Credential, reserved `JMS*`, `jms`, and `camel` names are always denied. Use valid JMS property names (for example `orderId`, not `order-id`). |

> `brokerUrl`, `username`, and `password` are **not** URI parameters — credentials live in `Camel.toml` or programmatic config. `broker_type` is inferred from the scheme (`activemq:` → ActiveMQ Classic, `artemis:` → Artemis) or from the broker config entry.

## Overview

JMS component for rust-camel via Java bridge.

## Camel.toml Configuration

```toml
[default.components.jms]
default_broker = "main"      # which broker to use when no ?broker= param is given

[default.components.jms.brokers.main]
broker_url  = "tcp://localhost:61616"
broker_type = "activemq"     # "activemq" | "artemis"
username    = "admin"        # optional
password    = "admin"        # optional
```

Multiple brokers are supported — add as many `[default.components.jms.brokers.<name>]` entries as needed.

### Pool health settings

These settings apply to the whole pool. They live in `[default.components.jms]`.

| Setting | Default | Values | Description |
|---------|---------|--------|-------------|
| `health_check_interval_ms` | `5000` | positive integer | Delay between liveness probes. The default and meaning are unchanged. |
| `health_timeout_ms` | `3000` | positive integer | Deadline for one liveness probe. An expired probe counts as a failure. |
| `health_failure_threshold` | `3` | positive integer | Number of consecutive failed probes that restarts a bridge. |
| `health_broker_probe_mode` | `reuse` | `reuse` or `skip` | Broker probe strategy. `reuse` keeps one provider diagnostic connection. `skip` performs no broker I/O. |

Only consecutive failed liveness probes restart a bridge. A failed probe is an RPC error or an expired `health_timeout_ms`. A successful RPC resets the failure count, even when the response reports a broker problem. Broker status fields never cause a restart. With the default settings, the worst-case dead-process detection is about 24 s: `3 x (5 s + 3 s)`.

One channel refresh has a fixed 2 s budget. The budget covers process-lock acquisition and the selected connector future. Progress does not renew the budget. After a failed or expired refresh, the caller requests a liveness probe within 500 ms. It never restarts the process. Slow broker work alone cannot restart a live bridge.

The broker-latency isolation guarantee requires the updated Java health handler. No protobuf change is needed. An older blocking bridge cannot provide the guarantee.

## Quick Start

```rust
use camel_component_jms::{BrokerType, JmsBridgePool, JmsComponent, JmsPoolConfig};
use std::sync::Arc;

// Option A: single broker (programmatic)
let pool_config = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
let pool = Arc::new(JmsBridgePool::from_config(pool_config).unwrap());

ctx.register_component(JmsComponent::with_scheme("jms", Arc::clone(&pool)));
ctx.register_component(JmsComponent::with_scheme("activemq", Arc::clone(&pool)));
ctx.register_component(JmsComponent::with_scheme("artemis", Arc::clone(&pool)));

// Option B: config from Camel.toml (via camel-config crate)
// JmsPoolConfig is built automatically by configure_context()
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

// Producer — explicit broker selection (multi-broker setup)
let route = RouteBuilder::from("timer:tick?period=1000")
    .set_body("hello".to_string())
    .to("activemq:queue:events?broker=secondary")
    .build()?;

// Producer — Artemis topic
let route = RouteBuilder::from("timer:tick?period=1000")
    .set_body("hello".to_string())
    .to("artemis:topic:notifications")
    .build()?;
```

## How It Works

The component manages a Java bridge process (GraalVM native executable) internally:

1. On first use, downloads the bridge binary from GitHub releases (SHA256-verified, cached at `~/.cache/rust-camel/jms-bridge/`).
2. Spawns the bridge with broker config passed via environment variables.
3. Reads the ephemeral gRPC port from stdout (`{"status":"ready","port":N}`).
4. Polls the health endpoint until ready, then routes messages over gRPC.

No Java runtime is required on the host — the bridge is a native binary.

### Bridge Health Monitor

Each bridge runs a background health monitor that:
- Probes bridge health every `health_check_interval_ms` (default: 5 s) with a `health_timeout_ms` deadline (default: 3 s)
- Restarts the bridge process only after `health_failure_threshold` consecutive failed probes (default: 3), with exponential backoff (5 s → 10 s → 20 s → ... up to 120 s)
- Resets the failure count on any completed health RPC, even when the response reports a broker problem
- Caps restart attempts at **10** before transitioning to a permanent `Degraded` state

### Graceful Shutdown

When using the bridge pool programmatically, call `begin_shutdown()` **before** `ctx.stop()` to prevent the health monitor from restarting the bridge during shutdown:

```rust
pool.begin_shutdown();          // Signal health monitors to stop restarting
ctx.stop().await?;             // Stop routes and services
pool.shutdown().await?;        // Clean up bridge processes
```

When using `camel-cli`, Ctrl+C handles shutdown automatically. A second Ctrl+C force-exits if shutdown hangs.

### Consumer log lines

The bridge sends no response and no initial metadata at attach. Because `Subscribe` is server-streaming, its response headers arrive with the first `onNext`, so the Rust subscribe future resolves at first delivery — the broker's consumer count rises first.

- `JMS consumer attach requested; awaiting bridge response` — the request was sent, before the RPC is awaited. Every retry logs it again.
- `JMS consumer attached; first delivery received` — the subscribe future resolved, which means the first message was delivered.

There is no separate attach acknowledgement in the protocol.

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
- Queue messages are acknowledged after the route completes. An `Ok` result acknowledges; an unhandled `Err` rolls back and the broker redelivers. After 6 delivery attempts (`JMS_MAX_DELIVERY_ATTEMPTS`) the bridge acknowledges and discards the message, logging only its ID — no dead-letter forwarding. A broker-native redelivery limit that fires first takes precedence. Attempt counts persist under `$XDG_STATE_HOME/rust-camel/jms-bridge` (else `$HOME/.local/state/rust-camel/jms-bridge`); see `docs/src/components/jms.md` in the repository. The broker acknowledgement is issued synchronously (ActiveMQ Classic `sendAcksAsync(false)`, Artemis `blockOnAcknowledge(true)`), so a crash immediately after the broker commits cannot redeliver a message whose attempt budget was already cleared.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-jms = "*"
```
