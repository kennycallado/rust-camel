// lifecycle/adapters/route_ordering_impl.rs
// impl RouteOrderingPort for RouteControllerHandle.
//
// The trait is declared in lifecycle/application (the use-case layer owns
// the abstraction); the impl lives here in adapters, satisfying the
// dependency rule (application does not know the concrete adapter).
//
// Established in Tier C Task C2 (`rc-d0pu.3`).

use async_trait::async_trait;
use camel_api::CamelError;

use crate::lifecycle::adapters::controller_actor::RouteControllerHandle;
use crate::lifecycle::application::ports::RouteOrderingPort;

#[async_trait]
impl RouteOrderingPort for RouteControllerHandle {
    async fn auto_startup_route_ids(&self) -> Result<Vec<String>, CamelError> {
        RouteControllerHandle::auto_startup_route_ids(self).await
    }

    async fn shutdown_route_ids(&self) -> Result<Vec<String>, CamelError> {
        RouteControllerHandle::shutdown_route_ids(self).await
    }
}
