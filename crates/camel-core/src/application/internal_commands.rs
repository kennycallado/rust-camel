// application/internal_commands.rs
// Internal command bus for Rust API route registration.
// NOT exported from camel-core — for use only within the crate.

use async_trait::async_trait;
use camel_api::CamelError;

use crate::application::route_types::RouteDefinition;

/// Internal command bus for the Rust API path.
/// Implemented by RuntimeBus. Not public outside camel-core.
#[async_trait]
pub(crate) trait InternalRuntimeCommandBus: Send + Sync {
    async fn register_route(&self, def: RouteDefinition) -> Result<(), CamelError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::CamelError;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct StubBus {
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl InternalRuntimeCommandBus for StubBus {
        async fn register_route(&self, def: RouteDefinition) -> Result<(), CamelError> {
            self.calls.lock().await.push(def.route_id().to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn internal_bus_trait_is_dispatchable() {
        let calls = Arc::new(Mutex::new(vec![]));
        let bus: Arc<dyn InternalRuntimeCommandBus> = Arc::new(StubBus {
            calls: calls.clone(),
        });
        let def = RouteDefinition::new("timer:test", vec![]).with_route_id("test-route");
        bus.register_route(def).await.unwrap();
        assert_eq!(*calls.lock().await, vec!["test-route".to_string()]);
    }
}
