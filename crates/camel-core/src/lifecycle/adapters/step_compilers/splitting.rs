//! Splitting step compilers: Split, DeclarativeSplit, DeclarativeStreamSplit, Aggregate, Multicast.
//!
//! These steps split exchanges into fragments or aggregate them back together.

use std::sync::Arc;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::pin::Pin;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use camel_api::{
    Body, BoxProcessor, CamelError, Exchange, MulticastStrategy, StreamSplitFormat, Value,
};

use super::{
    CompilationContext, CompileOutcome, CompiledStep, StepCompiler, StepCompilerRegistry,
    pack_lifecycles,
};
use crate::lifecycle::adapters::route_compiler::compose_outcome_segment;
use crate::lifecycle::adapters::route_controller::SharedLanguageRegistry;
use crate::lifecycle::adapters::step_resolution::{await_eval, compile_language_expression};
use crate::lifecycle::application::route_definition::BuilderStep;

/// Collect a byte stream into a single `Bytes` value, enforcing a size limit.
async fn collect_stream_with_limit(
    stream: futures::stream::BoxStream<'static, Result<Bytes, CamelError>>,
    max_bytes: u64,
) -> Result<Bytes, CamelError> {
    let mut stream = stream;
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let new_len = buf.len().saturating_add(chunk.len());
        if new_len as u64 > max_bytes {
            return Err(CamelError::TypeConversionFailed(format!(
                "ZIP archive exceeds max compressed size: {max_bytes}"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(buf))
}

/// Extract bytes from a materialized body (Bytes or Text), creating a parent exchange
/// with an empty body.
fn extract_bytes_from_body(exchange: &Exchange) -> Result<(Bytes, Exchange), CamelError> {
    match &exchange.input.body {
        Body::Bytes(b) => {
            let bytes = b.clone();
            let mut parent = exchange.clone();
            parent.input.body = Body::Empty;
            Ok((bytes, parent))
        }
        Body::Text(s) => {
            let bytes = Bytes::copy_from_slice(s.as_bytes());
            let mut parent = exchange.clone();
            parent.input.body = Body::Empty;
            Ok((bytes, parent))
        }
        _ => Err(CamelError::ProcessorError(
            "ZIP split requires Body::Bytes, Body::Text, or Body::Stream".into(),
        )),
    }
}

type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, CamelError>> + Send>>;

/// Extract the byte stream from a StreamBody (single-consumption).
fn take_stream(stream_body: &camel_api::StreamBody) -> Result<ByteStream, CamelError> {
    match stream_body.stream.try_lock() {
        Ok(mut guard) => match guard.take() {
            Some(s) => Ok(s),
            None => Err(CamelError::ProcessorError(
                "stream body already consumed".into(),
            )),
        },
        Err(_) => Err(CamelError::ProcessorError("stream body locked".into())),
    }
}

pub(crate) struct SplittingCompiler;

impl StepCompiler for SplittingCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        ctx: &CompilationContext,
        registry: &StepCompilerRegistry,
    ) -> Result<CompileOutcome, CamelError> {
        match step {
            // ── Split (programmatic) ──
            BuilderStep::Split { config, steps } => {
                let (sub_segments, lifecycles) = ctx.compile_children_segments(steps, registry)?;
                let body_segment = compose_outcome_segment(sub_segments);
                let split_segment = camel_processor::SplitSegment {
                    splitter: config.expression,
                    body: body_segment,
                    parallel: config.parallel,
                    parallel_limit: config.parallel_limit,
                    stop_on_exception: config.stop_on_exception,
                    aggregation: config.aggregation,
                };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(split_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                }))
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
                let lang_expr = compile_language_expression(ctx.languages, &expression)?;
                let split_fn: camel_api::splitter::SplitExpression =
                    Arc::new(move |exchange: &Exchange| {
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
                    });

                let (sub_segments, lifecycles) = ctx.compile_children_segments(steps, registry)?;
                let body_segment = compose_outcome_segment(sub_segments);
                let split_segment = camel_processor::SplitSegment {
                    splitter: split_fn,
                    body: body_segment,
                    parallel,
                    parallel_limit,
                    stop_on_exception,
                    aggregation,
                };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(split_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                }))
            }

            // ── DeclarativeStreamSplit ──
            BuilderStep::DeclarativeStreamSplit {
                stream_config,
                aggregation,
                stop_on_exception,
                steps,
            } => {
                stream_config
                    .validate()
                    .map_err(|e| CamelError::Config(format!("invalid stream config: {e}")))?;
                let (sub_segments, lifecycles) = ctx.compile_children_segments(steps, registry)?;
                let body_segment = compose_outcome_segment(sub_segments);

                let config_clone = stream_config.clone();
                let zip_config = camel_processor::zip_splitter::ZipSplitConfig::default();
                let expression: camel_api::StreamingSplitExpression = Arc::new(
                    move |exchange: Exchange| {
                        let config = config_clone.clone();
                        match config.format {
                            StreamSplitFormat::Zip => {
                                let zip_config = zip_config.clone();
                                match &exchange.input.body {
                                    Body::Stream(sb) => {
                                        let sb = sb.clone();
                                        let mut parent = exchange.clone();
                                        parent.input.body = Body::Empty;
                                        let stream = match take_stream(&sb) {
                                            Ok(s) => s,
                                            Err(e) => {
                                                return Box::pin(futures::stream::once(
                                                    async move { Err(e) },
                                                ));
                                            }
                                        };
                                        Box::pin(async_stream::stream! {
                                            let collected =
                                                match collect_stream_with_limit(
                                                    stream,
                                                    zip_config.max_compressed_size,
                                                )
                                                .await
                                                {
                                                    Ok(b) => b,
                                                    Err(e) => {
                                                        yield Err(e);
                                                        return;
                                                    }
                                                };
                                            parent.set_property(
                                                "CamelSplitMaterialized",
                                                Value::Bool(true),
                                            );
                                            parent.set_property(
                                                "CamelSplitMaterializedBytes",
                                                Value::from(collected.len() as u64),
                                            );
                                            let mut result_stream =
                                                camel_processor::zip_splitter::split_zip_bytes(
                                                    parent,
                                                    collected,
                                                    zip_config,
                                                );
                                            while let Some(item) = result_stream.next().await {
                                                yield item;
                                            }
                                        })
                                    }
                                    _ => match extract_bytes_from_body(&exchange) {
                                        Ok((bytes, mut parent)) => {
                                            parent.set_property(
                                                "CamelSplitMaterialized",
                                                Value::Bool(true),
                                            );
                                            parent.set_property(
                                                "CamelSplitMaterializedBytes",
                                                Value::from(bytes.len() as u64),
                                            );
                                            camel_processor::zip_splitter::split_zip_bytes(
                                                parent, bytes, zip_config,
                                            )
                                        }
                                        Err(e) => {
                                            Box::pin(futures::stream::once(async move { Err(e) }))
                                        }
                                    },
                                }
                            }
                            _ => {
                                // Incremental codec path (ndjson, lines, chunks)
                                let (stream_body, parent) = match &exchange.input.body {
                                    Body::Stream(sb) => (sb.clone(), {
                                        let mut p = exchange.clone();
                                        p.input.body = Body::Empty;
                                        p
                                    }),
                                    _ => {
                                        return Box::pin(futures::stream::once(async {
                                            Err(CamelError::ProcessorError(
                                                "streaming split requires Body::Stream".into(),
                                            ))
                                        }));
                                    }
                                };
                                let stream = match take_stream(&stream_body) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        return Box::pin(futures::stream::once(
                                            async move { Err(e) },
                                        ));
                                    }
                                };
                                let input = camel_processor::stream_codec::StreamSplitInput {
                                    parent,
                                    stream,
                                    metadata: stream_body.metadata,
                                };
                                match camel_processor::stream_codec::resolve_split(
                                    &config.format,
                                    &input.metadata,
                                ) {
                                    Ok(
                                        camel_processor::stream_codec::ResolvedStreamSplit::Incremental(
                                            codec,
                                        ),
                                    ) => codec.split(input, config),
                                    Ok(
                                        camel_processor::stream_codec::ResolvedStreamSplit::MaterializedArchive(
                                            _,
                                        ),
                                    ) => Box::pin(futures::stream::once(async {
                                        Err(CamelError::ProcessorError(
                                            "materialized archive formats require Body::Bytes, Body::Text, or Body::Stream"
                                                .into(),
                                        ))
                                    })),
                                    Err(e) => {
                                        Box::pin(futures::stream::once(async move { Err(e) }))
                                    }
                                }
                            }
                        }
                    },
                );

                let segment = camel_processor::StreamingSplitSegment {
                    expression,
                    body: body_segment,
                    aggregation,
                    stop_on_exception,
                };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                }))
            }

            // ── Aggregate ──
            BuilderStep::Aggregate { config } => {
                let (late_tx, _late_rx) = mpsc::channel(256);
                let registry: SharedLanguageRegistry =
                    Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
                let cancel = CancellationToken::new();
                let svc =
                    camel_processor::AggregatorService::new(config, late_tx, registry, cancel);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Multicast ──
            BuilderStep::Multicast { config, steps } => {
                let mut branch_segments: Vec<camel_api::OutcomeSegment> = Vec::new();
                let mut all_lifecycles = Vec::new();
                for step in steps {
                    let (sub_segments, lifecycles) =
                        ctx.compile_children_segments(vec![step], registry)?;
                    all_lifecycles.extend(lifecycles);
                    branch_segments.push(compose_outcome_segment(sub_segments));
                }
                let strategy = config.aggregation.clone();
                let aggregator: Arc<dyn Fn(Vec<Exchange>) -> Exchange + Send + Sync> = Arc::new(
                    move |outputs| match &strategy {
                        MulticastStrategy::LastWins => {
                            outputs.into_iter().last().unwrap_or_default()
                        }
                        MulticastStrategy::CollectAll => {
                            let bodies: Vec<Value> = outputs
                                .iter()
                                .map(|ex| match &ex.input.body {
                                    Body::Text(s) => Value::String(s.clone()),
                                    Body::Json(v) => v.clone(),
                                    Body::Xml(s) => Value::String(s.clone()),
                                    Body::Bytes(b) => {
                                        Value::String(String::from_utf8_lossy(b).into_owned())
                                    }
                                    Body::Empty => Value::Null,
                                    Body::Stream(s) => serde_json::json!({
                                        "_stream": {
                                            "origin": s.metadata.origin,
                                            "placeholder": true,
                                            "hint": "Materialize exchange body with .into_bytes() before multicast aggregation"
                                        }
                                    }),
                                })
                                .collect();
                            let mut out = Exchange::default();
                            out.input.body = Body::Json(Value::Array(bodies));
                            out
                        }
                        MulticastStrategy::Original => {
                            outputs.into_iter().next().unwrap_or_default()
                        }
                        MulticastStrategy::Custom(fold_fn) => {
                            let mut iter = outputs.into_iter();
                            let first = iter.next().unwrap_or_default();
                            iter.fold(first, |acc, next| fold_fn(acc, next))
                        }
                    },
                );
                let segment = camel_processor::MulticastSegment {
                    branches: branch_segments,
                    parallel: config.parallel,
                    parallel_limit: config.parallel_limit,
                    stop_on_exception: config.stop_on_exception,
                    timeout: config.timeout,
                    aggregator,
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
