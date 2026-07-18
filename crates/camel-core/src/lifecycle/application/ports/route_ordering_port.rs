// lifecycle/application/ports/route_ordering_port.rs
// RouteOrderingPort — read-side abstraction over the route controller's
// route-ordering metadata. The application layer (use-cases) depends on this
// trait; the concrete impl lives in lifecycle/adapters (dependency rule).
//
// Established in Tier C Task C2 (`rc-d0pu.3`): the start_context and
// stop_context use-cases accept `&dyn RouteOrderingPort` instead of the
// concrete RouteControllerHandle. The only place the concrete handle is
// still taken is abort_context, for the destructive `shutdown()` call —
// recorded as an accepted exception in ADR-0045 §4.

use async_trait::async_trait;
use camel_api::CamelError;

#[async_trait]
pub trait RouteOrderingPort: Send + Sync {
    /// Return the route IDs the controller would start on boot, in the
    /// controller's startup order (lower `startup_order` first).
    async fn auto_startup_route_ids(&self) -> Result<Vec<String>, CamelError>;

    /// Return the route IDs the controller would stop on shutdown, in the
    /// controller's shutdown order.
    async fn shutdown_route_ids(&self) -> Result<Vec<String>, CamelError>;
}
