//! Route lifecycle management types.
//!
//! This module provides the [`RouteController`] trait for managing route lifecycle
//! (start, stop, suspend, resume) and the [`RouteStatus`] and [`RouteAction`] enums
//! for tracking and controlling route state.

use crate::CamelError;
use async_trait::async_trait;

/// Represents the current lifecycle status of a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteStatus {
    /// Route is stopped and not running.
    Stopped,
    /// Route is in the process of starting.
    Starting,
    /// Route is running and processing messages.
    Started,
    /// Route is in the process of stopping.
    Stopping,
    /// Route is suspended (temporarily paused).
    Suspended,
    /// Route has failed with an error message.
    Failed(String),
}

/// Represents actions that can be performed on a route.
#[derive(Debug, Clone)]
pub enum RouteAction {
    /// Start the route.
    Start,
    /// Stop the route.
    Stop,
    /// Suspend the route (pause without full shutdown).
    Suspend,
    /// Resume a suspended route.
    Resume,
    /// Restart the route (stop then start).
    Restart,
    /// Get the current status of the route.
    Status,
}

/// Trait for managing route lifecycle operations.
///
/// Implementations of this trait provide the ability to start, stop, suspend,
/// resume, and query the status of routes within a Camel context.
#[async_trait]
pub trait RouteController: Send + Sync {
    /// Start a specific route by its ID.
    ///
    /// # Errors
    ///
    /// Returns a `CamelError` if the route cannot be started.
    async fn start_route(&mut self, route_id: &str) -> Result<(), CamelError>;

    /// Stop a specific route by its ID.
    ///
    /// # Errors
    ///
    /// Returns a `CamelError` if the route cannot be stopped.
    async fn stop_route(&mut self, route_id: &str) -> Result<(), CamelError>;

    /// Restart a specific route by its ID.
    ///
    /// # Errors
    ///
    /// Returns a `CamelError` if the route cannot be restarted.
    async fn restart_route(&mut self, route_id: &str) -> Result<(), CamelError>;

    /// Suspend a specific route by its ID.
    ///
    /// # Errors
    ///
    /// Returns a `CamelError` if the route cannot be suspended.
    async fn suspend_route(&mut self, route_id: &str) -> Result<(), CamelError>;

    /// Resume a suspended route by its ID.
    ///
    /// # Errors
    ///
    /// Returns a `CamelError` if the route cannot be resumed.
    async fn resume_route(&mut self, route_id: &str) -> Result<(), CamelError>;

    /// Get the current status of a route by its ID.
    ///
    /// Returns `None` if the route does not exist.
    fn route_status(&self, route_id: &str) -> Option<RouteStatus>;

    /// Start all routes in the context.
    ///
    /// # Errors
    ///
    /// Returns a `CamelError` if any route cannot be started.
    async fn start_all_routes(&mut self) -> Result<(), CamelError>;

    /// Stop all routes in the context.
    ///
    /// # Errors
    ///
    /// Returns a `CamelError` if any route cannot be stopped.
    async fn stop_all_routes(&mut self) -> Result<(), CamelError>;
}
