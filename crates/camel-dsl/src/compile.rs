use std::time::Duration;

use camel_api::aggregator::{AggregationStrategy as AggregatorStrategy, AggregatorConfig};
use camel_api::body_converter::BodyType;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::multicast::{MulticastConfig, MulticastStrategy};
use camel_api::splitter::{
    split_body_json_array, split_body_lines, AggregationStrategy as SplitAggregation,
    SplitterConfig,
};
use camel_api::{CamelError, CircuitBreakerConfig, IdentityProcessor};
use camel_component::ConcurrencyModel;
use camel_core::route::{BuilderStep, DeclarativeWhenStep, RouteDefinition};
use camel_processor::{ConvertBodyTo, LogLevel, StopService};

use crate::model::{
    AggregateStepDef, AggregateStrategyDef, BodyTypeDef, ChoiceStepDef, DeclarativeCircuitBreaker,
    DeclarativeConcurrency, DeclarativeErrorHandler, DeclarativeRoute, DeclarativeStep,
    LanguageExpressionDef, LogLevelDef, LogStepDef, MulticastAggregationDef, MulticastStepDef,
    ScriptStepDef, SetBodyStepDef, SetHeaderStepDef, SplitAggregationDef, SplitExpressionDef,
    SplitStepDef, ToStepDef, ValueSourceDef, WireTapStepDef,
};

pub fn compile_declarative_route(route: DeclarativeRoute) -> Result<RouteDefinition, CamelError> {
    let steps = compile_declarative_steps(route.steps)?;

    let mut definition = RouteDefinition::new(route.from, steps)
        .with_route_id(route.route_id)
        .with_auto_startup(route.auto_startup)
        .with_startup_order(route.startup_order);

    if let Some(concurrency) = route.concurrency {
        definition = match concurrency {
            DeclarativeConcurrency::Sequential => {
                definition.with_concurrency(ConcurrencyModel::Sequential)
            }
            DeclarativeConcurrency::Concurrent { max } => {
                definition.with_concurrency(ConcurrencyModel::Concurrent { max })
            }
        };
    }

    if let Some(error_handler) = route.error_handler {
        definition = definition.with_error_handler(compile_error_handler(error_handler)?);
    }

    if let Some(circuit_breaker) = route.circuit_breaker {
        definition = definition.with_circuit_breaker(compile_circuit_breaker(circuit_breaker));
    }

    Ok(definition)
}

fn compile_error_handler(def: DeclarativeErrorHandler) -> Result<ErrorHandlerConfig, CamelError> {
    let mut config = if let Some(uri) = def.dead_letter_channel {
        ErrorHandlerConfig::dead_letter_channel(uri)
    } else {
        ErrorHandlerConfig::log_only()
    };

    if let Some(retry) = def.retry {
        let mut builder = config.on_exception(|_e| true).retry(retry.max_attempts);
        builder = builder.with_backoff(
            Duration::from_millis(retry.initial_delay_ms),
            retry.multiplier,
            Duration::from_millis(retry.max_delay_ms),
        );
        if let Some(uri) = retry.handled_by {
            builder = builder.handled_by(uri);
        }
        config = builder.build();
    }

    Ok(config)
}

fn compile_circuit_breaker(def: DeclarativeCircuitBreaker) -> CircuitBreakerConfig {
    CircuitBreakerConfig::new()
        .failure_threshold(def.failure_threshold)
        .open_duration(Duration::from_millis(def.open_duration_ms))
}

fn compile_declarative_steps(steps: Vec<DeclarativeStep>) -> Result<Vec<BuilderStep>, CamelError> {
    steps.into_iter().map(compile_declarative_step).collect()
}

pub fn compile_declarative_step(step: DeclarativeStep) -> Result<BuilderStep, CamelError> {
    match step {
        DeclarativeStep::To(ToStepDef { uri }) => Ok(BuilderStep::To(uri)),
        DeclarativeStep::WireTap(WireTapStepDef { uri }) => Ok(BuilderStep::WireTap { uri }),
        DeclarativeStep::Log(LogStepDef { message, level }) => {
            let compiled_level = compile_log_level(level);
            match message {
                ValueSourceDef::Literal(v) => {
                    let s = match v {
                        serde_json::Value::String(s) => s,
                        other => other.to_string(),
                    };
                    Ok(BuilderStep::Processor(camel_api::BoxProcessor::new(
                        camel_processor::LogProcessor::new(compiled_level, s),
                    )))
                }
                ValueSourceDef::Expression(_) => Ok(BuilderStep::DeclarativeLog {
                    level: compiled_level,
                    message,
                }),
            }
        }
        DeclarativeStep::SetHeader(SetHeaderStepDef { key, value }) => {
            compile_set_header_step(key, value)
        }
        DeclarativeStep::SetBody(SetBodyStepDef { value }) => compile_set_body_step(value),
        DeclarativeStep::Script(ScriptStepDef { expression }) => {
            Ok(BuilderStep::DeclarativeScript { expression })
        }
        DeclarativeStep::Stop => Ok(BuilderStep::Processor(camel_api::BoxProcessor::new(
            StopService,
        ))),
        DeclarativeStep::Filter(def) => compile_filter_step(def.predicate, def.steps),
        DeclarativeStep::Choice(ChoiceStepDef { whens, otherwise }) => {
            let mut compiled_whens = Vec::with_capacity(whens.len());
            for when in whens {
                let predicate = when.predicate;
                let steps = compile_declarative_steps(when.steps)?;
                compiled_whens.push(DeclarativeWhenStep { predicate, steps });
            }

            let compiled_otherwise = match otherwise {
                Some(steps) => Some(compile_declarative_steps(steps)?),
                None => None,
            };

            Ok(BuilderStep::DeclarativeChoice {
                whens: compiled_whens,
                otherwise: compiled_otherwise,
            })
        }
        DeclarativeStep::Split(def) => compile_split_step(def),
        DeclarativeStep::Aggregate(def) => compile_aggregate_step(def),
        DeclarativeStep::Multicast(def) => compile_multicast_step(def),
        DeclarativeStep::ConvertBodyTo(def) => {
            let target = match def {
                BodyTypeDef::Text => BodyType::Text,
                BodyTypeDef::Json => BodyType::Json,
                BodyTypeDef::Bytes => BodyType::Bytes,
                BodyTypeDef::Empty => BodyType::Empty,
            };
            Ok(BuilderStep::Processor(camel_api::BoxProcessor::new(
                ConvertBodyTo::new(IdentityProcessor, target),
            )))
        }
    }
}

fn compile_split_step(def: SplitStepDef) -> Result<BuilderStep, CamelError> {
    let aggregation = match def.aggregation {
        SplitAggregationDef::LastWins => SplitAggregation::LastWins,
        SplitAggregationDef::CollectAll => SplitAggregation::CollectAll,
        SplitAggregationDef::Original => SplitAggregation::Original,
    };

    match def.expression {
        SplitExpressionDef::BodyLines => {
            let config = SplitterConfig::new(split_body_lines())
                .aggregation(aggregation)
                .parallel(def.parallel)
                .stop_on_exception(def.stop_on_exception);
            let config = if let Some(limit) = def.parallel_limit {
                config.parallel_limit(limit)
            } else {
                config
            };
            Ok(BuilderStep::Split {
                config,
                steps: compile_declarative_steps(def.steps)?,
            })
        }
        SplitExpressionDef::BodyJsonArray => {
            let config = SplitterConfig::new(split_body_json_array())
                .aggregation(aggregation)
                .parallel(def.parallel)
                .stop_on_exception(def.stop_on_exception);
            let config = if let Some(limit) = def.parallel_limit {
                config.parallel_limit(limit)
            } else {
                config
            };
            Ok(BuilderStep::Split {
                config,
                steps: compile_declarative_steps(def.steps)?,
            })
        }
        SplitExpressionDef::Language(expression) => Ok(BuilderStep::DeclarativeSplit {
            expression,
            aggregation,
            parallel: def.parallel,
            parallel_limit: def.parallel_limit,
            stop_on_exception: def.stop_on_exception,
            steps: compile_declarative_steps(def.steps)?,
        }),
    }
}

fn compile_aggregate_step(def: AggregateStepDef) -> Result<BuilderStep, CamelError> {
    let completion_size = def.completion_size.unwrap_or(1);

    if def.completion_timeout_ms.is_some() {
        return Err(CamelError::RouteError(
            "aggregate.completion_timeout_ms is not yet implemented".to_string(),
        ));
    }

    if def.completion_predicate.is_some() {
        return Err(CamelError::RouteError(
            "aggregate.completion_predicate is not yet implemented".to_string(),
        ));
    }

    let mut builder =
        AggregatorConfig::correlate_by(def.header).complete_when_size(completion_size);
    builder = match def.strategy {
        AggregateStrategyDef::CollectAll => builder.strategy(AggregatorStrategy::CollectAll),
    };
    if let Some(max_buckets) = def.max_buckets {
        builder = builder.max_buckets(max_buckets);
    }
    if let Some(ttl_ms) = def.bucket_ttl_ms {
        builder = builder.bucket_ttl(Duration::from_millis(ttl_ms));
    }

    Ok(BuilderStep::Aggregate {
        config: builder.build(),
    })
}

fn compile_multicast_step(def: MulticastStepDef) -> Result<BuilderStep, CamelError> {
    let aggregation = match def.aggregation {
        MulticastAggregationDef::LastWins => MulticastStrategy::LastWins,
        MulticastAggregationDef::CollectAll => MulticastStrategy::CollectAll,
        MulticastAggregationDef::Original => MulticastStrategy::Original,
    };

    let mut config = MulticastConfig::new()
        .parallel(def.parallel)
        .stop_on_exception(def.stop_on_exception)
        .aggregation(aggregation);
    if let Some(limit) = def.parallel_limit {
        config = config.parallel_limit(limit);
    }
    if let Some(timeout_ms) = def.timeout_ms {
        config = config.timeout(Duration::from_millis(timeout_ms));
    }

    Ok(BuilderStep::Multicast {
        steps: compile_declarative_steps(def.steps)?,
        config,
    })
}

fn compile_filter_step(
    predicate: LanguageExpressionDef,
    steps: Vec<DeclarativeStep>,
) -> Result<BuilderStep, CamelError> {
    Ok(BuilderStep::DeclarativeFilter {
        predicate,
        steps: compile_declarative_steps(steps)?,
    })
}

fn compile_set_header_step(key: String, value: ValueSourceDef) -> Result<BuilderStep, CamelError> {
    Ok(BuilderStep::DeclarativeSetHeader { key, value })
}

fn compile_set_body_step(value: ValueSourceDef) -> Result<BuilderStep, CamelError> {
    Ok(BuilderStep::DeclarativeSetBody { value })
}

fn compile_log_level(level: LogLevelDef) -> LogLevel {
    match level {
        LogLevelDef::Trace => LogLevel::Trace,
        LogLevelDef::Debug => LogLevel::Debug,
        LogLevelDef::Info => LogLevel::Info,
        LogLevelDef::Warn => LogLevel::Warn,
        LogLevelDef::Error => LogLevel::Error,
    }
}
