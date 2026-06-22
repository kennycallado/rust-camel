//! Splitting step compilers: Split, DeclarativeSplit, DeclarativeStreamSplit, Aggregate, Multicast.
//!
//! These steps split exchanges into fragments or aggregate them back together.

use std::sync::Arc;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::pin::Pin;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use camel_api::{BoxProcessor, CamelError, Exchange, StreamSplitFormat, Value, body::Body};

use super::{
    CompilationContext, CompiledStep, StepCompileResult, StepCompiler, StepCompilerRegistry,
};
use crate::lifecycle::adapters::route_compiler::compose_pipeline;
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
    ) -> StepCompileResult {
        match step {
            // ── Split (programmatic) ──
            BuilderStep::Split { config, steps } => {
                let sub_pairs = match ctx.compile_children(steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_processors: Vec<CompiledStep> = sub_pairs
                    .into_iter()
                    .map(|c| match c {
                        CompiledStep::Process { .. } => c,
                        CompiledStep::Stop => CompiledStep::Process {
                            processor: BoxProcessor::new(camel_processor::StopService),
                            body_contract: None,
                        },
                    })
                    .collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let splitter =
                    match camel_processor::splitter::SplitterService::new(config, sub_pipeline) {
                        Ok(s) => s,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(splitter),
                    body_contract: None,
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
                let sub_processors: Vec<CompiledStep> = sub_pairs
                    .into_iter()
                    .map(|c| match c {
                        CompiledStep::Process { .. } => c,
                        CompiledStep::Stop => CompiledStep::Process {
                            processor: BoxProcessor::new(camel_processor::StopService),
                            body_contract: None,
                        },
                    })
                    .collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let splitter =
                    match camel_processor::splitter::SplitterService::new(config, sub_pipeline) {
                        Ok(s) => s,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(splitter),
                    body_contract: None,
                }))
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
                let sub_processors: Vec<CompiledStep> = sub_pairs
                    .into_iter()
                    .map(|c| match c {
                        CompiledStep::Process { .. } => c,
                        CompiledStep::Stop => CompiledStep::Process {
                            processor: BoxProcessor::new(camel_processor::StopService),
                            body_contract: None,
                        },
                    })
                    .collect();
                let sub_pipeline = compose_pipeline(sub_processors);

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

                let splitter = camel_processor::streaming_splitter::StreamingSplitterService::new(
                    expression,
                    sub_pipeline,
                    aggregation,
                    stop_on_exception,
                );
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(splitter),
                    body_contract: None,
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
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            // ── Multicast ──
            BuilderStep::Multicast { config, steps } => {
                let mut endpoints = Vec::new();
                for step in steps {
                    let sub_pairs = match ctx.compile_children(vec![step], registry) {
                        Ok(p) => p,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    let endpoint = compose_pipeline(sub_pairs);
                    endpoints.push(endpoint);
                }
                let svc = match camel_processor::MulticastService::new(endpoints, config) {
                    Ok(s) => s,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            _ => StepCompileResult::NotHandled(step),
        }
    }
}
