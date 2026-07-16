//! B3 — error-disposition metrics integration tests.
//!
//! Verifies that `run_steps` emits a `pipeline_disposition` counter at every
//! disposition arm in [`run_steps`]:
//!
//! - no handler (no disposition arm reached) → no counter
//! - `Recovered` → `recovered`
//! - `Stopped`   → `stopped`
//! - `Exhausted + Propagate` → `propagated`
//! - `Exhausted + Handled`   → `handled`
//! - `Exhausted + Continued` → `continued`
//! - `Exhausted + handler Err` → `handler_error`
//!
//! Each counter MUST carry a `route_id` label matching the ctx.
//!
//! [`run_steps`]: crate::lifecycle::adapters::route_compiler::run_steps

use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_api::error_handler::{BoundaryKind, PolicyId, RetryOutcome, StepDisposition};
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message, MetricsCollector};
use camel_core::route::{
    CompiledStep, PipelineRuntimeCtx, compose_pipeline, compose_pipeline_with_handler,
};
use camel_processor::error_handler::RouteErrorHandler;
use tower::Service;

// ── Test metrics double ─────────────────────────────────────────────────

#[derive(Default)]
#[allow(clippy::type_complexity)]
struct RecMetrics {
    counters: Mutex<Vec<(String, Vec<(String, String)>)>>,
}

impl RecMetrics {
    fn names(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.counters.lock().expect("metrics lock").clone()
    }

    fn find(&self, disposition: &str) -> Option<Vec<(String, String)>> {
        self.counters
            .lock()
            .expect("metrics lock")
            .iter()
            .find(|(name, labels)| {
                name == "pipeline_disposition"
                    && labels
                        .iter()
                        .any(|(k, v)| k == "disposition" && v == disposition)
            })
            .map(|(_, labels)| labels.clone())
    }
}

impl MetricsCollector for RecMetrics {
    fn record_exchange_duration(&self, _: &str, _: Duration) {}
    fn increment_errors(&self, _: &str, _: &str) {}
    fn increment_exchanges(&self, _: &str) {}
    fn set_queue_depth(&self, _: &str, _: usize) {}
    fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}

    fn record_counter(&self, name: &str, _value: f64, labels: &[(&str, &str)]) {
        self.counters.lock().expect("metrics lock").push((
            name.to_string(),
            labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ));
    }
}

fn ctx_with(metrics: Arc<RecMetrics>) -> PipelineRuntimeCtx {
    PipelineRuntimeCtx {
        metrics: metrics as Arc<dyn MetricsCollector>,
        route_id: Arc::from("test-route"),
    }
}

fn failing_step() -> CompiledStep {
    CompiledStep::Process {
        processor: BoxProcessor::from_fn(|_| {
            Box::pin(async {
                Err::<Exchange, CamelError>(CamelError::ProcessorError("boom".into()))
            })
        }),
        body_contract: None,
        lifecycle: None,
    }
}

fn pass_through_step() -> CompiledStep {
    CompiledStep::Process {
        processor: BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })),
        body_contract: None,
        lifecycle: None,
    }
}

// ── Handler test doubles ────────────────────────────────────────────────

struct RecoveredHandler;
#[async_trait::async_trait]
impl RouteErrorHandler for RecoveredHandler {
    fn match_policy(&self, _: &CamelError) -> Option<PolicyId> {
        Some(PolicyId(0))
    }
    async fn retry_step(
        &self,
        _: Option<PolicyId>,
        _: &mut dyn camel_api::error_handler::RetryableStep,
        original: Exchange,
        _error: CamelError,
    ) -> RetryOutcome {
        // Fabricate a successful exchange so run_steps surfaces Recovered.
        // The retry mechanism is exercised by other tests; this fixture
        // only needs to drive the Recovered branch in run_steps.
        RetryOutcome::Recovered(original)
    }
    async fn handle_step(
        &self,
        _: Option<PolicyId>,
        _: Exchange,
        _: CamelError,
    ) -> Result<StepDisposition, CamelError> {
        unreachable!("recovered path should not call handle_step")
    }
    async fn handle_boundary(
        &self,
        _: BoundaryKind,
        ex: Exchange,
        _: CamelError,
    ) -> Result<Exchange, CamelError> {
        Ok(ex)
    }
}

struct StoppedHandler;
#[async_trait::async_trait]
impl RouteErrorHandler for StoppedHandler {
    fn match_policy(&self, _: &CamelError) -> Option<PolicyId> {
        Some(PolicyId(0))
    }
    async fn retry_step(
        &self,
        _: Option<PolicyId>,
        _: &mut dyn camel_api::error_handler::RetryableStep,
        original: Exchange,
        _error: CamelError,
    ) -> RetryOutcome {
        RetryOutcome::Stopped(original)
    }
    async fn handle_step(
        &self,
        _: Option<PolicyId>,
        _: Exchange,
        _: CamelError,
    ) -> Result<StepDisposition, CamelError> {
        unreachable!("stopped path should not call handle_step")
    }
    async fn handle_boundary(
        &self,
        _: BoundaryKind,
        ex: Exchange,
        _: CamelError,
    ) -> Result<Exchange, CamelError> {
        Ok(ex)
    }
}

struct PropagateHandler;
#[async_trait::async_trait]
impl RouteErrorHandler for PropagateHandler {
    fn match_policy(&self, _: &CamelError) -> Option<PolicyId> {
        None
    }
    async fn retry_step(
        &self,
        _: Option<PolicyId>,
        _: &mut dyn camel_api::error_handler::RetryableStep,
        original: Exchange,
        error: CamelError,
    ) -> RetryOutcome {
        RetryOutcome::Exhausted {
            exchange: original,
            error,
            policy: None,
        }
    }
    async fn handle_step(
        &self,
        _: Option<PolicyId>,
        _ex: Exchange,
        error: CamelError,
    ) -> Result<StepDisposition, CamelError> {
        Ok(StepDisposition::Propagate(error))
    }
    async fn handle_boundary(
        &self,
        _: BoundaryKind,
        ex: Exchange,
        _: CamelError,
    ) -> Result<Exchange, CamelError> {
        Ok(ex)
    }
}

struct HandledHandler;
#[async_trait::async_trait]
impl RouteErrorHandler for HandledHandler {
    fn match_policy(&self, _: &CamelError) -> Option<PolicyId> {
        Some(PolicyId(0))
    }
    async fn retry_step(
        &self,
        _: Option<PolicyId>,
        _: &mut dyn camel_api::error_handler::RetryableStep,
        original: Exchange,
        error: CamelError,
    ) -> RetryOutcome {
        RetryOutcome::Exhausted {
            exchange: original,
            error,
            policy: Some(PolicyId(0)),
        }
    }
    async fn handle_step(
        &self,
        _: Option<PolicyId>,
        ex: Exchange,
        _: CamelError,
    ) -> Result<StepDisposition, CamelError> {
        Ok(StepDisposition::Handled(ex))
    }
    async fn handle_boundary(
        &self,
        _: BoundaryKind,
        ex: Exchange,
        _: CamelError,
    ) -> Result<Exchange, CamelError> {
        Ok(ex)
    }
}

struct ContinuedHandler;
#[async_trait::async_trait]
impl RouteErrorHandler for ContinuedHandler {
    fn match_policy(&self, _: &CamelError) -> Option<PolicyId> {
        Some(PolicyId(0))
    }
    async fn retry_step(
        &self,
        _: Option<PolicyId>,
        _: &mut dyn camel_api::error_handler::RetryableStep,
        original: Exchange,
        error: CamelError,
    ) -> RetryOutcome {
        RetryOutcome::Exhausted {
            exchange: original,
            error,
            policy: Some(PolicyId(0)),
        }
    }
    async fn handle_step(
        &self,
        _: Option<PolicyId>,
        mut ex: Exchange,
        _: CamelError,
    ) -> Result<StepDisposition, CamelError> {
        ex.clear_error();
        Ok(StepDisposition::Continued(ex))
    }
    async fn handle_boundary(
        &self,
        _: BoundaryKind,
        ex: Exchange,
        _: CamelError,
    ) -> Result<Exchange, CamelError> {
        Ok(ex)
    }
}

struct HandlerErrorHandler;
#[async_trait::async_trait]
impl RouteErrorHandler for HandlerErrorHandler {
    fn match_policy(&self, _: &CamelError) -> Option<PolicyId> {
        Some(PolicyId(0))
    }
    async fn retry_step(
        &self,
        _: Option<PolicyId>,
        _: &mut dyn camel_api::error_handler::RetryableStep,
        original: Exchange,
        error: CamelError,
    ) -> RetryOutcome {
        RetryOutcome::Exhausted {
            exchange: original,
            error,
            policy: Some(PolicyId(0)),
        }
    }
    async fn handle_step(
        &self,
        _: Option<PolicyId>,
        _: Exchange,
        _: CamelError,
    ) -> Result<StepDisposition, CamelError> {
        Err(CamelError::ProcessorError("handler exploded".into()))
    }
    async fn handle_boundary(
        &self,
        _: BoundaryKind,
        ex: Exchange,
        _: CamelError,
    ) -> Result<Exchange, CamelError> {
        Ok(ex)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn failed_step_without_handler_emits_no_disposition_counter() {
    let metrics = Arc::new(RecMetrics::default());
    let mut pipeline = compose_pipeline(vec![failing_step()], ctx_with(metrics.clone()));
    let _ = pipeline.call(Exchange::new(Message::new("test"))).await;
    let counters = metrics.names();
    let disposition_counters: Vec<_> = counters
        .iter()
        .filter(|(n, _)| n == "pipeline_disposition")
        .collect();
    assert!(
        disposition_counters.is_empty(),
        "no handler → no disposition counters, got: {disposition_counters:?}"
    );
}

#[tokio::test]
async fn recovered_disposition_emits_counter_with_route_id() {
    let metrics = Arc::new(RecMetrics::default());
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(RecoveredHandler);
    // First step fails; RecoveredHandler's retry_step re-invokes the step
    // and surfaces Recovered. The second step never runs because the
    // recovered exchange is the final exchange.
    let mut pipeline = compose_pipeline_with_handler(
        vec![failing_step()],
        Some(handler),
        ctx_with(metrics.clone()),
    );
    let _ = pipeline.call(Exchange::new(Message::new("test"))).await;

    let labels = metrics
        .find("recovered")
        .expect("expected pipeline_disposition{disposition=recovered} counter");
    assert!(
        labels
            .iter()
            .any(|(k, v)| k == "route_id" && v == "test-route"),
        "route_id label missing or wrong: {labels:?}"
    );
}

#[tokio::test]
async fn stopped_disposition_emits_counter() {
    let metrics = Arc::new(RecMetrics::default());
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(StoppedHandler);
    let mut pipeline = compose_pipeline_with_handler(
        vec![failing_step()],
        Some(handler),
        ctx_with(metrics.clone()),
    );
    let _ = pipeline.call(Exchange::new(Message::new("test"))).await;

    assert!(
        metrics.find("stopped").is_some(),
        "expected pipeline_disposition{{disposition=stopped}} counter, got: {:?}",
        metrics.names()
    );
}

#[tokio::test]
async fn propagated_disposition_emits_counter() {
    let metrics = Arc::new(RecMetrics::default());
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(PropagateHandler);
    let mut pipeline = compose_pipeline_with_handler(
        vec![failing_step()],
        Some(handler),
        ctx_with(metrics.clone()),
    );
    let _ = pipeline.call(Exchange::new(Message::new("test"))).await;

    assert!(
        metrics.find("propagated").is_some(),
        "expected pipeline_disposition{{disposition=propagated}} counter, got: {:?}",
        metrics.names()
    );
}

#[tokio::test]
async fn handled_disposition_emits_counter() {
    let metrics = Arc::new(RecMetrics::default());
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(HandledHandler);
    let mut pipeline = compose_pipeline_with_handler(
        vec![failing_step()],
        Some(handler),
        ctx_with(metrics.clone()),
    );
    let _ = pipeline.call(Exchange::new(Message::new("test"))).await;

    assert!(
        metrics.find("handled").is_some(),
        "expected pipeline_disposition{{disposition=handled}} counter, got: {:?}",
        metrics.names()
    );
}

#[tokio::test]
async fn continued_disposition_emits_counter() {
    let metrics = Arc::new(RecMetrics::default());
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(ContinuedHandler);
    let mut pipeline = compose_pipeline_with_handler(
        vec![failing_step(), pass_through_step()],
        Some(handler),
        ctx_with(metrics.clone()),
    );
    let _ = pipeline.call(Exchange::new(Message::new("test"))).await;

    assert!(
        metrics.find("continued").is_some(),
        "expected pipeline_disposition{{disposition=continued}} counter, got: {:?}",
        metrics.names()
    );
}

#[tokio::test]
async fn handler_error_emits_counter() {
    let metrics = Arc::new(RecMetrics::default());
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(HandlerErrorHandler);
    let mut pipeline = compose_pipeline_with_handler(
        vec![failing_step()],
        Some(handler),
        ctx_with(metrics.clone()),
    );
    let _ = pipeline.call(Exchange::new(Message::new("test"))).await;

    assert!(
        metrics.find("handler_error").is_some(),
        "expected pipeline_disposition{{disposition=handler_error}} counter, got: {:?}",
        metrics.names()
    );
}
