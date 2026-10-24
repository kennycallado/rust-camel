//! Pinning tests for error-handler retry independent of a dead letter channel.
//!
//! Oracle ruling (e_gpt, bd rc-bbpg3): the retry defect was NOT reproducible on
//! main. These tests lock in the correct behavior instead of manufacturing a
//! failing baseline.
//!
//! Each case uses its own counter and a failing step that always returns
//! `CamelError::ProcessorError`. The retry backoff is 1 ms with multiplier 1.0,
//! so the suite never waits for a real backoff.
//!
//! Case matrix:
//! 1. Route-level catch-all retry, no DLC: initial attempt + 3 retries, error
//!    propagates.
//! 2. Route-level catch-all retry, with DLC: 4 step invocations, DLC invoked
//!    exactly once on exhaustion with the original error set, error propagates.
//! 3. on_exceptions clause retry, no DLC: 4 step invocations, error propagates.
//! 4. on_exceptions clause retry, with DLC: 4 step invocations, DLC invoked
//!    exactly once with the original error set, error propagates.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message, SpanKindHint};
use camel_core::route::{
    CompiledStep, PipelineRuntimeCtx, RouteChannelService, compose_pipeline_with_handler,
};
use camel_processor::error_handler::{DefaultRouteErrorHandler, RouteErrorHandler};
use tower::ServiceExt;

const MAX_ATTEMPTS: u32 = 3;
const EXPECTED_STEP_INVOCATIONS: usize = 1 + MAX_ATTEMPTS as usize;

fn process_step(processor: BoxProcessor) -> CompiledStep {
    CompiledStep::Process {
        kind_hint: SpanKindHint::Internal,
        processor,
        body_contract: None,
        lifecycle: None,
        label: None,
        to_uri: None,
    }
}

/// A step that counts every invocation and always fails with `ProcessorError`.
fn failing_step(counter: Arc<AtomicUsize>) -> BoxProcessor {
    BoxProcessor::from_fn(move |_ex| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(CamelError::ProcessorError("boom".into()))
        })
    })
}

/// A DLC producer that counts every delivery, records the delivered error
/// state, and accepts the exchange.
fn recording_dlc(
    counter: Arc<AtomicUsize>,
    delivered_errors: Arc<Mutex<Vec<Option<CamelError>>>>,
) -> BoxProcessor {
    BoxProcessor::from_fn(move |ex| {
        let counter = Arc::clone(&counter);
        let delivered_errors = Arc::clone(&delivered_errors);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            delivered_errors
                .lock()
                .expect("delivered-error record mutex poisoned")
                .push(ex.error.clone());
            Ok(ex)
        })
    })
}

fn build_handler(
    config: ErrorHandlerConfig,
    dlc: Option<BoxProcessor>,
) -> Arc<dyn RouteErrorHandler> {
    let policies = config.policies.into_iter().map(|p| (p, None)).collect();
    Arc::new(DefaultRouteErrorHandler::new(dlc, policies))
}

/// Run one failing step through the production route composition.
async fn run_route(
    config: ErrorHandlerConfig,
    step_counter: Arc<AtomicUsize>,
    dlc: Option<BoxProcessor>,
) -> Result<Exchange, CamelError> {
    let handler = build_handler(config, dlc);
    let pipeline = compose_pipeline_with_handler(
        vec![process_step(failing_step(step_counter))],
        Some(handler.clone()),
        PipelineRuntimeCtx::compile_time(),
    );
    let service = BoxProcessor::new(RouteChannelService::new(
        handler, None, None, pipeline, false,
    ));
    service
        .oneshot(Exchange::new(Message::new("error-handler retry pinning")))
        .await
}

fn route_level_retry_config() -> ErrorHandlerConfig {
    ErrorHandlerConfig::log_only()
        .on_exception(|_| true)
        .retry(MAX_ATTEMPTS)
        .with_backoff(Duration::from_millis(1), 1.0, Duration::from_millis(1))
        .build()
}

fn clause_retry_config() -> ErrorHandlerConfig {
    ErrorHandlerConfig::log_only()
        .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
        .retry(MAX_ATTEMPTS)
        .with_backoff(Duration::from_millis(1), 1.0, Duration::from_millis(1))
        .build()
}

fn assert_original_error_propagates(result: Result<Exchange, CamelError>) {
    match result {
        Err(CamelError::ProcessorError(message)) => {
            assert_eq!(message, "boom", "the original step error must propagate");
        }
        other => panic!("expected the original ProcessorError to propagate, got {other:?}"),
    }
}

/// Assert the DLC was invoked exactly once and received the original error.
fn assert_dlc_received_original_error(delivered_errors: &Arc<Mutex<Vec<Option<CamelError>>>>) {
    let recorded = delivered_errors
        .lock()
        .expect("delivered-error record mutex poisoned");
    assert_eq!(
        recorded.len(),
        1,
        "the exhausted exchange must reach the DLC exactly once"
    );
    match recorded.first().and_then(Option::as_ref) {
        Some(CamelError::ProcessorError(message)) => assert_eq!(
            message, "boom",
            "the DLC must receive the exchange with the original error set"
        ),
        other => panic!(
            "expected the DLC to receive the original ProcessorError set on the exchange, got {other:?}"
        ),
    }
}

#[tokio::test]
async fn route_retry_without_dlc_retries_then_propagates() {
    let step_runs = Arc::new(AtomicUsize::new(0));
    let result = run_route(route_level_retry_config(), Arc::clone(&step_runs), None).await;

    assert_eq!(
        step_runs.load(Ordering::SeqCst),
        EXPECTED_STEP_INVOCATIONS,
        "route-level retry must run the initial attempt plus max_attempts retries"
    );
    assert_original_error_propagates(result);
}

#[tokio::test]
async fn route_retry_with_dlc_retries_then_delivers_once() {
    let step_runs = Arc::new(AtomicUsize::new(0));
    let dlc_runs = Arc::new(AtomicUsize::new(0));
    let delivered_errors = Arc::new(Mutex::new(Vec::new()));
    let dlc = recording_dlc(Arc::clone(&dlc_runs), Arc::clone(&delivered_errors));
    let result = run_route(
        route_level_retry_config(),
        Arc::clone(&step_runs),
        Some(dlc),
    )
    .await;

    assert_eq!(
        step_runs.load(Ordering::SeqCst),
        EXPECTED_STEP_INVOCATIONS,
        "configuring a DLC must not change the retry count"
    );
    assert_eq!(
        dlc_runs.load(Ordering::SeqCst),
        1,
        "the exhausted exchange must reach the DLC exactly once"
    );
    assert_dlc_received_original_error(&delivered_errors);
    assert_original_error_propagates(result);
}

#[tokio::test]
async fn clause_retry_without_dlc_retries_then_propagates() {
    let step_runs = Arc::new(AtomicUsize::new(0));
    let result = run_route(clause_retry_config(), Arc::clone(&step_runs), None).await;

    assert_eq!(
        step_runs.load(Ordering::SeqCst),
        EXPECTED_STEP_INVOCATIONS,
        "clause-level retry must run the initial attempt plus max_attempts retries"
    );
    assert_original_error_propagates(result);
}

#[tokio::test]
async fn clause_retry_with_dlc_retries_then_delivers_once() {
    let step_runs = Arc::new(AtomicUsize::new(0));
    let dlc_runs = Arc::new(AtomicUsize::new(0));
    let delivered_errors = Arc::new(Mutex::new(Vec::new()));
    let dlc = recording_dlc(Arc::clone(&dlc_runs), Arc::clone(&delivered_errors));
    let result = run_route(clause_retry_config(), Arc::clone(&step_runs), Some(dlc)).await;

    assert_eq!(
        step_runs.load(Ordering::SeqCst),
        EXPECTED_STEP_INVOCATIONS,
        "configuring a DLC must not change the clause retry count"
    );
    assert_eq!(
        dlc_runs.load(Ordering::SeqCst),
        1,
        "the exhausted exchange must reach the DLC exactly once"
    );
    assert_dlc_received_original_error(&delivered_errors);
    assert_original_error_propagates(result);
}
