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
use crate::lifecycle::application::ports::{RouteDestructiveTeardownPort, RouteOrderingPort};

#[async_trait]
impl RouteOrderingPort for RouteControllerHandle {
    async fn auto_startup_route_ids(&self) -> Result<Vec<String>, CamelError> {
        RouteControllerHandle::auto_startup_route_ids(self).await
    }

    async fn shutdown_route_ids(&self) -> Result<Vec<String>, CamelError> {
        RouteControllerHandle::shutdown_route_ids(self).await
    }

    // Both gate methods act directly on the shared Arc — no actor
    // round-trip. Indirection through the actor command queue would
    // deadlock when a test hook blocks the actor while the gate is being
    // reset; the watch send is synchronous (send_if_modified), so direct
    // shared-state preserves the gate's idempotency contract. The methods
    // stay async only for trait-shape consistency.
    async fn reset_cohort(&self) {
        self.cohort.close();
    }

    async fn activate_cohort(&self) {
        self.cohort.open();
    }
}

#[async_trait]
impl RouteDestructiveTeardownPort for RouteControllerHandle {
    async fn shutdown(&self) -> Result<(), CamelError> {
        RouteControllerHandle::shutdown(self).await
    }
}

#[cfg(test)]
mod route_ordering_port_gate {
    use super::*;
    use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::shared::components::domain::Registry;
    use std::sync::Arc;

    fn spawned_handle() -> RouteControllerHandle {
        let controller = DefaultRouteController::new(
            Arc::new(std::sync::Mutex::new(Registry::new())),
            Arc::new(camel_api::NoopPlatformService::default()),
        );
        let (handle, _actor) = spawn_controller_actor(controller);
        handle
    }

    #[tokio::test]
    async fn route_ordering_port_gate_reset_then_activate_roundtrip() {
        let handle = spawned_handle();
        let port: &dyn RouteOrderingPort = &handle;

        port.reset_cohort().await;
        port.activate_cohort().await;

        assert!(handle.cohort_gate().is_open());
    }

    #[tokio::test]
    async fn route_ordering_port_gate_activate_idempotent() {
        let handle = spawned_handle();
        let port: &dyn RouteOrderingPort = &handle;

        port.activate_cohort().await;
        port.activate_cohort().await;
        port.activate_cohort().await;

        assert!(handle.cohort_gate().is_open());
    }

    #[tokio::test]
    async fn route_ordering_port_gate_reset_rearms() {
        let handle = spawned_handle();
        let port: &dyn RouteOrderingPort = &handle;

        port.activate_cohort().await;
        assert!(handle.cohort_gate().is_open());

        port.reset_cohort().await;

        assert!(!handle.cohort_gate().is_open());
    }
}
