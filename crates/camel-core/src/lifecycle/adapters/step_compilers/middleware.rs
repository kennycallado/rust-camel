//! Middleware step compiler: `Layered` (a Tower layer wrapping a sub-pipeline).
//!
//! `Layered` compiles its children into one outcome-aware sub-pipeline, bridges
//! that sub-pipeline into the Tower world with [`camel_api::SegmentProcessor`],
//! wraps it with the builder-supplied layer, and re-erases the result as an
//! [`crate::lifecycle::adapters::route_compiler::BoxProcessorSegment`].
//!
//! Segment-mode is MANDATORY (ADR-0023 IdempotentConsumer precedent): a `Stop`
//! inside the section must surface as `PipelineOutcome::Stopped`. A Process-mode
//! wrap would flatten the stop to `Ok(ex)` and downstream steps would run.

use camel_api::{BoxProcessor, CamelError, SegmentProcessor};

use super::{
    CompilationContext, CompileOutcome, CompiledStep, StepCompiler, StepCompilerRegistry,
    pack_lifecycles,
};
use crate::lifecycle::adapters::route_compiler::{BoxProcessorSegment, compose_outcome_segment};
use crate::lifecycle::application::route_definition::BuilderStep;

pub(crate) struct MiddlewareCompiler;

impl StepCompiler for MiddlewareCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        ctx: &CompilationContext,
        registry: &StepCompilerRegistry,
    ) -> Result<CompileOutcome, CamelError> {
        match step {
            // ── Layered (section granularity) ──
            //
            // Stop propagation chain: child `Stop` → `StopSegment` →
            // `Stopped(ex)` → `SegmentProcessor` sets `CAMEL_STOP=true` and
            // returns `Ok(ex)` → `BoxProcessorSegment` re-derives `Stopped`
            // via `is_camel_stop` → `run_steps`' Segment arm propagates it.
            BuilderStep::Layered { factory, steps } => {
                let (children, lifecycles) = ctx.compile_children_segments(steps, registry)?;
                let body = compose_outcome_segment(children);
                let processor = factory.wrap(BoxProcessor::new(SegmentProcessor::new(body)))?;
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(BoxProcessorSegment::new(
                        processor,
                    ))),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                    label: None,
                }))
            }

            _ => Ok(CompileOutcome::NotHandled(step)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use camel_api::{
        BoxProcessor, BoxProcessorExt, CamelError, CamelErrorAdapter, Exchange, LayerFactory,
        Message, OpaqueProcessor, PipelineOutcome, ProcessorLayer, is_camel_stop,
    };
    use camel_bean::BeanRegistry;
    use camel_component_api::test_support::NoopRuntimeObservability;
    use camel_component_api::{
        Component, ComponentContext, Endpoint, ProducerContext, RuntimeObservability,
    };
    use tower::Layer;

    use super::*;
    use crate::lifecycle::adapters::route_controller::SharedLanguageRegistry;
    use crate::lifecycle::adapters::step_resolution::FunctionStagingMode as ResolutionStagingMode;

    /// Owns the long-lived `CompilationContext` dependencies so each test can
    /// borrow a fresh context without repeating the 12-field constructor.
    struct Harness {
        pc: ProducerContext,
        rt: Arc<dyn RuntimeObservability>,
        languages: SharedLanguageRegistry,
        beans: Arc<Mutex<BeanRegistry>>,
        component_ctx: Arc<dyn ComponentContext>,
        staging: ResolutionStagingMode,
        idempotent_repositories: crate::IdempotentRegistry,
        claim_check_repositories: crate::ClaimCheckRegistry,
        cache_repositories: crate::CacheRegistry,
    }

    impl Harness {
        fn new(component_ctx: Arc<dyn ComponentContext>) -> Self {
            Self {
                pc: ProducerContext::default(),
                rt: Arc::new(NoopRuntimeObservability),
                languages: Arc::new(Mutex::new(HashMap::new())),
                beans: Arc::new(Mutex::new(BeanRegistry::new())),
                component_ctx,
                staging: ResolutionStagingMode::DirectAdd,
                idempotent_repositories: crate::IdempotentRegistry::new(),
                claim_check_repositories: crate::ClaimCheckRegistry::new(),
                cache_repositories: crate::CacheRegistry::new(),
            }
        }

        fn ctx(&self) -> CompilationContext<'_> {
            CompilationContext {
                producer_ctx: &self.pc,
                rt: Arc::clone(&self.rt),
                languages: &self.languages,
                beans: &self.beans,
                function_invoker: None,
                component_ctx: Arc::clone(&self.component_ctx),
                route_id: None,
                staging_mode: &self.staging,
                idempotent_repositories: &self.idempotent_repositories,
                claim_check_repositories: &self.claim_check_repositories,
                cache_repositories: &self.cache_repositories,
                intercept: crate::intercept::InterceptRules::default(),
            }
        }
    }

    /// Producer factory used by the mock endpoint harness.
    type ProducerFactory = Arc<dyn Fn() -> BoxProcessor + Send + Sync>;

    struct FactoryEndpoint {
        uri: String,
        make: ProducerFactory,
    }

    impl Endpoint for FactoryEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }

        fn create_consumer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
        ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed("not a consumer".into()))
        }

        fn create_producer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Ok((self.make)())
        }
    }

    struct FactoryComponent {
        scheme: String,
        make: ProducerFactory,
    }

    #[async_trait]
    impl Component for FactoryComponent {
        fn scheme(&self) -> &str {
            &self.scheme
        }

        fn create_endpoint(
            &self,
            uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(FactoryEndpoint {
                uri: uri.to_string(),
                make: Arc::clone(&self.make),
            }))
        }
    }

    /// `ComponentContext` resolving `test:` to the supplied producer factory.
    struct TestContext {
        make: ProducerFactory,
    }

    impl ComponentContext for TestContext {
        fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
            if scheme == "test" {
                Some(Arc::new(FactoryComponent {
                    scheme: "test".to_string(),
                    make: Arc::clone(&self.make),
                }))
            } else {
                None
            }
        }

        fn resolve_language(&self, _name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
            None
        }

        fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
            Arc::new(camel_api::NoOpMetrics)
        }

        fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
            Arc::new(camel_api::NoopPlatformService::default())
        }

        fn register_route_health_check(
            &self,
            _route_id: &str,
            _check: Arc<dyn camel_api::AsyncHealthCheck>,
        ) {
        }

        fn unregister_route_health_check(&self, _route_id: &str) {}
    }

    fn identity() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    /// Producer that records concurrent in-flight calls and exposes the peak.
    fn inflight_producer(
        inflight: Arc<AtomicUsize>,
        max_inflight: Arc<AtomicUsize>,
        delay: Duration,
    ) -> BoxProcessor {
        BoxProcessor::from_fn(move |ex| {
            let inflight = Arc::clone(&inflight);
            let max_inflight = Arc::clone(&max_inflight);
            Box::pin(async move {
                let current = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                max_inflight.fetch_max(current, Ordering::SeqCst);
                // allow-test-sleep: 50ms window keeps concurrent probes
                // overlapping long enough for the concurrency limit to be
                // observable.
                tokio::time::sleep(delay).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
                Ok(ex)
            })
        })
    }

    fn compile_layered(
        harness: &Harness,
        factory: LayerFactory,
        steps: Vec<BuilderStep>,
    ) -> CompiledStep {
        let compiled = super::super::build_registry()
            .compile_step(BuilderStep::Layered { factory, steps }, 0, &harness.ctx())
            .expect("layered compilation should succeed")
            .expect("MiddlewareCompiler must handle Layered");
        assert!(
            matches!(compiled, CompiledStep::Segment { .. }),
            "Layered must compile to Segment, got {compiled:?}"
        );
        compiled
    }

    fn into_segment(step: CompiledStep) -> camel_api::OutcomeSegment {
        match step {
            CompiledStep::Segment { segment, .. } => segment,
            other => panic!("expected CompiledStep::Segment, got {other:?}"),
        }
    }

    /// `Layered` must compile to an outcome-aware Segment step (NOT Process),
    /// so a Stop inside the section can propagate.
    #[tokio::test]
    async fn layered_section_compiles_to_segment_step() {
        let harness = Harness::new(Arc::new(TestContext {
            make: Arc::new(identity),
        }));

        let compiled = compile_layered(
            &harness,
            LayerFactory::new(tower::limit::ConcurrencyLimitLayer::new(2)),
            vec![
                BuilderStep::Processor(OpaqueProcessor(identity())),
                BuilderStep::To("test:out".into()),
            ],
        );

        assert!(
            matches!(compiled, CompiledStep::Segment { .. }),
            "Layered must be Segment-mode, got {compiled:?}"
        );
    }

    /// A Stop inside the section must surface as `PipelineOutcome::Stopped`,
    /// not flatten to `Completed`, and the following step must not run.
    #[tokio::test]
    async fn stop_inside_layered_section_stops_not_flattens() {
        let harness = Harness::new(Arc::new(TestContext {
            make: Arc::new(identity),
        }));

        let mutating = OpaqueProcessor(BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                ex.input.body = camel_api::Body::from("mutated");
                Ok(ex)
            })
        }));

        let compiled = compile_layered(
            &harness,
            LayerFactory::new(tower::limit::ConcurrencyLimitLayer::new(2)),
            vec![BuilderStep::Stop, BuilderStep::Processor(mutating)],
        );

        let mut segment = into_segment(compiled);
        let outcome = segment.run(Exchange::new(Message::new("original"))).await;

        match outcome {
            PipelineOutcome::Stopped(ex) => {
                assert!(
                    is_camel_stop(&ex),
                    "Stopped exchange must carry the CamelStop marker"
                );
                assert_eq!(
                    ex.input.body.as_text(),
                    Some("original"),
                    "the step after Stop must never run"
                );
            }
            other => panic!("expected PipelineOutcome::Stopped, got {other:?}"),
        }
    }

    /// The configured concurrency limit must apply across cloned segments:
    /// five concurrent runs never see more than two children in flight.
    #[tokio::test]
    async fn layered_section_wraps_children_with_limit() {
        let inflight = Arc::new(AtomicUsize::new(0));
        let max_inflight = Arc::new(AtomicUsize::new(0));
        let harness = Harness::new(Arc::new(TestContext {
            make: Arc::new(identity),
        }));

        let counting = inflight_producer(
            Arc::clone(&inflight),
            Arc::clone(&max_inflight),
            Duration::from_millis(50),
        );

        let compiled = compile_layered(
            &harness,
            LayerFactory::new(tower::limit::ConcurrencyLimitLayer::new(2)),
            vec![BuilderStep::Processor(OpaqueProcessor(counting))],
        );
        let segment = into_segment(compiled);

        let runs: Vec<_> = (0..5)
            .map(|i| {
                let mut segment = segment.clone();
                async move {
                    segment
                        .run(Exchange::new(Message::new(format!("m{i}"))))
                        .await
                }
            })
            .collect();

        for outcome in futures::future::join_all(runs).await {
            assert!(
                matches!(outcome, PipelineOutcome::Completed(_)),
                "every section run should complete, got {outcome:?}"
            );
        }

        assert_eq!(
            max_inflight.load(Ordering::SeqCst),
            2,
            "the section's concurrency limit must be shared across clones"
        );
        assert_eq!(inflight.load(Ordering::SeqCst), 0);
    }

    /// A `ProcessorLayer` that builds the buffered rate limiter INSIDE the
    /// factory (the probe P3 pattern). `Buffer` shares one worker across
    /// clones, so the rate state lives with the compiled section.
    struct BufferedRateLimitLayer {
        rate: u64,
        period: Duration,
        bound: usize,
    }

    impl ProcessorLayer for BufferedRateLimitLayer {
        fn wrap(&self, inner: BoxProcessor) -> Result<BoxProcessor, CamelError> {
            let limited = tower::limit::RateLimitLayer::new(self.rate, self.period).layer(inner);
            let buffered = tower::buffer::Buffer::new(limited, self.bound);
            Ok(BoxProcessor::new(CamelErrorAdapter::new(buffered)))
        }
    }

    /// The section's rate-limit state must be shared across cloned segments:
    /// five runs at 2 per 100ms take at least `floor((5-1)/2) * 100ms`.
    #[tokio::test]
    async fn layered_section_with_buffered_rate_limit_shares_state() {
        let harness = Harness::new(Arc::new(TestContext {
            make: Arc::new(identity),
        }));

        let factory = LayerFactory::new(BufferedRateLimitLayer {
            rate: 2,
            period: Duration::from_millis(100),
            bound: 16,
        });

        let child = BoxProcessor::from_fn(|ex| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
                Ok(ex)
            })
        });

        let compiled = compile_layered(
            &harness,
            factory,
            vec![
                BuilderStep::Processor(OpaqueProcessor(child)),
                BuilderStep::To("test:out".into()),
            ],
        );
        let segment = into_segment(compiled);

        let started = Instant::now();
        let runs: Vec<_> = (0..5)
            .map(|i| {
                let mut segment = segment.clone();
                async move {
                    segment
                        .run(Exchange::new(Message::new(format!("m{i}"))))
                        .await
                }
            })
            .collect();

        for outcome in futures::future::join_all(runs).await {
            assert!(
                matches!(outcome, PipelineOutcome::Completed(_)),
                "every rate-limited run should complete, got {outcome:?}"
            );
        }

        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(200),
            "rate state must be shared across clones: 5 runs at 2/100ms need \
             at least 200ms, took {elapsed:?}"
        );
        assert!(
            elapsed <= Duration::from_millis(250),
            "rate state must be shared, not rebuilt per run: took {elapsed:?}"
        );
    }
}
