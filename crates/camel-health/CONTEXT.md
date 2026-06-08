# Health

Health endpoint server for Camels. Exposes `/health`, `/healthz`, `/readyz`, and `/startupz`
HTTP endpoints.

## ADR-0012 log-policy sites

The single site in this crate is category **(c) system-broken** — a health server lifecycle failure
that occurs outside any route pipeline. The `error!` level is preserved; the call site carries a
`// log-policy: system-broken` annotation.

| File | Line | Category | Annotation |
|------|------|----------|------------|
| `server.rs` | 113 | (c) system-broken | `// log-policy: system-broken` — health server axum error |

## Metrics

Metrics instrumentation is not yet wired for the health server.
