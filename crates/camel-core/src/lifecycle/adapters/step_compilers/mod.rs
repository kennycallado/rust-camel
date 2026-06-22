//! StepCompiler registry pattern — extract from step_resolution.rs
//!
//! Each compiler group is responsible for matching specific `BuilderStep` variants.
//! The registry dispatches each step to compilers in registration order; the first
//! compiler that returns `Matched` wins. Compilers that don't handle a variant
//! return `NotHandled(step)` to pass it to the next compiler.

use std::sync::Arc;

use camel_api::{BodyType, BoxProcessor, CamelError, FunctionInvoker, ProducerContext};
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
/// Tower service.
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
    },
    /// Stop EIP marker. `run_steps` produces `PipelineOutcome::Stopped(ex)`
    /// without invoking a Tower service. Replaces `StopService` (Task 7).
    Stop,
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
