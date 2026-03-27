use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use camel_api::CamelError;

use crate::application::route_types::RouteDefinition;
use crate::ports::RuntimeExecutionPort;
use crate::route_controller::RouteControllerInternal;

/// Runtime side-effect adapter backed by the technical route controller.
#[derive(Clone)]
pub struct RuntimeExecutionAdapter {
    controller: Arc<Mutex<dyn RouteControllerInternal>>,
}

impl RuntimeExecutionAdapter {
    pub fn new(controller: Arc<Mutex<dyn RouteControllerInternal>>) -> Self {
        Self { controller }
    }
}

#[async_trait]
impl RuntimeExecutionPort for RuntimeExecutionAdapter {
    async fn register_route(&self, definition: RouteDefinition) -> Result<(), CamelError> {
        let mut controller = self.controller.lock().await;
        controller.add_route(definition)
    }

    async fn start_route(&self, route_id: &str) -> Result<(), CamelError> {
        let mut controller = self.controller.lock().await;
        controller.start_route(route_id).await
    }

    async fn stop_route(&self, route_id: &str) -> Result<(), CamelError> {
        let mut controller = self.controller.lock().await;
        controller.stop_route(route_id).await
    }

    async fn suspend_route(&self, route_id: &str) -> Result<(), CamelError> {
        let mut controller = self.controller.lock().await;
        controller.suspend_route(route_id).await
    }

    async fn resume_route(&self, route_id: &str) -> Result<(), CamelError> {
        let mut controller = self.controller.lock().await;
        controller.resume_route(route_id).await
    }

    async fn reload_route(&self, route_id: &str) -> Result<(), CamelError> {
        let mut controller = self.controller.lock().await;
        controller.restart_route(route_id).await
    }

    async fn remove_route(&self, route_id: &str) -> Result<(), CamelError> {
        let mut controller = self.controller.lock().await;
        controller.remove_route(route_id)
    }
}
