# Proposal: runtime-endpoint-registry

## Why

The `RuntimeExecutionPort` trait (camel-core, ADR-0045 charter) has a `TODO(CORE-003)`:
`list_endpoints` returns an empty default, and two methods (`health_check_endpoint`,
`route_for_endpoint`) are absent entirely. camel-core tracks routes as first-class
citizens but not endpoints — there is no URI-to-route mapping for introspection or
management API completeness.

This blocks any management/observability tool that needs to answer "which route owns
`timer:tick`?" or "is `direct:orders` healthy?" without iterating all routes.

Discovered during the ADR-0045 §4 charter purge audit (commit `1a944712`, bd rc-56vk).
Not blocking 1.0 compliance — filed as tracked post-1.0 API completeness work.

## What Changes

**Included:**
- Add `routes_for_endpoint(uri) -> Result<Vec<String>, DomainError>` and
  `health_check_endpoint(uri) -> Result<HealthStatus, DomainError>` to `RuntimeExecutionPort`,
  each with trait default implementations (empty vec / endpoint-not-found error) so external
  implementations remain source-compatible.
- Override the `list_endpoints` default in `RuntimeExecutionAdapter` to return actual URIs.
- Build an endpoint index mapping source endpoint URI to the set of route_ids that consume
  from it (one-to-many: multiple routes may share a `direct:` or `seda:` source URI).
  Populated across all live-route mutation paths: `add_route`,
  `add_route_with_generation`, `insert_prepared_route`, `remove_route`, and
  `remove_route_preserving_functions`.
- `health_check_endpoint` resolves all owning routes and returns the worst HealthStatus
  across them by delegating to a new `check_route(route_id)` method on the existing
  `HealthCheckRegistry`.
- Remove the stale `TODO(CORE-003)` comment.

**Excluded:**
- Producer/destination endpoint tracking (To-step URIs). Producer endpoints lack Consumer
  lifecycle ownership — their URIs may be dynamic or conditionally resolved at runtime, and
  they have no independent health state distinct from the route pipeline that invokes them.

## Acceptance criteria

- `list_endpoints()` returns all registered source endpoint URIs.
- `routes_for_endpoint("direct:orders")` returns all route_ids consuming from it.
- `health_check_endpoint("timer:tick")` returns the worst HealthStatus across owning routes.
- Removing a route removes its endpoint from the index (all mutation paths covered).
- All existing camel-core tests pass; new tests cover each scenario above.
- External `RuntimeExecutionPort` implementations compile unchanged (trait defaults).

## Risk budget

Low risk — additive trait methods with default impls (non-breaking). No charter boundary
violations (endpoint index is operational execution-state read alongside existing
controller state, not a CQRS command). Bd: rc-56vk.
