use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use camel_api::{CamelError, RuntimeHandle};
use camel_component_api::{ComponentContext, ConcurrencyModel, Consumer, ConsumerContext};
use camel_endpoint::parse_uri;

use crate::lifecycle::adapters::route_controller::{
    CrashNotification, ManagedRoute, handle_is_running, publish_runtime_failure,
};
use crate::shared::components::domain::Registry;

pub(crate) fn create_route_consumer(
    registry: &Arc<std::sync::Mutex<Registry>>,
    from_uri: &str,
    component_ctx: &dyn ComponentContext,
) -> Result<(Box<dyn Consumer>, ConcurrencyModel), CamelError> {
    let parsed = parse_uri(from_uri)?;
    let component = {
        let guard = registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");
        guard.get_or_err(&parsed.scheme)?.clone()
    };
    let endpoint = component.create_endpoint(from_uri, component_ctx)?;
    let consumer = endpoint.create_consumer()?;
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
        if let Err(e) = consumer.start(consumer_ctx).await {
            if is_resume {
                error!(route_id = %route_id, "Consumer error on resume: {e}");
            } else {
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
        }
    })
}

pub(super) async fn stop_route_internal(
    routes: &mut HashMap<String, ManagedRoute>,
    route_id: &str,
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
        .expect("invariant: route must exist after prior existence check");
    managed.consumer_cancel_token.cancel();

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check");
    if let Some(agg_svc) = &managed.agg_service {
        let guard = agg_svc
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");
        guard.force_complete_all();
    }

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check");
    managed.pipeline_cancel_token.cancel();

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check");
    let consumer_handle = managed.consumer_handle.take();
    let pipeline_handle = managed.pipeline_handle.take();

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check");
    managed.channel_sender = None;

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

    let managed = routes
        .get_mut(route_id)
        .expect("invariant: route must exist after prior existence check");
    managed.consumer_cancel_token = CancellationToken::new();
    managed.pipeline_cancel_token = CancellationToken::new();

    info!(route_id = %route_id, "Route stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::lifecycle::adapters::route_controller::SyncBoxProcessor;
    use crate::lifecycle::application::route_definition::RouteDefinition;
    use arc_swap::ArcSwap;
    use async_trait::async_trait;
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
            pipeline: Arc::new(ArcSwap::from_pointee(SyncBoxProcessor(BoxProcessor::new(
                IdentityProcessor,
            )))),
            concurrency: None,
            consumer_handle,
            pipeline_handle,
            consumer_cancel_token: CancellationToken::new(),
            pipeline_cancel_token: CancellationToken::new(),
            channel_sender,
            in_flight: None,
            aggregate_split: None,
            agg_service: None,
        }
    }

    #[test]
    fn create_route_consumer_returns_err_for_unknown_scheme() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));

        let err = match create_route_consumer(
            &registry,
            "unknown:foo",
            &crate::lifecycle::adapters::route_controller::ControllerComponentContext::new(
                Arc::clone(&registry),
                Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
                Arc::new(camel_api::NoOpMetrics),
                Arc::new(camel_api::NoopPlatformService::default()),
            ),
        ) {
            Ok(_) => panic!("unknown scheme should fail consumer creation"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("unknown"));
    }

    #[tokio::test]
    async fn stop_route_internal_returns_not_found_when_route_absent() {
        let mut routes = HashMap::new();

        let err = stop_route_internal(&mut routes, "missing-route")
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

        let result = stop_route_internal(&mut routes, "route-1").await;

        assert!(result.is_ok());
        let managed = routes.get("route-1").expect("route must still exist");
        assert!(managed.channel_sender.is_some());
    }

    #[tokio::test]
    async fn spawn_consumer_task_resume_failure_sends_crash_notification() {
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, CancellationToken::new());
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
}
