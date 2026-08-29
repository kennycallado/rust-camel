# camel-master

> Leader-only route execution component for rust-camel

## Overview

The Master component ensures that only the current leader consumes from a route while delegating endpoint behavior to any wrapped component URI.

It wraps a delegate URI using `master:<lockname>:<delegate-uri>` and uses the configured leader elector to start or stop the delegate consumer based on leadership.

## Features

- Consumer-only leadership gating
- Leader election driven startup and shutdown
- Producer passthrough to delegate endpoints
- Graceful lifecycle management with drain timeout

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-master = "*"
```

## URI Format

```
master:<lockname>:<delegate-uri>
```

## URI Options

The master component has no URI query parameters of its own. Any query parameters belong to the delegate URI.

| Option | Default | Description |
|--------|---------|-------------|
| delegate URI options | delegate-defined | All options after `<delegate-uri>` are parsed by the delegate component |

## Bundle Config

`camel-master` supports bundle configuration in `Camel.toml` under `[components.master]`.

| Section | Key | Default | Description |
|--------|-----|---------|-------------|
| `[components.master]` | `drain_timeout_ms` | `5000` | Max time to wait for delegate consumer shutdown on leadership loss |
| `[components.master]` | `delegate_retry_max_attempts` | `0` | Max attempts to start the delegate consumer while leading. `0` = unlimited |

## Usage

### Basic Leader-Only Timer

```rust
from("master:orders:timer:tick?period=5000")
```

Only the active leader processes timer ticks for the `orders` lock.

### With Kafka

```rust
from("master:invoices:kafka:my-topic")
```

Only the route instance that holds leadership consumes from the delegated Kafka endpoint.

### Producer Passthrough

```rust
to("master:mylock:http://api.example.com")
```

Producer calls are passed through to the delegate producer and are not gated by leadership.

## How It Works

When a master consumer starts, it joins leader election with the configured lock name. The delegate consumer is not started until leadership is acquired. On leadership loss, the delegate consumer is stopped and drained up to `drain_timeout_ms`.

The drain runs after leadership is already lost, so it can overlap a successor's lease. Split-brain safety does not come from drain-before-takeover ordering. The `x-camel-leader-epoch` fencing token stamped on every envelope enables split-brain safety when a sink opts into rejecting stale epochs; enforcement is opt-in per sink (ADR-0035).

If leadership is gained again, the delegate consumer is started again with the same delegate URI.

### Lock sharing

Two `master:X:` routes in one process share lock X. On Kubernetes, they share one cached elector: exclusion is per-process — when this process leads, both routes' delegates run; only one process holds the lease. The default Noop platform reports every route as leader and provides no cross-route or cross-process exclusion.

## Backends

- `NoopPlatformService` (default): single-node behavior, always leader
- `KubernetesPlatformService`: distributed leader election backed by Kubernetes Leases, configured via `[platform]` in `Camel.toml`

## Example Camel.toml

```toml
[components.master]
drain_timeout_ms = 5000

[platform]
type = "kubernetes"
namespace = "default"
```

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
