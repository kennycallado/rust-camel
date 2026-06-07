//! Producer context for dependency injection.
//!
//! This module provides [`ProducerContext`] for holding shared dependencies
//! that producers need access to, such as the runtime command/query handle.

use crate::runtime::RuntimeHandle;
use std::collections::HashMap;
use std::sync::Arc;

/// Context provided to producers for dependency injection.
///
/// `ProducerContext` holds references to shared infrastructure components
/// that producers may need access to during message production.
///
/// Extensible via `default_headers` (headers injected into every outgoing
/// message) and `timeout_ms` (per-producer timeout override).
#[derive(Clone)]
pub struct ProducerContext {
    runtime: Option<Arc<dyn RuntimeHandle>>,
    /// The route_id this producer is bound to, if any.
    ///
    /// Available for ADR-0012 metrics/health calls that require a route_id
    /// (categories (b′), (e), (g)). Set at producer creation time by the
    /// route build pipeline (step_resolution.rs).
    route_id: Option<String>,
    /// Default headers to inject into every outgoing message produced.
    default_headers: HashMap<String, String>,
    /// Optional timeout in milliseconds for producer operations.
    timeout_ms: Option<u64>,
}

impl ProducerContext {
    /// Creates a new empty `ProducerContext`.
    pub fn new() -> Self {
        Self {
            runtime: None,
            route_id: None,
            default_headers: HashMap::new(),
            timeout_ms: None,
        }
    }

    /// Attaches a runtime command/query handle.
    pub fn with_runtime(mut self, runtime: Arc<dyn RuntimeHandle>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Attaches the route_id this producer is bound to.
    pub fn with_route_id(mut self, route_id: impl Into<String>) -> Self {
        self.route_id = Some(route_id.into());
        self
    }

    /// Returns the route_id this producer is bound to, if set.
    pub fn route_id(&self) -> Option<&str> {
        self.route_id.as_deref()
    }

    /// Returns the runtime command/query handle, if configured.
    pub fn runtime(&self) -> Option<&Arc<dyn RuntimeHandle>> {
        self.runtime.as_ref()
    }

    /// Sets a default header that will be injected into every outgoing message.
    pub fn with_default_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.default_headers.insert(key.into(), value.into());
        self
    }

    /// Returns the default headers map.
    pub fn default_headers(&self) -> &HashMap<String, String> {
        &self.default_headers
    }

    /// Sets a timeout in milliseconds for producer operations.
    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Returns the configured timeout in milliseconds, if any.
    pub fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }
}

impl Default for ProducerContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CamelError;
    use crate::runtime::{
        RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult, RuntimeQuery, RuntimeQueryBus,
        RuntimeQueryResult,
    };
    use async_trait::async_trait;
    use futures::executor::block_on;

    struct NoopRuntime;

    #[async_trait]
    impl RuntimeCommandBus for NoopRuntime {
        async fn execute(&self, _cmd: RuntimeCommand) -> Result<RuntimeCommandResult, CamelError> {
            Ok(RuntimeCommandResult::Accepted)
        }
    }

    #[async_trait]
    impl RuntimeQueryBus for NoopRuntime {
        async fn ask(&self, _query: RuntimeQuery) -> Result<RuntimeQueryResult, CamelError> {
            Ok(RuntimeQueryResult::Routes { route_ids: vec![] })
        }
    }

    #[test]
    fn producer_context_new_is_empty() {
        let ctx = ProducerContext::new();
        assert!(ctx.runtime().is_none());
    }

    #[test]
    fn producer_context_default_is_empty() {
        let ctx = ProducerContext::default();
        assert!(ctx.runtime().is_none());
    }

    #[test]
    fn producer_context_with_runtime_sets_handle() {
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(NoopRuntime);
        let ctx = ProducerContext::new().with_runtime(runtime.clone());

        let attached = ctx.runtime().expect("runtime should be set");
        assert!(Arc::ptr_eq(attached, &runtime));
    }

    #[test]
    fn producer_context_clone_keeps_same_runtime_handle() {
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(NoopRuntime);
        let ctx = ProducerContext::new().with_runtime(runtime.clone());
        let cloned = ctx.clone();

        assert!(Arc::ptr_eq(cloned.runtime().unwrap(), &runtime));
    }

    #[test]
    fn producer_context_with_runtime_can_execute_command() {
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(NoopRuntime);
        let ctx = ProducerContext::new().with_runtime(runtime);
        let result = block_on(ctx.runtime().unwrap().execute(RuntimeCommand::StartRoute {
            route_id: "r1".into(),
            command_id: "c1".into(),
            causation_id: None,
        }))
        .unwrap();

        assert_eq!(result, RuntimeCommandResult::Accepted);
    }

    #[test]
    fn producer_context_with_runtime_can_execute_query() {
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(NoopRuntime);
        let ctx = ProducerContext::new().with_runtime(runtime);
        let result = block_on(ctx.runtime().unwrap().ask(RuntimeQuery::ListRoutes)).unwrap();

        assert_eq!(result, RuntimeQueryResult::Routes { route_ids: vec![] });
    }

    #[test]
    fn producer_context_with_runtime_replaces_previous_runtime() {
        let first: Arc<dyn RuntimeHandle> = Arc::new(NoopRuntime);
        let second: Arc<dyn RuntimeHandle> = Arc::new(NoopRuntime);
        let ctx = ProducerContext::new()
            .with_runtime(first)
            .with_runtime(second.clone());

        assert!(Arc::ptr_eq(ctx.runtime().unwrap(), &second));
    }

    #[test]
    fn producer_context_route_id_is_set_via_builder() {
        let ctx = ProducerContext::new().with_route_id("my-route");
        assert_eq!(ctx.route_id(), Some("my-route"));
    }

    #[test]
    fn producer_context_route_id_none_by_default() {
        let ctx = ProducerContext::new();
        assert_eq!(ctx.route_id(), None);
    }

    #[test]
    fn producer_context_default_headers_empty() {
        let ctx = ProducerContext::new();
        assert!(ctx.default_headers().is_empty());
    }

    #[test]
    fn producer_context_with_default_header() {
        let ctx = ProducerContext::new()
            .with_default_header("X-Source", "camel")
            .with_default_header("X-Trace", "enabled");
        let headers = ctx.default_headers();
        assert_eq!(headers.get("X-Source").unwrap(), "camel");
        assert_eq!(headers.get("X-Trace").unwrap(), "enabled");
    }

    #[test]
    fn producer_context_timeout_ms() {
        let ctx = ProducerContext::new();
        assert!(ctx.timeout_ms().is_none());

        let ctx = ctx.with_timeout_ms(5000);
        assert_eq!(ctx.timeout_ms(), Some(5000));
    }
}
