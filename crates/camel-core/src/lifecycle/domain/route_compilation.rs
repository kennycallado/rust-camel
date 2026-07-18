//! Route compilation contract types — the value objects that route compilation
//! produces and that downstream ports/use-cases consume.
//!
//! `CompiledPipeline` is a pure contract value type: no runtime handles, no
//! I/O, no framework coupling. Its fields reference the [`camel_api`] contract
//! crate (a `BoxProcessor` and the `StepLifecycle` trait), so it can safely
//! live in the domain ring and be referenced by application-ring ports without
//! violating the dependency rule (`domain ← application ← adapters`).
//!
//! `PreparedRoute` is a thin `{ route_id: String }` token returned by the
//! prepare/insert two-phase commit. The heavy `ManagedRoute` is staged
//! internally on `DefaultRouteController::prepared_staging`; this token
//! carries only the lookup key across the port boundary.
//!
//! ADR-0045 §1 rings — domain is the innermost ring; ports (application ring)
//! depend on domain types, never on adapter types.

use std::sync::Arc;

use camel_api::{BoxProcessor, StepLifecycle};

/// A compiled pipeline bundle carrying both the processor and its lifecycle
/// handles. Returned by route compilation so callers (especially the hot-reload
/// Restart path) can thread lifecycle into the raw pipeline swap path.
#[derive(Debug)]
pub(crate) struct CompiledPipeline {
    pub(crate) processor: BoxProcessor,
    pub(crate) lifecycle: Vec<Arc<dyn StepLifecycle>>,
}

/// Thin token returned by `ReloadExecutorPort::prepare_route_definition_with_generation`.
///
/// The companion `ManagedRoute` (heavy adapter-internal bundle of
/// `JoinHandle`, `CancellationToken`, `SharedPipeline`, `Arc<AggregatorService>`,
/// `CompiledRoute`) is staged internally on the `DefaultRouteController`'s
/// `prepared_staging: HashMap<String, ManagedRoute>` map; this token carries
/// only the `route_id` lookup key across the port boundary. Living in
/// `lifecycle/domain`, the type satisfies the dependency rule (ports may
/// import contract types from domain).
#[derive(Debug)]
pub(crate) struct PreparedRoute {
    pub(crate) route_id: String,
}
