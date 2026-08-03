## ADDED Requirements

### Requirement: Endpoint index population

The system SHALL maintain a one-to-many index mapping each registered route's source
endpoint URI (`from_uri`) to the set of route_ids consuming from it. The index SHALL be
synchronized across all live-route mutation paths: `add_route`,
`add_route_with_generation`, `insert_prepared_route`, `remove_route`, and
`remove_route_preserving_functions`.

#### Scenario: Add route registers endpoint

- **GIVEN** no route is registered with endpoint URI `timer:tick`
- **WHEN** a route with `from_uri = "timer:tick"` and `route_id = "tick-route"` is added
- **THEN** `routes_for_endpoint("timer:tick")` returns `Ok(vec!["tick-route"])`

#### Scenario: Add route with generation registers endpoint

- **GIVEN** no route is registered with endpoint URI `direct:gen`
- **WHEN** a route is added via `add_route_with_generation` with `from_uri = "direct:gen"`
- **THEN** `routes_for_endpoint("direct:gen")` returns a vector containing that route_id

#### Scenario: Insert prepared route registers endpoint

- **GIVEN** a `PreparedRoute` token whose staged `ManagedRoute` has `from_uri = "seda:staged"`
- **WHEN** `insert_prepared_route(prepared)` is called
- **THEN** `routes_for_endpoint("seda:staged")` returns a vector containing that route_id

#### Scenario: Remove route clears endpoint

- **GIVEN** a route with `from_uri = "timer:tick"` and `route_id = "tick-route"` is registered
- **WHEN** the route is removed via `remove_route`
- **THEN** `routes_for_endpoint("timer:tick")` returns `Ok(vec![])`

#### Scenario: Remove route preserving functions clears endpoint

- **GIVEN** a route with `from_uri = "direct:cleanup"` is registered
- **WHEN** the route is removed via `remove_route_preserving_functions`
- **THEN** `routes_for_endpoint("direct:cleanup")` returns `Ok(vec![])`

#### Scenario: Multiple routes share same source URI

- **GIVEN** two routes with `from_uri = "direct:orders"` and route_ids `a` and `b`
- **WHEN** `routes_for_endpoint("direct:orders")` is called
- **THEN** the result contains both `a` and `b`

#### Scenario: List endpoints returns all source URIs

- **GIVEN** three routes with `from_uri` values `timer:a`, `direct:b`, `seda:c`
- **WHEN** `list_endpoints()` is called
- **THEN** the result contains all three URIs

#### Scenario: List endpoints empty when no routes

- **GIVEN** no routes are registered
- **WHEN** `list_endpoints()` is called
- **THEN** the result is `Ok(Vec::new())`

### Requirement: routes_for_endpoint lookup

The system SHALL provide `routes_for_endpoint(uri) -> Result<Vec<String>, DomainError>`
on `RuntimeExecutionPort` with a trait default returning an empty vector. Given a source
endpoint URI, it returns all route_ids that consume from it.

#### Scenario: Known endpoint returns route_ids

- **GIVEN** a route with `from_uri = "direct:orders"` and `route_id = "order-route"` is started
- **WHEN** `routes_for_endpoint("direct:orders")` is called
- **THEN** the result is `Ok(vec!["order-route"])`

#### Scenario: Unknown endpoint returns empty

- **GIVEN** no route has `from_uri = "direct:nonexistent"`
- **WHEN** `routes_for_endpoint("direct:nonexistent")` is called
- **THEN** the result is `Ok(vec![])`

#### Scenario: Trait default returns empty for external implementations

- **GIVEN** a custom `RuntimeExecutionPort` implementation that does not override the method
- **WHEN** `routes_for_endpoint("any:uri")` is called
- **THEN** the result is `Ok(Vec::new())` (source-compatible default)

### Requirement: health_check_endpoint

The system SHALL provide `health_check_endpoint(uri) -> Result<HealthStatus, DomainError>`
on `RuntimeExecutionPort` with a trait default returning an endpoint-not-found error. It
resolves all owning routes from the endpoint index and returns the worst HealthStatus
across them.

#### Scenario: Healthy endpoint with single route

- **GIVEN** a route with `from_uri = "timer:tick"` is registered and its probes are healthy
- **WHEN** `health_check_endpoint("timer:tick")` is called
- **THEN** the result is `Ok(HealthStatus::Healthy)`

#### Scenario: Endpoint not in index

- **GIVEN** no route has `from_uri = "direct:unknown"`
- **WHEN** `health_check_endpoint("direct:unknown")` is called
- **THEN** the result is `Err(DomainError)` indicating the endpoint was not found

#### Scenario: Unhealthy endpoint reflects route health

- **GIVEN** a route with `from_uri = "timer:tick"` is registered and its health was
  forced unhealthy via `force_unhealthy_for_route`
- **WHEN** `health_check_endpoint("timer:tick")` is called
- **THEN** the result is `Ok(HealthStatus::Unhealthy)`

#### Scenario: Worst status across multiple routes

- **GIVEN** two routes sharing `from_uri = "direct:shared"`, one healthy and one unhealthy
- **WHEN** `health_check_endpoint("direct:shared")` is called
- **THEN** the result is `Ok(HealthStatus::Unhealthy)` (worst wins)

#### Scenario: Route with no probes returns healthy

- **GIVEN** a route with `from_uri = "direct:noprobes"` is registered but has no health probes
- **WHEN** `health_check_endpoint("direct:noprobes")` is called
- **THEN** the result is `Ok(HealthStatus::Healthy)` (no issues detected)

#### Scenario: Stopped route returns healthy

- **GIVEN** a route with `from_uri = "timer:stopped"` is registered but stopped (inactive)
- **WHEN** `health_check_endpoint("timer:stopped")` is called
- **THEN** the result is `Ok(HealthStatus::Healthy)` (not in failure state)

#### Scenario: Trait default returns error for external implementations

- **GIVEN** a custom `RuntimeExecutionPort` implementation that does not override the method
- **WHEN** `health_check_endpoint("any:uri")` is called
- **THEN** the result is `Err(DomainError)` (source-compatible default)

### Requirement: Per-route health check method

The system SHALL add `async fn check_route(&self, route_id: &str) -> HealthStatus` to
`HealthCheckRegistry` that returns the health status for a single route. It preserves the
same semantics as `check_all` but scoped to one route: forced-unhealthy state takes
precedence, then probe results are aggregated via worst-status (Unhealthy > Degraded >
Healthy) with the same timeout/panic handling as `check_all`.

#### Scenario: Route with healthy probes

- **GIVEN** a route `r1` with probes that all return healthy
- **WHEN** `check_route("r1").await` is called
- **THEN** the result is `HealthStatus::Healthy`

#### Scenario: Route with unhealthy probe

- **GIVEN** a route `r1` with one probe returning unhealthy
- **WHEN** `check_route("r1").await` is called
- **THEN** the result is `HealthStatus::Unhealthy`

#### Scenario: Route with no probes returns healthy

- **GIVEN** a route `r1` is registered with no health probes
- **WHEN** `check_route("r1").await` is called
- **THEN** the result is `HealthStatus::Healthy`

#### Scenario: Forced unhealthy with no probes returns unhealthy

- **GIVEN** a route `r1` has no probes but its health was forced unhealthy via
  `force_unhealthy_for_route`
- **WHEN** `check_route("r1").await` is called
- **THEN** the result is `HealthStatus::Unhealthy`

#### Scenario: Unknown route returns healthy

- **GIVEN** no route with id `unknown` is registered in the health registry
- **WHEN** `check_route("unknown").await` is called
- **THEN** the result is `HealthStatus::Healthy` (no negative signal)
