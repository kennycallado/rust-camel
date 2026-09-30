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
    // Fields stay pub(crate): the leadership.rs extraction is complete, and
    // both endpoint.rs (`create_producer`) and leadership.rs
    // (`reconcile_event`, ~line 283) construct this struct via field-init
    // syntax. Narrowing these fields to private would break the crate.
    // (This supersedes the earlier stale note that predicted they could be
    // narrowed after the leadership.rs extraction.)
    pub(crate) delegate_component: Arc<dyn Component>,
    pub(crate) metrics: Arc<dyn MetricsCollector>,
    pub(crate) platform_service: Arc<dyn PlatformService>,
}

/// Endpoint-creation-time adapter [`ComponentContext`] handed to a delegate
/// component by [`MasterEndpoint::create_producer`].
///
/// Boundary decision (bd rc-4bfnk): this context deliberately keeps the
/// trait's `None` default for `shutdown_token`. It is an
/// endpoint-creation-time adapter context, so it must not snapshot a shutdown
/// token — such a snapshot goes stale across `CamelContext` stop/start.
/// Wasm delegates created through this context do not need it: they resolve
/// tokens from the registration-time slot-bound context captured by
/// `WasmComponent` (`camel-component-wasm/src/lib.rs:70-84`; endpoint
/// construction passes `self.registry.clone()`, never the `ctx` passed
/// here). The only production callers of the trait method are
/// `camel-component-wasm/src/producer.rs:150` and
/// `camel-component-wasm/src/bean.rs:58`.
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
