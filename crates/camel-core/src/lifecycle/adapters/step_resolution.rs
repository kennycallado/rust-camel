use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use camel_api::{
    BoxProcessor, CamelError, Exchange, FilterPredicate, IdentityProcessor, ProducerContext, Value,
    body::Body,
};
use camel_bean::BeanRegistry;
use camel_component_api::ComponentContext;
use camel_endpoint::parse_uri;
use camel_language_api::{Expression, Language, LanguageError, Predicate};
use camel_processor::script_mutator::ScriptMutator;
use camel_processor::{ChoiceService, WhenClause};

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
        .expect("mutex poisoned: another thread panicked while holding this lock");
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
        predicate.matches(exchange).unwrap_or(false)
    }))
}

fn value_to_body(value: Value) -> Body {
    match value {
        Value::Null => Body::Empty,
        Value::String(text) => Body::Text(text),
        other => Body::Json(other),
    }
}

#[allow(clippy::only_used_in_recursion)]
pub(crate) fn resolve_steps(
    steps: Vec<BuilderStep>,
    producer_ctx: &ProducerContext,
    registry: &Arc<std::sync::Mutex<Registry>>,
    languages: &SharedLanguageRegistry,
    beans: &Arc<std::sync::Mutex<BeanRegistry>>,
    component_ctx: Arc<dyn ComponentContext>,
) -> Result<Vec<(BoxProcessor, Option<camel_api::BodyType>)>, CamelError> {
    let resolve_producer = |uri: &str| -> Result<BoxProcessor, CamelError> {
        let parsed = parse_uri(uri)?;
        let component = component_ctx
            .resolve_component(&parsed.scheme)
            .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
        let endpoint = component.create_endpoint(uri, component_ctx.as_ref())?;
        endpoint.create_producer(producer_ctx)
    };

    let mut processors: Vec<(BoxProcessor, Option<camel_api::BodyType>)> = Vec::new();
    for step in steps {
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
                let producer = endpoint.create_producer(producer_ctx)?;
                processors.push((producer, contract));
            }
            BuilderStep::Stop => {
                processors.push((BoxProcessor::new(camel_processor::StopService), None));
            }
            BuilderStep::Delay { config } => {
                let svc = camel_processor::delayer::DelayerService::new(config);
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
                        move |exchange: &Exchange| {
                            expression.evaluate(exchange).unwrap_or(Value::Null)
                        },
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
                            let value = expression.evaluate(exchange).unwrap_or(Value::Null);
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
                    registry,
                    languages,
                    beans,
                    Arc::clone(&component_ctx),
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
                        registry,
                        languages,
                        beans,
                        Arc::clone(&component_ctx),
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
                        registry,
                        languages,
                        beans,
                        Arc::clone(&component_ctx),
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
                                let value = expression.evaluate(exchange).unwrap_or(Value::Null);
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
            BuilderStep::Split { config, steps } => {
                let sub_pairs = resolve_steps(
                    steps,
                    producer_ctx,
                    registry,
                    languages,
                    beans,
                    Arc::clone(&component_ctx),
                )?;
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let splitter =
                    camel_processor::splitter::SplitterService::new(config, sub_pipeline);
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
                    let value = lang_expr.evaluate(exchange).unwrap_or(Value::Null);
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
                    registry,
                    languages,
                    beans,
                    Arc::clone(&component_ctx),
                )?;
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let splitter =
                    camel_processor::splitter::SplitterService::new(config, sub_pipeline);
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
                    registry,
                    languages,
                    beans,
                    Arc::clone(&component_ctx),
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
                        registry,
                        languages,
                        beans,
                        Arc::clone(&component_ctx),
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
                        registry,
                        languages,
                        beans,
                        Arc::clone(&component_ctx),
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
            BuilderStep::Multicast { config, steps } => {
                // Each top-level step in the multicast scope becomes an independent endpoint.
                let mut endpoints = Vec::new();
                for step in steps {
                    let sub_pairs = resolve_steps(
                        vec![step],
                        producer_ctx,
                        registry,
                        languages,
                        beans,
                        Arc::clone(&component_ctx),
                    )?;
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
                    let endpoint = compose_pipeline(sub_processors);
                    endpoints.push(endpoint);
                }
                let svc = camel_processor::MulticastService::new(endpoints, config);
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
                        expression
                            .evaluate(exchange)
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
                    registry,
                    languages,
                    beans,
                    Arc::clone(&component_ctx),
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
                        registry,
                        languages,
                        beans,
                        Arc::clone(&component_ctx),
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
                    let producer = match endpoint.create_producer(&producer_ctx_clone) {
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
                        let value = expression.evaluate(exchange).unwrap_or(Value::Null);
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
                    let producer = match endpoint.create_producer(&producer_ctx_clone) {
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
                    let producer = match endpoint.create_producer(&producer_ctx_clone) {
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
                        let value = expression.evaluate(exchange).unwrap_or(Value::Null);
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
                    let producer = match endpoint.create_producer(&producer_ctx_clone) {
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
                    let producer = match endpoint.create_producer(&producer_ctx_clone) {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    Some(BoxProcessor::new(producer))
                });

                let svc =
                    camel_processor::recipient_list::RecipientListService::new(config, resolver);
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
                        let value = expression.evaluate(exchange).unwrap_or(Value::Null);
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
                    let producer = match endpoint.create_producer(&producer_ctx_clone) {
                        Ok(p) => p,
                        Err(_) => return None,
                    };
                    Some(BoxProcessor::new(producer))
                });

                let _ = aggregation; // aggregation strategy name — reserved for future use
                let svc =
                    camel_processor::recipient_list::RecipientListService::new(config, resolver);
                processors.push((BoxProcessor::new(svc), None));
            }
        }
    }
    Ok(processors)
}
