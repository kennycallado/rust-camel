use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

pub(crate) enum FunctionStagingMode {
    DirectAdd,
    DryCompile,
    HotReload { generation: u64 },
}

use camel_api::{
    BoxProcessor, CamelError, Exchange, FilterPredicate, FunctionInvoker, IdentityProcessor,
    ProducerContext, Value,
    body::Body,
    loop_eip::{LoopConfig, LoopMode},
};
use camel_bean::BeanRegistry;
use camel_component_api::{ComponentContext, RuntimeObservability};
use camel_endpoint::parse_uri;
use camel_language_api::{Expression, Language, LanguageError, Predicate};
use camel_processor::script_mutator::ScriptMutator;
use camel_processor::{
    ChoiceService, EnrichService, EnrichmentStrategy, PollEnrichService, UseEnrichedBody,
    WhenClause,
};

/// Helper to evaluate an async expression from a sync closure context.
///
/// These closures are stored in sync callback types (FilterPredicate, SetBody callback, etc.)
/// but are only executed at runtime inside Tower services (async context). We use
/// `tokio::task::block_in_place` + `Handle::block_on` to bridge the sync→async gap.
fn await_eval(expr: &Arc<dyn Expression>, exchange: &Exchange) -> Value {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::try_current()
            .expect("await_eval: must be called from within a tokio runtime") // allow-unwrap
            .block_on(expr.evaluate(exchange))
    })
    .unwrap_or(Value::Null)
}

/// Helper to evaluate an async predicate from a sync closure context.
fn await_matches(pred: &Arc<dyn Predicate>, exchange: &Exchange) -> bool {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::try_current()
            .expect("await_matches: must be called from within a tokio runtime") // allow-unwrap
            .block_on(pred.matches(exchange))
    })
    .unwrap_or(false)
}

use crate::lifecycle::adapters::route_compiler::compose_pipeline;
use crate::lifecycle::adapters::route_controller::SharedLanguageRegistry;
use crate::lifecycle::application::route_definition::{
    BuilderStep, LanguageExpressionDef, ValueSourceDef,
};
use crate::shared::components::domain::Registry;

pub(crate) fn resolve_language(
    languages: &SharedLanguageRegistry,
    language: &str,
) -> Result<Arc<dyn Language>, CamelError> {
    let guard = languages
        .lock()
        .expect("mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
    guard.get(language).cloned().ok_or_else(|| {
        CamelError::RouteError(format!(
            "language `{language}` is not registered in CamelContext"
        ))
    })
}

pub(crate) fn compile_language_expression(
    languages: &SharedLanguageRegistry,
    expression: &LanguageExpressionDef,
) -> Result<Arc<dyn Expression>, CamelError> {
    let language = resolve_language(languages, &expression.language)?;
    let compiled = language
        .create_expression(&expression.source)
        .map_err(|e| {
            CamelError::RouteError(format!(
                "failed to compile {} expression `{}`: {e}",
                expression.language, expression.source
            ))
        })?;
    Ok(Arc::from(compiled))
}

pub(crate) fn compile_language_predicate(
    languages: &SharedLanguageRegistry,
    expression: &LanguageExpressionDef,
) -> Result<Arc<dyn Predicate>, CamelError> {
    let language = resolve_language(languages, &expression.language)?;
    let compiled = language.create_predicate(&expression.source).map_err(|e| {
        CamelError::RouteError(format!(
            "failed to compile {} predicate `{}`: {e}",
            expression.language, expression.source
        ))
    })?;
    Ok(Arc::from(compiled))
}

pub(crate) fn compile_filter_predicate(
    languages: &SharedLanguageRegistry,
    expression: &LanguageExpressionDef,
) -> Result<FilterPredicate, CamelError> {
    let predicate = compile_language_predicate(languages, expression)?;
    Ok(Arc::new(move |exchange: &Exchange| {
        await_matches(&predicate, exchange)
    }))
}

fn value_to_body(value: Value) -> Body {
    match value {
        Value::Null => Body::Empty,
        Value::String(text) => Body::Text(text),
        other => Body::Json(other),
    }
}

fn resolve_enrichment_strategy(
    name: Option<String>,
) -> Result<Arc<dyn EnrichmentStrategy>, CamelError> {
    match name.as_deref() {
        None | Some("useEnrichedBody") => Ok(Arc::new(UseEnrichedBody)),
        Some(other) => Err(CamelError::ProcessorError(format!(
            "unknown EnrichmentStrategy `{}`; v1 only supports `useEnrichedBody` (or none for default)",
            other
        ))),
    }
}

#[allow(clippy::only_used_in_recursion, clippy::too_many_arguments)]
pub(crate) fn resolve_steps(
    steps: Vec<BuilderStep>,
    producer_ctx: &ProducerContext,
    rt: Arc<dyn RuntimeObservability>,
    registry: &Arc<std::sync::Mutex<Registry>>,
    languages: &SharedLanguageRegistry,
    beans: &Arc<std::sync::Mutex<BeanRegistry>>,
    function_invoker: Option<Arc<dyn FunctionInvoker>>,
    component_ctx: Arc<dyn ComponentContext>,
    route_id: Option<&str>,
    staging_mode: &FunctionStagingMode,
) -> Result<Vec<(BoxProcessor, Option<camel_api::BodyType>)>, CamelError> {
    let resolve_producer = |uri: &str| -> Result<BoxProcessor, CamelError> {
        let parsed = parse_uri(uri)?;
        let component = component_ctx
            .resolve_component(&parsed.scheme)
            .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
        let endpoint = component.create_endpoint(uri, component_ctx.as_ref())?;
        endpoint.create_producer(Arc::clone(&rt), producer_ctx)
    };

    let mut processors: Vec<(BoxProcessor, Option<camel_api::BodyType>)> = Vec::new();
    for (step_index, step) in steps.into_iter().enumerate() {
        match step {
            BuilderStep::Processor(svc) => {
                processors.push((svc, None));
            }
            BuilderStep::To(uri) => {
                let parsed = parse_uri(&uri)?;
                let component = component_ctx
                    .resolve_component(&parsed.scheme)
                    .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
                let endpoint = component.create_endpoint(&uri, component_ctx.as_ref())?;
                let contract = endpoint.body_contract();
                let producer = endpoint.create_producer(Arc::clone(&rt), producer_ctx)?;
                processors.push((producer, contract));
            }
            BuilderStep::Stop => {
                processors.push((BoxProcessor::new(camel_processor::StopService), None));
            }
            BuilderStep::Delay { config } => {
                let svc = camel_processor::delayer::DelayerService::new(config);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::Loop { config, steps } => {
                let sub_pairs = resolve_steps(
                    steps,
                    producer_ctx,
                    Arc::clone(&rt),
                    registry,
                    languages,
                    beans,
                    function_invoker.clone(),
                    Arc::clone(&component_ctx),
                    route_id,
                    staging_mode,
                )?;
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::loop_eip::LoopService::new(config, sub_pipeline);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::DeclarativeLoop {
                count,
                while_predicate,
                steps,
            } => {
                let mode = match (count, while_predicate) {
                    (Some(n), None) => LoopMode::Count(n),
                    (None, Some(pred)) => {
                        let predicate = compile_filter_predicate(languages, &pred)?;
                        LoopMode::While(predicate)
                    }
                    (Some(_), Some(_)) => {
                        return Err(CamelError::RouteError(
                            "loop: cannot specify both 'count' and 'while'".into(),
                        ));
                    }
                    (None, None) => {
                        return Err(CamelError::RouteError(
                            "loop: must specify either 'count' or 'while'".into(),
                        ));
                    }
                };
                let sub_pairs = resolve_steps(
                    steps,
                    producer_ctx,
                    Arc::clone(&rt),
                    registry,
                    languages,
                    beans,
                    function_invoker.clone(),
                    Arc::clone(&component_ctx),
                    route_id,
                    staging_mode,
                )?;
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let config = LoopConfig { mode };
                let svc = camel_processor::loop_eip::LoopService::new(config, sub_pipeline);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::Log { level, message } => {
                let svc = camel_processor::LogProcessor::new(level, message);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::DeclarativeSetHeader { key, value } => match value {
                ValueSourceDef::Literal(value) => {
                    let svc = camel_processor::SetHeader::new(IdentityProcessor, key, value);
                    processors.push((BoxProcessor::new(svc), None));
                }
                ValueSourceDef::Expression(expression) => {
                    let expression = compile_language_expression(languages, &expression)?;
                    let svc = camel_processor::DynamicSetHeader::new(
                        IdentityProcessor,
                        key,
                        move |exchange: &Exchange| await_eval(&expression, exchange),
                    );
                    processors.push((BoxProcessor::new(svc), None));
                }
            },
            BuilderStep::DeclarativeSetProperty { key, value_source } => match value_source {
                ValueSourceDef::Literal(value) => {
                    let svc = camel_processor::set_property::SetProperty::new(
                        IdentityProcessor,
                        key,
                        value,
                    );
                    processors.push((BoxProcessor::new(svc), None));
                }
                ValueSourceDef::Expression(expression) => {
                    let expression = compile_language_expression(languages, &expression)?;
                    let svc = camel_processor::DynamicSetProperty::new(
                        IdentityProcessor,
                        key,
                        move |exchange: &Exchange| await_eval(&expression, exchange),
                    );
                    processors.push((BoxProcessor::new(svc), None));
                }
            },
            BuilderStep::DeclarativeSetBody { value } => match value {
                ValueSourceDef::Literal(value) => {
                    let body = value_to_body(value);
                    let svc = camel_processor::SetBody::new(
                        IdentityProcessor,
                        move |_exchange: &Exchange| body.clone(),
                    );
                    processors.push((BoxProcessor::new(svc), None));
                }
                ValueSourceDef::Expression(expression) => {
                    let expression = compile_language_expression(languages, &expression)?;
                    let svc = camel_processor::SetBody::new(
                        IdentityProcessor,
                        move |exchange: &Exchange| {
                            let value = await_eval(&expression, exchange);
                            value_to_body(value)
                        },
                    );
                    processors.push((BoxProcessor::new(svc), None));
                }
            },
            BuilderStep::DeclarativeFilter { predicate, steps } => {
                let predicate = compile_filter_predicate(languages, &predicate)?;
                let sub_pairs = resolve_steps(
                    steps,
                    producer_ctx,
                    Arc::clone(&rt),
                    registry,
                    languages,
                    beans,
                    function_invoker.clone(),
                    Arc::clone(&component_ctx),
                    route_id,
                    staging_mode,
                )?;
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::FilterService::from_predicate(predicate, sub_pipeline);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::DeclarativeChoice { whens, otherwise } => {
                let mut when_clauses = Vec::new();
                for when_step in whens {
                    let predicate = compile_filter_predicate(languages, &when_step.predicate)?;
                    let sub_pairs = resolve_steps(
                        when_step.steps,
                        producer_ctx,
                        Arc::clone(&rt),
                        registry,
                        languages,
                        beans,
                        function_invoker.clone(),
                        Arc::clone(&component_ctx),
                        route_id,
                        staging_mode,
                    )?;
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
                    let pipeline = compose_pipeline(sub_processors);
                    when_clauses.push(WhenClause {
                        predicate,
                        pipeline,
                    });
                }
                let otherwise_pipeline = if let Some(otherwise_steps) = otherwise {
                    let sub_pairs = resolve_steps(
                        otherwise_steps,
                        producer_ctx,
                        Arc::clone(&rt),
                        registry,
                        languages,
                        beans,
                        function_invoker.clone(),
                        Arc::clone(&component_ctx),
                        route_id,
                        staging_mode,
                    )?;
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
                    Some(compose_pipeline(sub_processors))
                } else {
                    None
                };
                let svc = ChoiceService::new(when_clauses, otherwise_pipeline);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::DeclarativeScript { expression } => {
                let lang = resolve_language(languages, &expression.language)?;
                match lang.create_mutating_expression(&expression.source) {
                    Ok(mut_expr) => {
                        processors.push((BoxProcessor::new(ScriptMutator::new(mut_expr)), None));
                    }
                    Err(LanguageError::NotSupported { .. }) => {
                        // Graceful degradation: YAML declarative routes fall back to read-only
                        // Expression → SetBody when the language doesn't support MutatingExpression.
                        // This preserves backwards compatibility for languages like Simple that
                        // only implement Expression. Contrast with the explicit .script() DSL step
                        // which hard-errors on NotSupported (user opted in to mutation semantics).
                        // TODO: add integration test asserting Simple language falls back to
                        // read-only path (requires full CamelContext test harness).
                        let expression = compile_language_expression(languages, &expression)?;
                        let svc = camel_processor::SetBody::new(
                            IdentityProcessor,
                            move |exchange: &Exchange| {
                                let value = await_eval(&expression, exchange);
                                value_to_body(value)
                            },
                        );
                        processors.push((BoxProcessor::new(svc), None));
                    }
                    Err(e) => {
                        return Err(CamelError::RouteError(format!(
                            "Failed to create mutating expression for language '{}': {}",
                            expression.language, e
                        )));
                    }
                }
            }
            BuilderStep::DeclarativeFunction { mut definition } => {
                let Some(invoker) = function_invoker.clone() else {
                    return Err(CamelError::Config(
                        "function: step requires FunctionRuntimeService registered via with_lifecycle"
                            .into(),
                    ));
                };
                definition.route_id = route_id.map(|s| s.to_string());
                definition.step_index = Some(step_index);
                match staging_mode {
                    FunctionStagingMode::DirectAdd => {
                        invoker.stage_pending(definition.clone(), route_id, 0);
                    }
                    FunctionStagingMode::HotReload { generation } => {
                        invoker.stage_pending(definition.clone(), route_id, *generation);
                    }
                    FunctionStagingMode::DryCompile => {}
                }
                let step = crate::step::function_step::FunctionStep::new(invoker, definition);
                processors.push((BoxProcessor::new(step), None));
            }
            BuilderStep::Split { config, steps } => {
                let sub_pairs = resolve_steps(
                    steps,
                    producer_ctx,
                    Arc::clone(&rt),
                    registry,
                    languages,
                    beans,
                    function_invoker.clone(),
                    Arc::clone(&component_ctx),
                    route_id,
                    staging_mode,
                )?;
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let splitter =
                    camel_processor::splitter::SplitterService::new(config, sub_pipeline)?;
                processors.push((BoxProcessor::new(splitter), None));
            }
            BuilderStep::DeclarativeSplit {
                expression,
                aggregation,
                parallel,
                parallel_limit,
                stop_on_exception,
                steps,
            } => {
                let lang_expr = compile_language_expression(languages, &expression)?;
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

                let sub_pairs = resolve_steps(
                    steps,
                    producer_ctx,
                    Arc::clone(&rt),
                    registry,
                    languages,
                    beans,
                    function_invoker.clone(),
                    Arc::clone(&component_ctx),
                    route_id,
                    staging_mode,
                )?;
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let splitter =
                    camel_processor::splitter::SplitterService::new(config, sub_pipeline)?;
                processors.push((BoxProcessor::new(splitter), None));
            }
            BuilderStep::DeclarativeStreamSplit {
                stream_config,
                aggregation,
                stop_on_exception,
                steps,
            } => {
                stream_config
                    .validate()
                    .map_err(|e| CamelError::Config(format!("invalid stream config: {e}")))?;
                let sub_pairs = resolve_steps(
                    steps,
                    producer_ctx,
                    Arc::clone(&rt),
                    registry,
                    languages,
                    beans,
                    function_invoker.clone(),
                    Arc::clone(&component_ctx),
                    route_id,
                    staging_mode,
                )?;
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
                            Err(e) => return Box::pin(futures::stream::once(async { Err(e) })),
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
                processors.push((BoxProcessor::new(splitter), None));
            }
            BuilderStep::Aggregate { config } => {
                let (late_tx, _late_rx) = mpsc::channel(256);
                let registry: SharedLanguageRegistry =
                    Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
                let cancel = CancellationToken::new();
                let svc =
                    camel_processor::AggregatorService::new(config, late_tx, registry, cancel);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::Filter { predicate, steps } => {
                let sub_pairs = resolve_steps(
                    steps,
                    producer_ctx,
                    Arc::clone(&rt),
                    registry,
                    languages,
                    beans,
                    function_invoker.clone(),
                    Arc::clone(&component_ctx),
                    route_id,
                    staging_mode,
                )?;
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::FilterService::from_predicate(predicate, sub_pipeline);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::Choice { whens, otherwise } => {
                // Resolve each when clause's sub-steps into a pipeline.
                let mut when_clauses = Vec::new();
                for when_step in whens {
                    let sub_pairs = resolve_steps(
                        when_step.steps,
                        producer_ctx,
                        Arc::clone(&rt),
                        registry,
                        languages,
                        beans,
                        function_invoker.clone(),
                        Arc::clone(&component_ctx),
                        route_id,
                        staging_mode,
                    )?;
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
                    let pipeline = compose_pipeline(sub_processors);
                    when_clauses.push(WhenClause {
                        predicate: when_step.predicate,
                        pipeline,
                    });
                }
                // Resolve otherwise branch (if present).
                let otherwise_pipeline = if let Some(otherwise_steps) = otherwise {
                    let sub_pairs = resolve_steps(
                        otherwise_steps,
                        producer_ctx,
                        Arc::clone(&rt),
                        registry,
                        languages,
                        beans,
                        function_invoker.clone(),
                        Arc::clone(&component_ctx),
                        route_id,
                        staging_mode,
                    )?;
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
                    Some(compose_pipeline(sub_processors))
                } else {
                    None
                };
                let svc = ChoiceService::new(when_clauses, otherwise_pipeline);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::WireTap { uri } => {
                let producer = resolve_producer(&uri)?;
                let svc = camel_processor::WireTapService::new(producer);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::Enrich { uri, strategy, .. } => {
                let producer = resolve_producer(&uri)?;
                let strategy_arc = resolve_enrichment_strategy(strategy)?;
                let svc = EnrichService::new(producer, strategy_arc);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::PollEnrich {
                uri,
                strategy,
                timeout_ms,
            } => {
                let parsed = parse_uri(&uri)?;
                let component = component_ctx
                    .resolve_component(&parsed.scheme)
                    .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
                let endpoint = component.create_endpoint(&uri, component_ctx.as_ref())?;
                let poller = endpoint.polling_consumer()
                    .ok_or_else(|| CamelError::EndpointCreationFailed(
                        format!("pollEnrich requires an endpoint that exposes a PollingConsumer; `{}` does not", uri)
                    ))?;
                let timeout = Duration::from_millis(timeout_ms.unwrap_or(5000));
                let strategy_arc = resolve_enrichment_strategy(strategy)?;
                let svc = PollEnrichService::new(poller, timeout, strategy_arc);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::Multicast { config, steps } => {
                // Each top-level step in the multicast scope becomes an independent endpoint.
                let mut endpoints = Vec::new();
                for step in steps {
                    let sub_pairs = resolve_steps(
                        vec![step],
                        producer_ctx,
                        Arc::clone(&rt),
                        registry,
                        languages,
                        beans,
                        function_invoker.clone(),
                        Arc::clone(&component_ctx),
                        route_id,
                        staging_mode,
                    )?;
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
                    let endpoint = compose_pipeline(sub_processors);
                    endpoints.push(endpoint);
                }
                let svc = camel_processor::MulticastService::new(endpoints, config)?;
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::DeclarativeLog { level, message } => {
                let ValueSourceDef::Expression(expression) = message else {
                    // Literal case is already converted to a Processor in compile.rs;
                    // this arm should never be reached for literals.
                    unreachable!(
                        "DeclarativeLog with Literal should have been compiled to a Processor"
                    );
                };
                let expression = compile_language_expression(languages, &expression)?;
                let svc =
                    camel_processor::log::DynamicLog::new(level, move |exchange: &Exchange| {
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::try_current()
                                .expect("DynamicLog expression: must be called from within a tokio runtime") // allow-unwrap
                                .block_on(expression.evaluate(exchange))
                        })
                        .unwrap_or_else(|e| {
                            warn!(error = %e, "log expression evaluation failed");
                            Value::Null
                        })
                        .to_string()
                    });
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::Bean { name, method } => {
                // Lock beans registry to lookup bean
                let beans = beans.lock().expect(
                    // allow-unwrap
                    "beans mutex poisoned: another thread panicked while holding this lock",
                );

                // Lookup bean by name
                let bean = beans.get(&name).ok_or_else(|| {
                    CamelError::ProcessorError(format!("Bean not found: {}", name))
                })?;

                // Clone Arc for async closure (release lock before async)
                let bean_clone = Arc::clone(&bean);
                let method = method.clone();

                // Create processor that invokes bean method
                let processor = tower::service_fn(move |mut exchange: Exchange| {
                    let bean = Arc::clone(&bean_clone);
                    let method = method.clone();

                    async move {
                        bean.call(&method, &mut exchange).await?;
                        Ok(exchange)
                    }
                });

                processors.push((BoxProcessor::new(processor), None));
            }
            BuilderStep::Script { language, script } => {
                let lang = resolve_language(languages, &language)?;
                match lang.create_mutating_expression(&script) {
                    Ok(mut_expr) => {
                        processors.push((BoxProcessor::new(ScriptMutator::new(mut_expr)), None));
                    }
                    Err(LanguageError::NotSupported {
                        feature,
                        language: ref lang_name,
                    }) => {
                        // Hard error: the .script() DSL step explicitly requests mutation semantics.
                        // If the language doesn't support MutatingExpression, the route is mis-configured.
                        return Err(CamelError::RouteError(format!(
                            "Language '{}' does not support {} (required for .script() step)",
                            lang_name, feature
                        )));
                    }
                    Err(e) => {
                        return Err(CamelError::RouteError(format!(
                            "Failed to create mutating expression for language '{}': {}",
                            language, e
                        )));
                    }
                }
            }
            BuilderStep::Throttle { config, steps } => {
                let sub_pairs = resolve_steps(
                    steps,
                    producer_ctx,
                    Arc::clone(&rt),
                    registry,
                    languages,
                    beans,
                    function_invoker.clone(),
                    Arc::clone(&component_ctx),
                    route_id,
                    staging_mode,
                )?;
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::throttler::ThrottlerService::new(config, sub_pipeline);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::LoadBalance { config, steps } => {
                // Each top-level step in the load_balance scope becomes an independent endpoint.
                let mut endpoints = Vec::new();
                for step in steps {
                    let sub_pairs = resolve_steps(
                        vec![step],
                        producer_ctx,
                        Arc::clone(&rt),
                        registry,
                        languages,
                        beans,
                        function_invoker.clone(),
                        Arc::clone(&component_ctx),
                        route_id,
                        staging_mode,
                    )?;
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
                    let endpoint = compose_pipeline(sub_processors);
                    endpoints.push(endpoint);
                }
                let svc =
                    camel_processor::load_balancer::LoadBalancerService::new(endpoints, config);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::DynamicRouter { config } => {
                use camel_api::EndpointResolver;

                let producer_ctx_clone = producer_ctx.clone();
                let component_ctx_clone = Arc::clone(&component_ctx);
                let rt_clone = Arc::clone(&rt);
                let resolver: EndpointResolver = Arc::new(move |uri: &str| {
                    let parsed = match parse_uri(uri) {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    let component = match component_ctx_clone.resolve_component(&parsed.scheme) {
                        Some(c) => c,
                        None => return None,
                    };
                    let endpoint =
                        match component.create_endpoint(uri, component_ctx_clone.as_ref()) {
                            Ok(e) => e,
                            Err(_) => return None,
                        };
                    let producer = match endpoint
                        .create_producer(Arc::clone(&rt_clone), &producer_ctx_clone)
                    {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    Some(BoxProcessor::new(producer))
                });
                let svc =
                    camel_processor::dynamic_router::DynamicRouterService::new(config, resolver);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::DeclarativeDynamicRouter {
                expression,
                uri_delimiter,
                cache_size,
                ignore_invalid_endpoints,
                max_iterations,
            } => {
                use camel_api::EndpointResolver;

                let expression = compile_language_expression(languages, &expression)?;
                let expression: camel_api::RouterExpression =
                    Arc::new(move |exchange: &Exchange| {
                        let value = await_eval(&expression, exchange);
                        match value {
                            Value::Null => None,
                            Value::String(s) => Some(s),
                            other => Some(other.to_string()),
                        }
                    });

                // Note: timeout defaults to 60s (DynamicRouterConfig::new default).
                // Apache Camel does not expose a timeout option on dynamicRouter —
                // our timeout is a rust-camel extension.
                let config = camel_api::DynamicRouterConfig::new(expression)
                    .uri_delimiter(uri_delimiter)
                    .cache_size(cache_size)
                    .ignore_invalid_endpoints(ignore_invalid_endpoints)
                    .max_iterations(max_iterations);

                let producer_ctx_clone = producer_ctx.clone();
                let component_ctx_clone = Arc::clone(&component_ctx);
                let rt_clone = Arc::clone(&rt);
                let resolver: EndpointResolver = Arc::new(move |uri: &str| {
                    let parsed = match parse_uri(uri) {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    let component = match component_ctx_clone.resolve_component(&parsed.scheme) {
                        Some(c) => c,
                        None => return None,
                    };
                    let endpoint =
                        match component.create_endpoint(uri, component_ctx_clone.as_ref()) {
                            Ok(e) => e,
                            Err(_) => return None,
                        };
                    let producer = match endpoint
                        .create_producer(Arc::clone(&rt_clone), &producer_ctx_clone)
                    {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    Some(BoxProcessor::new(producer))
                });
                let svc =
                    camel_processor::dynamic_router::DynamicRouterService::new(config, resolver);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::RoutingSlip { config } => {
                use camel_api::EndpointResolver;

                let producer_ctx_clone = producer_ctx.clone();
                let component_ctx_clone = Arc::clone(&component_ctx);
                let rt_clone = Arc::clone(&rt);
                let resolver: EndpointResolver = Arc::new(move |uri: &str| {
                    let parsed = match parse_uri(uri) {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    let component = match component_ctx_clone.resolve_component(&parsed.scheme) {
                        Some(c) => c,
                        None => return None,
                    };
                    let endpoint =
                        match component.create_endpoint(uri, component_ctx_clone.as_ref()) {
                            Ok(e) => e,
                            Err(_) => return None,
                        };
                    let producer = match endpoint
                        .create_producer(Arc::clone(&rt_clone), &producer_ctx_clone)
                    {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    Some(BoxProcessor::new(producer))
                });

                let svc = camel_processor::routing_slip::RoutingSlipService::new(config, resolver);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::DeclarativeRoutingSlip {
                expression,
                uri_delimiter,
                cache_size,
                ignore_invalid_endpoints,
            } => {
                use camel_api::EndpointResolver;

                let expression = compile_language_expression(languages, &expression)?;
                let expression: camel_api::RoutingSlipExpression =
                    Arc::new(move |exchange: &Exchange| {
                        let value = await_eval(&expression, exchange);
                        match value {
                            Value::Null => None,
                            Value::String(s) => Some(s),
                            other => Some(other.to_string()),
                        }
                    });

                let config = camel_api::RoutingSlipConfig::new(expression)
                    .uri_delimiter(uri_delimiter)
                    .cache_size(cache_size)
                    .ignore_invalid_endpoints(ignore_invalid_endpoints);

                let producer_ctx_clone = producer_ctx.clone();
                let component_ctx_clone = Arc::clone(&component_ctx);
                let rt_clone = Arc::clone(&rt);
                let resolver: EndpointResolver = Arc::new(move |uri: &str| {
                    let parsed = match parse_uri(uri) {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    let component = match component_ctx_clone.resolve_component(&parsed.scheme) {
                        Some(c) => c,
                        None => return None,
                    };
                    let endpoint =
                        match component.create_endpoint(uri, component_ctx_clone.as_ref()) {
                            Ok(e) => e,
                            Err(_) => return None,
                        };
                    let producer = match endpoint
                        .create_producer(Arc::clone(&rt_clone), &producer_ctx_clone)
                    {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    Some(BoxProcessor::new(producer))
                });

                let svc = camel_processor::routing_slip::RoutingSlipService::new(config, resolver);
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::RecipientList { config } => {
                use camel_api::EndpointResolver;

                let producer_ctx_clone = producer_ctx.clone();
                let component_ctx_clone = Arc::clone(&component_ctx);
                let rt_clone = Arc::clone(&rt);
                let resolver: EndpointResolver = Arc::new(move |uri: &str| {
                    let parsed = match parse_uri(uri) {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    let component = match component_ctx_clone.resolve_component(&parsed.scheme) {
                        Some(c) => c,
                        None => return None,
                    };
                    let endpoint =
                        match component.create_endpoint(uri, component_ctx_clone.as_ref()) {
                            Ok(e) => e,
                            Err(_) => return None,
                        };
                    let producer = match endpoint
                        .create_producer(Arc::clone(&rt_clone), &producer_ctx_clone)
                    {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    Some(BoxProcessor::new(producer))
                });

                let svc =
                    camel_processor::recipient_list::RecipientListService::new(config, resolver)?;
                processors.push((BoxProcessor::new(svc), None));
            }
            BuilderStep::DeclarativeRecipientList {
                expression,
                delimiter,
                parallel,
                parallel_limit,
                stop_on_exception,
                aggregation,
            } => {
                use camel_api::EndpointResolver;

                let expression = compile_language_expression(languages, &expression)?;
                let expression: camel_api::recipient_list::RecipientListExpression =
                    Arc::new(move |exchange: &Exchange| {
                        let value = await_eval(&expression, exchange);
                        match value {
                            Value::Null => String::new(),
                            Value::String(s) => s,
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

                let producer_ctx_clone = producer_ctx.clone();
                let component_ctx_clone = Arc::clone(&component_ctx);
                let rt_clone = Arc::clone(&rt);
                let resolver: EndpointResolver = Arc::new(move |uri: &str| {
                    let parsed = match parse_uri(uri) {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    let component = match component_ctx_clone.resolve_component(&parsed.scheme) {
                        Some(c) => c,
                        None => return None,
                    };
                    let endpoint =
                        match component.create_endpoint(uri, component_ctx_clone.as_ref()) {
                            Ok(e) => e,
                            Err(_) => return None,
                        };
                    let producer = match endpoint
                        .create_producer(Arc::clone(&rt_clone), &producer_ctx_clone)
                    {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    Some(BoxProcessor::new(producer))
                });

                let _ = aggregation; // aggregation strategy name — reserved for future use
                let svc =
                    camel_processor::recipient_list::RecipientListService::new(config, resolver)?;
                processors.push((BoxProcessor::new(svc), None));
            }
        }
    }
    Ok(processors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::application::route_definition::LanguageExpressionDef;
    use crate::shared::components::domain::Registry;

    /// A mock endpoint that returns `None` for polling_consumer (default).
    struct MockEndpoint {
        uri: String,
    }

    impl camel_component_api::endpoint::Endpoint for MockEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }
        fn create_consumer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
        ) -> Result<Box<dyn camel_component_api::consumer::Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed(
                "mock not a consumer".into(),
            ))
        }
        fn create_producer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
            _ctx: &camel_component_api::ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Err(CamelError::ProcessorError("mock not a producer".into()))
        }
    }

    /// A mock component that vends MockEndpoint.
    struct MockComponent;

    #[async_trait::async_trait]
    impl camel_component_api::Component for MockComponent {
        fn scheme(&self) -> &str {
            "mock"
        }
        fn create_endpoint(
            &self,
            uri: &str,
            _ctx: &dyn camel_component_api::ComponentContext,
        ) -> Result<Box<dyn camel_component_api::endpoint::Endpoint>, CamelError> {
            Ok(Box::new(MockEndpoint {
                uri: uri.to_string(),
            }))
        }
    }

    /// Minimal ComponentContext that resolves exactly one scheme to a mock component.
    struct TestComponentContext;

    impl camel_component_api::ComponentContext for TestComponentContext {
        fn resolve_component(
            &self,
            scheme: &str,
        ) -> Option<Arc<dyn camel_component_api::Component>> {
            if scheme == "mock" {
                Some(Arc::new(MockComponent))
            } else {
                None
            }
        }
        fn resolve_language(&self, _name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
            None
        }
        fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
            Arc::new(camel_api::NoOpMetrics)
        }
        fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
            Arc::new(camel_api::NoopPlatformService::default())
        }
        fn register_route_health_check(
            &self,
            _route_id: &str,
            _check: Arc<dyn camel_api::AsyncHealthCheck>,
        ) {
        }
        fn unregister_route_health_check(&self, _route_id: &str) {}
    }

    async fn languages_with_simple() -> SharedLanguageRegistry {
        let mut map: std::collections::HashMap<String, Arc<dyn Language>> =
            std::collections::HashMap::new();
        map.insert(
            "simple".to_string(),
            Arc::new(camel_language_simple::SimpleLanguage::new()),
        );
        Arc::new(std::sync::Mutex::new(map))
    }

    #[tokio::test]
    async fn resolve_language_returns_error_for_unregistered_name() {
        let languages = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let err = match resolve_language(&languages, "missing") {
            Ok(_) => panic!("resolve_language should fail for unregistered language"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("missing"));
    }

    #[tokio::test]
    async fn compile_language_expression_and_predicate_work_for_simple_language() {
        let languages = languages_with_simple().await;
        let expression = LanguageExpressionDef {
            language: "simple".into(),
            source: "${header.answer}".into(),
        };
        let predicate_expression = LanguageExpressionDef {
            language: "simple".into(),
            source: "${header.answer} == '42'".into(),
        };

        let compiled_expression = compile_language_expression(&languages, &expression).unwrap();
        let compiled_predicate =
            compile_language_predicate(&languages, &predicate_expression).unwrap();

        let mut msg = camel_api::message::Message::default();
        msg.set_header("answer", Value::String("42".into()));
        let exchange = Exchange::new(msg);

        assert_eq!(
            compiled_expression.evaluate(&exchange).await.unwrap(),
            Value::String("42".into())
        );
        assert!(compiled_predicate.matches(&exchange).await.unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compile_filter_predicate_returns_boolean_result() {
        let languages = languages_with_simple().await;
        let expression = LanguageExpressionDef {
            language: "simple".into(),
            source: "${header.flag} == 'yes'".into(),
        };
        let predicate = compile_filter_predicate(&languages, &expression).unwrap();

        let mut msg = camel_api::message::Message::default();
        msg.set_header("flag", Value::String("yes".into()));
        let exchange = Exchange::new(msg);
        assert!(predicate(&exchange));
    }

    #[tokio::test]
    async fn value_to_body_covers_null_string_and_json() {
        assert!(matches!(value_to_body(Value::Null), Body::Empty));
        assert!(matches!(
            value_to_body(Value::String("x".into())),
            Body::Text(ref s) if s == "x"
        ));
        assert!(matches!(
            value_to_body(Value::Number(serde_json::Number::from(7))),
            Body::Json(Value::Number(_))
        ));
    }

    #[tokio::test]
    async fn resolve_steps_validates_declarative_loop_shape() {
        let languages = languages_with_simple().await;
        let producer_ctx = ProducerContext::new();
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let beans = Arc::new(std::sync::Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> =
            Arc::new(camel_component_api::NoOpComponentContext);
        let rt: Arc<dyn RuntimeObservability> = Arc::new(camel_component_api::NoOpComponentContext);

        let both = resolve_steps(
            vec![BuilderStep::DeclarativeLoop {
                count: Some(2),
                while_predicate: Some(LanguageExpressionDef {
                    language: "simple".into(),
                    source: "${header.k} == 'v'".into(),
                }),
                steps: vec![],
            }],
            &producer_ctx,
            Arc::clone(&rt),
            &registry,
            &languages,
            &beans,
            None,
            Arc::clone(&component_ctx),
            Some("r1"),
            &FunctionStagingMode::DirectAdd,
        )
        .unwrap_err();
        assert!(
            both.to_string()
                .contains("cannot specify both 'count' and 'while'")
        );

        let neither = resolve_steps(
            vec![BuilderStep::DeclarativeLoop {
                count: None,
                while_predicate: None,
                steps: vec![],
            }],
            &producer_ctx,
            Arc::clone(&rt),
            &registry,
            &languages,
            &beans,
            None,
            component_ctx,
            Some("r1"),
            &FunctionStagingMode::DirectAdd,
        )
        .unwrap_err();
        assert!(
            neither
                .to_string()
                .contains("must specify either 'count' or 'while'")
        );
    }

    #[tokio::test]
    async fn resolve_steps_returns_component_not_found_for_to_step() {
        let languages = languages_with_simple().await;
        let producer_ctx = ProducerContext::new();
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let beans = Arc::new(std::sync::Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> =
            Arc::new(camel_component_api::NoOpComponentContext);
        let rt: Arc<dyn RuntimeObservability> = Arc::new(camel_component_api::NoOpComponentContext);

        let err = resolve_steps(
            vec![BuilderStep::To("unknown:dest".into())],
            &producer_ctx,
            Arc::clone(&rt),
            &registry,
            &languages,
            &beans,
            None,
            component_ctx,
            Some("r1"),
            &FunctionStagingMode::DirectAdd,
        )
        .unwrap_err();

        assert!(err.to_string().contains("unknown"));
    }

    #[tokio::test]
    async fn compile_language_expression_and_predicate_propagate_compile_errors() {
        let languages = languages_with_simple().await;
        let bad_expr = LanguageExpressionDef {
            language: "simple".into(),
            source: "${header.a".into(),
        };
        let bad_pred = LanguageExpressionDef {
            language: "simple".into(),
            source: "${header.a == 'x'".into(),
        };

        let expr_err = match compile_language_expression(&languages, &bad_expr) {
            Ok(_) => panic!("expression compile should fail"),
            Err(err) => err,
        };
        assert!(
            expr_err
                .to_string()
                .contains("failed to compile simple expression")
        );

        let pred_err = match compile_language_predicate(&languages, &bad_pred) {
            Ok(_) => panic!("predicate compile should fail"),
            Err(err) => err,
        };
        assert!(
            pred_err
                .to_string()
                .contains("failed to compile simple predicate")
        );
    }

    #[tokio::test]
    async fn resolve_steps_covers_non_endpoint_variants() {
        use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_lines};
        use std::time::Duration;

        let expr = |source: &str| LanguageExpressionDef {
            language: "simple".into(),
            source: source.into(),
        };

        let languages = languages_with_simple().await;
        let producer_ctx = ProducerContext::new();
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let beans = Arc::new(std::sync::Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> =
            Arc::new(camel_component_api::NoOpComponentContext);
        let rt: Arc<dyn RuntimeObservability> = Arc::new(camel_component_api::NoOpComponentContext);

        let steps = vec![
            BuilderStep::Processor(BoxProcessor::new(IdentityProcessor)),
            BuilderStep::Stop,
            BuilderStep::Delay {
                config: camel_api::DelayConfig::new(1),
            },
            BuilderStep::Loop {
                config: camel_api::loop_eip::LoopConfig::new(camel_api::loop_eip::LoopMode::Count(
                    1,
                )),
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::DeclarativeLoop {
                count: Some(1),
                while_predicate: None,
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::Log {
                level: camel_processor::LogLevel::Info,
                message: "log".into(),
            },
            BuilderStep::DeclarativeSetHeader {
                key: "k".into(),
                value: ValueSourceDef::Literal(Value::String("v".into())),
            },
            BuilderStep::DeclarativeSetProperty {
                key: "p".into(),
                value_source: ValueSourceDef::Expression(expr("${header.k}")),
            },
            BuilderStep::DeclarativeSetBody {
                value: ValueSourceDef::Expression(expr("${header.k}")),
            },
            BuilderStep::DeclarativeFilter {
                predicate: expr("${header.k} == 'v'"),
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::DeclarativeChoice {
                whens: vec![
                    crate::lifecycle::application::route_definition::DeclarativeWhenStep {
                        predicate: expr("${header.k} == 'v'"),
                        steps: vec![BuilderStep::Stop],
                    },
                ],
                otherwise: Some(vec![BuilderStep::Stop]),
            },
            BuilderStep::DeclarativeScript {
                expression: expr("${header.k}"),
            },
            BuilderStep::Split {
                config: SplitterConfig::new(split_body_lines())
                    .aggregation(AggregationStrategy::CollectAll),
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::DeclarativeSplit {
                expression: expr("${body}"),
                aggregation: AggregationStrategy::Original,
                parallel: false,
                parallel_limit: Some(2),
                stop_on_exception: true,
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::Aggregate {
                config: camel_api::AggregatorConfig::correlate_by("id")
                    .complete_when_size(1)
                    .build()
                    .unwrap(),
            },
            BuilderStep::Filter {
                predicate: Arc::new(|_| true),
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::Choice {
                whens: vec![crate::lifecycle::application::route_definition::WhenStep {
                    predicate: Arc::new(|_| true),
                    steps: vec![BuilderStep::Stop],
                }],
                otherwise: Some(vec![BuilderStep::Stop]),
            },
            BuilderStep::Multicast {
                steps: vec![BuilderStep::Stop, BuilderStep::Stop],
                config: camel_api::MulticastConfig::new(),
            },
            BuilderStep::DeclarativeLog {
                level: camel_processor::LogLevel::Info,
                message: ValueSourceDef::Expression(expr("${header.k}")),
            },
            BuilderStep::Throttle {
                config: camel_api::ThrottlerConfig::new(10, Duration::from_millis(10)),
                steps: vec![BuilderStep::Stop],
            },
            BuilderStep::LoadBalance {
                config: camel_api::LoadBalancerConfig::round_robin(),
                steps: vec![BuilderStep::Stop, BuilderStep::Stop],
            },
            BuilderStep::DynamicRouter {
                config: camel_api::DynamicRouterConfig::new(Arc::new(|_| None)),
            },
            BuilderStep::DeclarativeDynamicRouter {
                expression: expr("${header.routes}"),
                uri_delimiter: ",".into(),
                cache_size: 8,
                ignore_invalid_endpoints: true,
                max_iterations: 3,
            },
            BuilderStep::RoutingSlip {
                config: camel_api::RoutingSlipConfig::new(Arc::new(|_| None)),
            },
            BuilderStep::DeclarativeRoutingSlip {
                expression: expr("${header.routes}"),
                uri_delimiter: ";".into(),
                cache_size: 16,
                ignore_invalid_endpoints: true,
            },
            BuilderStep::RecipientList {
                config: camel_api::recipient_list::RecipientListConfig::new(Arc::new(|_| {
                    String::new()
                })),
            },
            BuilderStep::DeclarativeRecipientList {
                expression: expr("${header.routes}"),
                delimiter: ",".into(),
                parallel: true,
                parallel_limit: Some(2),
                stop_on_exception: false,
                aggregation: "noop".into(),
            },
        ];

        let resolved = resolve_steps(
            steps,
            &producer_ctx,
            Arc::clone(&rt),
            &registry,
            &languages,
            &beans,
            None,
            component_ctx,
            Some("r1"),
            &FunctionStagingMode::DirectAdd,
        )
        .unwrap();

        assert!(!resolved.is_empty());
    }

    #[tokio::test]
    async fn poll_enrich_on_non_pollable_endpoint_returns_compile_error() {
        let languages = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let producer_ctx = ProducerContext::new();
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let beans = Arc::new(std::sync::Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(TestComponentContext);
        let rt: Arc<dyn RuntimeObservability> = Arc::new(camel_component_api::NoOpComponentContext);

        let err = resolve_steps(
            vec![BuilderStep::PollEnrich {
                uri: "mock:data".into(),
                strategy: None,
                timeout_ms: None,
            }],
            &producer_ctx,
            Arc::clone(&rt),
            &registry,
            &languages,
            &beans,
            None,
            component_ctx,
            Some("r1"),
            &FunctionStagingMode::DirectAdd,
        )
        .unwrap_err();

        let err_msg = err.to_string();
        assert!(
            err_msg.contains("pollEnrich requires"),
            "expected error about PollingConsumer, got: {err_msg}"
        );
        assert!(
            err_msg.contains("exposes a PollingConsumer"),
            "expected error about missing PollingConsumer, got: {err_msg}"
        );
    }
}
