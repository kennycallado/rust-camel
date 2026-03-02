use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{CamelError, RouteController, RouteStatus};
use camel_component::Component;

use crate::registry::Registry;
use crate::route::RouteDefinition;
use crate::route_controller::DefaultRouteController;

/// Counter for generating unique route IDs.
static ROUTE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique route ID.
fn generate_route_id() -> String {
    format!("route-{}", ROUTE_ID_COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// The CamelContext is the runtime engine that manages components, routes, and their lifecycle.
///
/// # Lifecycle
///
/// A `CamelContext` is single-use: call [`start()`](Self::start) once to launch routes,
/// then [`stop()`](Self::stop) or [`abort()`](Self::abort) to shut down. Restarting a
/// stopped context is not supported — create a new instance instead.
pub struct CamelContext {
    registry: Arc<std::sync::Mutex<Registry>>,
    route_controller: Arc<Mutex<DefaultRouteController>>,
    cancel_token: CancellationToken,
}

impl CamelContext {
    /// Create a new, empty CamelContext.
    pub fn new() -> Self {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let controller = Arc::new(Mutex::new(DefaultRouteController::new(Arc::clone(
            &registry,
        ))));

        // Set self-ref so DefaultRouteController can create ProducerContext
        // Use try_lock since we just created it and nobody else has access yet
        controller
            .try_lock()
            .unwrap()
            .set_self_ref(Arc::clone(&controller) as Arc<Mutex<dyn RouteController>>);

        Self {
            registry,
            route_controller: controller,
            cancel_token: CancellationToken::new(),
        }
    }

    /// Set a global error handler applied to all routes without a per-route handler.
    pub fn set_error_handler(&mut self, config: ErrorHandlerConfig) {
        self.route_controller
            .try_lock()
            .unwrap()
            .set_error_handler(config);
    }

    /// Register a component with this context.
    pub fn register_component<C: Component + 'static>(&mut self, component: C) {
        info!(scheme = component.scheme(), "Registering component");
        self.registry.lock().unwrap().register(component);
    }

    /// Add a route definition to this context.
    ///
    /// The route must have an ID. If None, one will be generated automatically.
    /// Steps are resolved immediately using registered components.
    pub fn add_route_definition(
        &mut self,
        mut definition: RouteDefinition,
    ) -> Result<(), CamelError> {
        // Auto-generate route ID if not set
        if definition.route_id().is_none() {
            definition = definition.with_route_id(generate_route_id());
        }

        info!(from = definition.from_uri(), route_id = ?definition.route_id(), "Adding route definition");

        self.route_controller
            .try_lock()
            .unwrap()
            .add_route(definition)
    }

    /// Access the component registry.
    pub fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry.lock().unwrap()
    }

    /// Access the route controller.
    pub fn route_controller(&self) -> &Arc<Mutex<DefaultRouteController>> {
        &self.route_controller
    }

    /// Get the status of a route by ID.
    pub fn route_status(&self, route_id: &str) -> Option<RouteStatus> {
        self.route_controller
            .try_lock()
            .ok()?
            .route_status(route_id)
    }

    /// Start all routes. Each route's consumer will begin producing exchanges.
    ///
    /// Only routes with `auto_startup == true` will be started, in order of their
    /// `startup_order` (lower values start first).
    pub async fn start(&mut self) -> Result<(), CamelError> {
        info!("Starting CamelContext");
        self.route_controller
            .lock()
            .await
            .start_all_routes()
            .await?;
        info!("CamelContext started");
        Ok(())
    }

    /// Graceful shutdown with default 30-second timeout.
    pub async fn stop(&mut self) -> Result<(), CamelError> {
        self.stop_timeout(std::time::Duration::from_secs(30)).await
    }

    /// Graceful shutdown with custom timeout.
    ///
    /// Note: The timeout parameter is currently not used directly; the RouteController
    /// manages its own shutdown timeout. This may change in a future version.
    pub async fn stop_timeout(&mut self, _timeout: std::time::Duration) -> Result<(), CamelError> {
        info!("Stopping CamelContext");

        // Signal cancellation (for any legacy code that might use it)
        self.cancel_token.cancel();

        // Stop all routes via the controller
        self.route_controller.lock().await.stop_all_routes().await?;

        info!("CamelContext stopped");
        Ok(())
    }

    /// Immediate abort — kills all tasks without draining.
    pub async fn abort(&mut self) {
        self.cancel_token.cancel();
        let _ = self.route_controller.lock().await.stop_all_routes().await;
    }
}

impl Default for CamelContext {
    fn default() -> Self {
        Self::new()
    }
}
