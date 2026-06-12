//! Splitting step compilers: Split, DeclarativeSplit, DeclarativeStreamSplit, Aggregate, Multicast.
//!
//! These steps split exchanges into fragments or aggregate them back together.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use camel_api::{BoxProcessor, CamelError, Exchange, Value, body::Body};

use super::{CompilationContext, StepCompileResult, StepCompiler, StepCompilerRegistry};
use crate::lifecycle::adapters::route_compiler::compose_pipeline;
use crate::lifecycle::adapters::route_controller::SharedLanguageRegistry;
use crate::lifecycle::adapters::step_resolution::{await_eval, compile_language_expression};
use crate::lifecycle::application::route_definition::BuilderStep;

pub(crate) struct SplittingCompiler;

impl StepCompiler for SplittingCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        ctx: &CompilationContext,
        registry: &StepCompilerRegistry,
    ) -> StepCompileResult {
        match step {
            // ── Split (programmatic) ──
            BuilderStep::Split { config, steps } => {
                let sub_pairs = match ctx.compile_children(steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let splitter =
                    match camel_processor::splitter::SplitterService::new(config, sub_pipeline) {
                        Ok(s) => s,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                StepCompileResult::Matched(Ok((BoxProcessor::new(splitter), None)))
            }

            // ── DeclarativeSplit ──
            BuilderStep::DeclarativeSplit {
                expression,
                aggregation,
                parallel,
                parallel_limit,
                stop_on_exception,
                steps,
            } => {
                let lang_expr = match compile_language_expression(ctx.languages, &expression) {
                    Ok(e) => e,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let split_fn = move |exchange: &Exchange| {
                    let value = await_eval(&lang_expr, exchange);
                    match value {
                        Value::String(s) => s
                            .lines()
                            .filter(|line| !line.is_empty())
                            .map(|line| {
                                let mut fragment = exchange.clone();
                                fragment.input.body = Body::from(line.to_string());
                                fragment
                            })
                            .collect(),
                        Value::Array(arr) => arr
                            .into_iter()
                            .map(|v| {
                                let mut fragment = exchange.clone();
                                fragment.input.body = Body::from(v);
                                fragment
                            })
                            .collect(),
                        _ => vec![exchange.clone()],
                    }
                };

                let mut config = camel_api::splitter::SplitterConfig::new(Arc::new(split_fn))
                    .aggregation(aggregation)
                    .parallel(parallel)
                    .stop_on_exception(stop_on_exception);
                if let Some(limit) = parallel_limit {
                    config = config.parallel_limit(limit);
                }

                let sub_pairs = match ctx.compile_children(steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let splitter =
                    match camel_processor::splitter::SplitterService::new(config, sub_pipeline) {
                        Ok(s) => s,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                StepCompileResult::Matched(Ok((BoxProcessor::new(splitter), None)))
            }

            // ── DeclarativeStreamSplit ──
            BuilderStep::DeclarativeStreamSplit {
                stream_config,
                aggregation,
                stop_on_exception,
                steps,
            } => {
                if let Err(e) = stream_config.validate() {
                    return StepCompileResult::Matched(Err(CamelError::Config(format!(
                        "invalid stream config: {e}"
                    ))));
                }
                let sub_pairs = match ctx.compile_children(steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);

                let config_clone = stream_config.clone();
                let expression: camel_api::StreamingSplitExpression =
                    Arc::new(move |exchange: Exchange| {
                        let config = config_clone.clone();
                        let (stream_body, parent) = match &exchange.input.body {
                            camel_api::Body::Stream(sb) => (sb.clone(), {
                                let mut p = exchange.clone();
                                p.input.body = camel_api::Body::Empty;
                                p
                            }),
                            _ => {
                                return Box::pin(futures::stream::once(async {
                                    Err(camel_api::CamelError::ProcessorError(
                                        "streaming split requires Body::Stream".into(),
                                    ))
                                }));
                            }
                        };
                        let stream = match stream_body.stream.try_lock() {
                            Ok(mut guard) => match guard.take() {
                                Some(s) => s,
                                None => {
                                    return Box::pin(futures::stream::once(async {
                                        Err(camel_api::CamelError::ProcessorError(
                                            "stream body already consumed".into(),
                                        ))
                                    }));
                                }
                            },
                            Err(_) => {
                                return Box::pin(futures::stream::once(async {
                                    Err(camel_api::CamelError::ProcessorError(
                                        "stream body locked".into(),
                                    ))
                                }));
                            }
                        };
                        let input = camel_processor::stream_codec::StreamSplitInput {
                            parent,
                            stream,
                            metadata: stream_body.metadata,
                        };
                        let resolved_format = match camel_processor::stream_codec::resolve_format(
                            &config.format,
                            &input.metadata,
                        ) {
                            Ok(f) => f,
                            Err(e) => {
                                return Box::pin(futures::stream::once(async { Err(e) }));
                            }
                        };
                        let codec = camel_processor::stream_codec::resolve_codec(&resolved_format);
                        codec.split(input, config)
                    });

                let splitter = camel_processor::streaming_splitter::StreamingSplitterService::new(
                    expression,
                    sub_pipeline,
                    aggregation,
                    stop_on_exception,
                );
                StepCompileResult::Matched(Ok((BoxProcessor::new(splitter), None)))
            }

            // ── Aggregate ──
            BuilderStep::Aggregate { config } => {
                let (late_tx, _late_rx) = mpsc::channel(256);
                let registry: SharedLanguageRegistry =
                    Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
                let cancel = CancellationToken::new();
                let svc =
                    camel_processor::AggregatorService::new(config, late_tx, registry, cancel);
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            // ── Multicast ──
            BuilderStep::Multicast { config, steps } => {
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
                let svc = match camel_processor::MulticastService::new(endpoints, config) {
                    Ok(s) => s,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            _ => StepCompileResult::NotHandled(step),
        }
    }
}
