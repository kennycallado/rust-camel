//! Supervising route controller with automatic crash recovery.
//!
//! This module provides [`SupervisingRouteController`], which wraps a
//! [`DefaultRouteController`] and monitors crashed routes, restarting them
//! with exponential backoff based on [`SupervisionConfig`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tokio::sync::Mutex;
use tracing::{error, info, warn};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{
    CamelError, MetricsCollector, RouteController, RuntimeCommand, RuntimeHandle, RuntimeQuery,
    RuntimeQueryResult, SupervisionConfig,
};

use crate::lifecycle::adapters::route_controller::{
    CrashNotification, DefaultRouteController, RouteControllerInternal, SharedLanguageRegistry,
};
use crate::lifecycle::application::route_definition::RouteDefinition;
use crate::shared::components::domain::Registry;

/// A route controller that automatically restarts crashed routes.
///
/// Wraps a [`DefaultRouteController`] and spawns a supervision loop that
/// receives crash notifications and restarts routes with exponential backoff.
pub struct SupervisingRouteController {
    /// The inner controller that manages actual routes.
    inner: DefaultRouteController,
    /// Supervision configuration.
    config: SupervisionConfig,
    /// Sender for crash notifications (cloned to inner controller).
    crash_tx: tokio::sync::mpsc::Sender<CrashNotification>,
    /// Receiver for crash notifications (taken when supervision loop starts).
    crash_rx: Option<tokio::sync::mpsc::Receiver<CrashNotification>>,
    /// Optional metrics collector.
    metrics: Option<Arc<dyn MetricsCollector>>,
}

static SUPERVISION_COMMAND_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_supervision_command_id(op: &str, route_id: &str) -> String {
    let seq = SUPERVISION_COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("supervision:{op}:{route_id}:{seq}")
}

impl SupervisingRouteController {
    /// Create a new supervising controller.
    pub fn new(registry: Arc<std::sync::Mutex<Registry>>, config: SupervisionConfig) -> Self {
        Self::with_languages(
            registry,
            config,
            Arc::new(std::sync::Mutex::new(HashMap::new())),
        )
    }

    /// Create a new supervising controller with shared language registry.
    pub fn with_languages(
        registry: Arc<std::sync::Mutex<Registry>>,
        config: SupervisionConfig,
        languages: SharedLanguageRegistry,
    ) -> Self {
        let (crash_tx, crash_rx) = tokio::sync::mpsc::channel(64);
        Self {
            inner: DefaultRouteController::with_languages(registry, languages),
            config,
            crash_tx,
            crash_rx: Some(crash_rx),
            metrics: None,
        }
    }

    /// Set a metrics collector for the supervision loop.
    pub fn with_metrics(mut self, metrics: Arc<dyn MetricsCollector>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    fn ensure_supervision_loop_started(&mut self) {
        self.inner.set_crash_notifier(self.crash_tx.clone());

        if self.crash_rx.is_none() {
            return;
        }

        let rx = self
            .crash_rx
            .take()
            .expect("crash_rx checked as Some above");
        let config = self.config.clone();
        let metrics = self.metrics.clone();
        let runtime = self.inner.runtime_handle_for_supervision();
        tokio::spawn(async move {
            supervision_loop(rx, runtime, config, metrics).await;
        });
    }
}

#[async_trait::async_trait]
impl RouteController for SupervisingRouteController {
    async fn start_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.ensure_supervision_loop_started();
        self.inner.start_route(route_id).await
    }

    async fn stop_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.inner.stop_route(route_id).await
    }

    async fn restart_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.inner.restart_route(route_id).await
    }

    async fn suspend_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.inner.suspend_route(route_id).await
    }

    async fn resume_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.inner.resume_route(route_id).await
    }

    async fn start_all_routes(&mut self) -> Result<(), CamelError> {
        self.ensure_supervision_loop_started();
        self.inner.start_all_routes().await
    }

    async fn stop_all_routes(&mut self) -> Result<(), CamelError> {
        self.inner.stop_all_routes().await
    }
}

#[async_trait::async_trait]
impl RouteControllerInternal for SupervisingRouteController {
    fn add_route(&mut self, def: RouteDefinition) -> Result<(), CamelError> {
        self.inner.add_route(def)
    }

    fn swap_pipeline(
        &self,
        route_id: &str,
        pipeline: camel_api::BoxProcessor,
    ) -> Result<(), CamelError> {
        self.inner.swap_pipeline(route_id, pipeline)
    }

    fn route_from_uri(&self, route_id: &str) -> Option<String> {
        self.inner.route_from_uri(route_id)
    }

    fn set_error_handler(&mut self, config: ErrorHandlerConfig) {
        self.inner.set_error_handler(config)
    }

    fn set_self_ref(&mut self, self_ref: Arc<Mutex<dyn RouteController>>) {
        self.inner.set_self_ref(self_ref)
    }

    fn set_runtime_handle(&mut self, runtime: Arc<dyn RuntimeHandle>) {
        self.inner.set_runtime_handle(runtime)
    }

    fn route_count(&self) -> usize {
        self.inner.route_count()
    }

    fn route_ids(&self) -> Vec<String> {
        self.inner.route_ids()
    }

    fn in_flight_count(&self, route_id: &str) -> Option<u64> {
        self.inner.in_flight_count(route_id)
    }

    fn route_exists(&self, route_id: &str) -> bool {
        self.inner.route_exists(route_id)
    }

    fn auto_startup_route_ids(&self) -> Vec<String> {
        self.inner.auto_startup_route_ids()
    }

    fn shutdown_route_ids(&self) -> Vec<String> {
        self.inner.shutdown_route_ids()
    }

    fn set_tracer_config(&mut self, config: &crate::shared::observability::domain::TracerConfig) {
        self.inner.set_tracer_config(config)
    }

    fn compile_route_definition(
        &self,
        def: RouteDefinition,
    ) -> Result<camel_api::BoxProcessor, camel_api::CamelError> {
        self.inner.compile_route_definition(def)
    }

    fn remove_route(&mut self, route_id: &str) -> Result<(), camel_api::CamelError> {
        self.inner.remove_route(route_id)
    }

    async fn start_route_reload(&mut self, route_id: &str) -> Result<(), camel_api::CamelError> {
        self.ensure_supervision_loop_started();
        self.inner.start_route(route_id).await
    }

    async fn stop_route_reload(&mut self, route_id: &str) -> Result<(), camel_api::CamelError> {
        self.inner.stop_route(route_id).await
    }
}

/// Supervision loop that restarts crashed routes.
///
/// Receives crash notifications and restarts routes with exponential backoff.
/// Tracks attempt counts per route and respects `max_attempts` from config.
async fn supervision_loop(
    mut rx: tokio::sync::mpsc::Receiver<CrashNotification>,
    runtime: Option<Arc<dyn RuntimeHandle>>,
    config: SupervisionConfig,
    _metrics: Option<Arc<dyn MetricsCollector>>,
) {
    let mut attempts: HashMap<String, u32> = HashMap::new();
    let mut last_restart_time: HashMap<String, Instant> = HashMap::new();
    let mut currently_restarting: HashSet<String> = HashSet::new();

    info!("Supervision loop started");

    while let Some(notification) = rx.recv().await {
        let route_id = notification.route_id.clone();
        let error = &notification.error;

        // Skip if already processing a restart for this route
        if currently_restarting.contains(&route_id) {
            continue;
        }

        info!(
            route_id = %route_id,
            error = %error,
            "Route crashed, checking restart policy"
        );

        // Reset attempt counter if route ran long enough before crashing
        // A route that runs for >= initial_delay is considered a "successful run"
        if let Some(last_time) = last_restart_time.get(&route_id)
            && last_time.elapsed() >= config.initial_delay
        {
            attempts.insert(route_id.clone(), 0);
        }

        // Increment attempt counter
        let current_attempt = attempts.entry(route_id.clone()).or_insert(0);
        *current_attempt += 1;

        // Check max attempts (collapse nested if-let)
        if config
            .max_attempts
            .is_some_and(|max| *current_attempt > max)
        {
            error!(
                route_id = %route_id,
                attempts = current_attempt,
                max = config.max_attempts.unwrap(),
                "Route exceeded max restart attempts, giving up"
            );
            continue;
        }

        // Compute delay with exponential backoff
        let delay = config.next_delay(*current_attempt);
        info!(
            route_id = %route_id,
            attempt = current_attempt,
            delay_ms = delay.as_millis(),
            "Scheduling route restart"
        );

        // Mark as currently being processed
        currently_restarting.insert(route_id.clone());

        // Sleep before restart
        tokio::time::sleep(delay).await;

        let Some(runtime) = &runtime else {
            warn!(
                route_id = %route_id,
                "Runtime handle unavailable, supervision restart skipped"
            );
            currently_restarting.remove(&route_id);
            continue;
        };

        // If runtime lifecycle has already been intentionally transitioned to a
        // non-running state, supervision must not revive it.
        let pre_status = match runtime
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: route_id.clone(),
            })
            .await
        {
            Ok(RuntimeQueryResult::RouteStatus { status, .. }) => status,
            Ok(other) => {
                warn!(
                    route_id = %route_id,
                    ?other,
                    "Unexpected runtime query result, skipping supervision restart"
                );
                currently_restarting.remove(&route_id);
                continue;
            }
            Err(err) => {
                warn!(
                    route_id = %route_id,
                    error = %err,
                    "Runtime status query failed, skipping supervision restart"
                );
                currently_restarting.remove(&route_id);
                continue;
            }
        };

        if matches!(pre_status.as_str(), "Registered" | "Stopped") {
            warn!(
                route_id = %route_id,
                status = %pre_status,
                "Runtime lifecycle is non-running; supervision restart skipped"
            );
            attempts.remove(&route_id);
            currently_restarting.remove(&route_id);
            continue;
        }

        // Record crash in runtime state first so the read-model remains authoritative
        // for subsequent restart decisions.
        if let Err(err) = runtime
            .execute(RuntimeCommand::FailRoute {
                route_id: route_id.clone(),
                error: error.clone(),
                command_id: next_supervision_command_id("fail", &route_id),
                causation_id: None,
            })
            .await
        {
            warn!(
                route_id = %route_id,
                error = %err,
                "Failed to persist crash state in runtime before restart check"
            );
        }

        // Check current status before restarting using runtime read-model only.
        let should_restart = match runtime
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: route_id.clone(),
            })
            .await
        {
            Ok(RuntimeQueryResult::RouteStatus { status, .. }) if status == "Failed" => true,
            Ok(RuntimeQueryResult::RouteStatus { status, .. }) => {
                warn!(
                    route_id = %route_id,
                    status = %status,
                    "Route no longer failed in runtime projection, skipping supervision restart"
                );
                attempts.remove(&route_id);
                false
            }
            Ok(other) => {
                warn!(
                    route_id = %route_id,
                    ?other,
                    "Unexpected runtime query result, skipping supervision restart"
                );
                false
            }
            Err(err) => {
                warn!(
                    route_id = %route_id,
                    error = %err,
                    "Runtime status query failed, skipping supervision restart"
                );
                false
            }
        };

        if should_restart {
            let restart_result = runtime
                .execute(RuntimeCommand::ReloadRoute {
                    route_id: route_id.clone(),
                    command_id: next_supervision_command_id("reload", &route_id),
                    causation_id: None,
                })
                .await
                .map(|_| ());

            match restart_result {
                Ok(()) => {
                    info!(route_id = %route_id, "Route restarted successfully");
                    // Record restart time instead of resetting attempts.
                    // The counter will be reset on next crash if route ran long enough.
                    last_restart_time.insert(route_id.clone(), Instant::now());
                }
                Err(e) => {
                    error!(route_id = %route_id, error = %e, "Failed to restart route");
                }
            }
        }

        // No longer processing this route
        currently_restarting.remove(&route_id);
    }

    info!("Supervision loop ended");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::adapters::{InMemoryRuntimeStore, RuntimeExecutionAdapter};
    use crate::lifecycle::application::runtime_bus::RuntimeBus;
    use crate::lifecycle::ports::RouteRegistrationPort as InternalRuntimeCommandBus;
    use async_trait::async_trait;
    use camel_api::RuntimeQueryBus;
    use camel_component::{Component, ConcurrencyModel, Consumer, ConsumerContext, Endpoint};
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    async fn attach_runtime_bus(
        controller: &StdArc<Mutex<dyn RouteControllerInternal>>,
    ) -> StdArc<RuntimeBus> {
        let store = InMemoryRuntimeStore::default();
        let runtime = StdArc::new(
            RuntimeBus::new(
                StdArc::new(store.clone()),
                StdArc::new(store.clone()),
                StdArc::new(store.clone()),
                StdArc::new(store.clone()),
            )
            .with_uow(StdArc::new(store))
            .with_execution(StdArc::new(RuntimeExecutionAdapter::new(StdArc::clone(
                controller,
            )))),
        );
        let runtime_handle: StdArc<dyn RuntimeHandle> = runtime.clone();
        controller.lock().await.set_runtime_handle(runtime_handle);
        runtime
    }

    #[test]
    fn supervision_command_id_is_unique_and_well_formed() {
        let id1 = next_supervision_command_id("start", "route-a");
        let id2 = next_supervision_command_id("start", "route-a");
        assert_ne!(id1, id2);
        assert!(id1.starts_with("supervision:start:route-a:"));
    }

    #[tokio::test]
    async fn supervision_loop_exits_cleanly_without_runtime_handle() {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let config = SupervisionConfig {
            max_attempts: Some(1),
            initial_delay: Duration::from_millis(5),
            backoff_multiplier: 1.0,
            max_delay: Duration::from_millis(5),
        };
        let handle = tokio::spawn(supervision_loop(rx, None, config, None));

        tx.send(CrashNotification {
            route_id: "r-no-runtime".into(),
            error: "boom".into(),
        })
        .await
        .unwrap();
        drop(tx);

        let join = tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("supervision loop should terminate");
        join.expect("supervision task should not panic");
    }

    #[test]
    fn with_metrics_stores_collector() {
        let registry = StdArc::new(std::sync::Mutex::new(Registry::new()));
        let controller = SupervisingRouteController::new(registry, SupervisionConfig::default())
            .with_metrics(StdArc::new(camel_api::NoOpMetrics));
        assert!(controller.metrics.is_some());
    }

    #[tokio::test]
    async fn ensure_supervision_loop_started_is_idempotent() {
        let registry = StdArc::new(std::sync::Mutex::new(Registry::new()));
        let mut controller =
            SupervisingRouteController::new(registry, SupervisionConfig::default());

        assert!(controller.crash_rx.is_some());
        controller.ensure_supervision_loop_started();
        assert!(controller.crash_rx.is_none());

        // Second call must be a no-op and keep receiver consumed.
        controller.ensure_supervision_loop_started();
        assert!(controller.crash_rx.is_none());
    }

    /// A consumer that crashes on first call, then blocks indefinitely.
    struct CrashThenBlockConsumer {
        call_count: StdArc<AtomicU32>,
    }

    #[async_trait]
    impl Consumer for CrashThenBlockConsumer {
        async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);

            if count == 0 {
                // First call: crash immediately
                return Err(CamelError::RouteError("simulated crash".into()));
            }

            // Second+ call: block until cancelled
            ctx.cancelled().await;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }

        fn concurrency_model(&self) -> ConcurrencyModel {
            ConcurrencyModel::Sequential
        }
    }

    struct CrashThenBlockEndpoint {
        call_count: StdArc<AtomicU32>,
    }

    impl Endpoint for CrashThenBlockEndpoint {
        fn uri(&self) -> &str {
            "crash-then-block:test"
        }

        fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
            Ok(Box::new(CrashThenBlockConsumer {
                call_count: StdArc::clone(&self.call_count),
            }))
        }

        fn create_producer(
            &self,
            _ctx: &camel_api::ProducerContext,
        ) -> Result<camel_api::BoxProcessor, CamelError> {
            Err(CamelError::RouteError("no producer".into()))
        }
    }

    struct CrashThenBlockComponent {
        call_count: StdArc<AtomicU32>,
    }

    impl Component for CrashThenBlockComponent {
        fn scheme(&self) -> &str {
            "crash-then-block"
        }

        fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(CrashThenBlockEndpoint {
                call_count: StdArc::clone(&self.call_count),
            }))
        }
    }

    #[tokio::test]
    async fn test_supervising_controller_restarts_crashed_route() {
        // Set up registry with crash-then-block component
        let registry = StdArc::new(std::sync::Mutex::new(Registry::new()));
        let call_count = StdArc::new(AtomicU32::new(0));
        registry.lock().unwrap().register(CrashThenBlockComponent {
            call_count: StdArc::clone(&call_count),
        });

        // Configure supervision with fast delays for testing
        let config = SupervisionConfig {
            max_attempts: Some(5),
            initial_delay: Duration::from_millis(50),
            backoff_multiplier: 1.0, // no growth
            max_delay: Duration::from_secs(60),
        };

        // Create supervising controller
        let controller: StdArc<Mutex<dyn RouteControllerInternal>> = StdArc::new(Mutex::new(
            SupervisingRouteController::new(StdArc::clone(&registry), config),
        ));

        // Set self-ref
        controller
            .try_lock()
            .unwrap()
            .set_self_ref(StdArc::clone(&controller) as StdArc<Mutex<dyn RouteController>>);
        let runtime = attach_runtime_bus(&controller).await;

        // Add a route
        let runtime_def = crate::route::RouteDefinition::new("crash-then-block:test", vec![])
            .with_route_id("crash-route");
        runtime.register_route(runtime_def).await.unwrap();

        // Start all routes
        controller.lock().await.start_all_routes().await.unwrap();

        // Wait for crash + restart
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Verify consumer was called at least twice (crash + restart)
        let count = call_count.load(Ordering::SeqCst);
        assert!(
            count >= 2,
            "expected at least 2 consumer calls (crash + restart), got {}",
            count
        );

        // Verify runtime projection reports Started.
        let status = match runtime
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: "crash-route".into(),
            })
            .await
            .unwrap()
        {
            RuntimeQueryResult::RouteStatus { status, .. } => status,
            other => panic!("unexpected query result: {other:?}"),
        };
        assert_eq!(status, "Started");
    }

    #[tokio::test]
    async fn test_supervising_controller_respects_max_attempts() {
        // Set up registry with always-crash component
        struct AlwaysCrashConsumer;
        #[async_trait]
        impl Consumer for AlwaysCrashConsumer {
            async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
                Err(CamelError::RouteError("always crashes".into()))
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn concurrency_model(&self) -> ConcurrencyModel {
                ConcurrencyModel::Sequential
            }
        }
        struct AlwaysCrashEndpoint;
        impl Endpoint for AlwaysCrashEndpoint {
            fn uri(&self) -> &str {
                "always-crash:test"
            }
            fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
                Ok(Box::new(AlwaysCrashConsumer))
            }
            fn create_producer(
                &self,
                _ctx: &camel_api::ProducerContext,
            ) -> Result<camel_api::BoxProcessor, CamelError> {
                Err(CamelError::RouteError("no producer".into()))
            }
        }
        struct AlwaysCrashComponent;
        impl Component for AlwaysCrashComponent {
            fn scheme(&self) -> &str {
                "always-crash"
            }
            fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
                Ok(Box::new(AlwaysCrashEndpoint))
            }
        }

        let registry = StdArc::new(std::sync::Mutex::new(Registry::new()));
        registry.lock().unwrap().register(AlwaysCrashComponent);

        // Configure with max 2 attempts
        let config = SupervisionConfig {
            max_attempts: Some(2),
            initial_delay: Duration::from_millis(10),
            backoff_multiplier: 1.0,
            max_delay: Duration::from_secs(1),
        };

        let controller: StdArc<Mutex<dyn RouteControllerInternal>> = StdArc::new(Mutex::new(
            SupervisingRouteController::new(StdArc::clone(&registry), config),
        ));

        controller
            .try_lock()
            .unwrap()
            .set_self_ref(StdArc::clone(&controller) as StdArc<Mutex<dyn RouteController>>);
        let runtime = attach_runtime_bus(&controller).await;

        let runtime_def = crate::route::RouteDefinition::new("always-crash:test", vec![])
            .with_route_id("always-crash-route");
        runtime.register_route(runtime_def).await.unwrap();

        controller.lock().await.start_all_routes().await.unwrap();

        // Wait for all attempts
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Runtime projection should be in Failed state (not restarted after max attempts).
        let status = match runtime
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: "always-crash-route".into(),
            })
            .await
            .unwrap()
        {
            RuntimeQueryResult::RouteStatus { status, .. } => status,
            other => panic!("unexpected query result: {other:?}"),
        };
        assert_eq!(status, "Failed");
    }

    #[tokio::test]
    async fn test_supervising_controller_delegates_to_inner() {
        let registry = StdArc::new(std::sync::Mutex::new(Registry::new()));
        let config = SupervisionConfig::default();
        let mut controller = SupervisingRouteController::new(StdArc::clone(&registry), config);

        // Set self-ref
        let self_ref: StdArc<Mutex<dyn RouteController>> = StdArc::new(Mutex::new(
            SupervisingRouteController::new(registry, SupervisionConfig::default()),
        ));
        controller.set_self_ref(self_ref);

        // Test route_count and route_ids (delegation)
        assert_eq!(controller.route_count(), 0);
        assert_eq!(controller.route_ids(), Vec::<String>::new());
    }

    /// Consumer that always crashes immediately, tracking call count.
    struct AlwaysCrashWithCountConsumer {
        call_count: StdArc<AtomicU32>,
    }

    #[async_trait]
    impl Consumer for AlwaysCrashWithCountConsumer {
        async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Err(CamelError::RouteError("always crashes".into()))
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }

        fn concurrency_model(&self) -> ConcurrencyModel {
            ConcurrencyModel::Sequential
        }
    }

    struct AlwaysCrashWithCountEndpoint {
        call_count: StdArc<AtomicU32>,
    }

    impl Endpoint for AlwaysCrashWithCountEndpoint {
        fn uri(&self) -> &str {
            "always-crash-count:test"
        }

        fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
            Ok(Box::new(AlwaysCrashWithCountConsumer {
                call_count: StdArc::clone(&self.call_count),
            }))
        }

        fn create_producer(
            &self,
            _ctx: &camel_api::ProducerContext,
        ) -> Result<camel_api::BoxProcessor, CamelError> {
            Err(CamelError::RouteError("no producer".into()))
        }
    }

    struct AlwaysCrashWithCountComponent {
        call_count: StdArc<AtomicU32>,
    }

    impl Component for AlwaysCrashWithCountComponent {
        fn scheme(&self) -> &str {
            "always-crash-count"
        }

        fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(AlwaysCrashWithCountEndpoint {
                call_count: StdArc::clone(&self.call_count),
            }))
        }
    }

    #[tokio::test]
    async fn test_supervision_gives_up_after_max_attempts() {
        // Set up registry with always-crash component that tracks call count
        let registry = StdArc::new(std::sync::Mutex::new(Registry::new()));
        let call_count = StdArc::new(AtomicU32::new(0));
        registry
            .lock()
            .unwrap()
            .register(AlwaysCrashWithCountComponent {
                call_count: StdArc::clone(&call_count),
            });

        // Configure supervision with max_attempts=2 and fast delays
        let config = SupervisionConfig {
            max_attempts: Some(2),
            initial_delay: Duration::from_millis(50),
            backoff_multiplier: 1.0,
            max_delay: Duration::from_secs(60),
        };

        let controller: StdArc<Mutex<dyn RouteControllerInternal>> = StdArc::new(Mutex::new(
            SupervisingRouteController::new(StdArc::clone(&registry), config),
        ));

        controller
            .try_lock()
            .unwrap()
            .set_self_ref(StdArc::clone(&controller) as StdArc<Mutex<dyn RouteController>>);
        let runtime = attach_runtime_bus(&controller).await;

        let runtime_def = crate::route::RouteDefinition::new("always-crash-count:test", vec![])
            .with_route_id("give-up-route");
        runtime.register_route(runtime_def).await.unwrap();

        controller.lock().await.start_all_routes().await.unwrap();

        // Wait enough time for: initial start + 2 restart attempts
        // With 50ms initial delay and no backoff: 50ms + 50ms = 100ms minimum
        // Wait 800ms to be safe
        tokio::time::sleep(Duration::from_millis(800)).await;

        // Verify consumer was called exactly max_attempts + 1 times
        // (1 initial start + 2 restart attempts = 3 total)
        let count = call_count.load(Ordering::SeqCst);
        assert_eq!(
            count, 3,
            "expected exactly 3 consumer calls (initial + 2 restarts), got {}",
            count
        );

        // Verify runtime projection is in Failed state (supervision gave up).
        let status = match runtime
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: "give-up-route".into(),
            })
            .await
            .unwrap()
        {
            RuntimeQueryResult::RouteStatus { status, .. } => status,
            other => panic!("unexpected query result: {other:?}"),
        };
        assert_eq!(status, "Failed");
    }

    /// Consumer that crashes on odd calls, blocks briefly then crashes on even calls.
    /// This simulates a route that sometimes runs successfully before crashing.
    struct CrashOnOddBlockOnEvenConsumer {
        call_count: StdArc<AtomicU32>,
    }

    #[async_trait]
    impl Consumer for CrashOnOddBlockOnEvenConsumer {
        async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            // count is 0-indexed: 0=call1, 1=call2, 2=call3, etc.
            // Odd calls (count 0, 2, 4, ...) crash immediately
            // Even calls (count 1, 3, 5, ...) block for 100ms then crash

            if count.is_multiple_of(2) {
                // Odd-numbered call (1st, 3rd, 5th, ...): crash immediately
                return Err(CamelError::RouteError("odd call crash".into()));
            }

            // Even-numbered call (2nd, 4th, 6th, ...): block briefly then crash
            // This simulates "successful" operation before crashing
            tokio::select! {
                _ = ctx.cancelled() => {
                    // Cancelled externally
                    return Ok(());
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    // Simulated "uptime" before crash
                    return Err(CamelError::RouteError("even call crash after uptime".into()));
                }
            }
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }

        fn concurrency_model(&self) -> ConcurrencyModel {
            ConcurrencyModel::Sequential
        }
    }

    struct CrashOnOddBlockOnEvenEndpoint {
        call_count: StdArc<AtomicU32>,
    }

    impl Endpoint for CrashOnOddBlockOnEvenEndpoint {
        fn uri(&self) -> &str {
            "crash-odd-block-even:test"
        }

        fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
            Ok(Box::new(CrashOnOddBlockOnEvenConsumer {
                call_count: StdArc::clone(&self.call_count),
            }))
        }

        fn create_producer(
            &self,
            _ctx: &camel_api::ProducerContext,
        ) -> Result<camel_api::BoxProcessor, CamelError> {
            Err(CamelError::RouteError("no producer".into()))
        }
    }

    struct CrashOnOddBlockOnEvenComponent {
        call_count: StdArc<AtomicU32>,
    }

    impl Component for CrashOnOddBlockOnEvenComponent {
        fn scheme(&self) -> &str {
            "crash-odd-block-even"
        }

        fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(CrashOnOddBlockOnEvenEndpoint {
                call_count: StdArc::clone(&self.call_count),
            }))
        }
    }

    #[tokio::test]
    async fn test_supervision_resets_attempt_count_on_success() {
        // Set up registry with crash-on-odd, block-on-even component
        let registry = StdArc::new(std::sync::Mutex::new(Registry::new()));
        let call_count = StdArc::new(AtomicU32::new(0));
        registry
            .lock()
            .unwrap()
            .register(CrashOnOddBlockOnEvenComponent {
                call_count: StdArc::clone(&call_count),
            });

        // Configure with max_attempts=2 - allows continued restarts when successful runs reset the counter
        // Without reset: would give up after 2 restarts (3 total calls)
        // With reset: successful runs (100ms >= 50ms) reset counter, allowing continued restarts
        let config = SupervisionConfig {
            max_attempts: Some(2),
            initial_delay: Duration::from_millis(50),
            backoff_multiplier: 1.0,
            max_delay: Duration::from_secs(60),
        };

        let controller: StdArc<Mutex<dyn RouteControllerInternal>> = StdArc::new(Mutex::new(
            SupervisingRouteController::new(StdArc::clone(&registry), config),
        ));

        controller
            .try_lock()
            .unwrap()
            .set_self_ref(StdArc::clone(&controller) as StdArc<Mutex<dyn RouteController>>);
        let runtime = attach_runtime_bus(&controller).await;

        let runtime_def = crate::route::RouteDefinition::new("crash-odd-block-even:test", vec![])
            .with_route_id("reset-attempt-route");
        runtime.register_route(runtime_def).await.unwrap();

        controller.lock().await.start_all_routes().await.unwrap();

        // Wait long enough for multiple crash-restart cycles:
        // - Call 1: crash immediately → restart (attempt reset to 0)
        // - Call 2: block 100ms, crash → restart (attempt reset to 0)
        // - Call 3: crash immediately → restart (attempt reset to 0)
        // - Call 4: block 100ms, crash → restart (attempt reset to 0)
        // With 50ms initial delay and 100ms block time, each full cycle takes ~150ms
        // Wait 1s to ensure we get at least 4 calls
        tokio::time::sleep(Duration::from_millis(1000)).await;

        // Verify consumer was called at least 4 times
        // (proving the attempt count reset allows continued restarts)
        let count = call_count.load(Ordering::SeqCst);
        assert!(
            count >= 4,
            "expected at least 4 consumer calls (proving attempt reset), got {}",
            count
        );

        // Verify runtime projection is NOT Failed (polling, since route crashes in a loop).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let status = match runtime
                .ask(RuntimeQuery::GetRouteStatus {
                    route_id: "reset-attempt-route".into(),
                })
                .await
                .unwrap()
            {
                RuntimeQueryResult::RouteStatus { status, .. } => status,
                other => panic!("unexpected query result: {other:?}"),
            };
            if status != "Failed" {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "route remained in Failed state for 2s — supervision likely gave up"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
