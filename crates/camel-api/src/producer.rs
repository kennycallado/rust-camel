//! Producer context for dependency injection.
//!
//! This module provides [`ProducerContext`] for holding shared dependencies
//! that producers need access to, such as the route controller.

use crate::route_controller::RouteController;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Context provided to producers for dependency injection.
///
/// `ProducerContext` holds references to shared infrastructure components
/// that producers may need access to during message production.
#[derive(Clone)]
pub struct ProducerContext {
    route_controller: Arc<Mutex<dyn RouteController>>,
}

impl ProducerContext {
    /// Creates a new `ProducerContext` with the given route controller.
    pub fn new(route_controller: Arc<Mutex<dyn RouteController>>) -> Self {
        Self { route_controller }
    }

    /// Returns a reference to the route controller.
    pub fn route_controller(&self) -> &Arc<Mutex<dyn RouteController>> {
        &self.route_controller
    }
}
