//! Transform step compilers: Enrich, PollEnrich.
//!
//! These steps enrich or transform the exchange by resolving external data
//! through producers or polling consumers.

use std::time::Duration;

use camel_api::{BoxProcessor, CamelError};
use camel_endpoint::parse_uri;
use camel_processor::{EnrichService, PollEnrichService};

use super::{
    CompilationContext, CompiledStep, StepCompileResult, StepCompiler, StepCompilerRegistry,
    resolve_producer,
};
use crate::lifecycle::adapters::step_resolution::resolve_enrichment_strategy;
use crate::lifecycle::application::route_definition::BuilderStep;

pub(crate) struct TransformsCompiler;

impl StepCompiler for TransformsCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        ctx: &CompilationContext,
        _registry: &StepCompilerRegistry,
    ) -> StepCompileResult {
        match step {
            // ── Enrich ──
            BuilderStep::Enrich { uri, strategy, .. } => {
                let producer = match resolve_producer(ctx, &uri) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let strategy_arc = match resolve_enrichment_strategy(strategy) {
                    Ok(s) => s,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let svc = EnrichService::new(producer, strategy_arc);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            // ── PollEnrich ──
            BuilderStep::PollEnrich {
                uri,
                strategy,
                timeout_ms,
            } => {
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
                let poller = match endpoint.polling_consumer() {
                    Some(p) => p,
                    None => {
                        return StepCompileResult::Matched(Err(
                            CamelError::EndpointCreationFailed(format!(
                                "pollEnrich requires an endpoint that exposes a PollingConsumer; `{}` does not",
                                uri
                            )),
                        ));
                    }
                };
                let timeout = Duration::from_millis(timeout_ms.unwrap_or(5000));
                let strategy_arc = match resolve_enrichment_strategy(strategy) {
                    Ok(s) => s,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let svc = PollEnrichService::new(poller, timeout, strategy_arc);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            _ => StepCompileResult::NotHandled(step),
        }
    }
}
