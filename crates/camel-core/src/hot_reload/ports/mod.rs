//! Ports for the `hot_reload` bounded context.
//!
//! Currently inhabited by [`ReloadIntrospectionPort`] — the route-introspection
//! read-model the `#[cfg(test)]` reload-diff helper needs. Production reload is
//! already abstracted through `RuntimeExecutionHandle`; this port exists so test
//! code does not name the concrete `DefaultRouteController` adapter.

/// Read-model introspection over active routes, used by the reload diff tests.
/// Implemented by `DefaultRouteController`; deliberately camel-core-internal and
/// NOT added to the public `camel_api::RouteController` (which is imperative-only
/// by boundary test `public_route_controller_trait_exposes_no_lifecycle_read_model`).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) trait ReloadIntrospectionPort: Send + Sync {
    fn route_ids(&self) -> Vec<String>;
    fn route_from_uri(&self, route_id: &str) -> Option<String>;
    fn route_source_hash(&self, route_id: &str) -> Option<u64>;
}
