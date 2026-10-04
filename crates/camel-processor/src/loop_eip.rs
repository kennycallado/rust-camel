use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Service, ServiceExt};

use camel_api::loop_eip::{LoopConfig, LoopMode};
use camel_api::{BoxProcessor, CamelError, Exchange, Value};

pub const CAMEL_LOOP_INDEX: &str = "CamelLoopIndex";
pub const CAMEL_LOOP_SIZE: &str = "CamelLoopSize";

#[derive(Clone)]
pub struct LoopService {
    config: LoopConfig,
    sub_pipeline: BoxProcessor,
}

impl LoopService {
    pub fn new(config: LoopConfig, sub_pipeline: BoxProcessor) -> Self {
        Self {
            config,
            sub_pipeline,
        }
    }
}

impl Service<Exchange> for LoopService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let mut pipeline = self.sub_pipeline.clone();

        Box::pin(async move {
            match config.mode {
                LoopMode::Count(n) => {
                    // clamp to config.max_iterations. The audit finding was that
                    // Count(n) had no cap (only While did); a `count: u32::MAX`
                    // would exhaust CPU. The clamp is applied uniformly to Count
                    // and While. The CAMEL_LOOP_SIZE property is set to the
                    // *clamped* count so downstream steps observe the effective
                    // iteration count.
                    let n_clamped = n.min(config.max_iterations);
                    if n > config.max_iterations {
                        tracing::warn!(
                            requested = n,
                            clamped_to = config.max_iterations,
                            "LoopMode::Count exceeded max_iterations; clamping"
                        );
                    }
                    exchange.set_property(CAMEL_LOOP_SIZE, Value::from(n_clamped as u64));
                    for i in 0..n_clamped {
                        exchange.set_property(CAMEL_LOOP_INDEX, Value::from(i as u64));
                        exchange = pipeline.ready().await?.call(exchange).await?;
                    }
                }
                LoopMode::While(ref predicate) => {
                    exchange.set_property(CAMEL_LOOP_SIZE, Value::from(0u64));
                    // Every predicate evaluation error propagates (never
                    // loop-end success), including the post-exhaustion
                    // safety-guard probe. `exhausted` tracks whether that
                    // re-check applies so a side-effectful predicate is not
                    // re-evaluated after a normal false exit.
                    let mut exhausted = true;
                    for i in 0..config.max_iterations {
                        match predicate.matches(&exchange).await {
                            Ok(true) => {}
                            Ok(false) => {
                                exhausted = false;
                                break;
                            }
                            Err(err) => return Err(err),
                        }
                        exchange.set_property(CAMEL_LOOP_INDEX, Value::from(i as u64));
                        exchange = pipeline.ready().await?.call(exchange).await?;
                    }
                    if exhausted {
                        match predicate.matches(&exchange).await {
                            Ok(true) => {
                                tracing::warn!(
                                    "Loop while-mode hit max_iterations ({}) safety guard. Predicate still true.",
                                    config.max_iterations
                                );
                            }
                            Ok(false) => {}
                            Err(err) => return Err(err),
                        }
                    }
                }
                // Future loop modes: no iteration, pass exchange through unchanged.
                _ => {}
            }
            Ok(exchange)
        })
    }
}

// ── LoopSegment (ADR-0025 OutcomePipeline) ─────────────────────────────

/// Outcome-aware structural EIP segment for the Loop pattern.
///
/// Operates at the `PipelineOutcome` layer so that `Stopped(ex)` from a
/// sub-step (e.g. Stop EIP) is preserved with the exchange including all
/// mutations. Supports both Count and While modes, mirroring `LoopService`
/// semantics exactly.
///
/// Unlike `LoopService` (which operates at the Tower layer), `LoopSegment`
/// correctly short-circuits on `PipelineOutcome::Stopped` or `Failed`.
pub struct LoopSegment {
    pub config: camel_api::loop_eip::LoopConfig,
    pub body: camel_api::OutcomeSegment,
}

impl Clone for LoopSegment {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            body: self.body.clone(),
        }
    }
}

impl camel_api::OutcomePipeline for LoopSegment {
    fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: camel_api::Exchange,
    ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
        use camel_api::{PipelineOutcome, Value};

        let config = self.config.clone();
        let body = &mut self.body;

        Box::pin(async move {
            match config.mode {
                camel_api::loop_eip::LoopMode::Count(n) => {
                    let n_clamped = n.min(config.max_iterations);
                    if n > config.max_iterations {
                        tracing::warn!(
                            requested = n,
                            clamped_to = config.max_iterations,
                            "LoopMode::Count exceeded max_iterations; clamping"
                        );
                    }
                    let mut ex = exchange;
                    ex.set_property(CAMEL_LOOP_SIZE, Value::from(n_clamped as u64));
                    for i in 0..n_clamped {
                        ex.set_property(CAMEL_LOOP_INDEX, Value::from(i as u64));
                        match body.run(ex).await {
                            PipelineOutcome::Completed(next) => {
                                ex = next;
                            }
                            other => return other,
                        }
                    }
                    PipelineOutcome::Completed(ex)
                }
                camel_api::loop_eip::LoopMode::While(ref predicate) => {
                    let mut ex = exchange;
                    ex.set_property(CAMEL_LOOP_SIZE, Value::from(0u64));
                    // Every predicate evaluation error propagates as
                    // `Failed(err)` (never as loop-end Completed), including
                    // the post-exhaustion safety-guard probe.
                    let mut i = 0u64;
                    let mut exhausted = true;
                    while i < config.max_iterations as u64 {
                        match predicate.matches(&ex).await {
                            Ok(true) => {}
                            Ok(false) => {
                                exhausted = false;
                                break;
                            }
                            Err(err) => return PipelineOutcome::Failed(err),
                        }
                        ex.set_property(CAMEL_LOOP_INDEX, Value::from(i));
                        match body.run(ex).await {
                            PipelineOutcome::Completed(next) => {
                                ex = next;
                            }
                            other => return other,
                        }
                        i += 1;
                    }
                    if exhausted {
                        match predicate.matches(&ex).await {
                            Ok(true) => {
                                tracing::warn!(
                                    "Loop while-mode hit max_iterations ({}) safety guard. Predicate still true.",
                                    config.max_iterations
                                );
                            }
                            Ok(false) => {}
                            Err(err) => return PipelineOutcome::Failed(err),
                        }
                    }
                    PipelineOutcome::Completed(ex)
                }
                // Future loop modes: no iteration, complete with the exchange unchanged.
                _ => PipelineOutcome::Completed(exchange),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use camel_api::loop_eip::{LoopConfig, LoopMode, MAX_LOOP_ITERATIONS};
    use camel_api::{
        Body, BoxProcessor, BoxProcessorExt, CamelError, Exchange, FilterPredicate,
        IdentityProcessor, Message,
    };
    use tower::{Service, ServiceExt};

    use super::{CAMEL_LOOP_INDEX, CAMEL_LOOP_SIZE, LoopSegment, LoopService};

    fn identity_pipeline() -> BoxProcessor {
        BoxProcessor::new(IdentityProcessor)
    }

    fn counter_pipeline(counter: Arc<AtomicUsize>) -> BoxProcessor {
        BoxProcessor::from_fn(move |exchange: Exchange| {
            let counter = Arc::clone(&counter);
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(exchange)
            })
        })
    }

    #[tokio::test]
    async fn test_loop_count_iterates_n_times() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = LoopConfig::new(LoopMode::Count(3));
        let mut service = LoopService::new(config, counter_pipeline(Arc::clone(&counter)));

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_loop_count_sets_properties() {
        let seen_indices = Arc::new(Mutex::new(Vec::<u64>::new()));
        let seen_indices_for_pipeline = Arc::clone(&seen_indices);

        let pipeline = BoxProcessor::from_fn(move |exchange: Exchange| {
            let seen_indices = Arc::clone(&seen_indices_for_pipeline);
            Box::pin(async move {
                if let Some(index) = exchange.property(CAMEL_LOOP_INDEX).and_then(|v| v.as_u64()) {
                    seen_indices.lock().unwrap().push(index);
                }
                Ok(exchange)
            })
        });

        let config = LoopConfig::new(LoopMode::Count(3));
        let mut service = LoopService::new(config, pipeline);

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(*seen_indices.lock().unwrap(), vec![0, 1, 2]);
        assert_eq!(
            result.property(CAMEL_LOOP_SIZE).and_then(|v| v.as_u64()),
            Some(3)
        );
    }

    #[tokio::test]
    async fn test_loop_count_zero_is_noop() {
        let config = LoopConfig::new(LoopMode::Count(0));
        let mut service = LoopService::new(config, identity_pipeline());

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("test"));
        assert_eq!(
            result.property(CAMEL_LOOP_SIZE).and_then(|v| v.as_u64()),
            Some(0)
        );
        assert!(result.property(CAMEL_LOOP_INDEX).is_none());
    }

    #[tokio::test]
    async fn test_loop_while_stops_when_predicate_false() {
        let counter = Arc::new(AtomicUsize::new(0));

        let predicate = FilterPredicate::new(|exchange: &Exchange| {
            exchange
                .property("iterations")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                < 2
        });

        let counter_for_pipeline = Arc::clone(&counter);
        let pipeline = BoxProcessor::from_fn(move |mut exchange: Exchange| {
            let counter = Arc::clone(&counter_for_pipeline);
            Box::pin(async move {
                let current = exchange
                    .property("iterations")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                exchange.set_property("iterations", current + 1);
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(exchange)
            })
        });

        let config = LoopConfig::new(LoopMode::While(PredicateSource::Sync(predicate)));
        let mut service = LoopService::new(config, pipeline);

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert_eq!(
            result.property("iterations").and_then(|v| v.as_u64()),
            Some(2)
        );
        assert_eq!(
            result.property(CAMEL_LOOP_INDEX).and_then(|v| v.as_u64()),
            Some(1)
        );
        assert_eq!(
            result.property(CAMEL_LOOP_SIZE).and_then(|v| v.as_u64()),
            Some(0)
        );
    }

    #[tokio::test]
    async fn test_loop_while_respects_max_iterations() {
        let counter = Arc::new(AtomicUsize::new(0));
        let predicate = FilterPredicate::new(|_exchange: &Exchange| true);
        let config = LoopConfig::new(LoopMode::While(PredicateSource::Sync(predicate)));
        let mut service = LoopService::new(config, counter_pipeline(Arc::clone(&counter)));

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), MAX_LOOP_ITERATIONS);
    }

    #[tokio::test]
    async fn test_loop_error_propagation() {
        let pipeline = BoxProcessor::from_fn(|_exchange: Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        });

        let config = LoopConfig::new(LoopMode::Count(3));
        let mut service = LoopService::new(config, pipeline);

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await;

        assert!(matches!(result, Err(CamelError::ProcessorError(msg)) if msg == "boom"));
    }

    // ── D-M9 Batch 1: LoopMode::Count(n) clamped to MAX_LOOP_ITERATIONS ──

    /// D-M9: a `Count(u32::MAX as usize)` (or any value above
    /// `MAX_LOOP_ITERATIONS`) is clamped to `MAX_LOOP_ITERATIONS` and
    /// runs at most that many iterations. Without the clamp, a malicious
    /// or typo'd `count: 4294967295` would exhaust CPU. The pre-existing
    /// `Count(3)` test continues to pass — small values are unaffected.
    #[tokio::test]
    async fn test_loop_count_clamped_to_max_iterations() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = LoopConfig::new(LoopMode::Count(usize::MAX));
        let mut service = LoopService::new(config, counter_pipeline(Arc::clone(&counter)));

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
        // Must run exactly MAX_LOOP_ITERATIONS, not u32::MAX iterations.
        assert_eq!(counter.load(Ordering::SeqCst), MAX_LOOP_ITERATIONS);
        // The CAMEL_LOOP_SIZE property is set to the *clamped* count.
        assert_eq!(
            result
                .unwrap()
                .property(CAMEL_LOOP_SIZE)
                .and_then(|v| v.as_u64()),
            Some(MAX_LOOP_ITERATIONS as u64)
        );
    }

    #[tokio::test]
    async fn test_loop_count_with_custom_max_iterations() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = LoopConfig::new(LoopMode::Count(15_000)).with_max_iterations(15_000);
        let mut service = LoopService::new(config, counter_pipeline(Arc::clone(&counter)));
        let exchange = Exchange::new(Message::new("test"));
        let _ = service.ready().await.unwrap().call(exchange).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 15_000);
    }

    #[tokio::test]
    async fn test_loop_while_with_custom_max_iterations() {
        let counter = Arc::new(AtomicUsize::new(0));
        let predicate = FilterPredicate::new(|_| true);
        let config = LoopConfig::new(LoopMode::While(PredicateSource::Sync(predicate)))
            .with_max_iterations(50);
        let mut service = LoopService::new(config, counter_pipeline(Arc::clone(&counter)));
        let exchange = Exchange::new(Message::new("test"));
        let _ = service.ready().await.unwrap().call(exchange).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 50);
    }

    #[tokio::test]
    async fn test_loop_pipeline_chaining() {
        let pipeline = BoxProcessor::from_fn(|mut exchange: Exchange| {
            Box::pin(async move {
                if let Body::Text(s) = &exchange.input.body {
                    exchange.input.body = Body::Text(format!("{s}x"));
                }
                Ok(exchange)
            })
        });

        let config = LoopConfig::new(LoopMode::Count(3));
        let mut service = LoopService::new(config, pipeline);

        let exchange = Exchange::new(Message::new("start"));
        let result = service.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("startxxx"));
    }

    // ── Fallible predicate path (language-value-boundary task 1.4) ──

    use camel_api::{ExpressionErrorClass, PredicateSource};

    fn expression_failed() -> CamelError {
        CamelError::ExpressionFailed {
            language: "rhai".to_string(),
            route_id: "r1".to_string(),
            step_id: "step#0".to_string(),
            verb: "loop".to_string(),
            class: ExpressionErrorClass::Runtime,
            position: None,
            conversion: None,
            cause: None,
        }
    }

    fn async_err_predicate(err: CamelError) -> PredicateSource {
        PredicateSource::Async(Arc::new(move |_: &Exchange| {
            let err = err.clone();
            Box::pin(async move { Err(err) }) as camel_api::BoxBoolFuture
        }))
    }

    /// Async predicate that returns `Ok(true)` on its first call and
    /// `Err(expression_failed())` afterwards. Paired with
    /// `max_iterations = 1` it drives the loop into the exhaustion
    /// safety-guard probe on the second evaluation.
    fn exhausted_then_err_predicate(calls: Arc<AtomicUsize>) -> PredicateSource {
        PredicateSource::Async(Arc::new(move |_: &Exchange| {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            let err = expression_failed();
            Box::pin(async move { if call == 0 { Ok(true) } else { Err(err) } })
                as camel_api::BoxBoolFuture
        }))
    }

    /// OutcomePipeline body that always completes with the exchange unchanged.
    #[derive(Clone)]
    struct CompletedBody;

    impl camel_api::OutcomePipeline for CompletedBody {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(self.clone())
        }

        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = camel_api::PipelineOutcome> + Send + 'a>,
        > {
            Box::pin(async move { camel_api::PipelineOutcome::Completed(exchange) })
        }
    }

    #[tokio::test]
    async fn loop_while_predicate_error_fails_loop() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = LoopConfig::new(LoopMode::While(async_err_predicate(expression_failed())));
        let mut service = LoopService::new(config, counter_pipeline(Arc::clone(&counter)));

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await;

        match result {
            Err(CamelError::ExpressionFailed { .. }) => {}
            other => panic!("expected Err(ExpressionFailed), got {other:?}"),
        }
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "loop body must not run when the while predicate errors"
        );
    }

    #[tokio::test]
    async fn loop_guard_predicate_error_propagates() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::new(AtomicUsize::new(0));
        let config = LoopConfig::new(LoopMode::While(exhausted_then_err_predicate(Arc::clone(
            &calls,
        ))))
        .with_max_iterations(1);
        let mut service = LoopService::new(config, counter_pipeline(Arc::clone(&counter)));

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await;

        match result {
            Err(CamelError::ExpressionFailed { .. }) => {}
            other => panic!("expected Err(ExpressionFailed), got {other:?}"),
        }
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "body runs once before the exhaustion guard probe errors"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "predicate evaluated once in-loop and once by the safety guard"
        );
    }

    #[tokio::test]
    async fn loop_guard_predicate_error_propagates_segment() {
        let calls = Arc::new(AtomicUsize::new(0));
        let config = LoopConfig::new(LoopMode::While(exhausted_then_err_predicate(Arc::clone(
            &calls,
        ))))
        .with_max_iterations(1);
        let mut segment = LoopSegment {
            config,
            body: camel_api::OutcomeSegment::new(Box::new(CompletedBody)),
        };

        let exchange = Exchange::new(Message::new("test"));
        let result = camel_api::OutcomePipeline::run(&mut segment, exchange).await;

        match result {
            camel_api::PipelineOutcome::Failed(CamelError::ExpressionFailed { .. }) => {}
            other => panic!("expected Failed(ExpressionFailed), got {other:?}"),
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "predicate evaluated once in-loop and once by the safety guard"
        );
    }
}
