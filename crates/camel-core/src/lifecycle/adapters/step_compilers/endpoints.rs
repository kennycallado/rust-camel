//! Endpoint step compilers: To, WireTap.
//!
//! These steps create producers from URIs at compile time.

use std::sync::Arc;

use camel_api::{BoxProcessor, CamelError};

use camel_endpoint::parse_uri;

use super::{
    CompilationContext, CompileOutcome, CompiledStep, StepCompiler, StepCompilerRegistry,
    resolve_producer,
};
use crate::lifecycle::application::route_definition::BuilderStep;

pub(crate) struct EndpointsCompiler;

impl StepCompiler for EndpointsCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        ctx: &CompilationContext,
        _registry: &StepCompilerRegistry,
    ) -> Result<CompileOutcome, CamelError> {
        match step {
            // ── To ──
            BuilderStep::To(uri) => {
                let parsed = parse_uri(&uri)?;
                let component = ctx
                    .component_ctx
                    .resolve_component(&parsed.scheme)
                    .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
                let endpoint = component.create_endpoint(&uri, ctx.component_ctx.as_ref())?;
                let contract = endpoint.body_contract();
                let producer = endpoint.create_producer(Arc::clone(&ctx.rt), ctx.producer_ctx)?;
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: producer,
                    body_contract: contract,
                    lifecycle: None,
                }))
            }

            // ── WireTap ──
            BuilderStep::WireTap { uri } => {
                let producer = resolve_producer(ctx, &uri)?;
                let svc = camel_processor::WireTapService::new(producer);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            _ => Ok(CompileOutcome::NotHandled(step)),
        }
    }
}
