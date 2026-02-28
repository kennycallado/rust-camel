use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::{Layer, Service, ServiceExt};
use tracing::{error, info};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{BoxProcessor, CamelError};
use camel_component::{Component, ConsumerContext, consumer::ExchangeEnvelope};
use camel_endpoint::parse_uri;
use camel_processor::error_handler::ErrorHandlerLayer;
use camel_processor::circuit_breaker::CircuitBreakerLayer;
use camel_processor::splitter::SplitterService;

use crate::registry::Registry;
use crate::route::{BuilderStep, Route, RouteDefinition, compose_pipeline};

/// The CamelContext is the runtime engine that manages components, routes, and their lifecycle.
///
/// # Lifecycle
///
/// A `CamelContext` is single-use: call [`start()`](Self::start) once to launch routes,
/// then [`stop()`](Self::stop) or [`abort()`](Self::abort) to shut down. Restarting a
/// stopped context is not supported — create a new instance instead.
pub struct CamelContext {
    registry: Registry,
    routes: Vec<Route>,
    tasks: Vec<JoinHandle<()>>,
    cancel_token: CancellationToken,
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
            cancel_token: CancellationToken::new(),
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

    /// Resolve BuilderSteps into BoxProcessors, handling To(uri) resolution
    /// and recursive Split sub-pipeline resolution.
    fn resolve_steps(&self, steps: Vec<BuilderStep>) -> Result<Vec<BoxProcessor>, CamelError> {
        let mut processors: Vec<BoxProcessor> = Vec::new();
        for step in steps {
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
                BuilderStep::Split { config, steps } => {
                    let sub_processors = self.resolve_steps(steps)?;
                    let sub_pipeline = compose_pipeline(sub_processors);
                    let splitter = SplitterService::new(config, sub_pipeline);
                    processors.push(BoxProcessor::new(splitter));
                }
            }
        }
        Ok(processors)
    }

    /// Add a route definition. The definition's "to" URIs are resolved immediately
    /// using the registered components.
    pub fn add_route_definition(&mut self, definition: RouteDefinition) -> Result<(), CamelError> {
        info!(from = definition.from_uri(), "Adding route definition");

        let processors = self.resolve_steps(definition.steps)?;
        let pipeline = compose_pipeline(processors);

        // Apply circuit breaker if configured (wraps the step pipeline).
        let pipeline = if let Some(cb_config) = definition.circuit_breaker {
            let cb_layer = CircuitBreakerLayer::new(cb_config);
            BoxProcessor::new(cb_layer.layer(pipeline))
        } else {
            pipeline
        };

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

        // Take routes to iterate
        let routes = std::mem::take(&mut self.routes);

        for route in routes {
            let from_uri = route.from_uri().to_string();
            let from = parse_uri(&from_uri)?;
            let component = self.registry.get_or_err(&from.scheme)?;
            let endpoint = component.create_endpoint(&from_uri)?;

            // Create a channel for the consumer to send exchanges into
            let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(256);
            let child_token = self.cancel_token.child_token();
            let consumer_ctx = ConsumerContext::new(tx, child_token);

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

            // Spawn a task that reads exchanges and runs them through the Tower pipeline.
            // This task exits when the channel is fully drained (all senders dropped).
            // During graceful shutdown, the consumer stops first (via CancellationToken),
            // which drops its sender, causing the channel to close after in-flight
            // exchanges are processed — achieving drain-then-exit semantics.
            let pipeline_cancel = self.cancel_token.child_token();
            let pipeline_handle = tokio::spawn(async move {
                while let Some(envelope) = rx.recv().await {
                    let ExchangeEnvelope { exchange, reply_tx } = envelope;

                    // Wait for the service to be ready. If the circuit breaker is
                    // open, poll_ready returns CircuitOpen — back off and retry
                    // instead of killing the route permanently.
                    //
                    // Note: while backing off, the current exchange is held here in
                    // the loop. The channel continues to buffer incoming exchanges
                    // (up to its capacity). On shutdown:
                    // - The held exchange gets a CircuitOpen reply (via the cancel branch).
                    // - Exchanges buffered in the channel are dropped silently (fire-and-forget)
                    //   or their callers receive RecvError mapped to ChannelClosed (request-reply
                    //   via direct:).
                    let svc = loop {
                        match pipeline.ready().await {
                            Ok(svc) => break svc,
                            Err(CamelError::CircuitOpen(ref msg)) => {
                                tracing::warn!("Circuit open, backing off: {msg}");
                                tokio::select! {
                                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                                        continue;
                                    }
                                    _ = pipeline_cancel.cancelled() => {
                                        // Shutting down — don't retry.
                                        if let Some(tx) = reply_tx {
                                            let _ = tx.send(Err(CamelError::CircuitOpen(msg.clone())));
                                        }
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Pipeline not ready: {e}");
                                if let Some(tx) = reply_tx {
                                    let _ = tx.send(Err(e));
                                }
                                return;
                            }
                        }
                    };

                    let result = svc.call(exchange).await;
                    if let Some(tx) = reply_tx {
                        // Request-reply: send result back to consumer (e.g. direct:).
                        let _ = tx.send(result);
                    } else if let Err(e) = result {
                        error!("Pipeline error: {e}");
                    }
                }
            });
            self.tasks.push(pipeline_handle);
        }

        info!("CamelContext started");
        Ok(())
    }

    /// Graceful shutdown with default 30-second timeout.
    pub async fn stop(&mut self) -> Result<(), CamelError> {
        self.stop_timeout(std::time::Duration::from_secs(30)).await
    }

    /// Graceful shutdown with custom timeout.
    pub async fn stop_timeout(&mut self, timeout: std::time::Duration) -> Result<(), CamelError> {
        info!("Stopping CamelContext");

        // 1. Signal all consumers to stop
        self.cancel_token.cancel();

        // 2. Wait for all tasks to complete within timeout
        let mut tasks = std::mem::take(&mut self.tasks);
        let result = tokio::time::timeout(timeout, async {
            for task in &mut tasks {
                let _ = task.await;
            }
        })
        .await;

        // 3. If timeout expired, abort remaining tasks
        if result.is_err() {
            tracing::warn!(
                "Graceful shutdown timed out after {:?}, aborting remaining tasks",
                timeout
            );
            for task in &tasks {
                task.abort();
            }
        }

        info!("CamelContext stopped");
        Ok(())
    }

    /// Immediate abort — kills all tasks without draining.
    pub async fn abort(&mut self) {
        self.cancel_token.cancel();
        let tasks = std::mem::take(&mut self.tasks);
        for task in &tasks {
            task.abort();
        }
    }
}

impl Default for CamelContext {
    fn default() -> Self {
        Self::new()
    }
}
