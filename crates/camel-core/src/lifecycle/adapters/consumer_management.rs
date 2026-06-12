use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use camel_api::{CamelError, RuntimeHandle};
use camel_component_api::{
    ComponentContext, ConcurrencyModel, Consumer, ConsumerContext, RuntimeObservability,
};
use camel_endpoint::parse_uri;

use crate::lifecycle::adapters::route_helpers::{
    CrashNotification, ManagedRoute, handle_is_running, publish_runtime_failure,
};
use crate::shared::components::domain::Registry;

pub(crate) fn create_route_consumer(
    rt: Arc<dyn RuntimeObservability>,
    registry: &Arc<std::sync::Mutex<Registry>>,
    from_uri: &str,
    component_ctx: &dyn ComponentContext,
) -> Result<(Box<dyn Consumer>, ConcurrencyModel), CamelError> {
    let parsed = parse_uri(from_uri)?;
    let component = {
        let guard = registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
        guard.get_or_err(&parsed.scheme)?.clone()
    };
    let endpoint = component.create_endpoint(from_uri, component_ctx)?;
    let consumer = endpoint.create_consumer(rt)?;
    let concurrency = consumer.concurrency_model();
    Ok((consumer, concurrency))
}

pub(crate) fn spawn_consumer_task(
    route_id: String,
    mut consumer: Box<dyn Consumer>,
    consumer_ctx: ConsumerContext,
    crash_notifier: Option<mpsc::Sender<CrashNotification>>,
    runtime_for_consumer: Option<Weak<dyn RuntimeHandle>>,
    is_resume: bool,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = consumer.start(consumer_ctx.clone()).await {
            if is_resume {
                // log-policy: system-broken
                error!(route_id = %route_id, "Consumer error on resume: {e}");
            } else {
                // log-policy: system-broken
                error!(route_id = %route_id, "Consumer error: {e}");
            }

            let error_msg = e.to_string();
            if let Some(tx) = crash_notifier {
                let _ = tx
                    .send(CrashNotification {
                        route_id: route_id.clone(),
                        error: error_msg.clone(),
                    })
                    .await;
            }

            publish_runtime_failure(runtime_for_consumer, &route_id, &error_msg).await;
            return;
        }

        // Consumer started successfully. If it detached a background task,
        // monitor the handle for unexpected exits (crash propagation per
        // ADR-0007).
        if let Some(mut bg_handle) = consumer.background_task_handle() {
            tokio::select! {
                result = &mut bg_handle => {
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) if !consumer_ctx.is_cancelled() => {
                            let error_msg = e.to_string();
                            // log-policy: system-broken
                            error!(route_id = %route_id, "Consumer background task failed: {error_msg}");
                            if let Some(ref tx) = crash_notifier {
                                let _ = tx
                                    .send(CrashNotification {
                                        route_id: route_id.clone(),
                                        error: error_msg.clone(),
                                    })
                                    .await;
                            }
                            publish_runtime_failure(runtime_for_consumer, &route_id, &error_msg).await;
                        }
                        Ok(Err(e)) => {
                            tracing::debug!(route_id = %route_id, "Consumer bg task error during shutdown: {e}");
                        }
                        Err(join_err) if !consumer_ctx.is_cancelled() => {
                            let error_msg = format!("Consumer task panicked: {join_err}");
                            // log-policy: system-broken
                            error!(route_id = %route_id, "{error_msg}");
                            if let Some(ref tx) = crash_notifier {
                                let _ = tx
                                    .send(CrashNotification {
                                        route_id: route_id.clone(),
                                        error: error_msg.clone(),
                                    })
                                    .await;
                            }
                            publish_runtime_failure(runtime_for_consumer, &route_id, &error_msg).await;
                        }
                        Err(join_err) => {
                            tracing::debug!(
                                route_id = %route_id,
                                "Consumer bg task panicked during shutdown: {join_err}"
                            );
                        }
                    }
                }
                _ = consumer_ctx.cancelled() => {
                    bg_handle.abort();
                }
            }
        } else {
            // Inline consumer: start() returned, meaning the consumer's run
            // loop completed naturally or the context was cancelled mid-run.
            // No need to wait — just proceed to stop().
        }

        // "finally" — always call stop() after start() succeeds
        let _ = consumer.stop().await;
    })
}

pub(super) async fn stop_route_internal(
    routes: &mut HashMap<String, ManagedRoute>,
    route_id: &str,
    shutdown_timeout: Duration,
) -> Result<(), CamelError> {
    let managed = routes
        .get_mut(route_id)
        .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

    if !handle_is_running(&managed.consumer_handle) && !handle_is_running(&managed.pipeline_handle)
    {
        return Ok(());
    }

    info!(route_id = %route_id, "Stopping route");

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    managed.consumer_cancel_token.cancel();

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    if let Some(agg_svc) = &managed.agg_service {
        let guard = agg_svc
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
        guard.force_complete_all();
    }

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    managed.pipeline_cancel_token.cancel();

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    let consumer_handle = managed.consumer_handle.take();
    let pipeline_handle = managed.pipeline_handle.take();

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    managed.channel_sender = None;

    let timeout_result = tokio::time::timeout(shutdown_timeout, async {
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
        warn!(route_id = %route_id, "Route shutdown timed out after {:.0?} — tasks may still be running", shutdown_timeout);
    }

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check"); // allow-unwrap
    managed.consumer_cancel_token = CancellationToken::new();
    managed.pipeline_cancel_token = CancellationToken::new();

    info!(route_id = %route_id, "Route stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::lifecycle::adapters::route_runtime_state;
    use crate::lifecycle::application::route_definition::RouteDefinition;
    use arc_swap::ArcSwap;
    use async_trait::async_trait;
    use camel_api::SyncBoxProcessor;
    use camel_api::{BoxProcessor, IdentityProcessor};

    struct FailingConsumer {
        message: &'static str,
    }

    #[async_trait]
    impl Consumer for FailingConsumer {
        async fn start(&mut self, _context: ConsumerContext) -> Result<(), CamelError> {
            Err(CamelError::RouteError(self.message.into()))
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    fn managed_route_with_handles(
        consumer_handle: Option<JoinHandle<()>>,
        pipeline_handle: Option<JoinHandle<()>>,
        channel_sender: Option<mpsc::Sender<camel_component_api::consumer::ExchangeEnvelope>>,
    ) -> ManagedRoute {
        ManagedRoute {
            definition: RouteDefinition::new("timer:test", vec![])
                .with_route_id("route-1")
                .to_info(),
            from_uri: "timer:test".into(),
            pipeline: Arc::new(ArcSwap::from_pointee(SyncBoxProcessor::new(
                BoxProcessor::new(IdentityProcessor),
            ))),
            concurrency: None,
            consumer_handle,
            pipeline_handle,
            consumer_cancel_token: CancellationToken::new(),
            pipeline_cancel_token: CancellationToken::new(),
            channel_sender,
            in_flight: None,
            aggregate_split: None,
            agg_service: None,
            compiled: route_runtime_state::CompiledRoute {
                security_policy: None,
                security_authenticator: None,
            },
        }
    }

    #[test]
    fn create_route_consumer_returns_err_for_unknown_scheme() {
        use crate::lifecycle::adapters::controller_component_context::ControllerComponentContext;

        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let component_ctx = Arc::new(ControllerComponentContext::new(
            Arc::clone(&registry),
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            Arc::new(camel_api::NoOpMetrics),
            Arc::new(camel_api::NoopPlatformService::default()),
            Arc::new(crate::health_registry::HealthCheckRegistry::new(
                std::time::Duration::from_secs(5),
            )),
            None,
        ));
        let rt: Arc<dyn RuntimeObservability> = Arc::clone(&component_ctx) as Arc<_>;

        let err = match create_route_consumer(rt, &registry, "unknown:foo", component_ctx.as_ref())
        {
            Ok(_) => panic!("unknown scheme should fail consumer creation"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("unknown"));
    }

    #[tokio::test]
    async fn stop_route_internal_returns_not_found_when_route_absent() {
        let mut routes = HashMap::new();

        let err = stop_route_internal(&mut routes, "missing-route", Duration::from_secs(5))
            .await
            .expect_err("stopping a missing route should fail");

        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn stop_route_internal_short_circuits_when_already_stopped() {
        let (tx, _rx) = mpsc::channel(1);
        let mut routes = HashMap::new();
        routes.insert(
            "route-1".to_string(),
            managed_route_with_handles(None, None, Some(tx)),
        );

        let result = stop_route_internal(&mut routes, "route-1", Duration::from_secs(5)).await;

        assert!(result.is_ok());
        let managed = routes.get("route-1").expect("route must still exist");
        assert!(managed.channel_sender.is_some());
    }

    #[tokio::test]
    async fn spawn_consumer_task_resume_failure_sends_crash_notification() {
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(
            tx,
            CancellationToken::new(),
            "consumer-mgmt-test-route".to_string(),
        );
        let (crash_tx, mut crash_rx) = mpsc::channel(1);

        let handle = spawn_consumer_task(
            "route-resume".to_string(),
            Box::new(FailingConsumer {
                message: "resume start failed",
            }),
            ctx,
            Some(crash_tx),
            None,
            true,
        );

        handle.await.expect("consumer task should join cleanly");

        let notification = crash_rx
            .recv()
            .await
            .expect("crash notification should be sent");
        assert_eq!(notification.route_id, "route-resume");
        assert!(notification.error.contains("resume start failed"));
    }

    // --- GRL-001: Deferred failure crash propagation ---

    struct DeferredFailConsumer {
        handle: Option<tokio::task::JoinHandle<Result<(), CamelError>>>,
    }

    impl DeferredFailConsumer {
        fn new(err: &'static str) -> Self {
            let err_msg = err.to_string();
            let handle = tokio::task::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                Err(CamelError::ProcessorError(err_msg))
            });
            Self {
                handle: Some(handle),
            }
        }
    }

    #[async_trait]
    impl Consumer for DeferredFailConsumer {
        async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
        fn background_task_handle(
            &mut self,
        ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
            self.handle.take()
        }
    }

    #[tokio::test]
    async fn spawn_consumer_task_deferred_failure_sends_crash_notification() {
        let (exchange_tx, _rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(exchange_tx, cancel, "consumer-mgmt-test-route".to_string());
        let (crash_tx, mut crash_rx) = mpsc::channel(1);

        let handle = spawn_consumer_task(
            "route-deferred".to_string(),
            Box::new(DeferredFailConsumer::new("broker lost")),
            ctx,
            Some(crash_tx),
            None,
            false,
        );

        handle.await.expect("outer task should complete");

        let notification = crash_rx.recv().await.expect("crash notification expected");
        assert_eq!(notification.route_id, "route-deferred");
        assert!(notification.error.contains("broker lost"));
    }

    #[tokio::test]
    async fn spawn_consumer_task_deferred_failure_suppressed_on_cancellation() {
        let (exchange_tx, _rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(
            exchange_tx,
            cancel.clone(),
            "consumer-mgmt-test-route".to_string(),
        );
        let (crash_tx, mut crash_rx) = mpsc::channel(1);

        // Cancel BEFORE the bg task exits — simulates graceful shutdown
        cancel.cancel();

        let handle = spawn_consumer_task(
            "route-cancel".to_string(),
            Box::new(DeferredFailConsumer::new("shutdown error")),
            ctx,
            Some(crash_tx),
            None,
            false,
        );

        handle.await.expect("outer task should complete");

        // Should receive NO crash notification — error during shutdown is suppressed
        crash_rx.close();
        assert!(
            crash_rx.recv().await.is_none(),
            "no crash notification expected on cancelled shutdown"
        );
    }

    #[tokio::test]
    async fn spawn_consumer_task_calls_stop_on_cancellation() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static STOP_CALLED: AtomicBool = AtomicBool::new(false);

        struct StopTrackingConsumer;

        #[async_trait]
        impl Consumer for StopTrackingConsumer {
            async fn start(&mut self, _context: ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                STOP_CALLED.store(true, Ordering::SeqCst);
                Ok(())
            }
        }

        let cancel = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, cancel.clone(), "consumer-mgmt-test-route".to_string());

        let handle = spawn_consumer_task(
            "test-route".into(),
            Box::new(StopTrackingConsumer),
            ctx,
            None,
            None,
            false,
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(
            result.is_ok(),
            "spawn_consumer_task should complete after cancellation"
        );
        assert!(
            STOP_CALLED.load(Ordering::SeqCst),
            "consumer.stop() should have been called"
        );
    }
}
