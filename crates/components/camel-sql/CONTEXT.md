# camel-sql

SQL component for rust-camel: query execution, streaming (StreamList), and batch processing against SQL databases via `sqlx`.

## Language

(none yet — add terms specific to this component when they crystallise)

## Log-level policy

Per ADR-0012.

**Bridging:** this component supports `bridgeErrorHandler=true` (URI param). When enabled, poll-loop errors are wrapped as Exchanges and routed through the owning route's error handler — the consumer logs at `warn!` on the bridged path to avoid duplicate `error!` (Apache Camel ErrorHandler pattern).

**Labels planned for category (b′) side-effect failures** (wiring deferred to `bd rc-mf3` — Endpoint trait change to expose `ComponentContext::metrics()` to consumers/producers):
- `b-prime:sql:on-consume` — post-process single row failure (`consumer.rs:103`).
- `b-prime:sql:on-consume-batch` — post-process batch row failure (`consumer.rs:139`).
- `b-prime:sql:poll-failed` — unbridged poll failure (`consumer.rs:429` unbridged branch).
- `b-prime:sql:stream-list` — StreamList downstream send failure (`consumer.rs:205`).

**Note on consumer.rs:205 (StreamList):** This is a **normal-data `send_and_wait` → Err** site, NOT a deliberate error-handoff (bridge) site. Per ADR-0012 "b-bridged discriminator", `send_and_wait → Err` means the route handler did NOT absorb the failure — the consumer's `error!` is the only ERROR signal for the unhandled failure. The site keeps `error!` with `// log-policy: outside-contract` + `// TODO(ADR-0012-e-metrics)` marker. Once `bd rc-mf3` lands, the metric call will be added and the site may optionally downgrade to `warn!` if the metric proves operationally sufficient. Protected by regression test `unbridged_send_and_wait_failure_emits_error_loud`.

**Category (g) — Producer/Endpoint creation failure** (wiring deferred to `bd rc-1mo` — ComponentContext extension for `force_unhealthy_for_route`):
- `producer.rs:162` (lazy pool init) uses inline `// allow-log-levels` escape + `// TODO(ADR-0012-g)` marker. Once rc-1mo lands, replace with `force_unhealthy_for_route(route_id, "endpoint-creation", reason)` + `// log-policy: outside-contract` annotation.

**Migration status (Phase 2 close):**
- All handler-owned sites (categories a, b-bridged) → `warn!`.
- All system-broken sites (category c) → `// log-policy: system-broken` + `error!`.
- All outside-contract sites (categories b′, e, g) → deferred to `bd rc-mf3` / `bd rc-1mo` with inline markers.
- Duplicate logs at `producer.rs:171` and `consumer.rs:160` removed.
- `consumer.rs:429` duplicate-`error!` bug fixed (split into bridged warn + deferred unbridged error).
