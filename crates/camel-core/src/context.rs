use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tower::{Service, ServiceExt};
use tracing::{error, info};

use camel_api::{BoxProcessor, CamelError, Exchange};
use camel_component::{Component, ConsumerContext};
use camel_endpoint::parse_uri;

use crate::registry::Registry;
use crate::route::Route;
use crate::route::RouteDefinition;

/// The CamelContext is the runtime engine that manages components, routes, and their lifecycle.
pub struct CamelContext {
    registry: Registry,
    routes: Vec<Route>,
    tasks: Vec<JoinHandle<()>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl CamelContext {
    /// Create a new, empty CamelContext.
    pub fn new() -> Self {
        Self {
            registry: Registry::new(),
            routes: Vec::new(),
            tasks: Vec::new(),
            shutdown_tx: None,
        }
    }

    /// Register a component with this context.
    pub fn register_component<C: Component + 'static>(&mut self, component: C) {
        info!(scheme = component.scheme(), "Registering component");
        self.registry.register(component);
    }

    /// Add a route to this context.
    pub fn add_route(&mut self, route: Route) {
        info!(from = route.from_uri(), "Adding route");
        self.routes.push(route);
    }

    /// Add a route definition. The definition's "to" URIs are resolved immediately
    /// using the registered components.
    pub fn add_route_definition(&mut self, definition: RouteDefinition) -> Result<(), CamelError> {
        use crate::route::{BuilderStep, compose_pipeline};

        info!(from = definition.from_uri(), "Adding route definition");

        let mut processors: Vec<BoxProcessor> = Vec::new();

        for step in definition.steps {
            match step {
                BuilderStep::Processor(svc) => {
                    processors.push(svc);
                }
                BuilderStep::To(uri) => {
                    let parsed = parse_uri(&uri)?;
                    let component = self.registry.get_or_err(&parsed.scheme)?;
                    let endpoint = component.create_endpoint(&uri)?;
                    let producer = endpoint.create_producer()?;
                    processors.push(producer);
                }
            }
        }

        let pipeline = compose_pipeline(processors);
        let route = Route::new(definition.from_uri, pipeline);
        self.add_route(route);
        Ok(())
    }

    /// Access the component registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Start all routes. Each route's consumer will begin producing exchanges.
    pub async fn start(&mut self) -> Result<(), CamelError> {
        info!("Starting CamelContext with {} routes", self.routes.len());

        let (shutdown_tx, _shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        // Take routes to iterate
        let routes = std::mem::take(&mut self.routes);

        for route in routes {
            let from_uri = route.from_uri().to_string();
            let from = parse_uri(&from_uri)?;
            let component = self.registry.get_or_err(&from.scheme)?;
            let endpoint = component.create_endpoint(&from_uri)?;

            // Create a channel for the consumer to send exchanges into
            let (tx, mut rx) = mpsc::channel::<Exchange>(256);
            let consumer_ctx = ConsumerContext::new(tx);

            // Start the consumer in a background task
            let mut consumer = endpoint.create_consumer()?;
            let consumer_handle = tokio::spawn(async move {
                if let Err(e) = consumer.start(consumer_ctx).await {
                    error!("Consumer error: {e}");
                }
            });
            self.tasks.push(consumer_handle);

            // Get the pipeline from the route
            let mut pipeline = route.into_pipeline();
            let shutdown_tx_clone = shutdown_tx.clone();

            // Spawn a task that reads exchanges and runs them through the Tower pipeline
            let pipeline_handle = tokio::spawn(async move {
                let _shutdown = shutdown_tx_clone;

                while let Some(exchange) = rx.recv().await {
                    // Ensure the service is ready before calling
                    match pipeline.ready().await {
                        Ok(svc) => {
                            if let Err(e) = svc.call(exchange).await {
                                error!("Pipeline error: {e}");
                            }
                        }
                        Err(e) => {
                            error!("Pipeline not ready: {e}");
                            break;
                        }
                    }
                }
            });
            self.tasks.push(pipeline_handle);
        }

        info!("CamelContext started");
        Ok(())
    }

    /// Stop the context, shutting down all routes.
    pub async fn stop(&mut self) -> Result<(), CamelError> {
        info!("Stopping CamelContext");

        // Drop shutdown sender to signal tasks
        self.shutdown_tx.take();

        // Abort all tasks
        for task in self.tasks.drain(..) {
            task.abort();
        }

        info!("CamelContext stopped");
        Ok(())
    }
}

impl Default for CamelContext {
    fn default() -> Self {
        Self::new()
    }
}
