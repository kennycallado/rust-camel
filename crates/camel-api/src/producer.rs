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
