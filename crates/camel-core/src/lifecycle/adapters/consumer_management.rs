use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use camel_api::{CamelError, RuntimeHandle};
use camel_component_api::{ConcurrencyModel, Consumer, ConsumerContext};
use camel_endpoint::parse_uri;

use crate::lifecycle::adapters::route_controller::{
    CrashNotification, ManagedRoute, handle_is_running, publish_runtime_failure,
};
use crate::shared::components::domain::Registry;

pub(crate) fn create_route_consumer(
    registry: &Arc<std::sync::Mutex<Registry>>,
    from_uri: &str,
) -> Result<(Box<dyn Consumer>, ConcurrencyModel), CamelError> {
    let parsed = parse_uri(from_uri)?;
    let consumer = {
        let registry = registry
            .lock()
            .expect("mutex poisoned: another thread panicked while holding this lock");
        let component = registry.get_or_err(&parsed.scheme)?;
        let endpoint = component.create_endpoint(from_uri)?;
        endpoint.create_consumer()?
    };
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
