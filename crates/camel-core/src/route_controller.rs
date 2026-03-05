//! Default implementation of RouteController.
//!
//! This module provides [`DefaultRouteController`], which manages route lifecycle
//! including starting, stopping, suspending, and resuming routes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::{Layer, Service, ServiceExt};
use tracing::{error, info, warn};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{BoxProcessor, CamelError, ProducerContext, RouteController, RouteStatus};
use camel_component::{ConcurrencyModel, ConsumerContext, consumer::ExchangeEnvelope};
use camel_endpoint::parse_uri;
use camel_processor::circuit_breaker::CircuitBreakerLayer;
use camel_processor::error_handler::ErrorHandlerLayer;
use camel_processor::{ChoiceService, WhenClause};

use crate::registry::Registry;
use crate::route::{BuilderStep, RouteDefinition, RouteDefinitionInfo, compose_pipeline};
use arc_swap::ArcSwap;

/// Notification sent when a route crashes.
///
/// Used by [`SupervisingRouteController`](crate::supervising_route_controller::SupervisingRouteController)
/// to monitor and restart failed routes.
#[derive(Debug, Clone)]
pub struct CrashNotification {
    /// The ID of the crashed route.
    pub route_id: String,
    /// The error that caused the crash.
    pub error: String,
}

/// Newtype to make BoxProcessor Sync-safe for ArcSwap.
///
/// # Safety
///
/// BoxProcessor (BoxCloneService) is Send but not Sync because the inner
/// Box<dyn CloneServiceInner> lacks a Sync bound. However:
///
/// 1. We ONLY access BoxProcessor via clone(), which is a read-only operation
///    (creates a new boxed service from the inner clone).
/// 2. The clone is owned by the calling thread and never shared.
/// 3. ArcSwap guarantees we only get & references (no &mut).
///
/// Therefore, concurrent access to &BoxProcessor for cloning is safe because
/// clone() does not mutate shared state and each thread gets an independent copy.
pub(crate) struct SyncBoxProcessor(pub(crate) BoxProcessor);
unsafe impl Sync for SyncBoxProcessor {}

type SharedPipeline = Arc<ArcSwap<SyncBoxProcessor>>;

/// Internal trait extending [`RouteController`] with methods needed by [`CamelContext`]
/// that are not part of the public lifecycle API.
///
/// Both [`DefaultRouteController`] and the future `SupervisingRouteController` implement
/// this trait, allowing `CamelContext` to hold either as `Arc<Mutex<dyn RouteControllerInternal>>`.
pub trait RouteControllerInternal: RouteController + Send {
    /// Add a route definition to the controller.
    fn add_route(&mut self, def: RouteDefinition) -> Result<(), CamelError>;

    /// Atomically swap the pipeline of a running route (for hot-reload).
    fn swap_pipeline(&self, route_id: &str, pipeline: BoxProcessor) -> Result<(), CamelError>;

    /// Returns the `from_uri` of a route by ID.
    fn route_from_uri(&self, route_id: &str) -> Option<String>;

    /// Set a global error handler applied to all routes.
    fn set_error_handler(&mut self, config: ErrorHandlerConfig);

    /// Set the self-reference needed to create `ProducerContext`.
    fn set_self_ref(&mut self, self_ref: Arc<Mutex<dyn RouteController>>);

    /// Returns the number of routes in the controller.
    fn route_count(&self) -> usize;

    /// Returns all route IDs.
    fn route_ids(&self) -> Vec<String>;
}

/// Internal state for a managed route.
struct ManagedRoute {
    /// The route definition metadata (for introspection).
    definition: RouteDefinitionInfo,
    /// Source endpoint URI.
    from_uri: String,
    /// Resolved processor pipeline (wrapped for atomic swap).
    pipeline: SharedPipeline,
    /// Concurrency model override (if any).
    concurrency: Option<ConcurrencyModel>,
    /// Shared lifecycle status — written by both the controller and spawned consumer tasks.
    status: Arc<std::sync::Mutex<RouteStatus>>,
    /// Handle for the consumer task (if running).
    consumer_handle: Option<JoinHandle<()>>,
    /// Handle for the pipeline task (if running).
    pipeline_handle: Option<JoinHandle<()>>,
    /// Cancellation token for stopping this route.
    cancel_token: CancellationToken,
}

/// Wait for a pipeline service to be ready with circuit breaker backoff.
///
/// This helper encapsulates the pattern of repeatedly calling `ready()` on a
/// service while handling `CircuitOpen` errors with a fixed 1-second backoff and
/// cancellation checks. It returns `Ok(())` when the service is ready, or
/// `Err(e)` if cancellation occurred or a fatal error was encountered.
async fn ready_with_backoff(
    pipeline: &mut BoxProcessor,
    cancel: &CancellationToken,
) -> Result<(), CamelError> {
    loop {
        match pipeline.ready().await {
            Ok(_) => return Ok(()),
            Err(CamelError::CircuitOpen(ref msg)) => {
                warn!("Circuit open, backing off: {msg}");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
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

/// Default implementation of [`RouteController`].
///
/// Manages route lifecycle with support for:
/// - Starting/stopping individual routes
/// - Suspending and resuming routes
/// - Auto-startup with startup ordering
/// - Graceful shutdown
pub struct DefaultRouteController {
    /// Routes indexed by route ID.
    routes: HashMap<String, ManagedRoute>,
    /// Reference to the component registry for resolving endpoints.
    registry: Arc<std::sync::Mutex<Registry>>,
    /// Self-reference for creating ProducerContext.
    /// Set after construction via `set_self_ref()`.
    self_ref: Option<Arc<Mutex<dyn RouteController>>>,
    /// Optional global error handler applied to all routes without a per-route handler.
    global_error_handler: Option<ErrorHandlerConfig>,
    /// Optional crash notifier for supervision.
    crash_notifier: Option<mpsc::Sender<CrashNotification>>,
}

impl DefaultRouteController {
    /// Create a new `DefaultRouteController` with the given registry.
    pub fn new(registry: Arc<std::sync::Mutex<Registry>>) -> Self {
        Self {
            routes: HashMap::new(),
            registry,
            self_ref: None,
            global_error_handler: None,
            crash_notifier: None,
        }
    }

    /// Set the self-reference for creating ProducerContext.
    ///
    /// This must be called after wrapping the controller in `Arc<Mutex<>>`.
    pub fn set_self_ref(&mut self, self_ref: Arc<Mutex<dyn RouteController>>) {
        self.self_ref = Some(self_ref);
    }

    /// Get the self-reference, if set.
    ///
    /// Used by [`SupervisingRouteController`](crate::supervising_route_controller::SupervisingRouteController)
    /// to spawn the supervision loop.
    pub fn self_ref_for_supervision(&self) -> Option<Arc<Mutex<dyn RouteController>>> {
        self.self_ref.clone()
    }

    /// Set the crash notifier for supervision.
    ///
    /// When set, the controller will send a [`CrashNotification`] whenever
    /// a consumer crashes.
    pub fn set_crash_notifier(&mut self, tx: mpsc::Sender<CrashNotification>) {
        self.crash_notifier = Some(tx);
    }

    /// Set a global error handler applied to all routes without a per-route handler.
    pub fn set_error_handler(&mut self, config: ErrorHandlerConfig) {
        self.global_error_handler = Some(config);
    }

    /// Resolve an `ErrorHandlerConfig` into an `ErrorHandlerLayer`.
    fn resolve_error_handler(
        &self,
        config: ErrorHandlerConfig,
        producer_ctx: &ProducerContext,
        registry: &Registry,
    ) -> Result<ErrorHandlerLayer, CamelError> {
        // Resolve DLC URI → producer.
        let dlc_producer = if let Some(ref uri) = config.dlc_uri {
            let parsed = parse_uri(uri)?;
            let component = registry.get_or_err(&parsed.scheme)?;
            let endpoint = component.create_endpoint(uri)?;
            Some(endpoint.create_producer(producer_ctx)?)
        } else {
            None
        };

        // Resolve per-policy `handled_by` URIs.
        let mut resolved_policies = Vec::new();
        for policy in config.policies {
            let handler_producer = if let Some(ref uri) = policy.handled_by {
                let parsed = parse_uri(uri)?;
                let component = registry.get_or_err(&parsed.scheme)?;
                let endpoint = component.create_endpoint(uri)?;
                Some(endpoint.create_producer(producer_ctx)?)
            } else {
                None
            };
            resolved_policies.push((policy, handler_producer));
        }

        Ok(ErrorHandlerLayer::new(dlc_producer, resolved_policies))
    }

    /// Resolve BuilderSteps into BoxProcessors.
    fn resolve_steps(
        &self,
        steps: Vec<BuilderStep>,
        producer_ctx: &ProducerContext,
        registry: &Registry,
    ) -> Result<Vec<BoxProcessor>, CamelError> {
        let mut processors: Vec<BoxProcessor> = Vec::new();
        for step in steps {
            match step {
                BuilderStep::Processor(svc) => {
                    processors.push(svc);
                }
                BuilderStep::To(uri) => {
                    let parsed = parse_uri(&uri)?;
                    let component = registry.get_or_err(&parsed.scheme)?;
                    let endpoint = component.create_endpoint(&uri)?;
                    let producer = endpoint.create_producer(producer_ctx)?;
                    processors.push(producer);
                }
                BuilderStep::Split { config, steps } => {
                    let sub_processors = self.resolve_steps(steps, producer_ctx, registry)?;
                    let sub_pipeline = compose_pipeline(sub_processors);
                    let splitter =
                        camel_processor::splitter::SplitterService::new(config, sub_pipeline);
                    processors.push(BoxProcessor::new(splitter));
                }
                BuilderStep::Aggregate { config } => {
                    let svc = camel_processor::AggregatorService::new(config);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Filter { predicate, steps } => {
                    let sub_processors = self.resolve_steps(steps, producer_ctx, registry)?;
                    let sub_pipeline = compose_pipeline(sub_processors);
                    let svc =
                        camel_processor::FilterService::from_predicate(predicate, sub_pipeline);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Choice { whens, otherwise } => {
                    // Resolve each when clause's sub-steps into a pipeline.
                    let mut when_clauses = Vec::new();
                    for when_step in whens {
                        let sub_processors =
                            self.resolve_steps(when_step.steps, producer_ctx, registry)?;
                        let pipeline = compose_pipeline(sub_processors);
                        when_clauses.push(WhenClause {
                            predicate: when_step.predicate,
                            pipeline,
                        });
                    }
                    // Resolve otherwise branch (if present).
                    let otherwise_pipeline = if let Some(otherwise_steps) = otherwise {
                        let sub_processors =
                            self.resolve_steps(otherwise_steps, producer_ctx, registry)?;
                        Some(compose_pipeline(sub_processors))
                    } else {
                        None
                    };
                    let svc = ChoiceService::new(when_clauses, otherwise_pipeline);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::WireTap { uri } => {
                    let parsed = parse_uri(&uri)?;
                    let component = registry.get_or_err(&parsed.scheme)?;
                    let endpoint = component.create_endpoint(&uri)?;
                    let producer = endpoint.create_producer(producer_ctx)?;
                    let svc = camel_processor::WireTapService::new(producer);
                    processors.push(BoxProcessor::new(svc));
                }
                BuilderStep::Multicast { config, steps } => {
                    // Each top-level step in the multicast scope becomes an independent endpoint.
                    let mut endpoints = Vec::new();
                    for step in steps {
                        let sub_processors =
                            self.resolve_steps(vec![step], producer_ctx, registry)?;
                        let endpoint = compose_pipeline(sub_processors);
                        endpoints.push(endpoint);
                    }
                    let svc = camel_processor::MulticastService::new(endpoints, config);
                    processors.push(BoxProcessor::new(svc));
                }
            }
        }
        Ok(processors)
    }

    /// Add a route definition to the controller.
    ///
    /// Steps are resolved immediately using the registry.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - A route with the same ID already exists
    /// - Step resolution fails
    pub fn add_route(&mut self, definition: RouteDefinition) -> Result<(), CamelError> {
        let route_id = definition.route_id().to_string();

        if self.routes.contains_key(&route_id) {
            return Err(CamelError::RouteError(format!(
                "Route '{}' already exists",
                route_id
            )));
        }

        info!(route_id = %route_id, "Adding route to controller");

        // Extract definition info for storage before steps are consumed
        let definition_info = definition.to_info();
        let from_uri = definition.from_uri.to_string();
        let concurrency = definition.concurrency;

        // Create ProducerContext from self_ref for step resolution
        let producer_ctx = self
            .self_ref
            .clone()
            .map(ProducerContext::new)
            .ok_or_else(|| CamelError::RouteError("RouteController self_ref not set".into()))?;

        // Lock registry for step resolution
        let registry = self
            .registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");

        // Resolve steps into processors (takes ownership of steps)
        let processors = self.resolve_steps(definition.steps, &producer_ctx, &registry)?;
        let mut pipeline = compose_pipeline(processors);

        // Apply circuit breaker if configured
        if let Some(cb_config) = definition.circuit_breaker {
            let cb_layer = CircuitBreakerLayer::new(cb_config);
            pipeline = BoxProcessor::new(cb_layer.layer(pipeline));
        }

        // Determine which error handler config to use (per-route takes precedence)
        let eh_config = definition
            .error_handler
            .or_else(|| self.global_error_handler.clone());

        if let Some(config) = eh_config {
            let layer = self.resolve_error_handler(config, &producer_ctx, &registry)?;
            pipeline = BoxProcessor::new(layer.layer(pipeline));
        }

        // Drop the lock before modifying self.routes
        drop(registry);

        self.routes.insert(
            route_id.clone(),
            ManagedRoute {
                definition: definition_info,
                from_uri,
                pipeline: Arc::new(ArcSwap::from_pointee(SyncBoxProcessor(pipeline))),
                concurrency,
                status: Arc::new(std::sync::Mutex::new(RouteStatus::Stopped)),
                consumer_handle: None,
                pipeline_handle: None,
                cancel_token: CancellationToken::new(),
            },
        );

        Ok(())
    }

    /// Returns the number of routes in the controller.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Returns all route IDs.
    pub fn route_ids(&self) -> Vec<String> {
        self.routes.keys().cloned().collect()
    }

    /// Atomically swap the pipeline of a route.
    ///
    /// In-flight requests finish with the old pipeline (kept alive by Arc).
    /// New requests immediately use the new pipeline.
    pub fn swap_pipeline(
        &self,
        route_id: &str,
        new_pipeline: BoxProcessor,
    ) -> Result<(), CamelError> {
        let managed = self
            .routes
            .get(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        managed
            .pipeline
            .store(Arc::new(SyncBoxProcessor(new_pipeline)));
        info!(route_id = %route_id, "Pipeline swapped atomically");
        Ok(())
    }

    /// Returns the from_uri of a route, if it exists.
    pub fn route_from_uri(&self, route_id: &str) -> Option<String> {
        self.routes.get(route_id).map(|r| r.from_uri.clone())
    }

    /// Get a clone of the current pipeline for a route.
    ///
    /// This is useful for testing and introspection.
    /// Returns `None` if the route doesn't exist.
    pub fn get_pipeline(&self, route_id: &str) -> Option<BoxProcessor> {
        self.routes
            .get(route_id)
            .map(|r| r.pipeline.load().0.clone())
    }

    /// Internal stop implementation that can set custom status.
    async fn stop_route_internal(&mut self, route_id: &str) -> Result<(), CamelError> {
        let managed = self
            .routes
            .get_mut(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        let current_status = managed
            .status
            .lock()
            .expect("status mutex poisoned")
            .clone();
        if current_status != RouteStatus::Started && current_status != RouteStatus::Suspended {
            return Ok(()); // Already stopped or stopping
        }

        info!(route_id = %route_id, "Stopping route");
        *managed.status.lock().expect("status mutex poisoned") = RouteStatus::Stopping;

        // Cancel the token to signal shutdown
        managed.cancel_token.cancel();

        // Take handles directly (no Arc<Mutex> wrapper needed)
        let consumer_handle = managed.consumer_handle.take();
        let pipeline_handle = managed.pipeline_handle.take();

        // Wait for tasks to complete with timeout
        // The CancellationToken already signaled tasks to stop gracefully.
        // If timeout fires, log a warning — tasks will stop on their own when
        // they check the cancel token. This is standard Tokio shutdown practice.
        let timeout_result = tokio::time::timeout(Duration::from_secs(30), async {
            match (consumer_handle, pipeline_handle) {
                (Some(c), Some(p)) => {
                    let _ = tokio::join!(c, p);
                }
                (Some(c), None) => {
                    let _ = c.await;
                }
                (None, Some(p)) => {
                    let _ = p.await;
                }
                (None, None) => {}
            }
        })
        .await;

        if timeout_result.is_err() {
            warn!(route_id = %route_id, "Route shutdown timed out after 30s — tasks may still be running");
        }

        // Get the managed route again (can't hold across await)
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");

        // Create a fresh cancellation token for next start
        managed.cancel_token = CancellationToken::new();
        *managed.status.lock().expect("status mutex poisoned") = RouteStatus::Stopped;

        info!(route_id = %route_id, "Route stopped");
        Ok(())
    }
}

#[async_trait::async_trait]
impl RouteController for DefaultRouteController {
    async fn start_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        // Check if route exists and can be started, and update status atomically
        {
            let managed = self
                .routes
                .get_mut(route_id)
                .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

            let current_status = managed
                .status
                .lock()
                .expect("status mutex poisoned")
                .clone();
            match current_status {
                RouteStatus::Started => return Ok(()), // Already running
                RouteStatus::Starting => {
                    return Err(CamelError::RouteError(format!(
                        "Route '{}' is already starting",
                        route_id
                    )));
                }
                RouteStatus::Stopped | RouteStatus::Failed(_) => {} // OK to start
                RouteStatus::Stopping => {
                    return Err(CamelError::RouteError(format!(
                        "Route '{}' is stopping",
                        route_id
                    )));
                }
                RouteStatus::Suspended => {} // OK to resume
            }
            *managed.status.lock().expect("status mutex poisoned") = RouteStatus::Starting;
        }

        info!(route_id = %route_id, "Starting route");

        // Get the resolved route info
        let (from_uri, pipeline, concurrency, status_for_consumer) = {
            let managed = self
                .routes
                .get(route_id)
                .expect("invariant: route must exist after prior existence check");
            (
                managed.from_uri.clone(),
                Arc::clone(&managed.pipeline),
                managed.concurrency.clone(),
                Arc::clone(&managed.status),
            )
        };

        // Clone crash notifier for consumer task
        let crash_notifier = self.crash_notifier.clone();

        // Parse from URI and create consumer (lock registry for lookup)
        let parsed = parse_uri(&from_uri)?;
        let registry = self
            .registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");
        let component = registry.get_or_err(&parsed.scheme)?;
        let endpoint = component.create_endpoint(&from_uri)?;
        let mut consumer = endpoint.create_consumer()?;
        let consumer_concurrency = consumer.concurrency_model();
        // Drop the lock before spawning tasks
        drop(registry);

        // Resolve effective concurrency: route override > consumer default
        let effective_concurrency = concurrency.unwrap_or(consumer_concurrency);

        // Get the managed route for mutation
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");

        // Create channel for consumer to send exchanges
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(256);
        let child_token = managed.cancel_token.child_token();
        let consumer_ctx = ConsumerContext::new(tx, child_token.clone());

        // Start consumer in background task.
        // Status is shared via Arc<Mutex<>> so the task can update it on crash.
        let route_id_for_consumer = route_id.to_string();
        let consumer_handle = tokio::spawn(async move {
            if let Err(e) = consumer.start(consumer_ctx).await {
                error!(route_id = %route_id_for_consumer, "Consumer error: {e}");
                let error_msg = e.to_string();
                *status_for_consumer.lock().expect("status mutex poisoned") =
                    RouteStatus::Failed(error_msg.clone());

                // Send crash notification if notifier is configured
                if let Some(tx) = crash_notifier {
                    let _ = tx
                        .send(CrashNotification {
                            route_id: route_id_for_consumer.clone(),
                            error: error_msg,
                        })
                        .await;
                }
            }
        });

        // Spawn pipeline task
        let pipeline_cancel = child_token;
        let pipeline_handle = match effective_concurrency {
            ConcurrencyModel::Sequential => {
                tokio::spawn(async move {
                    while let Some(envelope) = rx.recv().await {
                        let ExchangeEnvelope { exchange, reply_tx } = envelope;

                        // Load current pipeline from ArcSwap (picks up hot-reloaded pipelines)
                        let mut pipeline = pipeline.load().0.clone();

                        if let Err(e) = ready_with_backoff(&mut pipeline, &pipeline_cancel).await {
                            if let Some(tx) = reply_tx {
                                let _ = tx.send(Err(e));
                            }
                            return;
                        }

                        let result = pipeline.call(exchange).await;
                        if let Some(tx) = reply_tx {
                            let _ = tx.send(result);
                        } else if let Err(ref e) = result
                            && !matches!(e, CamelError::Stopped)
                        {
                            error!("Pipeline error: {e}");
                        }
                    }
                })
            }
            ConcurrencyModel::Concurrent { max } => {
                let sem = max.map(|n| Arc::new(tokio::sync::Semaphore::new(n)));
                tokio::spawn(async move {
                    while let Some(envelope) = rx.recv().await {
                        let ExchangeEnvelope { exchange, reply_tx } = envelope;
                        let pipe_ref = Arc::clone(&pipeline);
                        let sem = sem.clone();
                        let cancel = pipeline_cancel.clone();
                        tokio::spawn(async move {
                            // Acquire semaphore permit if bounded
                            let _permit = match &sem {
                                Some(s) => Some(s.acquire().await.expect("semaphore closed")),
                                None => None,
                            };

                            // Load current pipeline from ArcSwap
                            let mut pipe = pipe_ref.load().0.clone();

                            // Wait for service ready with circuit breaker backoff
                            if let Err(e) = ready_with_backoff(&mut pipe, &cancel).await {
                                if let Some(tx) = reply_tx {
                                    let _ = tx.send(Err(e));
                                }
                                return;
                            }

                            let result = pipe.call(exchange).await;
                            if let Some(tx) = reply_tx {
                                let _ = tx.send(result);
                            } else if let Err(ref e) = result
                                && !matches!(e, CamelError::Stopped)
                            {
                                error!("Pipeline error: {e}");
                            }
                        });
                    }
                })
            }
        };

        // Store handles and update status
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");
        managed.consumer_handle = Some(consumer_handle);
        managed.pipeline_handle = Some(pipeline_handle);
        // Only mark as Started if consumer hasn't already crashed
        let mut status_guard = managed.status.lock().expect("status mutex poisoned");
        if !matches!(*status_guard, RouteStatus::Failed(_)) {
            *status_guard = RouteStatus::Started;
        }
        drop(status_guard);

        info!(route_id = %route_id, "Route started");
        Ok(())
    }

    async fn stop_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route_internal(route_id).await
    }

    async fn restart_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route(route_id).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.start_route(route_id).await
    }

    async fn suspend_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route_internal(route_id).await?;
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check");
        *managed.status.lock().expect("status mutex poisoned") = RouteStatus::Suspended;
        info!(route_id = %route_id, "Route suspended");
        Ok(())
    }

    async fn resume_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        // Resume only if Suspended
        let is_suspended = self
            .routes
            .get(route_id)
            .map(|r| *r.status.lock().expect("status mutex poisoned") == RouteStatus::Suspended)
            .unwrap_or(false);

        if !is_suspended {
            return Err(CamelError::RouteError(format!(
                "Route '{}' is not suspended",
                route_id
            )));
        }

        self.start_route(route_id).await
    }

    fn route_status(&self, route_id: &str) -> Option<RouteStatus> {
        self.routes
            .get(route_id)
            .map(|r| r.status.lock().expect("status mutex poisoned").clone())
    }

    async fn start_all_routes(&mut self) -> Result<(), CamelError> {
        // Only start routes where auto_startup() == true
        // Sort by startup_order() ascending before starting
        let route_ids: Vec<String> = {
            let mut pairs: Vec<_> = self
                .routes
                .iter()
                .filter(|(_, r)| r.definition.auto_startup())
                .map(|(id, r)| (id.clone(), r.definition.startup_order()))
                .collect();
            pairs.sort_by_key(|(_, order)| *order);
            pairs.into_iter().map(|(id, _)| id).collect()
        };

        info!("Starting {} auto-startup routes", route_ids.len());

        // Collect errors but continue starting remaining routes
        let mut errors: Vec<String> = Vec::new();
        for route_id in route_ids {
            if let Err(e) = self.start_route(&route_id).await {
                errors.push(format!("Route '{}': {}", route_id, e));
            }
        }

        if !errors.is_empty() {
            return Err(CamelError::RouteError(format!(
                "Failed to start routes: {}",
                errors.join(", ")
            )));
        }

        info!("All auto-startup routes started");
        Ok(())
    }

    async fn stop_all_routes(&mut self) -> Result<(), CamelError> {
        // Sort by startup_order descending (reverse order)
        let route_ids: Vec<String> = {
            let mut pairs: Vec<_> = self
                .routes
                .iter()
                .map(|(id, r)| (id.clone(), r.definition.startup_order()))
                .collect();
            pairs.sort_by_key(|(_, order)| std::cmp::Reverse(*order));
            pairs.into_iter().map(|(id, _)| id).collect()
        };

        info!("Stopping {} routes", route_ids.len());

        for route_id in route_ids {
            let _ = self.stop_route(&route_id).await;
        }

        info!("All routes stopped");
        Ok(())
    }
}

impl RouteControllerInternal for DefaultRouteController {
    fn add_route(&mut self, def: RouteDefinition) -> Result<(), CamelError> {
        DefaultRouteController::add_route(self, def)
    }

    fn swap_pipeline(&self, route_id: &str, pipeline: BoxProcessor) -> Result<(), CamelError> {
        DefaultRouteController::swap_pipeline(self, route_id, pipeline)
    }

    fn route_from_uri(&self, route_id: &str) -> Option<String> {
        // Call the inherent method which now returns Option<String>
        DefaultRouteController::route_from_uri(self, route_id)
    }

    fn set_error_handler(&mut self, config: ErrorHandlerConfig) {
        DefaultRouteController::set_error_handler(self, config)
    }

    fn set_self_ref(&mut self, self_ref: Arc<Mutex<dyn RouteController>>) {
        DefaultRouteController::set_self_ref(self, self_ref)
    }

    fn route_count(&self) -> usize {
        DefaultRouteController::route_count(self)
    }

    fn route_ids(&self) -> Vec<String> {
        DefaultRouteController::route_ids(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_route_controller_internal_is_object_safe() {
        // Verifies the trait compiles as a trait object.
        // If RouteControllerInternal is not object-safe, this test fails to compile.
        let _: Option<Box<dyn RouteControllerInternal>> = None;
    }

    #[tokio::test]
    async fn test_swap_pipeline_updates_stored_pipeline() {
        use camel_api::IdentityProcessor;

        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let mut controller = DefaultRouteController::new(registry);

        let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
        ));
        controller.set_self_ref(controller_arc);

        let definition =
            crate::route::RouteDefinition::new("timer:tick", vec![]).with_route_id("swap-test");
        controller.add_route(definition).unwrap();

        // Swap pipeline should succeed
        let new_pipeline = BoxProcessor::new(IdentityProcessor);
        let result = controller.swap_pipeline("swap-test", new_pipeline);
        assert!(result.is_ok());

        // Swap on non-existent route should fail
        let new_pipeline = BoxProcessor::new(IdentityProcessor);
        let result = controller.swap_pipeline("nonexistent", new_pipeline);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_route_duplicate_id_fails() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let mut controller = DefaultRouteController::new(registry);

        // Set self_ref to avoid error during add_route
        let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
        ));
        controller.set_self_ref(controller_arc);

        let definition = crate::route::RouteDefinition::new("timer:tick", vec![])
            .with_route_id("duplicate-route");
        assert!(controller.add_route(definition).is_ok());

        // Adding a route with the same ID should fail
        let definition2 = crate::route::RouteDefinition::new("timer:tock", vec![])
            .with_route_id("duplicate-route");
        let result = controller.add_route(definition2);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("already exists"),
            "error should mention 'already exists', got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_add_route_with_id_succeeds() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let mut controller = DefaultRouteController::new(registry);

        // Set self_ref to avoid error during add_route
        let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
        ));
        controller.set_self_ref(controller_arc);

        let definition =
            crate::route::RouteDefinition::new("timer:tick", vec![]).with_route_id("test-route");
        assert!(controller.add_route(definition).is_ok());
        assert_eq!(controller.route_count(), 1);
    }

    #[tokio::test]
    async fn test_crashed_consumer_sets_failed_status() {
        use async_trait::async_trait;
        use camel_api::{CamelError, RouteStatus};
        use camel_component::{ConcurrencyModel, ConsumerContext, Endpoint};

        struct CrashingConsumer;
        #[async_trait]
        impl camel_component::Consumer for CrashingConsumer {
            async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
                Err(CamelError::RouteError("boom".into()))
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn concurrency_model(&self) -> ConcurrencyModel {
                ConcurrencyModel::Sequential
            }
        }
        struct CrashingEndpoint;
        impl Endpoint for CrashingEndpoint {
            fn uri(&self) -> &str {
                "crash:test"
            }
            fn create_consumer(&self) -> Result<Box<dyn camel_component::Consumer>, CamelError> {
                Ok(Box::new(CrashingConsumer))
            }
            fn create_producer(
                &self,
                _ctx: &camel_api::ProducerContext,
            ) -> Result<camel_api::BoxProcessor, CamelError> {
                Err(CamelError::RouteError("no producer".into()))
            }
        }
        struct CrashingComponent;
        impl camel_component::Component for CrashingComponent {
            fn scheme(&self) -> &str {
                "crash"
            }
            fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
                let _ = uri; // satisfy unused variable warning
                Ok(Box::new(CrashingEndpoint))
            }
        }

        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry.lock().unwrap().register(CrashingComponent);
        let mut controller = DefaultRouteController::new(Arc::clone(&registry));
        let self_ref: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::clone(&registry)),
        ));
        controller.set_self_ref(self_ref);

        let def =
            crate::route::RouteDefinition::new("crash:test", vec![]).with_route_id("crash-route");
        controller.add_route(def).unwrap();
        controller.start_route("crash-route").await.unwrap();

        // Give consumer task time to crash and update status
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let status = controller.route_status("crash-route").unwrap();
        assert!(
            matches!(status, RouteStatus::Failed(_)),
            "expected Failed, got {:?}",
            status
        );
    }
}
