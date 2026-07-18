// lifecycle/application/ports/route_destructive_teardown_port.rs
// RouteDestructiveTeardownPort — narrow port for the abort use-case.
//
// Established in `rc-d0pu.3`-purge to close the last §4 use-case purity gap.
// The abort use-case needs the destructive `shutdown()` call (non-restartable,
// kills the controller actor) which does not fit `RouteOrderingPort`
// (semantics: restartable ordering queries). Rather than take the concrete
// `RouteControllerHandle` (dependency-rule violation), the use-case takes
// `&dyn RouteDestructiveTeardownPort`; the impl lives in
// `lifecycle/adapters/route_ordering_impl.rs`.

use async_trait::async_trait;
use camel_api::CamelError;

/// Destructive, non-restartable teardown of the route controller actor.
///
/// Distinct from `RouteOrderingPort` (which exposes restartable ordering
/// queries like `auto_startup_route_ids` / `shutdown_route_ids`): this port
/// carries ONLY the kill-the-actor `shutdown()` method.
#[async_trait]
pub trait RouteDestructiveTeardownPort: Send + Sync {
    /// Kill the controller actor. Non-restartable — the controller cannot be
    /// reused after this call.
    async fn shutdown(&self) -> Result<(), CamelError>;
}
