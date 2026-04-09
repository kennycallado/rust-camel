use async_trait::async_trait;

use camel_api::{CamelError, RuntimeQueryResult};

use crate::lifecycle::adapters::controller_actor::RouteControllerHandle;
use crate::lifecycle::application::RouteDefinition;
use crate::lifecycle::domain::DomainError;
use crate::lifecycle::ports::RuntimeExecutionPort;

/// Runtime side-effect adapter backed by the technical route controller.
#[derive(Clone)]
pub struct RuntimeExecutionAdapter {
    controller: RouteControllerHandle,
}

impl RuntimeExecutionAdapter {
    pub fn new(controller: RouteControllerHandle) -> Self {
        Self { controller }
    }
}

fn to_domain(e: CamelError) -> DomainError {
    DomainError::InvalidState(e.to_string())
}

#[async_trait]
impl RuntimeExecutionPort for RuntimeExecutionAdapter {
    async fn register_route(&self, definition: RouteDefinition) -> Result<(), DomainError> {
        self.controller
            .add_route(definition)
            .await
            .map_err(to_domain)
    }

    async fn start_route(&self, route_id: &str) -> Result<(), DomainError> {
        self.controller
            .start_route(route_id)
            .await
            .map_err(to_domain)
    }

    async fn stop_route(&self, route_id: &str) -> Result<(), DomainError> {
        self.controller
            .stop_route(route_id)
            .await
            .map_err(to_domain)
    }

    async fn suspend_route(&self, route_id: &str) -> Result<(), DomainError> {
        self.controller
            .suspend_route(route_id)
            .await
            .map_err(to_domain)
    }

    async fn resume_route(&self, route_id: &str) -> Result<(), DomainError> {
        self.controller
            .resume_route(route_id)
            .await
            .map_err(to_domain)
    }

    async fn reload_route(&self, route_id: &str) -> Result<(), DomainError> {
        self.controller
            .restart_route(route_id)
            .await
            .map_err(to_domain)
    }

    async fn remove_route(&self, route_id: &str) -> Result<(), DomainError> {
        self.controller
            .remove_route(route_id)
            .await
            .map_err(to_domain)
    }

    async fn in_flight_count(&self, route_id: &str) -> Result<RuntimeQueryResult, DomainError> {
        Ok(
            match self
                .controller
                .in_flight_count(route_id)
                .await
                .map_err(to_domain)?
            {
                Some(count) => RuntimeQueryResult::InFlightCount {
                    route_id: route_id.to_string(),
                    count,
                },
                None => RuntimeQueryResult::RouteNotFound {
                    route_id: route_id.to_string(),
                },
            },
        )
    }
}
