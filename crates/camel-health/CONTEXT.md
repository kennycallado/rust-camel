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

## Timeout ordering

`HealthServer::stop()` computes the shutdown timeout at runtime as
`handler_timeout + 2 seconds`. It sends the graceful-shutdown signal (a `oneshot`),
then waits for the spawned task to finish.

- If the task completes normally, `stop()` returns `Ok`.
- If the task panicked, the `JoinError` is logged at `error!` level and `stop()`
  returns `Ok`.
- If the timeout expires, the task is `.abort()`-ed and awaited. This ensures the
  listener socket is released — no detached-drain window.

## `HealthServer` lifecycle architecture

`HealthServer` implements the control-plane `Lifecycle` trait. `start()` binds the listener,
spawns one Tokio task, and stores `Started` in an `AtomicU8`. A `oneshot` sender requests Axum
graceful shutdown. `stop()` sends that request and waits for the task within the shutdown timeout.

camel-config creates and registers `HealthServer` during context setup. camel-prometheus reuses
the health router when it co-hosts health and metrics endpoints.

## ADR-0012 log-policy sites

The sites in this crate are category **(c) system-broken** — health server lifecycle
failures outside any route pipeline. The `error!` level is preserved; each call site
carries a `// log-policy: system-broken` annotation.

| File | Line | Site | Category | Annotation |
|------|------|------|----------|------------|
| `server.rs` | ~121 | `Lifecycle::start` Axum serve error | (c) system-broken | `// log-policy: system-broken` |
| `server.rs` | ~143 | `Lifecycle::stop` JoinError handler | (c) system-broken | `// log-policy: system-broken` |

## Metrics

Metrics instrumentation is not yet wired for the health server.
