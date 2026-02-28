use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tower::{Layer, Service, ServiceExt};
use tracing::{error, info};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{BoxProcessor, CamelError};
use camel_component::{Component, ConsumerContext, consumer::ExchangeEnvelope};
use camel_endpoint::parse_uri;
use camel_processor::error_handler::ErrorHandlerLayer;

use crate::registry::Registry;
use crate::route::Route;
use crate::route::RouteDefinition;

/// The CamelContext is the runtime engine that manages components, routes, and their lifecycle.
pub struct CamelContext {
    registry: Registry,
    routes: Vec<Route>,
    tasks: Vec<JoinHandle<()>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// Optional global error handler applied to all routes that don't have a per-route handler.
    global_error_handler: Option<ErrorHandlerConfig>,
}

impl CamelContext {
    /// Create a new, empty CamelContext.
    pub fn new() -> Self {
        Self {
            registry: Registry::new(),
            routes: Vec::new(),
            tasks: Vec::new(),
            shutdown_tx: None,
            global_error_handler: None,
        }
    }

    /// Set a global error handler applied to all routes without a per-route handler.
    pub fn set_error_handler(&mut self, config: ErrorHandlerConfig) {
        self.global_error_handler = Some(config);
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

    /// Resolve an `ErrorHandlerConfig` into an `ErrorHandlerLayer` by looking up
    /// DLC and per-policy `handled_by` URIs in the component registry.
    fn resolve_error_handler(
        &self,
        config: ErrorHandlerConfig,
    ) -> Result<ErrorHandlerLayer, CamelError> {
        // Resolve DLC URI → producer.
        let dlc_producer = if let Some(ref uri) = config.dlc_uri {
            let parsed = parse_uri(uri)?;
            let component = self.registry.get_or_err(&parsed.scheme)?;
            let endpoint = component.create_endpoint(uri)?;
            Some(endpoint.create_producer()?)
        } else {
            None
        };

        // Resolve per-policy `handled_by` URIs.
        let mut resolved_policies = Vec::new();
        for policy in config.policies {
            let handler_producer = if let Some(ref uri) = policy.handled_by {
                let parsed = parse_uri(uri)?;
                let component = self.registry.get_or_err(&parsed.scheme)?;
                let endpoint = component.create_endpoint(uri)?;
                Some(endpoint.create_producer()?)
            } else {
                None
            };
            resolved_policies.push((policy, handler_producer));
        }

        Ok(ErrorHandlerLayer::new(dlc_producer, resolved_policies))
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

        // Determine which error handler config to use (per-route takes precedence).
        let eh_config = definition
            .error_handler
            .or_else(|| self.global_error_handler.clone());

        let pipeline: BoxProcessor = if let Some(config) = eh_config {
            let layer = self.resolve_error_handler(config)?;
            BoxProcessor::new(layer.layer(pipeline))
        } else {
            pipeline
        };

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
            let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(256);
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

                while let Some(envelope) = rx.recv().await {
                    let ExchangeEnvelope { exchange, reply_tx } = envelope;
                    // Ensure the service is ready before calling
                    match pipeline.ready().await {
                        Ok(svc) => {
                            let result = svc.call(exchange).await;
                            if let Some(tx) = reply_tx {
                                // Request-reply: send result back to consumer (e.g. direct:).
                                let _ = tx.send(result);
                            } else if let Err(e) = result {
                                error!("Pipeline error: {e}");
                            }
                        }
                        Err(e) => {
                            error!("Pipeline not ready: {e}");
                            if let Some(tx) = reply_tx {
                                let _ = tx.send(Err(e));
                            }
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
