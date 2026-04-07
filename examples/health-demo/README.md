# health-demo

This demo shows how to enable the standalone health server through `CamelConfig` and `configure_context()` without wiring `HealthServer` manually. It also starts a simple timer route so the context has active runtime work.

## Run

```bash
cargo run -p health-demo
```

## Endpoints

While the demo is running, the following endpoints are available:

- `http://0.0.0.0:8080/healthz`
- `http://0.0.0.0:8080/readyz`
- `http://0.0.0.0:8080/health`

## Camel.toml example

The demo configures health in code, but the equivalent `Camel.toml` section is:

```toml
[observability.health]
enabled = true
host = "0.0.0.0"
port = 8080
```
