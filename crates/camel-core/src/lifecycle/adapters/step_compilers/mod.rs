//! StepCompiler registry pattern — extract from step_resolution.rs
//!
//! Each compiler group is responsible for matching specific `BuilderStep` variants.
//! The registry dispatches each step to compilers in registration order; the first
//! compiler that returns `Matched` wins. Compilers that don't handle a variant
//! return `NotHandled(step)` to pass it to the next compiler.

use std::sync::Arc;

use camel_api::{
    BodyType, BoxProcessor, CamelError, FunctionInvoker, ProducerContext, StepLifecycle,
};
use camel_component_api::{ComponentContext, RuntimeObservability};
use camel_endpoint::parse_uri;

use crate::lifecycle::adapters::route_controller::SharedLanguageRegistry;
use crate::lifecycle::adapters::step_resolution::FunctionStagingMode;
use crate::lifecycle::application::route_definition::BuilderStep;
use camel_bean::BeanRegistry;

mod control_flow;
mod core;
mod endpoints;
mod error_handling;
mod routing;
mod splitting;
mod transforms;

/// A compiled pipeline step.
///
/// `Process` is the normal case: a boxed processor plus its optional body
/// contract. `Stop` (added in Task 3b) is the Stop EIP marker — `run_steps`
/// recognises it and produces `PipelineOutcome::Stopped` without invoking a
/// Tower service. `Segment` (added in Task 3) wraps an `OutcomeSegment` for
/// structural EIPs with outcome-aware sub-pipelines.
///
/// **Boundary:** `CompiledStep` is the compile-time representation. At runtime,
/// `run_steps` consumes a `Vec<CompiledStep>` (Stop variants included) and
/// produces a `PipelineOutcome`; the wrapping `Service<Exchange>` impl
/// translates `PipelineOutcome` back to `Result<Exchange, CamelError>`. See
/// ADR-0024.
#[derive(Debug, Clone)]
pub enum CompiledStep {
    Process {
        processor: BoxProcessor,
        body_contract: Option<BodyType>,
        /// Lifecycle handle for this processor, if it is stateful.
        /// `None` for stateless processors (the common case).
        lifecycle: Option<Arc<dyn StepLifecycle>>,
    },
    /// Stop EIP marker. `run_steps` produces `PipelineOutcome::Stopped(ex)`
    /// without invoking a Tower service. Replaces `StopService` (Task 7).
    Stop,
    /// Outcome-aware structural EIP segment. `run_steps` invokes
    /// `segment.run(ex)` and matches on the returned `PipelineOutcome`.
    /// See ADR-0025.
    Segment {
        segment: camel_api::OutcomeSegment,
        body_contract: Option<BodyType>,
        /// Lifecycle handles from children nested inside this segment.
        /// `Option<Vec<...>>` (not `Option<Arc<...>>`) so multiple stateful
        /// children (e.g. Idempotent+Resequencer inside Filter) each
        /// register independently.
        lifecycle: Option<Vec<Arc<dyn StepLifecycle>>>,
    },
}

/// Result from a compiler: either it handled the step (with success or error),
/// or it did not recognize the variant and returns the step for the next compiler.
pub(crate) enum StepCompileResult {
    Matched(Result<CompiledStep, CamelError>),
    NotHandled(BuilderStep),
}

/// A compiler that can handle one or more `BuilderStep` variants.
///
/// The `compile` method receives ownership of the step. If the compiler recognizes
/// the variant it returns `StepCompileResult::Matched(...)`. Otherwise it returns the
/// step back via `NotHandled(step)`.
pub(crate) trait StepCompiler: Send + Sync {
    fn compile(
        &self,
        step: BuilderStep,
        step_index: usize,
        ctx: &CompilationContext,
        registry: &StepCompilerRegistry,
    ) -> StepCompileResult;
}

/// Shared context passed to every compiler invocation.
pub(crate) struct CompilationContext<'a> {
    pub producer_ctx: &'a ProducerContext,
    pub rt: Arc<dyn RuntimeObservability>,
    pub languages: &'a SharedLanguageRegistry,
    pub beans: &'a Arc<std::sync::Mutex<BeanRegistry>>,
    pub function_invoker: Option<Arc<dyn FunctionInvoker>>,
    pub component_ctx: Arc<dyn ComponentContext>,
    pub route_id: Option<&'a str>,
    pub staging_mode: &'a FunctionStagingMode,
}

impl<'a> CompilationContext<'a> {
    /// Recursively compile child steps. Used by compilers that have sub-pipelines
    /// (Filter, Choice, Split, Loop, etc.).
    pub fn compile_children(
        &self,
        steps: Vec<BuilderStep>,
        registry: &StepCompilerRegistry,
    ) -> Result<Vec<CompiledStep>, CamelError> {
        registry.compile_steps(steps, self)
    }

    /// Recursively compile child steps and map them into outcome-aware segments.
    ///
    /// Each `CompiledStep` variant is converted to a `Box<dyn OutcomePipeline>`:
    /// - `Process` → `BoxProcessorSegment`, optionally wrapped in `BodyCoercingSegment`
    /// - `Stop` → `StopSegment` (produces `PipelineOutcome::Stopped(ex)`)
    /// - `Segment` → its inner `OutcomeSegment` (which now implements OutcomePipeline)
    ///
    /// This replaces the 22-line duplicated closure in Filter/DeclarativeFilter
    /// (and will prevent 14+ more duplicates in T9–T16).
    #[allow(clippy::type_complexity)]
    pub fn compile_children_segments(
        &self,
        steps: Vec<BuilderStep>,
        registry: &StepCompilerRegistry,
    ) -> Result<
        (
            Vec<Box<dyn camel_api::OutcomePipeline>>,
            Vec<Arc<dyn camel_api::StepLifecycle>>,
        ),
        CamelError,
    > {
        let pairs = self.compile_children(steps, registry)?;
        let mut lifecycle_handles: Vec<Arc<dyn camel_api::StepLifecycle>> = Vec::new();
        let segments: Vec<Box<dyn camel_api::OutcomePipeline>> = pairs
            .into_iter()
            .map(|c| match c {
                CompiledStep::Process {
                    processor,
                    body_contract,
                    lifecycle,
                } => {
                    if let Some(lc) = lifecycle {
                        lifecycle_handles.push(lc);
                    }
                    let inner: Box<dyn camel_api::OutcomePipeline> = Box::new(
                        crate::lifecycle::adapters::route_compiler::BoxProcessorSegment::new(
                            processor,
                        ),
                    );
                    match body_contract {
                        Some(contract) => Box::new(
                            crate::lifecycle::adapters::route_compiler::BodyCoercingSegment::new(
                                inner, contract,
                            ),
                        ),
                        None => inner,
                    }
                }
                CompiledStep::Stop => {
                    Box::new(crate::lifecycle::adapters::route_compiler::StopSegment)
                        as Box<dyn camel_api::OutcomePipeline>
                }
                CompiledStep::Segment {
                    segment,
                    body_contract: _,
                    lifecycle,
                } => {
                    if let Some(lcs) = lifecycle {
                        lifecycle_handles.extend(lcs);
                    }
                    Box::new(segment)
                }
            })
            .collect();
        Ok((segments, lifecycle_handles))
    }
}

/// Registry of step compilers. Steps are dispatched to compilers in registration
/// order. The first matching compiler handles the step.
pub(crate) struct StepCompilerRegistry {
    compilers: Vec<Box<dyn StepCompiler>>,
}

impl StepCompilerRegistry {
    pub fn new() -> Self {
        Self {
            compilers: Vec::new(),
        }
    }

    pub fn register(&mut self, compiler: Box<dyn StepCompiler>) {
        self.compilers.push(compiler);
    }

    /// Try each compiler in order. The first to return `Matched` wins.
    /// If all return `NotHandled`, returns `None`.
    pub fn compile_step(
        &self,
        step: BuilderStep,
        step_index: usize,
        ctx: &CompilationContext,
    ) -> Option<Result<CompiledStep, CamelError>> {
        let mut step = step;
        for compiler in &self.compilers {
            match compiler.compile(step, step_index, ctx, self) {
                StepCompileResult::Matched(result) => return Some(result),
                StepCompileResult::NotHandled(s) => step = s,
            }
        }
        None
    }

    /// Compile all steps in a vector.
    pub fn compile_steps(
        &self,
        steps: Vec<BuilderStep>,
        ctx: &CompilationContext,
    ) -> Result<Vec<CompiledStep>, CamelError> {
        let mut out = Vec::with_capacity(steps.len());
        for (i, step) in steps.into_iter().enumerate() {
            match self.compile_step(step, i, ctx) {
                Some(Ok(c)) => out.push(c),
                Some(Err(e)) => return Err(e),
                None => {
                    return Err(CamelError::RouteError(
                        "no compiler registered for step variant".into(),
                    ));
                }
            }
        }
        Ok(out)
    }
}

/// Parse a URI and create a producer, reusing `component_ctx`, `rt`, and `producer_ctx`
/// from the compilation context.
pub(crate) fn resolve_producer(
    ctx: &CompilationContext,
    uri: &str,
) -> Result<BoxProcessor, CamelError> {
    let parsed = parse_uri(uri)?;
    let component = ctx
        .component_ctx
        .resolve_component(&parsed.scheme)
        .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
    let endpoint = component.create_endpoint(uri, ctx.component_ctx.as_ref())?;
    endpoint.create_producer(Arc::clone(&ctx.rt), ctx.producer_ctx)
}

/// Pack a lifecycle Vec into `None` when empty, `Some` when non-empty.
/// Preserves the invariant that `Some` always implies ≥1 handle.
pub(super) fn pack_lifecycles(
    lifecycles: Vec<Arc<dyn StepLifecycle>>,
) -> Option<Vec<Arc<dyn StepLifecycle>>> {
    if lifecycles.is_empty() {
        None
    } else {
        Some(lifecycles)
    }
}

#[cfg(test)]
mod segment_tests {
    use super::*;
    use camel_api::{Exchange, OutcomePipeline, PipelineOutcome};
    use std::future::Future;
    use std::pin::Pin;

    #[derive(Clone)]
    struct EchoSegment;

    impl OutcomePipeline for EchoSegment {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(EchoSegment)
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            Box::pin(async move { PipelineOutcome::Completed(exchange) })
        }
    }

    #[test]
    fn compiled_step_segment_clone_compiles() {
        let seg = camel_api::OutcomeSegment::new(Box::new(EchoSegment));
        let step = CompiledStep::Segment {
            segment: seg,
            body_contract: None,
            lifecycle: None,
        };
        let _cloned = step.clone();
        if let CompiledStep::Segment { segment: _, .. } = _cloned {
            // ok
        } else {
            panic!("clone should preserve variant");
        }
    }

    #[test]
    fn compiled_step_segment_debug_renders() {
        let seg = camel_api::OutcomeSegment::new(Box::new(EchoSegment));
        let step = CompiledStep::Segment {
            segment: seg,
            body_contract: None,
            lifecycle: None,
        };
        let s = format!("{:?}", step);
        assert!(
            s.contains("Segment"),
            "debug should mention Segment variant: {s}"
        );
    }

    #[test]
    fn outcome_segment_satisfies_clone_send_static() {
        fn assert_traits<T: Clone + Send + 'static>() {}
        assert_traits::<camel_api::OutcomeSegment>();
    }

    #[tokio::test]
    async fn outcome_segment_survives_arcswap_swap() {
        use arc_swap::ArcSwap;
        use camel_api::{Exchange, Message, OutcomePipeline, PipelineOutcome};
        use std::sync::Arc;

        #[derive(Clone)]
        struct EchoSegment;
        impl OutcomePipeline for EchoSegment {
            fn clone_box(&self) -> Box<dyn OutcomePipeline> {
                Box::new(EchoSegment)
            }
            fn run<'a>(
                &'a mut self,
                ex: Exchange,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
            {
                Box::pin(async move { PipelineOutcome::Completed(ex) })
            }
        }

        let seg = camel_api::OutcomeSegment::new(Box::new(EchoSegment));
        let slot: ArcSwap<Option<camel_api::OutcomeSegment>> = ArcSwap::from_pointee(None);
        slot.store(Arc::new(Some(seg.clone())));
        slot.store(Arc::new(Some(seg)));

        let mut borrowed = slot.load().as_ref().clone().unwrap();
        let outcome = borrowed.run(Exchange::new(Message::new("ping"))).await;
        assert!(matches!(outcome, PipelineOutcome::Completed(_)));
    }

    /// Test lifecycle handle used by compile_children_segments_bubbles_child_lifecycle.
    #[derive(Debug)]
    struct TestLifecycle;

    #[async_trait::async_trait]
    impl camel_api::StepLifecycle for TestLifecycle {
        fn name(&self) -> &'static str {
            "test-lifecycle"
        }
        async fn shutdown(
            &self,
            _reason: camel_api::StepShutdownReason,
        ) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
    }

    /// Custom compiler that injects a lifecycle handle into every
    /// `BuilderStep::Processor` it compiles.
    struct LifecycleInjectorCompiler {
        handle: Arc<dyn camel_api::StepLifecycle>,
    }

    impl StepCompiler for LifecycleInjectorCompiler {
        fn compile(
            &self,
            step: BuilderStep,
            _step_index: usize,
            _ctx: &CompilationContext,
            _registry: &StepCompilerRegistry,
        ) -> StepCompileResult {
            match step {
                BuilderStep::Processor(svc) => {
                    StepCompileResult::Matched(Ok(CompiledStep::Process {
                        processor: svc,
                        body_contract: None,
                        lifecycle: Some(self.handle.clone()),
                    }))
                }
                other => StepCompileResult::NotHandled(other),
            }
        }
    }

    #[tokio::test]
    async fn compile_children_segments_bubbles_child_lifecycle() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        use camel_api::{BoxProcessor, BoxProcessorExt, StepLifecycle};
        use camel_bean::BeanRegistry;
        use camel_component_api::{
            ComponentContext, NoOpComponentContext, RuntimeObservability,
            test_support::NoopRuntimeObservability,
        };

        use crate::lifecycle::adapters::step_resolution::FunctionStagingMode;

        let handle: Arc<dyn StepLifecycle> = Arc::new(TestLifecycle);

        // Register lifecycle injector + real control-flow compiler so
        // compile_children_segments runs through a structural EIP path.
        let mut reg = StepCompilerRegistry::new();
        reg.register(Box::new(LifecycleInjectorCompiler {
            handle: handle.clone(),
        }));
        reg.register(Box::new(super::control_flow::ControlFlowCompiler));

        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(NoOpComponentContext);
        let staging = FunctionStagingMode::DirectAdd;

        let ctx = CompilationContext {
            producer_ctx: &pc,
            rt,
            languages: &languages,
            beans: &beans,
            function_invoker: None,
            component_ctx,
            route_id: None,
            staging_mode: &staging,
        };

        // Compile a Filter with a child Processor step.
        let filter_step = BuilderStep::Filter {
            predicate: Arc::new(|_| true),
            steps: vec![BuilderStep::Processor(BoxProcessor::from_fn(|ex| {
                Box::pin(async move { Ok(ex) })
            }))],
        };

        let result = reg.compile_step(filter_step, 0, &ctx);
        let compiled = result
            .expect("compilation should succeed")
            .expect("should match");

        match compiled {
            CompiledStep::Segment {
                lifecycle,
                body_contract,
                ..
            } => {
                assert_eq!(body_contract, None, "body_contract should be None");
                let handles = lifecycle.expect("Segment should have lifecycle handles");
                assert_eq!(handles.len(), 1, "expected 1 lifecycle handle");
                assert_eq!(handles[0].name(), "test-lifecycle", "handle name mismatch");
            }
            other => panic!("Expected CompiledStep::Segment, got {other:?}"),
        }
    }

    /// A lifecycle handle with a configurable name for multi-handle tests.
    #[derive(Debug)]
    struct NamedLifecycle(&'static str);

    #[async_trait::async_trait]
    impl camel_api::StepLifecycle for NamedLifecycle {
        fn name(&self) -> &'static str {
            self.0
        }
        async fn shutdown(
            &self,
            _reason: camel_api::StepShutdownReason,
        ) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
    }

    /// Test A: Multiple stateful children in one Segment → Vec length 2.
    #[tokio::test]
    async fn compile_children_segments_multiple_stateful_children() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        use crate::lifecycle::adapters::step_resolution::FunctionStagingMode;
        use camel_api::{BoxProcessor, BoxProcessorExt, StepLifecycle};
        use camel_bean::BeanRegistry;
        use camel_component_api::{
            ComponentContext, NoOpComponentContext, RuntimeObservability,
            test_support::NoopRuntimeObservability,
        };

        let handle: Arc<dyn StepLifecycle> = Arc::new(NamedLifecycle("multi"));

        let mut reg = StepCompilerRegistry::new();
        reg.register(Box::new(LifecycleInjectorCompiler {
            handle: handle.clone(),
        }));
        reg.register(Box::new(super::control_flow::ControlFlowCompiler));

        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(NoOpComponentContext);
        let staging = FunctionStagingMode::DirectAdd;

        let ctx = CompilationContext {
            producer_ctx: &pc,
            rt,
            languages: &languages,
            beans: &beans,
            function_invoker: None,
            component_ctx,
            route_id: None,
            staging_mode: &staging,
        };

        // Filter with TWO child Processors → both get the same lifecycle handle.
        let filter_step = BuilderStep::Filter {
            predicate: Arc::new(|_| true),
            steps: vec![
                BuilderStep::Processor(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))),
                BuilderStep::Processor(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))),
            ],
        };

        let result = reg.compile_step(filter_step, 0, &ctx);
        let compiled = result
            .expect("compilation should succeed")
            .expect("should match");

        match compiled {
            CompiledStep::Segment { lifecycle, .. } => {
                let handles = lifecycle.expect("Segment should have lifecycle handles");
                assert_eq!(
                    handles.len(),
                    2,
                    "expected 2 lifecycle handles for 2 children"
                );
                for h in &handles {
                    assert_eq!(h.name(), "multi", "all handles should be 'multi'");
                }
            }
            other => panic!("Expected CompiledStep::Segment, got {other:?}"),
        }
    }

    /// Test B: Multi-branch accumulation across Choice when-clauses.
    #[tokio::test]
    async fn compile_children_segments_multi_branch_accumulation() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        use crate::lifecycle::adapters::step_resolution::FunctionStagingMode;
        use crate::lifecycle::application::route_definition::WhenStep;
        use camel_api::{BoxProcessor, BoxProcessorExt, StepLifecycle};
        use camel_bean::BeanRegistry;
        use camel_component_api::{
            ComponentContext, NoOpComponentContext, RuntimeObservability,
            test_support::NoopRuntimeObservability,
        };

        let handle: Arc<dyn StepLifecycle> = Arc::new(NamedLifecycle("branch"));

        let mut reg = StepCompilerRegistry::new();
        reg.register(Box::new(LifecycleInjectorCompiler {
            handle: handle.clone(),
        }));
        reg.register(Box::new(super::control_flow::ControlFlowCompiler));

        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(NoOpComponentContext);
        let staging = FunctionStagingMode::DirectAdd;

        let ctx = CompilationContext {
            producer_ctx: &pc,
            rt,
            languages: &languages,
            beans: &beans,
            function_invoker: None,
            component_ctx,
            route_id: None,
            staging_mode: &staging,
        };

        // Choice with 2 when branches, each containing 1 stateful child.
        let choice_step = BuilderStep::Choice {
            whens: vec![
                WhenStep {
                    predicate: Arc::new(|_| true),
                    steps: vec![BuilderStep::Processor(BoxProcessor::from_fn(|ex| {
                        Box::pin(async move { Ok(ex) })
                    }))],
                },
                WhenStep {
                    predicate: Arc::new(|_| false),
                    steps: vec![BuilderStep::Processor(BoxProcessor::from_fn(|ex| {
                        Box::pin(async move { Ok(ex) })
                    }))],
                },
            ],
            otherwise: None,
        };

        let result = reg.compile_step(choice_step, 0, &ctx);
        let compiled = result
            .expect("compilation should succeed")
            .expect("should match");

        match compiled {
            CompiledStep::Segment { lifecycle, .. } => {
                let handles = lifecycle.expect("Segment should have lifecycle handles");
                assert_eq!(
                    handles.len(),
                    2,
                    "expected 2 lifecycle handles from 2 branches"
                );
                for h in &handles {
                    assert_eq!(h.name(), "branch", "all handles should be 'branch'");
                }
            }
            other => panic!("Expected CompiledStep::Segment, got {other:?}"),
        }
    }

    /// Test C: Nested Segment-in-Segment flattening — outer Segment contains
    /// innermost lifecycle handle from a grandchild Processor.
    #[tokio::test]
    async fn compile_children_segments_nested_segment_flattening() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        use crate::lifecycle::adapters::step_resolution::FunctionStagingMode;
        use camel_api::{BoxProcessor, BoxProcessorExt, StepLifecycle};
        use camel_bean::BeanRegistry;
        use camel_component_api::{
            ComponentContext, NoOpComponentContext, RuntimeObservability,
            test_support::NoopRuntimeObservability,
        };

        let inner_handle: Arc<dyn StepLifecycle> = Arc::new(NamedLifecycle("deep"));

        let mut reg = StepCompilerRegistry::new();
        reg.register(Box::new(LifecycleInjectorCompiler {
            handle: inner_handle.clone(),
        }));
        reg.register(Box::new(super::control_flow::ControlFlowCompiler));

        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(NoOpComponentContext);
        let staging = FunctionStagingMode::DirectAdd;

        let ctx = CompilationContext {
            producer_ctx: &pc,
            rt,
            languages: &languages,
            beans: &beans,
            function_invoker: None,
            component_ctx,
            route_id: None,
            staging_mode: &staging,
        };

        // Outer Filter containing an inner Filter that has a stateful Processor.
        // The outer Segment's lifecycle should contain the innermost handle
        // (proves recursive flattening through compile_children_segments).
        let inner_filter = BuilderStep::Filter {
            predicate: Arc::new(|_| true),
            steps: vec![BuilderStep::Processor(BoxProcessor::from_fn(|ex| {
                Box::pin(async move { Ok(ex) })
            }))],
        };

        let outer_filter = BuilderStep::Filter {
            predicate: Arc::new(|_| true),
            steps: vec![inner_filter],
        };

        let result = reg.compile_step(outer_filter, 0, &ctx);
        let compiled = result
            .expect("compilation should succeed")
            .expect("should match");

        match compiled {
            CompiledStep::Segment { lifecycle, .. } => {
                let handles = lifecycle.expect("outer Segment should have lifecycle handles");
                assert_eq!(handles.len(), 1, "expected 1 innermost lifecycle handle");
                assert_eq!(
                    handles[0].name(),
                    "deep",
                    "handle should be from innermost child"
                );
            }
            other => panic!("Expected CompiledStep::Segment, got {other:?}"),
        }
    }
}

/// Build the full registry with all compiler groups.
pub(crate) fn build_registry() -> StepCompilerRegistry {
    let mut reg = StepCompilerRegistry::new();
    reg.register(Box::new(core::CoreCompiler));
    reg.register(Box::new(endpoints::EndpointsCompiler));
    reg.register(Box::new(transforms::TransformsCompiler));
    reg.register(Box::new(routing::RoutingCompiler));
    reg.register(Box::new(control_flow::ControlFlowCompiler));
    reg.register(Box::new(splitting::SplittingCompiler));
    reg.register(Box::new(error_handling::ErrorHandlingCompiler));
    reg
}
