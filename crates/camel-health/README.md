# camel-health

Health HTTP endpoints for Kubernetes-style probes, plus a standalone `Lifecycle` server for rust-camel.

## Endpoints

`health_router()` exposes:

| Endpoint | Purpose | Response |
|----------|---------|----------|
| `/healthz` | Liveness probe | `200 OK` (always) |
| `/readyz` | Readiness probe | `200 OK` when healthy, `503 Service Unavailable` when unhealthy |
| `/health` | Full health report | `200 OK` + JSON `HealthReport` |

`/readyz` and `/health` return `HealthReport::default()` when no checker is provided.

## Usage: embed in an existing Axum router

```rust
use axum::Router;
use camel_health::health_router;
use camel_api::HealthChecker;

let checker: Option<HealthChecker> = None;

let app = Router::new()
    .nest("/", health_router(checker));
```

## `HealthServer` (standalone server)

`HealthServer` implements `Lifecycle` and manages its own Axum server:

- `start()` binds the configured address and serves health endpoints
- `stop()` aborts the server task
- `status()` reports `Stopped` / `Started` / `Failed`

This is used when health observability is enabled in `Camel.toml`.

## Camel.toml configuration

```toml
[observability.health]
enabled = true
port = 8081
```

Fields:

- `enabled` (bool): starts the standalone health server when `true`
- `port` (u16): TCP port for the server (default `8081`)

## CLI override

Use `--health-port PORT` to enable health server and override `observability.health.port` from config.
