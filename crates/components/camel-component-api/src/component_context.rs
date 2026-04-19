use std::sync::Arc;

use camel_api::{LeaderElector, MetricsCollector};
use camel_language_api::Language;

use crate::Component;

/// Runtime context passed to components during endpoint creation.
pub trait ComponentContext: Send + Sync {
    /// Resolve a component by scheme.
    fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>>;

    /// Resolve a language by name.
    fn resolve_language(&self, name: &str) -> Option<Arc<dyn Language>>;

    /// Access the active metrics collector.
    fn metrics(&self) -> Arc<dyn MetricsCollector>;

    /// Access the active leader elector.
    fn leader_elector(&self) -> Arc<dyn LeaderElector>;
}

/// Default no-op component context for tests/examples.
pub struct NoOpComponentContext;

impl ComponentContext for NoOpComponentContext {
    fn resolve_component(&self, _scheme: &str) -> Option<Arc<dyn Component>> {
        None
    }

    fn resolve_language(&self, _name: &str) -> Option<Arc<dyn Language>> {
        None
    }

    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::new(camel_api::NoOpMetrics)
    }

    fn leader_elector(&self) -> Arc<dyn LeaderElector> {
        Arc::new(camel_api::NoopLeaderElector)
    }
}
