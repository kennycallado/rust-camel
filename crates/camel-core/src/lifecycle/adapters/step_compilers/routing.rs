//! Routing step compilers: DynamicRouter, RoutingSlip, RecipientList, Throttle, LoadBalance.
//!
//! Steps that route exchanges to multiple destinations using endpoint resolvers.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use camel_api::{BoxProcessor, CamelError};

use super::{
    CompilationContext, CompileOutcome, CompiledStep, StepCompiler, StepCompilerRegistry,
    pack_lifecycles,
};
use crate::lifecycle::adapters::endpoint_resolver_factory;
use crate::lifecycle::adapters::route_compiler::compose_outcome_segment;
use crate::lifecycle::adapters::step_resolution::{await_eval, compile_language_expression};
use crate::lifecycle::application::route_definition::BuilderStep;

pub(crate) struct RoutingCompiler;

impl StepCompiler for RoutingCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        ctx: &CompilationContext,
        registry: &StepCompilerRegistry,
    ) -> Result<CompileOutcome, CamelError> {
        match step {
            // ── DynamicRouter ──
            BuilderStep::DynamicRouter { config } => {
                let resolver = endpoint_resolver_factory::make_endpoint_resolver(
                    Arc::clone(&ctx.component_ctx),
                    Arc::clone(&ctx.rt),
                    ctx.producer_ctx.clone(),
                );
                let svc =
                    camel_processor::dynamic_router::DynamicRouterService::new(config, resolver);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── DeclarativeDynamicRouter ──
            BuilderStep::DeclarativeDynamicRouter {
                expression,
                uri_delimiter,
                cache_size,
                ignore_invalid_endpoints,
                max_iterations,
            } => {
                let expression = compile_language_expression(ctx.languages, &expression)?;
                let expression: camel_api::RouterExpression =
                    Arc::new(move |exchange: &camel_api::Exchange| {
                        let value = await_eval(&expression, exchange);
                        match value {
                            camel_api::Value::Null => None,
                            camel_api::Value::String(s) => Some(s),
                            other => Some(other.to_string()),
                        }
                    });

                let config = camel_api::DynamicRouterConfig::new(expression)
                    .uri_delimiter(uri_delimiter)
                    .cache_size(cache_size)
                    .ignore_invalid_endpoints(ignore_invalid_endpoints)
                    .max_iterations(max_iterations);

                let resolver = endpoint_resolver_factory::make_endpoint_resolver(
                    Arc::clone(&ctx.component_ctx),
                    Arc::clone(&ctx.rt),
                    ctx.producer_ctx.clone(),
                );
                let svc =
                    camel_processor::dynamic_router::DynamicRouterService::new(config, resolver);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── RoutingSlip ──
            BuilderStep::RoutingSlip { config } => {
                let resolver = endpoint_resolver_factory::make_endpoint_resolver(
                    Arc::clone(&ctx.component_ctx),
                    Arc::clone(&ctx.rt),
                    ctx.producer_ctx.clone(),
                );
                let svc = camel_processor::routing_slip::RoutingSlipService::new(config, resolver);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── DeclarativeRoutingSlip ──
            BuilderStep::DeclarativeRoutingSlip {
                expression,
                uri_delimiter,
                cache_size,
                ignore_invalid_endpoints,
            } => {
                let expression = compile_language_expression(ctx.languages, &expression)?;
                let expression: camel_api::RoutingSlipExpression =
                    Arc::new(move |exchange: &camel_api::Exchange| {
                        let value = await_eval(&expression, exchange);
                        match value {
                            camel_api::Value::Null => None,
                            camel_api::Value::String(s) => Some(s),
                            other => Some(other.to_string()),
                        }
                    });

                let config = camel_api::RoutingSlipConfig::new(expression)
                    .uri_delimiter(uri_delimiter)
                    .cache_size(cache_size)
                    .ignore_invalid_endpoints(ignore_invalid_endpoints);

                let resolver = endpoint_resolver_factory::make_endpoint_resolver(
                    Arc::clone(&ctx.component_ctx),
                    Arc::clone(&ctx.rt),
                    ctx.producer_ctx.clone(),
                );
                let svc = camel_processor::routing_slip::RoutingSlipService::new(config, resolver);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── RecipientList ──
            BuilderStep::RecipientList { config } => {
                let resolver = endpoint_resolver_factory::make_endpoint_resolver(
                    Arc::clone(&ctx.component_ctx),
                    Arc::clone(&ctx.rt),
                    ctx.producer_ctx.clone(),
                );
                let svc =
                    camel_processor::recipient_list::RecipientListService::new(config, resolver)?;
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── DeclarativeRecipientList ──
            BuilderStep::DeclarativeRecipientList {
                expression,
                delimiter,
                parallel,
                parallel_limit,
                stop_on_exception,
                aggregation: _aggregation,
            } => {
                let expression = compile_language_expression(ctx.languages, &expression)?;
                let expression: camel_api::recipient_list::RecipientListExpression =
                    Arc::new(move |exchange: &camel_api::Exchange| {
                        let value = await_eval(&expression, exchange);
                        match value {
                            camel_api::Value::Null => String::new(),
                            camel_api::Value::String(s) => s,
                            other => other.to_string(),
                        }
                    });

                let config = camel_api::recipient_list::RecipientListConfig::new(expression)
                    .delimiter(&delimiter)
                    .parallel(parallel)
                    .stop_on_exception(stop_on_exception);
                let config = if let Some(limit) = parallel_limit {
                    config.parallel_limit(limit)
                } else {
                    config
                };

                let resolver = endpoint_resolver_factory::make_endpoint_resolver(
                    Arc::clone(&ctx.component_ctx),
                    Arc::clone(&ctx.rt),
                    ctx.producer_ctx.clone(),
                );
                let svc =
                    camel_processor::recipient_list::RecipientListService::new(config, resolver)?;
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Throttle (recursive — compiles child steps, outcome-aware) ──
            BuilderStep::Throttle { config, steps } => {
                let (sub_segments, lifecycles) = ctx.compile_children_segments(steps, registry)?;
                let body = compose_outcome_segment(sub_segments);
                let segment = camel_processor::ThrottleSegment::new(config, body);
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                }))
            }

            // ── LoadBalance (recursive — each step compiles to an OutcomeSegment) ──
            BuilderStep::LoadBalance { config, steps } => {
                let mut destinations: Vec<camel_api::OutcomeSegment> = Vec::new();
                let mut all_lifecycles = Vec::new();
                for step in steps {
                    let (sub_segments, lifecycles) =
                        ctx.compile_children_segments(vec![step], registry)?;
                    all_lifecycles.extend(lifecycles);
                    destinations.push(compose_outcome_segment(sub_segments));
                }
                let segment = camel_processor::LoadBalanceSegment {
                    destinations,
                    strategy: config.strategy,
                    round_robin_index: Arc::new(AtomicUsize::new(0)),
                };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(all_lifecycles),
                }))
            }

            _ => Ok(CompileOutcome::NotHandled(step)),
        }
    }
}
