# camel-sql

SQL component for rust-camel: query execution, streaming (StreamList), and batch processing against SQL databases via `sqlx`.

## Language

(none yet — add terms specific to this component when they crystallise)

## Log-level policy

Per ADR-0012.

**Bridging:** this component supports `bridgeErrorHandler=true` (URI param). When enabled, poll-loop errors are wrapped as Exchanges and routed through the owning route's error handler — the consumer logs at `warn!` on the bridged path to avoid duplicate `error!` (Apache Camel ErrorHandler pattern).

**Labels wired in Phase B (commits cfb7c74c + 0a801cec):**

All 4 sites are category (b′) outside-contract: a normal-data `send_and_wait` or post-processing call returned Err, meaning the route handler did NOT absorb the failure — the consumer's `error!` is the only ERROR signal. Each site calls `runtime.metrics().increment_errors(route_id, label)` via a shared `record_post_process_failure` helper (`consumer.rs:41-51`), then logs at `error!` with `// log-policy: outside-contract`:
- `b-prime:sql:on-consume` (`consumer.rs:145`) — post-process single row failure.
- `b-prime:sql:on-consume-batch` (`consumer.rs:187`) — post-process batch row failure.
- `b-prime:sql:stream-list` (`consumer.rs:257`) — StreamList downstream send failure.
- `b-prime:sql:poll-failed` (`consumer.rs:356`) — unbridged poll failure.

**Category (g) labels wired in Phase B (commit 78ef8430):**
- `g:sql:producer-pool-init` (`producer.rs:188`) — producer lazy pool init failure. Calls `runtime.health().force_unhealthy_for_route(route_id, label, reason)` + `// log-policy: outside-contract`.
- `g:sql:consumer-pool-init` (`consumer.rs:442`) — consumer pool init giving up after retry budget exhausted. Calls `runtime.health().force_unhealthy_for_route(route_id, label, reason)` + `// log-policy: outside-contract`.

**Migration status (Phase B close):**
- All handler-owned sites (categories a, b-bridged) → `warn!`.
- All system-broken sites (category c) → `// log-policy: system-broken` + `error!`.
- All outside-contract sites (categories b′, e, g) → WIRED in Phase B with real `increment_errors` / `force_unhealthy_for_route` calls + `// log-policy: outside-contract` annotations. See ADR-0012 Phase B closure notes.
- Duplicate logs at `producer.rs:171` and `consumer.rs:160` removed (Phase 2).
- `consumer.rs:429` duplicate-`error!` bug fixed (Phase 2; split into bridged warn + unbridged error with increment_errors).
