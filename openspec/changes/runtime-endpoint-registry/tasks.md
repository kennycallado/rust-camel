# Tasks: runtime-endpoint-registry

## camel-core

### Task 1: EndpointIndex data structure

**Files:**
- `crates/camel-core/src/lifecycle/adapters/endpoint_index.rs` (new)
- `crates/camel-core/src/lifecycle/adapters/mod.rs` (modified — add `pub(crate) mod endpoint_index;`)

**Steps:**
1. Create `EndpointIndex` struct wrapping `HashMap<String, Vec<String>>` (endpoint URI to route_ids).
2. Implement `pub fn insert(&mut self, uri: &str, route_id: &str)` — idempotent: if route_id already in the vector for this URI, no-op; otherwise push.
3. Implement `pub fn remove(&mut self, route_id: &str)` — removes route_id from every URI's vector; drops URI key if vector becomes empty.
4. Implement `pub fn routes_for(&self, uri: &str) -> Vec<String>` — returns a clone of the vector for the URI, or empty vec if not present.
5. Implement `pub fn list_uris(&self) -> Vec<String>` — returns all URI keys.
6. Implement `pub fn is_empty(&self) -> bool`.

**Tests:**
- `endpoint_index_insert_and_lookup`: insert `("timer:tick", "r1")` → `routes_for("timer:tick")` returns `vec!["r1"]`
- `endpoint_index_multiple_routes_same_uri`: insert `("direct:x", "a")` + `("direct:x", "b")` → `routes_for("direct:x")` returns `vec!["a", "b"]`
- `endpoint_index_insert_idempotent`: insert `("timer:tick", "r1")` twice → `routes_for("timer:tick")` returns `vec!["r1"]` (no dup)
- `endpoint_index_remove_route`: insert `("timer:tick", "r1")` → `remove("r1")` → `routes_for("timer:tick")` returns `vec![]`
- `endpoint_index_remove_drops_empty_uri`: insert `("timer:tick", "r1")` → `remove("r1")` → `list_uris()` does not contain `"timer:tick"`
- `endpoint_index_remove_preserves_other_routes`: insert `("direct:x", "a")` + `("direct:x", "b")` → `remove("a")` → `routes_for("direct:x")` returns `vec!["b"]`

**Acceptance:**
- `cargo test -p camel-core --lib endpoint_index` passes all 6 tests
- `cargo clippy -p camel-core -- -D warnings` clean

- [x] 1

### Task 2: RuntimeExecutionPort trait methods + defaults

**Files:**
- `crates/camel-core/src/lifecycle/application/ports/runtime_ports.rs` (modified)

**Steps:**
1. Add `use camel_api::HealthStatus;` to imports.
2. Add default method `async fn routes_for_endpoint(&self, _uri: &str) -> Result<Vec<String>, DomainError>` returning `Ok(Vec::new())`.
3. Add default method `async fn health_check_endpoint(&self, _uri: &str) -> Result<HealthStatus, DomainError>` returning `Err(DomainError::InvalidState("endpoint not found".into()))`.
4. Delete the `TODO(CORE-003, bd rc-56vk)` comment block (lines ~127-131).
5. Keep the existing `list_endpoints` default unchanged.

**Tests:**
- `runtime_port_default_routes_for_endpoint_returns_empty`: a dummy `RuntimeExecutionPort` impl that does not override → `routes_for_endpoint("any")` returns `Ok(vec![])`
- `runtime_port_default_health_check_endpoint_returns_error`: same dummy → `health_check_endpoint("any")` returns `Err(DomainError)`

**Acceptance:**
- `cargo test -p camel-core --lib runtime_port_default` passes
- `cargo build -p camel-core` clean (existing impls compile unchanged via defaults)
- `cargo clippy -p camel-core -- -D warnings` clean

- [x] 2

### Task 3: HealthCheckRegistry check_route method

**Files:**
- `crates/camel-core/src/health_registry.rs` (modified)

**Steps:**
1. Extract a `pub(crate) fn combine_worst(a: HealthStatus, b: HealthStatus) -> HealthStatus` helper from the existing inline logic in `check_all` (Unhealthy > Degraded > Healthy). Reuse it in `check_all`, `check_route`, and the endpoint health handler in Task 5.
2. Add `pub async fn check_route(&self, route_id: &str) -> HealthStatus`.
3. Run the same recovery pre-pass that `check_all` uses (R4-L12 write-locked forced-entry recovery: clears `forced` when `started_after_force && probe_generation > probe_generation_at_force`; expires TTL-expired reasons) but scoped to the single `route_id`. This prevents `check_route` from returning stale `Unhealthy` for a recovered route.
4. Logic order after recovery: (a) if route has a surviving forced-unhealthy entry → return `Unhealthy`; (b) if route is inactive (not in active set) → return `Healthy`; (c) if no probes registered for route → return `Healthy`; (d) otherwise: run all probes for this route_id, aggregate via `combine_worst` with same timeout/panic handling as `check_all`.
5. Use the internal `RouteHealth` / probe storage already present (same data `check_all` iterates).

**Tests:**
- `check_route_healthy_with_probes`: route `r1` registered with a probe returning `Healthy` → `check_route("r1").await` returns `Healthy`
- `check_route_unhealthy_probe`: route `r1` with one probe returning `Unhealthy` → returns `Unhealthy`
- `check_route_no_probes_healthy`: route `r1` registered with no probes → returns `Healthy`
- `check_route_forced_unhealthy_no_probes`: route `r1` has no probes but `force_unhealthy_for_route("r1", "test", "reason")` was called → returns `Unhealthy`
- `check_route_stopped_route_healthy`: route `r1` registered then stopped (marked inactive) → `check_route("r1").await` returns `Healthy`
- `check_route_unknown_route_healthy`: `check_route("nonexistent").await` → returns `Healthy`

**Acceptance:**
- `cargo test -p camel-core --lib check_route` passes all 6 tests
- `cargo clippy -p camel-core -- -D warnings` clean

- [x] 3

### Task 4: Wire EndpointIndex into DefaultRouteController

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_staging.rs` (modified)

**Steps:**
1. Add `endpoint_index: EndpointIndex` field to `DefaultRouteController`. Initialize `endpoint_index: EndpointIndex::new()` in every `Self { ... }` struct literal (`with_beans_and_platform_service`, `with_languages`, `with_languages_and_beans`).
2. In `add_route(definition)`: capture `let from_uri = definition.from_uri().to_string();` and `route_id` *before* `build_managed_route(definition, ...)` moves the definition. After successful registration, call `self.endpoint_index.insert(&from_uri, &route_id)`.
3. In `add_route_with_generation(...)`: capture `from_uri` and `route_id` before the definition is moved. After successful registration, call `self.endpoint_index.insert(&from_uri, &route_id)`.
4. In `insert_prepared_route(prepared: PreparedRoute)` (route_staging.rs): capture `let from_uri = managed.from_uri.clone();` and `let route_id = prepared.route_id.clone();` *before* `self.routes.insert(prepared.route_id, managed)` moves `managed`. After the insert, call `self.endpoint_index.insert(&from_uri, &route_id)`.
5. In `remove_route(route_id)`: call `self.endpoint_index.remove(route_id)`.
6. In `remove_route_preserving_functions(route_id)`: call `self.endpoint_index.remove(route_id)`.
7. Add `pub fn routes_for_endpoint(&self, uri: &str) -> Vec<String>` delegating to the index.
8. Add `pub fn list_endpoint_uris(&self) -> Vec<String>` delegating to the index.

**Tests:**
- `controller_add_route_indexes_endpoint`: call `add_route` with `from_uri = "timer:tick"` → `routes_for_endpoint("timer:tick")` returns `vec![route_id]`
- `controller_add_route_with_generation_indexes_endpoint`: call `add_route_with_generation` with `from_uri = "direct:gen"` → `routes_for_endpoint("direct:gen")` returns `vec![route_id]`
- `controller_insert_prepared_route_indexes_endpoint`: stage a `ManagedRoute` with `from_uri = "seda:staged"` via `prepare_route_definition_with_generation` → call `insert_prepared_route(prepared)` → `routes_for_endpoint("seda:staged")` returns `vec![route_id]`
- `controller_remove_route_clears_endpoint`: add then `remove_route` → `routes_for_endpoint` returns empty
- `controller_remove_preserving_clears_endpoint`: add then `remove_route_preserving_functions` → `routes_for_endpoint` returns empty
- `controller_multiple_routes_same_uri`: add two routes with same `from_uri` → `routes_for_endpoint` returns both
- `controller_list_endpoint_uris`: add 3 routes with different URIs → `list_endpoint_uris()` contains all 3
- `controller_list_endpoint_uris_empty`: no routes registered → `list_endpoint_uris()` returns `vec![]`

**Acceptance:**
- `cargo test -p camel-core --lib endpoint` passes all tests
- `cargo clippy -p camel-core -- -D warnings` clean

- [x] 4

### Task 5: Actor messages + RuntimeExecutionAdapter wiring

**Files:**
- `crates/camel-core/src/lifecycle/adapters/controller_actor_commands.rs` (modified — `RouteControllerCommand` enum + `RouteControllerHandle` forwarding)
- `crates/camel-core/src/lifecycle/adapters/runtime_execution.rs` (modified)

**Steps:**
1. Add message variant `ListEndpoints` to the controller actor enum. Handler returns `Vec<String>` by calling `controller.list_endpoint_uris()`.
2. Add message variant `RoutesForEndpoint { uri: String }`. Handler returns `Vec<String>` by calling `controller.routes_for_endpoint(&uri)`.
3. Add message variant `HealthCheckEndpoint { uri: String }`. Handler: (a) look up route_ids via `controller.routes_for_endpoint(&uri)`; (b) if empty → return `Err`; (c) spawn concurrent `health_registry.check_route(&route_id)` futures for each route_id (do NOT await sequentially — one slow probe must not block the actor loop); (d) aggregate results via `combine_worst` (the shared helper from Task 3).
4. Add forwarding methods to `RouteControllerHandle` for each message (following the pattern of existing methods like `in_flight_count`).
5. In `RuntimeExecutionAdapter`, override `list_endpoints` to delegate to `controller.list_endpoints().await`.
6. In `RuntimeExecutionAdapter`, implement `routes_for_endpoint` to delegate to `controller.routes_for_endpoint(uri).await`.
7. In `RuntimeExecutionAdapter`, implement `health_check_endpoint` to delegate to `controller.health_check_endpoint(uri).await` and map errors via `to_domain`.

**Tests:**
- `adapter_list_endpoints_returns_registered_uris`: register a route via adapter with `from_uri = "timer:tick"` → `list_endpoints().await` returns `vec!["timer:tick"]`
- `adapter_routes_for_endpoint_returns_route_id`: register route → `routes_for_endpoint("timer:tick").await` returns `Ok(vec![route_id])`
- `adapter_routes_for_endpoint_unknown_returns_empty`: `routes_for_endpoint("direct:unknown").await` returns `Ok(vec![])`
- `adapter_health_check_endpoint_returns_healthy`: register route → `health_check_endpoint("timer:tick").await` returns `Ok(HealthStatus::Healthy)`
- `adapter_health_check_endpoint_worst_status`: register two routes sharing `from_uri = "direct:shared"`, force one unhealthy → `health_check_endpoint("direct:shared").await` returns `Ok(HealthStatus::Unhealthy)` (worst wins)
- `adapter_health_check_endpoint_unknown_returns_error`: `health_check_endpoint("direct:unknown").await` returns `Err(DomainError)`
- `adapter_remove_route_clears_endpoint`: register then remove via adapter → `routes_for_endpoint` returns empty

**Acceptance:**
- `cargo test -p camel-core --lib endpoint` passes all tests
- `cargo clippy -p camel-core -- -D warnings` clean
- `cargo build --workspace` clean
- `cargo fmt --check --all` clean
- `cargo xtask lint-unwrap` clean

- [x] 5
