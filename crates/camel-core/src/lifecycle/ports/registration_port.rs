// lifecycle/ports/registration_port.rs
// Canonical location for RouteRegistrationPort.
// This port allows in-process route registration with compiled BoxProcessor pipelines.
// It is intentionally NOT serializable — in-process Rust API only.

use async_trait::async_trait;
use camel_api::CamelError;

use crate::lifecycle::application::route_definition::RouteDefinition;

/// Port for in-process route registration with compiled pipeline.
/// Non-serializable — in-process only. Implemented by RuntimeBus.
#[async_trait]
pub(crate) trait RouteRegistrationPort: Send + Sync {
    async fn register_route(&self, def: RouteDefinition) -> Result<(), CamelError>;
}
