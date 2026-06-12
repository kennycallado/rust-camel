//! Shared types and free functions used across route lifecycle management.
//!
//! Extracted from [`route_controller`](super::route_controller) to reduce its size.
//! These are the concrete types (`CrashNotification`, `AggregateSplitInfo`,
//! `ManagedRoute`, `PreparedRoute`) and pure helper functions that do not depend
//! on `DefaultRouteController` state.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use tracing::warn;

use camel_api::aggregator::AggregatorConfig;
use camel_api::{CamelError, Exchange, RuntimeCommand, RuntimeHandle};
use camel_auth::TokenAuthenticator;
use camel_component_api::{ConcurrencyModel, consumer::ExchangeEnvelope};
use camel_processor::aggregator::{AggregatorService, has_timeout_condition};

use crate::lifecycle::adapters::pipeline_runtime::SharedPipeline;
use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinitionInfo};

/// Notification sent when a route crashes.
#[derive(Debug, Clone)]
pub struct CrashNotification {
    /// The ID of the crashed route.
    pub route_id: String,
    /// The error that caused the crash.
    pub error: String,
}

#[cfg(test)]
type StartRouteEventHook = Arc<dyn Fn(&'static str) + Send + Sync + 'static>;

#[cfg(test)]
static START_ROUTE_EVENT_HOOK: std::sync::LazyLock<std::sync::Mutex<Option<StartRouteEventHook>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

#[cfg(test)]
pub(super) fn set_start_route_event_hook(hook: Option<StartRouteEventHook>) {
    *START_ROUTE_EVENT_HOOK
        .lock()
        .expect("start route event hook lock") = hook;
}

#[cfg(test)]
pub(super) fn emit_start_route_event(event: &'static str) {
    if let Some(hook) = START_ROUTE_EVENT_HOOK
        .lock()
        .expect("start route event hook lock")
        .as_ref()
    {
        hook(event);
    }
}

/// Internal state for a managed route.
#[derive(Clone)]
pub(super) struct AggregateSplitInfo {
    pub(super) pre_pipeline: SharedPipeline,
    pub(super) agg_config: AggregatorConfig,
    pub(super) post_pipeline: SharedPipeline,
}

/// A route that has been compiled and registered with the controller.
pub(super) struct ManagedRoute {
    /// The route definition metadata (for introspection).
    pub(super) definition: RouteDefinitionInfo,
    /// Source endpoint URI.
    pub(super) from_uri: String,
    /// Resolved processor pipeline (wrapped for atomic swap).
    pub(super) pipeline: SharedPipeline,
    /// Concurrency model override (if any).
    pub(super) concurrency: Option<ConcurrencyModel>,
    /// Handle for the consumer task (if running).
    pub(super) consumer_handle: Option<JoinHandle<()>>,
    /// Handle for the pipeline task (if running).
    pub(super) pipeline_handle: Option<JoinHandle<()>>,
    /// Cancellation token for stopping the consumer task.
    /// This allows independent control of the consumer lifecycle (for suspend/resume).
    pub(super) consumer_cancel_token: CancellationToken,
    /// Cancellation token for stopping the pipeline task.
    /// This allows independent control of the pipeline lifecycle (for suspend/resume).
    pub(super) pipeline_cancel_token: CancellationToken,
    /// Channel sender for sending exchanges to the pipeline.
    /// Stored to allow resuming a suspended route without recreating the channel.
    pub(super) channel_sender: Option<mpsc::Sender<ExchangeEnvelope>>,
    /// In-flight exchange counter. `None` when UoW is not configured for this route.
    pub(super) in_flight: Option<Arc<std::sync::atomic::AtomicU64>>,
    pub(super) aggregate_split: Option<AggregateSplitInfo>,
    pub(super) agg_service: Option<Arc<std::sync::Mutex<AggregatorService>>>,
    /// Stored security policy config for use when starting/resuming the consumer.
    pub(super) security_policy: Option<camel_api::security_policy::SecurityPolicyConfig>,
    /// Stored security authenticator for use when starting/resuming the consumer.
    pub(super) security_authenticator: Option<Arc<dyn TokenAuthenticator>>,
}

/// A prepared route (compiled but not yet inserted into the registry).
pub(crate) struct PreparedRoute {
    pub(crate) route_id: String,
    pub(super) managed: ManagedRoute,
}

/// Check if a task handle is still running.
pub(super) fn handle_is_running(handle: &Option<JoinHandle<()>>) -> bool {
    handle.as_ref().is_some_and(|h| !h.is_finished())
}

/// Return a human-readable lifecycle label based on consumer/pipeline handle state.
pub(super) fn inferred_lifecycle_label(managed: &ManagedRoute) -> &'static str {
    match (
        handle_is_running(&managed.consumer_handle),
        handle_is_running(&managed.pipeline_handle),
    ) {
        (true, true) => "Started",
        (false, true) => "Suspended",
        (true, false) => "Stopping",
        (false, false) => "Stopped",
    }
}

/// Find the top-level aggregate step that requires split processing (has timeout or force-completion).
pub(super) fn find_top_level_aggregate_requiring_split(
    steps: &[BuilderStep],
) -> Option<(usize, AggregatorConfig)> {
    for (i, step) in steps.iter().enumerate() {
        if let BuilderStep::Aggregate { config } = step {
            if has_timeout_condition(&config.completion) || config.force_completion_on_stop {
                return Some((i, config.clone()));
            }
            break;
        }
    }
    None
}

/// Check if an exchange is pending in the aggregator.
pub(super) fn is_pending(ex: &Exchange) -> bool {
    ex.property("CamelAggregatorPending")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Wait for a pipeline service to be ready with circuit breaker backoff.
///
/// This helper encapsulates the pattern of repeatedly calling `ready()` on a
/// service while handling `CircuitOpen` errors with a fixed 1-second backoff and
/// cancellation checks. It returns `Ok(())` when the service is ready, or
/// `Err(e)` if cancellation occurred or a fatal error was encountered.
pub(super) async fn ready_with_backoff(
    pipeline: &mut camel_api::BoxProcessor,
    cancel: &CancellationToken,
) -> Result<(), CamelError> {
    loop {
        match pipeline.ready().await {
            Ok(_) => return Ok(()),
            Err(CamelError::CircuitOpen(ref msg)) => {
                warn!("Circuit open, backing off: {msg}");
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
                // log-policy: system-broken
                tracing::error!("Pipeline not ready: {e}");
                return Err(e);
            }
        }
    }
}

/// Build a `FailRoute` command for a runtime failure.
pub(super) fn runtime_failure_command(route_id: &str, error: &str) -> RuntimeCommand {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    RuntimeCommand::FailRoute {
        route_id: route_id.to_string(),
        error: error.to_string(),
        command_id: format!("ctrl-fail-{route_id}-{stamp}"),
        causation_id: None,
    }
}

/// Publish a runtime failure notification to the runtime handle.
pub(super) async fn publish_runtime_failure(
    runtime: Option<std::sync::Weak<dyn RuntimeHandle>>,
    route_id: &str,
    error: &str,
) {
    let Some(runtime) = runtime.and_then(|weak| weak.upgrade()) else {
        return;
    };
    let command = runtime_failure_command(route_id, error);
    if let Err(runtime_error) = runtime.execute(command).await {
        warn!(
            route_id = %route_id,
            error = %runtime_error,
            "failed to synchronize route crash with runtime projection"
        );
    }
}
