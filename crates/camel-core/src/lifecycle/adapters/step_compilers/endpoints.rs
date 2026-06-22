//! Endpoint step compilers: To, WireTap.
//!
//! These steps create producers from URIs at compile time.

use std::sync::Arc;

use camel_api::{BoxProcessor, CamelError};

use camel_endpoint::parse_uri;

use super::{
    CompilationContext, CompiledStep, StepCompileResult, StepCompiler, StepCompilerRegistry,
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
    ) -> StepCompileResult {
        match step {
            // ── To ──
            BuilderStep::To(uri) => {
                let parsed = match parse_uri(&uri) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let component = ctx
                    .component_ctx
                    .resolve_component(&parsed.scheme)
                    .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()));
                let component = match component {
                    Ok(c) => c,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let endpoint = match component.create_endpoint(&uri, ctx.component_ctx.as_ref()) {
                    Ok(e) => e,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let contract = endpoint.body_contract();
                let producer = match endpoint.create_producer(Arc::clone(&ctx.rt), ctx.producer_ctx)
                {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: producer,
                    body_contract: contract,
                }))
            }

            // ── WireTap ──
            BuilderStep::WireTap { uri } => {
                let producer = match resolve_producer(ctx, &uri) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let svc = camel_processor::WireTapService::new(producer);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            _ => StepCompileResult::NotHandled(step),
        }
    }
}
