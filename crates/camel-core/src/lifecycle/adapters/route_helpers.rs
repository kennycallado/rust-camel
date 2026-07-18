//! Shared types and free functions used across route lifecycle management.
//!
//! Extracted from [`route_controller`](super::route_controller) to reduce its size.
//! These are the concrete types (`CrashNotification`, `AggregateSplitInfo`,
//! `ManagedRoute`, `PreparedRoute`) and pure helper functions that do not depend
//! on `DefaultRouteController` state.
//!
//! `CompiledPipeline` previously lived here too; it has been moved to
//! [`crate::lifecycle::domain::route_compilation`] because it is a pure
//! contract type and was being imported by `ReloadExecutorPort` (an
//! application-ring port), which violated the dependency rule.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use tracing::warn;

use camel_api::aggregator::AggregatorConfig;
use camel_api::{CamelError, Exchange, ResequencePolicyConfig, RuntimeCommand, RuntimeHandle};
use camel_component_api::{ConcurrencyModel, consumer::ExchangeEnvelope};
use camel_processor::aggregator::{AggregatorService, has_timeout_condition};

use crate::lifecycle::adapters::pipeline_runtime::SharedPipeline;
use crate::lifecycle::adapters::route_runtime_state;
use crate::lifecycle::adapters::step_compilers::CompiledStep;
use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinitionInfo};

/// Extract all lifecycle handles from a slice of compiled steps, preserving
/// route order. Used at route compilation time to populate
/// [`PipelineAssembly::lifecycle`](super::pipeline_runtime::PipelineAssembly).
///
/// Returns a flat `Vec` of handles. Stateless steps produce no handles;
/// `CompiledStep::Stop` produces none.
pub(crate) fn collect_lifecycle(steps: &[CompiledStep]) -> Vec<Arc<dyn camel_api::StepLifecycle>> {
    let mut out = Vec::new();
    for step in steps {
        match step {
            CompiledStep::Process { lifecycle, .. } => {
                if let Some(lc) = lifecycle {
                    out.push(Arc::clone(lc));
                }
            }
            CompiledStep::Segment { lifecycle, .. } => {
                if let Some(lcs) = lifecycle {
                    out.extend(lcs.iter().map(Arc::clone));
                }
            }
            CompiledStep::Stop => {}
        }
    }
    out
}

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
pub(crate) struct AggregateSplitInfo {
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
    /// Always-populated counter for shutdown drain coordination (ADR-0043 amend).
    /// Incremented when the pipeline task dequeues an exchange, decremented on
    /// completion. `stop_route_internal` waits for this to reach zero before
    /// cancelling the pipeline cancel token.
    pub(super) drain_in_flight: Arc<std::sync::atomic::AtomicU64>,
    pub(super) aggregate_split: Option<AggregateSplitInfo>,
    pub(super) agg_service: Option<Arc<AggregatorService>>,
    /// Compiled runtime state (security artifacts captured at add time).
    pub(super) compiled: route_runtime_state::CompiledRoute,
}

/// RAII guard that increments a counter on creation and decrements on drop.
/// Used to track in-flight exchanges for shutdown drain coordination.
pub(super) struct DrainGuard(Arc<AtomicU64>);

impl DrainGuard {
    pub(super) fn new(c: Arc<AtomicU64>) -> Self {
        c.fetch_add(1, Ordering::Relaxed);
        Self(c)
    }
}

impl Drop for DrainGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A prepared route (compiled but not yet inserted into the registry).
///
/// NOTE: This type carries `managed: ManagedRoute` (an adapter-internal bundle
/// of runtime handles, cancellation tokens, shared pipelines, etc.). It is
/// therefore NOT a pure contract type and cannot be relocated to the domain
/// ring without a separate refactor — the `ReloadExecutorPort` still imports
/// it from `crate::lifecycle::adapters::route_controller::PreparedRoute`,
/// which violates the dependency rule (application → adapters). The full
/// F2 fix is BLOCKED on this coupling; see Tier C blessing-gate report.
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

/// Info about the top-level resequencer split (N3: at most one).
#[derive(Debug, Clone)]
pub(super) struct ResequenceSplitInfo {
    pub(super) index: usize,
    pub(super) policy_config: ResequencePolicyConfig,
}

/// Find the top-level resequencer step.
///
/// Returns `Some(info)` if there is exactly ONE top-level `BuilderStep::Resequence`.
/// **N3:** Returns an error if more than one top-level `Resequence` is found
/// (the "resequencer is the LAST main-pipeline step" invariant holds for exactly one).
pub(super) fn find_top_level_resequencer_requiring_split(
    steps: &[BuilderStep],
) -> Result<Option<ResequenceSplitInfo>, CamelError> {
    let mut found: Option<ResequenceSplitInfo> = None;
    for (i, step) in steps.iter().enumerate() {
        if let BuilderStep::Resequence { policy_config } = step {
            if found.is_some() {
                return Err(CamelError::RouteError(
                    "Multiple top-level Resequence steps found — at most one allowed".into(),
                ));
            }
            found = Some(ResequenceSplitInfo {
                index: i,
                policy_config: policy_config.clone(),
            });
        }
    }
    Ok(found)
}

/// N2: Reject any route containing BOTH a top-level aggregate-requiring-split
/// step AND a top-level `BuilderStep::Resequence`. The two split mechanisms are
/// mutually exclusive.
///
/// **Predicate (M9):** Detects "aggregate-requiring-split" by testing timeout
/// / force-completion on EVERY top-level `Aggregate`, not just the first match.
pub(super) fn assert_no_mixed_top_level_splits(steps: &[BuilderStep]) -> Result<(), CamelError> {
    let has_aggregate_split = steps
        .iter()
        .any(|step| matches!(step, BuilderStep::Aggregate { config } if has_timeout_condition(&config.completion) || config.force_completion_on_stop));
    let has_resequence = steps
        .iter()
        .any(|step| matches!(step, BuilderStep::Resequence { .. }));
    if has_aggregate_split && has_resequence {
        return Err(CamelError::RouteError(
            "Route contains both a top-level Aggregate (requiring split) and a Resequence step — these split mechanisms are mutually exclusive".into(),
        ));
    }
    Ok(())
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

#[cfg(test)]
mod resequence_tests {
    use super::*;
    use camel_api::aggregator::AggregatorConfig;

    fn make_aggregate_with_timeout() -> BuilderStep {
        BuilderStep::Aggregate {
            config: AggregatorConfig::correlate_by("id")
                .complete_on_timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("build aggregate config"), // allow-unwrap: test-only
        }
    }

    fn make_aggregate_with_force_completion() -> BuilderStep {
        BuilderStep::Aggregate {
            config: AggregatorConfig::correlate_by("id")
                .complete_when_size(1)
                .force_completion_on_stop(true)
                .build()
                .expect("build aggregate config"), // allow-unwrap: test-only
        }
    }

    fn make_aggregate_simple() -> BuilderStep {
        BuilderStep::Aggregate {
            config: AggregatorConfig::correlate_by("id")
                .complete_when_size(5)
                .build()
                .expect("build aggregate config"), // allow-unwrap: test-only
        }
    }

    fn make_resequence() -> BuilderStep {
        BuilderStep::Resequence {
            policy_config: ResequencePolicyConfig::default(),
        }
    }

    fn make_log(msg: &str) -> BuilderStep {
        BuilderStep::Log {
            level: camel_processor::LogLevel::Info,
            message: msg.to_string(),
        }
    }

    fn make_set_body(body: &str) -> BuilderStep {
        use camel_api::{LanguageExpressionDef, ValueSourceDef};
        BuilderStep::DeclarativeSetBody {
            value: ValueSourceDef::Expression(LanguageExpressionDef {
                language: "simple".into(),
                source: body.to_string(),
            }),
        }
    }

    #[test]
    fn find_top_level_resequencer_requiring_split_detects_single() {
        let steps = vec![
            make_log("A"),
            make_resequence(),
            make_set_body("B"),
            make_log("C"),
        ];
        let result = find_top_level_resequencer_requiring_split(&steps);
        let info = result
            .expect("should succeed")
            .expect("should detect resequence");
        assert_eq!(info.index, 1);
        let pre = &steps[..info.index];
        let post = &steps[info.index + 1..];
        assert_eq!(pre.len(), 1);
        assert!(matches!(pre[0], BuilderStep::Log { .. }));
        assert_eq!(post.len(), 2);
    }

    #[test]
    fn find_top_level_resequencer_requiring_split_rejects_multiple() {
        let steps = vec![make_resequence(), make_resequence()];
        let result = find_top_level_resequencer_requiring_split(&steps);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, CamelError::RouteError(_)),
            "should be RouteError, got {err:?}"
        );
    }

    #[test]
    fn find_top_level_resequencer_requiring_split_none_when_absent() {
        let steps = vec![make_log("A"), make_set_body("B")];
        let result = find_top_level_resequencer_requiring_split(&steps);
        assert!(result.expect("should succeed").is_none());
    }

    #[test]
    fn assert_no_mixed_top_level_splits_rejects_aggregate_plus_resequencer() {
        let steps = vec![make_aggregate_with_timeout(), make_resequence()];
        let result = assert_no_mixed_top_level_splits(&steps);
        assert!(result.is_err());
    }

    #[test]
    fn assert_no_mixed_top_level_splits_rejects_force_completion_aggregate() {
        let steps = vec![make_resequence(), make_aggregate_with_force_completion()];
        let result = assert_no_mixed_top_level_splits(&steps);
        assert!(result.is_err());
    }

    #[test]
    fn assert_no_mixed_top_level_splits_allows_no_split() {
        let steps = vec![make_log("A"), make_set_body("B")];
        let result = assert_no_mixed_top_level_splits(&steps);
        assert!(result.is_ok());
    }

    #[test]
    fn assert_no_mixed_top_level_splits_allows_aggregate_only() {
        let steps = vec![make_aggregate_with_timeout()];
        let result = assert_no_mixed_top_level_splits(&steps);
        assert!(result.is_ok());
    }

    #[test]
    fn assert_no_mixed_top_level_splits_allows_simple_aggregate() {
        let steps = vec![make_aggregate_simple(), make_resequence()];
        let result = assert_no_mixed_top_level_splits(&steps);
        assert!(result.is_ok());
    }

    #[test]
    fn assert_no_mixed_top_level_splits_allows_resequence_only() {
        let steps = vec![make_resequence()];
        let result = assert_no_mixed_top_level_splits(&steps);
        assert!(result.is_ok());
    }

    #[test]
    fn find_top_level_resequencer_requiring_split_at_index_zero() {
        // Resequence is the first step — pre is empty, post is [Log, SetBody]
        let steps = vec![make_resequence(), make_log("C"), make_set_body("D")];
        let result = find_top_level_resequencer_requiring_split(&steps);
        let info = result
            .expect("should succeed")
            .expect("should detect resequence at index 0");
        assert_eq!(info.index, 0);
        let pre = &steps[..info.index];
        let post = &steps[info.index + 1..];
        assert!(
            pre.is_empty(),
            "pre should be empty when resequence is first"
        );
        assert_eq!(post.len(), 2, "post should contain Log and SetBody");
    }

    #[test]
    fn find_top_level_resequencer_requiring_split_at_last_index() {
        // Resequence is the last step — pre is [Log, SetBody], post is empty
        let steps = vec![make_log("A"), make_set_body("B"), make_resequence()];
        let result = find_top_level_resequencer_requiring_split(&steps);
        let info = result
            .expect("should succeed")
            .expect("should detect resequence at last index");
        assert_eq!(info.index, 2);
        let pre = &steps[..info.index];
        let post = &steps[info.index + 1..];
        assert_eq!(pre.len(), 2, "pre should contain Log and SetBody");
        assert!(
            post.is_empty(),
            "post should be empty when resequence is last"
        );
    }
}
