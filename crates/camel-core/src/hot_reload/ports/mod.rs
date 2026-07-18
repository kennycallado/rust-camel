//! Ports for the `hot_reload` bounded context.
//!
//! Inhabited by:
//! - [`ReloadIntrospectionPort`] — the route-introspection read-model the
//!   `#[cfg(test)]` reload-diff helper needs. Production reload is already
//!   abstracted through `RuntimeExecutionHandle`; this port exists so test
//!   code does not name the concrete `DefaultRouteController` adapter.
//! - [`ReloadExecutorPort`] — the executor surface that
//!   `hot_reload/application/` (apply_swap / apply_add / apply_remove /
//!   apply_restart / drain_route) programs against. The concrete
//!   `RuntimeExecutionHandle` implements this port via pure delegation, so
//!   callers can be written against `&dyn ReloadExecutorPort` without taking
//!   a hard dependency on the handle.

use std::sync::Arc;

use async_trait::async_trait;

use camel_api::{BoxProcessor, CamelError, RuntimeCommand, RuntimeCommandResult, StepLifecycle};

// §4 ACCEPTED EXCEPTION (ADR-0045): PreparedRoute lives in the adapter ring
// because its `managed: ManagedRoute` field bundles adapter-internal state
// (JoinHandle, CancellationToken, SharedPipeline, AggregatorService,
// CompiledRoute). Relocating it requires a port-semantics redesign (thin
// { route_id } contract + controller-internal HashMap) out of Tier C's scope.
// The companion CompiledPipeline WAS relocated to lifecycle::domain (pure
// contract). This is the ONLY adapter import allowed in a port — pinned by the
// `port_traits_do_not_import_from_adapter_ring` boundary test allow-list.
use crate::lifecycle::adapters::route_controller::PreparedRoute;
use crate::lifecycle::application::route_definition::RouteDefinition;
use crate::lifecycle::domain::CompiledPipeline;

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

/// Executor surface consumed by `hot_reload/application/`. Object-safe
/// (mirrors the `#[async_trait] + Send + Sync` pattern from
/// `lifecycle/application/ports/runtime_ports.rs`).
///
/// The `#[cfg(test)]`-gated method exists to keep the test-only seam
/// (`test_lifecycle_inject` field) behind the same port — so `apply_swap`
/// does not have to reach past `&dyn ReloadExecutorPort` to a concrete
/// handle to access it.
///
/// Deliberately NOT exposed: `route_source_hash` and
/// `controller_route_count_for_test`. Both are reachable on the concrete
/// `RuntimeExecutionHandle` (inherent), and their only call sites dispatch
/// through the concrete handle — adding them to this port would invite
/// dead-code churn without any `&dyn` consumer. If a future caller needs
/// either, prefer a separate, narrowly-scoped port.
#[async_trait]
pub(crate) trait ReloadExecutorPort: Send + Sync {
    async fn add_route_definition(&self, definition: RouteDefinition) -> Result<(), CamelError>;

    async fn compile_route_definition_pipeline(
        &self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<CompiledPipeline, CamelError>;

    async fn compile_route_definition_dry_pipeline(
        &self,
        definition: RouteDefinition,
    ) -> Result<CompiledPipeline, CamelError>;

    async fn prepare_route_definition_with_generation(
        &self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<PreparedRoute, CamelError>;

    async fn insert_prepared_route(&self, prepared: PreparedRoute) -> Result<(), CamelError>;

    async fn remove_route_preserving_functions(&self, route_id: String) -> Result<(), CamelError>;

    async fn register_route_aggregate(&self, route_id: String) -> Result<(), CamelError>;

    async fn swap_route_pipeline(
        &self,
        route_id: &str,
        pipeline: BoxProcessor,
    ) -> Result<(), CamelError>;

    async fn stop_route_reload(&self, route_id: &str) -> Result<(), CamelError>;

    async fn start_route_reload(&self, route_id: &str) -> Result<(), CamelError>;

    async fn swap_route_pipeline_raw(
        &self,
        route_id: &str,
        pipeline: BoxProcessor,
        lifecycle: Vec<Arc<dyn StepLifecycle>>,
    ) -> Result<(), CamelError>;

    async fn execute_runtime_command(
        &self,
        cmd: RuntimeCommand,
    ) -> Result<RuntimeCommandResult, CamelError>;

    async fn runtime_route_status(&self, route_id: &str) -> Result<Option<String>, CamelError>;

    async fn in_flight_count(&self, route_id: &str) -> Result<u64, CamelError>;

    async fn route_has_lifecycle(&self, route_id: &str) -> bool;

    /// Test seam: replaces the direct `test_lifecycle_inject.lock().unwrap().take()`
    /// that `apply_swap` previously needed. The field is `#[cfg(test)]`-gated, so
    /// this method is too.
    #[cfg(test)]
    fn take_test_lifecycle_inject(&self) -> Option<Vec<Arc<dyn StepLifecycle>>>;
}
