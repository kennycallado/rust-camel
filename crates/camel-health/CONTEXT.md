# Health

Health endpoint server for Camels. Exposes `/health`, `/healthz`, `/readyz`, and `/startupz`
HTTP endpoints.

## Health probe contract

Each handler wraps its `HealthSource` call in `tokio::time::timeout`. A timeout fails closed.
`/healthz`, `/readyz`, and `/startupz` return HTTP 503 with `Unhealthy` status. `Healthy` and
`Degraded` return HTTP 200 on these endpoints.

`/health` always returns HTTP 200 because it is a diagnostic report endpoint. A timeout still
produces an `Unhealthy` report with a failed `probe-timeout` service entry. The default handler
timeout is `DEFAULT_HANDLER_TIMEOUT` (6 seconds), and callers can set a different positive value.

## Timeout ordering and detached-drain window

`DEFAULT_HANDLER_TIMEOUT` is 6 seconds. `HealthServer::stop` waits at most
`SHUTDOWN_TIMEOUT` (5 seconds) for Axum graceful shutdown. An active default-timeout probe can
therefore outlive the shutdown wait by up to one second.

When the 5-second wait expires, dropping the `JoinHandle` detaches the server task. `stop()` then
sets the lifecycle status to `Stopped`, but the task can still drain and keep the port bound. This
is a known gap, not an intentional lifecycle guarantee. Issue `rc-7wus` tracks the fix.

## `HealthServer` lifecycle architecture

`HealthServer` implements the control-plane `Lifecycle` trait. `start()` binds the listener,
spawns one Tokio task, and stores `Started` in an `AtomicU8`. A `oneshot` sender requests Axum
graceful shutdown. `stop()` sends that request and waits for the task within the shutdown timeout.

camel-config creates and registers `HealthServer` during context setup. camel-prometheus reuses
the health router when it co-hosts health and metrics endpoints.

## ADR-0012 log-policy sites

The single site in this crate is category **(c) system-broken** — a health server lifecycle failure
that occurs outside any route pipeline. The `error!` level is preserved; the call site carries a
`// log-policy: system-broken` annotation.

| File | Line | Site | Category | Annotation |
|------|------|------|----------|------------|
| `server.rs` | 123 | `Lifecycle::start` Axum serve error | (c) system-broken | `// log-policy: system-broken` |

## Metrics

Metrics instrumentation is not yet wired for the health server.
