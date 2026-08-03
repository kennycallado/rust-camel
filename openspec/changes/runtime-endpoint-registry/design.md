# Design: runtime-endpoint-registry

## Approach

Introduce an `EndpointIndex` — a `HashMap<String, Vec<String>>` mapping each source endpoint
URI to the list of route_ids that consume from it — held by `DefaultRouteController`
alongside the existing `routes` and `prepared_staging` fields. This is a one-to-many
mapping because multiple routes may declare the same `from_uri` (common with `direct:` and
`seda:` endpoints).

### Index synchronization

The index is updated across all live-route mutation paths in `DefaultRouteController`:

| Method | Index action |
|--------|-------------|
| `add_route(definition)` | Insert `from_uri → route_id` |
| `add_route_with_generation(...)` | Insert `from_uri → route_id` |
| `insert_prepared_route(prepared: PreparedRoute)` | Retrieve `ManagedRoute` from staging, insert `managed.from_uri → route_id` |
| `remove_route(route_id)` | Remove `route_id` from all URI entries |
| `remove_route_preserving_functions(route_id)` | Remove `route_id` from all URI entries |

Insertions are idempotent (dedup by route_id within each URI's vector). Removals compact
the vector and drop the key if the vector becomes empty.

### New trait methods

Three methods on `RuntimeExecutionPort` (`runtime_ports.rs`):

```rust
/// Return all route_ids that consume from the given source endpoint URI.
/// Default: empty vec (source-compatible for external implementations).
async fn routes_for_endpoint(&self, _uri: &str) -> Result<Vec<String>, DomainError> {
    Ok(Vec::new())
}

/// Return the worst HealthStatus across all routes owning the endpoint.
/// Default: endpoint-not-found error (source-compatible).
async fn health_check_endpoint(&self, _uri: &str) -> Result<HealthStatus, DomainError> {
    Err(DomainError::InvalidState("endpoint not found".into()))
}

/// list_endpoints already has a default returning empty vec; adapters override.
```

### Per-route health lookup

`HealthCheckRegistry` currently exposes only `check_all()`. Add an async
`check_route(route_id) -> HealthStatus` method that returns the health status for a single
route. It preserves the same semantics as `check_all` but scoped to one route:
- Forced-unhealthy state (`force_unhealthy_for_route`) takes precedence — returns `Unhealthy`
  even if no probes are registered or probes are healthy.
- If the route is inactive (stopped/suspended) → `Healthy` (not in failure state).
- Otherwise → worst status across the route's registered async probes (same timeout/panic
  handling and worst-status aggregation as `check_all`, but scoped to one route).
- If the route has no probes registered and no forced state → `Healthy` (no issues detected).
- Unknown route_id → `Healthy` (no negative signal).

`health_check_endpoint(uri)` resolves all route_ids from the index, calls `check_route`
for each, and returns the worst status (Unhealthy > Degraded > Healthy). If the URI is not
in the index, returns `Err(DomainError::InvalidState("endpoint not found"))`.

### Actor wiring

Three new message variants in the controller actor (`controller_actor.rs`):
`ListEndpoints`, `RoutesForEndpoint`, `HealthCheckEndpoint`. Each is forwarded by
`RouteControllerHandle` and handled by `DefaultRouteController`. `RuntimeExecutionAdapter`
delegates as it does for `in_flight_count`.

## Affected crates

- **camel-core**: `EndpointIndex` struct in `lifecycle/adapters/`; trait methods + defaults
  on `RuntimeExecutionPort` (`runtime_ports.rs`); adapter impl in `runtime_execution.rs`;
  actor messages in `controller_actor.rs`; population hooks in `route_controller.rs` and
  `route_staging.rs`; `check_route` method on `HealthCheckRegistry` (`health_registry.rs`).
- **camel-api**: no changes (HealthStatus, DomainError already exist).

## Architecture boundaries

The endpoint index is **operational execution-state** — a read-side projection of the
controller's current route set, maintained as routes are added and removed. It lives in
**Interface Adapters** alongside `RouteRegistry`, `prepared_staging`, and
`HealthCheckRegistry` state. These are not CQRS commands (which mutate the domain); they
are read-only queries against the live adapter state, the same category as
`in_flight_count` and `route_from_uri` which already exist without charter exception.

References: ADR-0045 §4 (adapter-state classification — operational reads are in-scope for
Interface Adapters), ADR-0002 (RuntimeBus), CONTEXT-MAP.md "Key Terms"
(RuntimeExecutionPort, HealthCheckRegistry).

## Alternatives considered

- **Index in RouteRegistry**: rejected — RouteRegistry is route-centric; coupling endpoint
  lookup there mixes two concerns. A separate index follows the single-responsibility
  precedent of `HealthCheckRegistry`.
- **Walk routes on every query**: rejected — O(n) per lookup vs O(1). Routes can number in
  the hundreds; lookup latency matters for management API responsiveness.
- **One-to-one URI → route_id mapping**: rejected — multiple routes may share the same
  source URI (`direct:`, `seda:`). A one-to-one `HashMap` silently overwrites valid
  registrations; a one-to-many `Vec` preserves all owners.
- **Track producer endpoints**: rejected for initial cut — producer endpoints lack
  Consumer lifecycle ownership, their URIs may be dynamically resolved at runtime (Simple
  language expressions, Dynamic Router, Routing Slip), and they have no health state
  distinct from the route pipeline that invokes them. Can be added later if demand emerges.
