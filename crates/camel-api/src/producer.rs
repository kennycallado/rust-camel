//! Producer context for dependency injection.
//!
//! This module provides [`ProducerContext`] for holding shared dependencies
//! that producers need access to, such as the runtime command/query handle.

use crate::runtime::RuntimeHandle;
use std::sync::Arc;

/// Context provided to producers for dependency injection.
///
/// `ProducerContext` holds references to shared infrastructure components
/// that producers may need access to during message production.
#[derive(Clone)]
pub struct ProducerContext {
    runtime: Option<Arc<dyn RuntimeHandle>>,
}

impl ProducerContext {
    /// Creates a new empty `ProducerContext`.
    pub fn new() -> Self {
        Self { runtime: None }
    }

    /// Attaches a runtime command/query handle.
    pub fn with_runtime(mut self, runtime: Arc<dyn RuntimeHandle>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Returns the runtime command/query handle, if configured.
    pub fn runtime(&self) -> Option<&Arc<dyn RuntimeHandle>> {
        self.runtime.as_ref()
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
}
