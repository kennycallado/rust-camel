//! End-to-end integration test for `continued=true` error handler.
//!
//! Tests that a pipeline with an error handler configured with
//! `onException(ProcessorError, continued=true)` continues to subsequent steps
//! after a failing step, rather than propagating the error.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use camel_api::error_handler::{
    BoundaryKind, ErrorHandlerConfig, PolicyId, RetryOutcome, StepDisposition,
};
use camel_api::{
    BoxProcessor, BoxProcessorExt, CamelError, CircuitBreakerConfig, Exchange, Message,
};
use camel_core::route::{CompiledStep, RouteChannelService, compose_pipeline_with_handler};
use camel_processor::error_handler::{DefaultRouteErrorHandler, RouteErrorHandler};
use camel_processor::{CircuitBreakerDecision, CircuitBreakerGate};
use tower::ServiceExt;

#[tokio::test]
async fn continued_e2e_step3_executes_after_step2_failure() {
    // Step 1: identity (always succeeds)
    let step1 = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));

    // Step 2: always fails with ProcessorError
    let step2 = BoxProcessor::from_fn(|_ex| {
        Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
    });

    // Step 3: records that it was executed
    let step3_hit = Arc::new(AtomicBool::new(false));
    let hit = step3_hit.clone();
    let step3 = BoxProcessor::from_fn(move |ex| {
        let hit = hit.clone();
        Box::pin(async move {
            hit.store(true, Ordering::SeqCst);
            Ok(ex)
        })
    });

    // Build error handler config with continued=true for ProcessorError
    let config = ErrorHandlerConfig::log_only()
        .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
        .continued(true)
        .build();

    // Build the DefaultRouteErrorHandler from the config policies
    let policies: Vec<_> = config.policies.into_iter().map(|p| (p, None)).collect();
    let handler: Arc<dyn RouteErrorHandler> =
        Arc::new(DefaultRouteErrorHandler::new(None, policies));

    // Compose a pipeline with the handler
    let pipeline = compose_pipeline_with_handler(
        vec![
            CompiledStep::Process {
                processor: step1,
                body_contract: None,
                lifecycle: None,
            },
            CompiledStep::Process {
                processor: step2,
                body_contract: None,
                lifecycle: None,
            },
            CompiledStep::Process {
                processor: step3,
                body_contract: None,
                lifecycle: None,
            },
        ],
        Some(handler),
    );

    // Execute the exchange
    let exchange = Exchange::new(Message::new("test continued_e2e"));
    let result = pipeline.oneshot(exchange).await;

    // Assert: pipeline succeeds (no error propagated)
    assert!(
        result.is_ok(),
        "pipeline should return Ok with continued=true, got: {:?}",
        result
    );

    // Assert: step3 was executed
    assert!(
        step3_hit.load(Ordering::SeqCst),
        "step 3 should have executed after continued=true cleared the error"
    );
}

/// Test double for RouteErrorHandler that returns Continued disposition for
/// step errors and absorbs boundary errors.
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

#[tokio::test]
async fn continued_e2e_route_channel_service_with_cb_gate() {
    // Step 1: identity (always succeeds)
    let step1 = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));

    // Step 2: always fails with ProcessorError
    let step2 = BoxProcessor::from_fn(|_ex| {
        Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
    });

    // Step 3: records that it was executed
    let step3_hit = Arc::new(AtomicBool::new(false));
    let hit = step3_hit.clone();
    let step3 = BoxProcessor::from_fn(move |ex| {
        let hit = hit.clone();
        Box::pin(async move {
            hit.store(true, Ordering::SeqCst);
            Ok(ex)
        })
    });

    // Handler with Continued disposition
    let handler: Arc<dyn RouteErrorHandler> = Arc::new(ContinuedHandler);

    // Build pipeline with handler injected for step-level recovery
    let pipeline = compose_pipeline_with_handler(
        vec![
            CompiledStep::Process {
                processor: step1,
                body_contract: None,
                lifecycle: None,
            },
            CompiledStep::Process {
                processor: step2,
                body_contract: None,
                lifecycle: None,
            },
            CompiledStep::Process {
                processor: step3,
                body_contract: None,
                lifecycle: None,
            },
        ],
        Some(handler.clone()),
    );

    // CircuitBreakerGate — shared state via Arc
    let cb_gate = CircuitBreakerGate::new(CircuitBreakerConfig {
        failure_threshold: 1,
        open_duration: Duration::from_secs(60),
        success_threshold: 1,
        fallback: None,
    });

    // Clone so we can inspect state after the call (shared Arc<Mutex<...>>)
    let cb_gate_clone = cb_gate.clone();

    // Build RouteChannelService — the full gate chain
    let channel = RouteChannelService::new(handler.clone(), None, Some(cb_gate), pipeline, false);
    let service = BoxProcessor::new(channel);

    // Execute the exchange
    let exchange = Exchange::new(Message::new("test route_channel_e2e"));
    let result = service.oneshot(exchange).await;

    // 1. Assert pipeline succeeds (error handled with Continued)
    assert!(
        result.is_ok(),
        "pipeline should return Ok with continued=true, got: {:?}",
        result
    );

    // 2. Assert step3 was executed
    assert!(
        step3_hit.load(Ordering::SeqCst),
        "step 3 should have executed after continued=true cleared the error"
    );

    // 3. Assert CircuitBreakerGate's after_result received the handled result.
    //    Because continued=true clears the error, the pipeline returns Ok, so
    //    after_result receives Ok(...) and the gate stays Closed.
    //    We verify by calling before_call() — it must return Allow (not Reject).
    match cb_gate_clone.before_call() {
        CircuitBreakerDecision::Allow => { /* correct — gate stayed Closed */ }
        _ => panic!(
            "CB gate should be Allow (after_result received Ok), but got a different decision"
        ),
    }
}
