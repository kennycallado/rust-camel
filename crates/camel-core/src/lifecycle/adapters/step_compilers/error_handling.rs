//! Error-handling step compilers: ErrorHandler, OnError, TryCatch, Saga.
//!
//! Currently a no-op placeholder — these variants are not yet in `BuilderStep`.
//! When they are added, compilers in this module will handle them.

use super::{CompilationContext, CompileOutcome, StepCompiler, StepCompilerRegistry};
use crate::lifecycle::application::route_definition::BuilderStep;
use camel_api::CamelError;

pub(crate) struct ErrorHandlingCompiler;

impl StepCompiler for ErrorHandlingCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        _ctx: &CompilationContext,
        _registry: &StepCompilerRegistry,
    ) -> Result<CompileOutcome, CamelError> {
        // No error-handling variants exist in BuilderStep yet.
        // When added, handle them here.
        Ok(CompileOutcome::NotHandled(step))
    }
}
