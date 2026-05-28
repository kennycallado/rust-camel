# health-demo

This demo shows how to enable the standalone health server through `CamelConfig` and `configure_context()` without wiring `HealthServer` manually. It also starts a simple timer route so the context has active runtime work.

## Run

```bash
cargo run -p health-demo
```

## Endpoints

While the demo is running, the following endpoints are available:

- `http://0.0.0.0:8080/healthz` -- liveness probe
- `http://0.0.0.0:8080/readyz` -- readiness probe
- `http://0.0.0.0:8080/health` -- full health report (includes component checks)

## Component health checks

When components are registered with the `CamelContext`, they automatically contribute health checks to the `/health` endpoint. Each component implements `HealthChecker` and probes a specific aspect of its backend connectivity or internal state.

| Component | What it probes |
|-----------|---------------|
| **redis** | PING command against the Redis server |
| **sql** | Connection validation via the connection pool |
| **kafka** | Topic metadata request to the Kafka broker |
| **http** | TCP listener / connectivity check |
| **grpc** | TCP connectivity to the gRPC server |
| **ws** | TCP listener status |
| **file** | Directory metadata (existence and permissions) |
| **opensearch** | Cluster health API call |
| **container** | Docker ping |
| **wasm** | Engine state via `AtomicBool` |
| **jms** | Bridge gRPC probe |
| **cxf** | Bridge gRPC probe |
| **xslt** | Bridge gRPC probe |
| **xj** | Bridge gRPC probe |

Only components that are actually registered and started appear in the health report. Unregistered or stopped components are not probed.

## Camel.toml example

The demo configures health in code, but the equivalent `Camel.toml` section is:

```toml
[observability.health]
enabled = true
host = "0.0.0.0"
port = 8080
```
