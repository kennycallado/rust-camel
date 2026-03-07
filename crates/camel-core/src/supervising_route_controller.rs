//! Supervising route controller with automatic crash recovery.
//!
//! This module provides [`SupervisingRouteController`], which wraps a
//! [`DefaultRouteController`] and monitors crashed routes, restarting them
//! with exponential backoff based on [`SupervisionConfig`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Mutex;
use tracing::{error, info, warn};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{CamelError, MetricsCollector, RouteController, RouteStatus, SupervisionConfig};

use crate::registry::Registry;
use crate::route::RouteDefinition;
use crate::route_controller::{
    CrashNotification, DefaultRouteController, RouteControllerInternal, SharedLanguageRegistry,
};

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
}

#[async_trait::async_trait]
impl RouteController for SupervisingRouteController {
    async fn start_route(&mut self, route_id: &str) -> Result<(), CamelError> {
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

    fn route_status(&self, route_id: &str) -> Option<RouteStatus> {
        self.inner.route_status(route_id)
    }

    async fn start_all_routes(&mut self) -> Result<(), CamelError> {
        // Set up crash notification before starting routes
        self.inner.set_crash_notifier(self.crash_tx.clone());

        // Start all routes via inner controller
        self.inner.start_all_routes().await?;

        // Take the receiver and spawn supervision loop
        if let Some(rx) = self.crash_rx.take() {
            if let Some(controller_ref) = self.inner.self_ref_for_supervision() {
                let config = self.config.clone();
                let metrics = self.metrics.clone();
                tokio::spawn(async move {
                    supervision_loop(rx, controller_ref, config, metrics).await;
                });
            } else {
                warn!("SupervisingRouteController: self_ref not set, supervision loop not started");
            }
        }

        Ok(())
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

    fn route_count(&self) -> usize {
        self.inner.route_count()
    }

    fn route_ids(&self) -> Vec<String> {
        self.inner.route_ids()
    }

    fn set_tracer_config(&mut self, config: &crate::config::TracerConfig) {
        self.inner.set_tracer_config(config)
    }

    fn compile_route_definition(
        &self,
        def: crate::route::RouteDefinition,
    ) -> Result<camel_api::BoxProcessor, camel_api::CamelError> {
        self.inner.compile_route_definition(def)
    }

    fn remove_route(&mut self, route_id: &str) -> Result<(), camel_api::CamelError> {
        self.inner.remove_route(route_id)
    }

    async fn start_route_reload(&mut self, route_id: &str) -> Result<(), camel_api::CamelError> {
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
    controller: Arc<Mutex<dyn RouteController>>,
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

        // Check current status before restarting
        let mut ctrl = controller.lock().await;
        match ctrl.route_status(&route_id) {
            Some(RouteStatus::Failed(_)) => {
                // Route is still failed — proceed with restart
                match ctrl.restart_route(&route_id).await {
                    Ok(()) => {
                        info!(route_id = %route_id, "Route restarted successfully");
                        // Record restart time instead of resetting attempts
                        // The counter will be reset on next crash if route ran long enough
                        last_restart_time.insert(route_id.clone(), Instant::now());
                    }
                    Err(e) => {
                        error!(route_id = %route_id, error = %e, "Failed to restart route");
                    }
                }
            }
            Some(status) => {
                // Route was manually stopped/suspended — skip restart
                warn!(route_id = %route_id, ?status, "Route no longer failed, skipping supervision restart");
                attempts.remove(&route_id);
            }
            None => {
                warn!(route_id = %route_id, "Route not found during supervision restart");
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
    use async_trait::async_trait;
    use camel_component::{Component, ConcurrencyModel, Consumer, ConsumerContext, Endpoint};
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

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

        // Add a route
        let def = crate::route::RouteDefinition::new("crash-then-block:test", vec![])
            .with_route_id("crash-route");
        controller.try_lock().unwrap().add_route(def).unwrap();

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

        // Verify route is now started
        let status = controller.lock().await.route_status("crash-route").unwrap();
        assert!(
            matches!(status, RouteStatus::Started),
            "expected Started, got {:?}",
            status
        );
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

        let def = crate::route::RouteDefinition::new("always-crash:test", vec![])
            .with_route_id("always-crash-route");
        controller.try_lock().unwrap().add_route(def).unwrap();

        controller.lock().await.start_all_routes().await.unwrap();

        // Wait for all attempts
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Route should be in Failed state (not restarted after max attempts)
        let status = controller
            .lock()
            .await
            .route_status("always-crash-route")
            .unwrap();
        assert!(
            matches!(status, RouteStatus::Failed(_)),
            "expected Failed, got {:?}",
            status
        );
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

        let def = crate::route::RouteDefinition::new("always-crash-count:test", vec![])
            .with_route_id("give-up-route");
        controller.try_lock().unwrap().add_route(def).unwrap();

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

        // Verify route is in Failed state (supervision gave up)
        let status = controller
            .lock()
            .await
            .route_status("give-up-route")
            .unwrap();
        assert!(
            matches!(status, RouteStatus::Failed(_)),
            "expected Failed, got {:?}",
            status
        );
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

        let def = crate::route::RouteDefinition::new("crash-odd-block-even:test", vec![])
            .with_route_id("reset-attempt-route");
        controller.try_lock().unwrap().add_route(def).unwrap();

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

        // Verify route is NOT in Failed state
        // It should be either Started or in the process of restarting
        let status = controller
            .lock()
            .await
            .route_status("reset-attempt-route")
            .unwrap();
        assert!(
            !matches!(status, RouteStatus::Failed(_)),
            "expected route NOT to be Failed (supervision should continue due to attempt reset), got {:?}",
            status
        );
    }
}
