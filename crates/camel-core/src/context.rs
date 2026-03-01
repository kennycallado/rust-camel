use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::{Layer, Service, ServiceExt};
use tracing::{error, info};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{BoxProcessor, CamelError};
use camel_component::{Component, ConcurrencyModel, ConsumerContext, consumer::ExchangeEnvelope};
use camel_endpoint::parse_uri;
use camel_processor::AggregatorService;
use camel_processor::FilterService;
use camel_processor::MulticastService;
use camel_processor::WireTapService;
use camel_processor::circuit_breaker::CircuitBreakerLayer;
use camel_processor::error_handler::ErrorHandlerLayer;
use camel_processor::splitter::SplitterService;

use crate::registry::Registry;
use crate::route::{BuilderStep, Route, RouteDefinition, compose_pipeline};

/// Wait for a pipeline service to be ready with circuit breaker backoff.
///
/// This helper encapsulates the pattern of repeatedly calling `ready()` on a
/// service while handling `CircuitOpen` errors with a fixed 1-second backoff and
/// cancellation checks. It returns `Ok(())` when the service is ready, or
/// `Err(e)` if cancellation occurred or a fatal error was encountered.
///
/// # Parameters
/// - `pipeline`: The service to wait for (must implement `tower::Service`)
/// - `cancel`: CancellationToken for shutdown detection
///
/// # Returns
/// - `Ok(())`: Service is ready, caller can proceed to `pipeline.call()`
/// - `Err(CamelError)`: Either cancelled or fatal error occurred, caller should
///   send the error to reply_tx and return from the current async block
async fn ready_with_backoff(
    pipeline: &mut BoxProcessor,
    cancel: &CancellationToken,
) -> Result<(), CamelError> {
    loop {
        match pipeline.ready().await {
            Ok(_) => return Ok(()),
            Err(CamelError::CircuitOpen(ref msg)) => {
                tracing::warn!("Circuit open, backing off: {msg}");
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                        continue;
                    }
                    _ = cancel.cancelled() => {
                        // Shutting down — don't retry.
                        return Err(CamelError::CircuitOpen(msg.clone()));
                    }
                }
            }
            Err(e) => {
                error!("Pipeline not ready: {e}");
                return Err(e);
            }
        }
    }
}

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
                BuilderStep::Aggregate { config } => {
                    let svc = AggregatorService::new(config);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Filter { predicate, steps } => {
                    let sub_processors = self.resolve_steps(steps)?;
                    let sub_pipeline = compose_pipeline(sub_processors);
                    let svc = FilterService::from_predicate(predicate, sub_pipeline);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::WireTap { uri } => {
                    let parsed = parse_uri(&uri)?;
                    let component = self.registry.get_or_err(&parsed.scheme)?;
                    let endpoint = component.create_endpoint(&uri)?;
                    let producer = endpoint.create_producer()?;
                    let svc = WireTapService::new(producer);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Multicast { config, steps } => {
                    // Each top-level step in the multicast scope becomes an independent endpoint.
                    // We resolve each step individually (not all together) so that each becomes
                    // a separate endpoint in the multicast.
                    let mut endpoints = Vec::new();
                    for step in steps {
                        // Resolve this single step into processors
                        let sub_processors = self.resolve_steps(vec![step])?;
                        // Compose them into a single endpoint
                        let endpoint = compose_pipeline(sub_processors);
                        endpoints.push(endpoint);
                    }
                    let svc = MulticastService::new(endpoints, config);
                    processors.push(BoxProcessor::new(svc));
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

        let mut route = Route::new(definition.from_uri, pipeline);
        if let Some(concurrency) = definition.concurrency {
            route = route.with_concurrency(concurrency);
        }
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

            // Create consumer and query its concurrency model BEFORE moving it into the spawn
            let mut consumer = endpoint.create_consumer()?;
            let consumer_concurrency = consumer.concurrency_model();

            // Extract parts from route before consuming
            let (mut pipeline, route_concurrency_override) = route.into_parts();

            // Resolve effective concurrency: route override > consumer default
            let effective_concurrency = route_concurrency_override.unwrap_or(consumer_concurrency);

            // Start the consumer in a background task
            let consumer_handle = tokio::spawn(async move {
                if let Err(e) = consumer.start(consumer_ctx).await {
                    error!("Consumer error: {e}");
                }
            });
            self.tasks.push(consumer_handle);

            // Spawn a task that reads exchanges and runs them through the Tower pipeline.
            // This task exits when the channel is fully drained (all senders dropped).
            // During graceful shutdown, the consumer stops first (via CancellationToken),
            // which drops its sender, causing the channel to close after in-flight
            // exchanges are processed — achieving drain-then-exit semantics.
            let pipeline_cancel = self.cancel_token.child_token();
            let pipeline_handle = match effective_concurrency {
                ConcurrencyModel::Sequential => {
                    tokio::spawn(async move {
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
                            if let Err(e) =
                                ready_with_backoff(&mut pipeline, &pipeline_cancel).await
                            {
                                if let Some(tx) = reply_tx {
                                    let _ = tx.send(Err(e));
                                }
                                return;
                            }

                            let result = pipeline.call(exchange).await;
                            if let Some(tx) = reply_tx {
                                // Request-reply: send result back to consumer (e.g. direct:).
                                let _ = tx.send(result);
                            } else if let Err(ref e) = result {
                                // Stopped is a control-flow sentinel, not a real error — silence it.
                                if !matches!(e, CamelError::Stopped) {
                                    error!("Pipeline error: {e}");
                                }
                            }
                        }
                    })
                }
                ConcurrencyModel::Concurrent { max } => {
                    let sem = max.map(|n| std::sync::Arc::new(tokio::sync::Semaphore::new(n)));
                    tokio::spawn(async move {
                        while let Some(envelope) = rx.recv().await {
                            let ExchangeEnvelope { exchange, reply_tx } = envelope;
                            let mut pipe_clone = pipeline.clone();
                            let sem = sem.clone();
                            let cancel = pipeline_cancel.clone();
                            tokio::spawn(async move {
                                // Acquire semaphore permit if bounded
                                let _permit = match &sem {
                                    Some(s) => Some(s.acquire().await.expect("semaphore closed")),
                                    None => None,
                                };

                                // Wait for the service to be ready with circuit breaker backoff
                                if let Err(e) = ready_with_backoff(&mut pipe_clone, &cancel).await {
                                    if let Some(tx) = reply_tx {
                                        let _ = tx.send(Err(e));
                                    }
                                    return;
                                }

                                let result = pipe_clone.call(exchange).await;
                                if let Some(tx) = reply_tx {
                                    let _ = tx.send(result);
                                } else if let Err(ref e) = result {
                                    if !matches!(e, CamelError::Stopped) {
                                        error!("Pipeline error: {e}");
                                    }
                                }
                            });
                        }
                    })
                }
            };
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
