//! Transform step compilers: Enrich, PollEnrich.
//!
//! These steps enrich or transform the exchange by resolving external data
//! through producers or polling consumers.

use std::time::Duration;

use camel_api::{BoxProcessor, CamelError};
use camel_endpoint::parse_uri;
use camel_processor::{EnrichService, PollEnrichService};

use super::{
    CompilationContext, CompileOutcome, CompiledStep, StepCompiler, StepCompilerRegistry,
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
    ) -> Result<CompileOutcome, CamelError> {
        match step {
            // ── Enrich ──
            BuilderStep::Enrich { uri, strategy, .. } => {
                let producer = resolve_producer(ctx, &uri)?;
                let strategy_arc = resolve_enrichment_strategy(strategy)?;
                let svc = EnrichService::new(producer, strategy_arc);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── PollEnrich ──
            BuilderStep::PollEnrich {
                uri,
                strategy,
                timeout_ms,
            } => {
                let parsed = parse_uri(&uri)?;
                let component = ctx
                    .component_ctx
                    .resolve_component(&parsed.scheme)
                    .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
                let endpoint = component.create_endpoint(&uri, ctx.component_ctx.as_ref())?;
                let poller = endpoint.polling_consumer().ok_or_else(|| {
                    CamelError::EndpointCreationFailed(format!(
                        "pollEnrich requires an endpoint that exposes a PollingConsumer; `{}` does not",
                        uri
                    ))
                })?;
                let timeout = Duration::from_millis(timeout_ms.unwrap_or(5000));
                let strategy_arc = resolve_enrichment_strategy(strategy)?;
                let svc = PollEnrichService::new(poller, timeout, strategy_arc);
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
