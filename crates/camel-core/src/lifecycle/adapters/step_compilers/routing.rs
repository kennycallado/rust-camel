//! Routing step compilers: DynamicRouter, RoutingSlip, RecipientList, Throttle, LoadBalance.
//!
//! Steps that route exchanges to multiple destinations using endpoint resolvers.

use std::sync::Arc;

use camel_api::BoxProcessor;

use super::{CompilationContext, StepCompileResult, StepCompiler, StepCompilerRegistry};
use crate::lifecycle::adapters::endpoint_resolver_factory;
use crate::lifecycle::adapters::route_compiler::compose_pipeline;
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
    ) -> StepCompileResult {
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
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            // ── DeclarativeDynamicRouter ──
            BuilderStep::DeclarativeDynamicRouter {
                expression,
                uri_delimiter,
                cache_size,
                ignore_invalid_endpoints,
                max_iterations,
            } => {
                let expression = match compile_language_expression(ctx.languages, &expression) {
                    Ok(e) => e,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
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
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            // ── RoutingSlip ──
            BuilderStep::RoutingSlip { config } => {
                let resolver = endpoint_resolver_factory::make_endpoint_resolver(
                    Arc::clone(&ctx.component_ctx),
                    Arc::clone(&ctx.rt),
                    ctx.producer_ctx.clone(),
                );
                let svc = camel_processor::routing_slip::RoutingSlipService::new(config, resolver);
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            // ── DeclarativeRoutingSlip ──
            BuilderStep::DeclarativeRoutingSlip {
                expression,
                uri_delimiter,
                cache_size,
                ignore_invalid_endpoints,
            } => {
                let expression = match compile_language_expression(ctx.languages, &expression) {
                    Ok(e) => e,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
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
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            // ── RecipientList ──
            BuilderStep::RecipientList { config } => {
                let resolver = endpoint_resolver_factory::make_endpoint_resolver(
                    Arc::clone(&ctx.component_ctx),
                    Arc::clone(&ctx.rt),
                    ctx.producer_ctx.clone(),
                );
                let svc = match camel_processor::recipient_list::RecipientListService::new(
                    config, resolver,
                ) {
                    Ok(s) => s,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
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
                let expression = match compile_language_expression(ctx.languages, &expression) {
                    Ok(e) => e,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
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
                let svc = match camel_processor::recipient_list::RecipientListService::new(
                    config, resolver,
                ) {
                    Ok(s) => s,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            // ── Throttle (recursive — compiles child steps) ──
            BuilderStep::Throttle { config, steps } => {
                let sub_pairs = match ctx.compile_children(steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::throttler::ThrottlerService::new(config, sub_pipeline);
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            // ── LoadBalance (recursive — each step is an independent endpoint) ──
            BuilderStep::LoadBalance { config, steps } => {
                let mut endpoints = Vec::new();
                for step in steps {
                    let sub_pairs = match ctx.compile_children(vec![step], registry) {
                        Ok(p) => p,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
                    let endpoint = compose_pipeline(sub_processors);
                    endpoints.push(endpoint);
                }
                let svc =
                    camel_processor::load_balancer::LoadBalancerService::new(endpoints, config);
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            _ => StepCompileResult::NotHandled(step),
        }
    }
}
