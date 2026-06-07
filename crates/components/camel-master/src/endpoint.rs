use std::sync::Arc;
use std::time::Duration;

use camel_api::{CamelError, MetricsCollector, PlatformService};
use camel_component_api::{
    BoxProcessor, Component, ComponentContext, Consumer, Endpoint, NetworkRetryPolicy,
    ProducerContext,
};
use camel_language_api::Language;

use crate::consumer::MasterConsumer;

pub(crate) struct MasterEndpoint {
    pub(crate) uri: String,
    pub(crate) lock_name: String,
    pub(crate) delegate_uri: String,
    pub(crate) delegate_component: Arc<dyn Component>,
    // TODO(MST-001): MetricsCollector is wired through from ComponentContext but never called.
    // Should emit metrics on leader acquisition (increment_exchanges), leader loss
    // (increment_errors), and delegate start/stop events (record_circuit_breaker_change).
    pub(crate) metrics: Arc<dyn MetricsCollector>,
    pub(crate) platform_service: Arc<dyn PlatformService>,
    pub(crate) drain_timeout: Duration,
    pub(crate) reconnect: NetworkRetryPolicy,
}

impl Endpoint for MasterEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(MasterConsumer::new(
            self.lock_name.clone(),
            self.delegate_uri.clone(),
            Arc::clone(&self.delegate_component),
            Arc::clone(&self.metrics),
            Arc::clone(&self.platform_service),
            self.drain_timeout,
            self.reconnect.clone(),
            rt,
        )))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let delegate_ctx = MasterDelegateContext {
            delegate_component: Arc::clone(&self.delegate_component),
            metrics: Arc::clone(&self.metrics),
            platform_service: Arc::clone(&self.platform_service),
        };

        self.delegate_component
            .create_endpoint(&self.delegate_uri, &delegate_ctx)?
            .create_producer(rt, ctx)
    }
}

pub(crate) struct MasterDelegateContext {
    // Fields are pub(crate) during Tasks 1-2 because reconcile_event (still in
    // lib.rs) constructs this struct via field-init syntax. After Task 3 (Commit 2
    // — leadership.rs extraction), they CAN be narrowed to private since only
    // endpoint.rs constructs them (in create_producer).
    pub(crate) delegate_component: Arc<dyn Component>,
    pub(crate) metrics: Arc<dyn MetricsCollector>,
    pub(crate) platform_service: Arc<dyn PlatformService>,
}

impl ComponentContext for MasterDelegateContext {
    fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
        if self.delegate_component.scheme() == scheme {
            Some(Arc::clone(&self.delegate_component))
        } else {
            None
        }
    }

    fn resolve_language(&self, _name: &str) -> Option<Arc<dyn Language>> {
        None
    }

    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::clone(&self.metrics)
    }

    fn platform_service(&self) -> Arc<dyn PlatformService> {
        Arc::clone(&self.platform_service)
    }

    fn register_route_health_check(
        &self,
        _route_id: &str,
        _check: Arc<dyn camel_api::AsyncHealthCheck>,
    ) {
    }

    fn unregister_route_health_check(&self, _route_id: &str) {}
}
