//! YAML route definition parser.

use std::path::Path;

use camel_api::{CamelError, CanonicalRouteSpec};
use camel_core::route::RouteDefinition;

use crate::compile::{compile_declarative_route, compile_declarative_route_to_canonical};
use crate::contract::{DeclarativeStepKind, assert_contract_coverage};
use crate::model::{
    AggregateStepDef, AggregateStrategyDef, BodyTypeDef, ChoiceStepDef, DeclarativeCircuitBreaker,
    DeclarativeConcurrency, DeclarativeErrorHandler, DeclarativeOnException,
    DeclarativeRedeliveryPolicy, DeclarativeRoute, DeclarativeStep, LanguageExpressionDef,
    LogLevelDef, LogStepDef, MulticastAggregationDef, MulticastStepDef, ScriptStepDef,
    SetBodyStepDef, SetHeaderStepDef, SplitAggregationDef, SplitExpressionDef, SplitStepDef,
    ToStepDef, ValueSourceDef, WhenStepDef, WireTapStepDef,
};
pub use crate::yaml_ast::{
    AggregateData, AggregateStep, ChoiceData, ChoiceStep, FilterStep, LogConfig, LogMessageData,
    LogMessageExpr, LogStep, MulticastData, MulticastStep, PredicateBlock, ScriptData, ScriptStep,
    SetBodyConfig, SetBodyData, SetBodyStep, SetHeaderData, SetHeaderStep, SplitData,
    SplitExpressionConfig, SplitExpressionYaml, SplitStep, StopStep, ToStep, WireTapStep,
    YamlRoute, YamlRoutes, YamlStep,
};

const YAML_IMPLEMENTED_MANDATORY_STEPS: [DeclarativeStepKind; 13] = [
    DeclarativeStepKind::To,
    DeclarativeStepKind::Log,
    DeclarativeStepKind::SetHeader,
    DeclarativeStepKind::SetBody,
    DeclarativeStepKind::Filter,
    DeclarativeStepKind::Choice,
    DeclarativeStepKind::Split,
    DeclarativeStepKind::Aggregate,
    DeclarativeStepKind::WireTap,
    DeclarativeStepKind::Multicast,
    DeclarativeStepKind::Stop,
    DeclarativeStepKind::Script,
    DeclarativeStepKind::ConvertBodyTo,
];

const _: () = assert_contract_coverage(&YAML_IMPLEMENTED_MANDATORY_STEPS);

pub fn parse_yaml_to_declarative(yaml: &str) -> Result<Vec<DeclarativeRoute>, CamelError> {
    let routes: YamlRoutes = serde_yaml::from_str(yaml)
        .map_err(|e| CamelError::RouteError(format!("YAML parse error: {e}")))?;

    routes
        .routes
        .into_iter()
        .map(yaml_route_to_declarative_route)
        .collect()
}

pub fn parse_yaml(yaml: &str) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_yaml_to_declarative(yaml)?
        .into_iter()
        .map(compile_declarative_route)
        .collect()
}

pub fn parse_yaml_to_canonical(yaml: &str) -> Result<Vec<CanonicalRouteSpec>, CamelError> {
    parse_yaml_to_declarative(yaml)?
        .into_iter()
        .map(compile_declarative_route_to_canonical)
        .collect()
}

fn yaml_route_to_declarative_route(route: YamlRoute) -> Result<DeclarativeRoute, CamelError> {
    if route.id.is_empty() {
        return Err(CamelError::RouteError(
            "route 'id' must not be empty".into(),
        ));
    }

    if route.sequential && route.concurrent.is_some() {
        return Err(CamelError::RouteError(
            "route cannot set both 'sequential' and 'concurrent'".into(),
        ));
    }

    let concurrency = if route.sequential {
        Some(DeclarativeConcurrency::Sequential)
    } else {
        route
            .concurrent
            .map(|max| DeclarativeConcurrency::Concurrent {
                max: if max == 0 { None } else { Some(max) },
            })
    };

    let error_handler = route.error_handler.map(|eh| DeclarativeErrorHandler {
        dead_letter_channel: eh.dead_letter_channel,
        retry: eh.retry.map(|retry| DeclarativeRedeliveryPolicy {
            max_attempts: retry.max_attempts,
            initial_delay_ms: retry.initial_delay_ms,
            multiplier: retry.multiplier,
            max_delay_ms: retry.max_delay_ms,
            jitter_factor: retry.jitter_factor,
            handled_by: retry.handled_by,
        }),
        on_exceptions: eh.on_exceptions.map(|clauses| {
            clauses
                .into_iter()
                .map(|clause| DeclarativeOnException {
                    kind: clause.kind,
                    message_contains: clause.message_contains,
                    retry: clause.retry.map(|retry| DeclarativeRedeliveryPolicy {
                        max_attempts: retry.max_attempts,
                        initial_delay_ms: retry.initial_delay_ms,
                        multiplier: retry.multiplier,
                        max_delay_ms: retry.max_delay_ms,
                        jitter_factor: retry.jitter_factor,
                        handled_by: retry.handled_by,
                    }),
                })
                .collect()
        }),
    });

    let circuit_breaker = route.circuit_breaker.map(|cb| DeclarativeCircuitBreaker {
        failure_threshold: cb.failure_threshold,
        open_duration_ms: cb.open_duration_ms,
    });

    let unit_of_work = if route.on_complete.is_some() || route.on_failure.is_some() {
        Some(camel_api::UnitOfWorkConfig {
            on_complete: route.on_complete,
            on_failure: route.on_failure,
        })
    } else {
        None
    };

    let steps = route
        .steps
        .into_iter()
        .map(yaml_step_to_declarative_step)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(DeclarativeRoute {
        from: route.from,
        route_id: route.id,
        auto_startup: route.auto_startup,
        startup_order: route.startup_order,
        concurrency,
        error_handler,
        circuit_breaker,
        unit_of_work,
        steps,
    })
}

fn yaml_step_to_declarative_step(step: YamlStep) -> Result<DeclarativeStep, CamelError> {
    match step {
        YamlStep::To(ToStep { to }) => Ok(DeclarativeStep::To(ToStepDef::new(to))),
        YamlStep::WireTap(WireTapStep { wire_tap }) => {
            Ok(DeclarativeStep::WireTap(WireTapStepDef { uri: wire_tap }))
        }
        YamlStep::Stop(StopStep { stop }) => {
            if stop {
                Ok(DeclarativeStep::Stop)
            } else {
                Err(CamelError::RouteError(
                    "'stop: false' is invalid; remove the step or use 'stop: true'".into(),
                ))
            }
        }
        YamlStep::Log(LogStep { log }) => {
            let (message_data, level) = match log {
                crate::yaml_ast::LogBody::Message(message) => (
                    crate::yaml_ast::LogMessageData::Literal(message),
                    LogLevelDef::Info,
                ),
                crate::yaml_ast::LogBody::Config(config) => {
                    let level = match config.level.as_deref().unwrap_or("info") {
                        "trace" => LogLevelDef::Trace,
                        "debug" => LogLevelDef::Debug,
                        "info" => LogLevelDef::Info,
                        "warn" => LogLevelDef::Warn,
                        "error" => LogLevelDef::Error,
                        other => {
                            return Err(CamelError::RouteError(format!(
                                "unsupported log level `{other}`"
                            )));
                        }
                    };
                    (config.message, level)
                }
            };
            let message = match message_data {
                // A bare string like `log: "Got ${body}"` is always evaluated as Simple Language,
                // matching Apache Camel behaviour: the message field "uses simple language".
                crate::yaml_ast::LogMessageData::Literal(s) => {
                    ValueSourceDef::Expression(LanguageExpressionDef {
                        language: "simple".to_string(),
                        source: s,
                    })
                }
                crate::yaml_ast::LogMessageData::Expr(expr) => parse_value_source(
                    expr.value.map(serde_json::Value::String),
                    expr.language,
                    expr.source,
                    expr.simple,
                    expr.rhai,
                    "log.message",
                )?,
            };
            Ok(DeclarativeStep::Log(LogStepDef { message, level }))
        }
        YamlStep::SetHeader(SetHeaderStep { set_header }) => {
            let value = parse_value_source(
                set_header.value,
                set_header.language,
                set_header.source,
                set_header.simple,
                set_header.rhai,
                "set_header",
            )?;
            Ok(DeclarativeStep::SetHeader(SetHeaderStepDef {
                key: set_header.key,
                value,
            }))
        }
        YamlStep::SetBody(SetBodyStep { set_body }) => {
            let value = match set_body {
                SetBodyData::Literal(value) => ValueSourceDef::Literal(value),
                SetBodyData::Config(SetBodyConfig {
                    value,
                    language,
                    source,
                    simple,
                    rhai,
                }) => parse_value_source(value, language, source, simple, rhai, "set_body")?,
            };
            Ok(DeclarativeStep::SetBody(SetBodyStepDef { value }))
        }
        YamlStep::Script(ScriptStep {
            script: ScriptData { language, source },
        }) => Ok(DeclarativeStep::Script(ScriptStepDef {
            expression: LanguageExpressionDef { language, source },
        })),
        YamlStep::Filter(FilterStep { filter }) => {
            let predicate = parse_predicate_block(&filter, "filter")?;
            let steps = filter
                .steps
                .into_iter()
                .map(yaml_step_to_declarative_step)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DeclarativeStep::Filter(crate::model::FilterStepDef {
                predicate,
                steps,
            }))
        }
        YamlStep::Choice(ChoiceStep {
            choice: ChoiceData { when, otherwise },
        }) => {
            let whens = when
                .into_iter()
                .map(|block| {
                    let predicate = parse_predicate_block(&block, "choice.when")?;
                    let steps = block
                        .steps
                        .into_iter()
                        .map(yaml_step_to_declarative_step)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(WhenStepDef { predicate, steps })
                })
                .collect::<Result<Vec<_>, CamelError>>()?;

            let otherwise = match otherwise {
                Some(steps) => Some(
                    steps
                        .into_iter()
                        .map(yaml_step_to_declarative_step)
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                None => None,
            };

            Ok(DeclarativeStep::Choice(ChoiceStepDef { whens, otherwise }))
        }
        YamlStep::Split(SplitStep { split }) => {
            let expression = match split.expression {
                None => SplitExpressionDef::BodyLines,
                Some(SplitExpressionYaml::Simple(s)) => match s.as_str() {
                    "body_lines" | "lines" => SplitExpressionDef::BodyLines,
                    "body_json_array" | "json_array" => SplitExpressionDef::BodyJsonArray,
                    other => {
                        return Err(CamelError::RouteError(format!(
                            "unsupported split.expression `{other}`"
                        )));
                    }
                },
                Some(SplitExpressionYaml::Config(SplitExpressionConfig {
                    language,
                    source,
                    simple,
                    rhai,
                })) => {
                    let expr = parse_language_expression(
                        language,
                        source,
                        simple,
                        rhai,
                        "split.expression",
                    )?;
                    SplitExpressionDef::Language(expr)
                }
            };

            let aggregation = match split.aggregation.as_str() {
                "last_wins" => SplitAggregationDef::LastWins,
                "collect_all" => SplitAggregationDef::CollectAll,
                "original" => SplitAggregationDef::Original,
                other => {
                    return Err(CamelError::RouteError(format!(
                        "unsupported split.aggregation `{other}`"
                    )));
                }
            };

            let steps = split
                .steps
                .into_iter()
                .map(yaml_step_to_declarative_step)
                .collect::<Result<Vec<_>, _>>()?;

            Ok(DeclarativeStep::Split(SplitStepDef {
                expression,
                aggregation,
                parallel: split.parallel,
                parallel_limit: split.parallel_limit,
                stop_on_exception: split.stop_on_exception,
                steps,
            }))
        }
        YamlStep::Aggregate(AggregateStep { aggregate }) => {
            let strategy = match aggregate.strategy.as_str() {
                "collect_all" => AggregateStrategyDef::CollectAll,
                other => {
                    return Err(CamelError::RouteError(format!(
                        "unsupported aggregate.strategy `{other}`"
                    )));
                }
            };

            let completion_predicate = aggregate
                .completion_predicate
                .map(|block| parse_predicate_block(&block, "aggregate.completion_predicate"))
                .transpose()?;

            Ok(DeclarativeStep::Aggregate(AggregateStepDef {
                header: aggregate.header,
                completion_size: aggregate.completion_size,
                completion_timeout_ms: aggregate.completion_timeout_ms,
                completion_predicate,
                strategy,
                max_buckets: aggregate.max_buckets,
                bucket_ttl_ms: aggregate.bucket_ttl_ms,
            }))
        }
        YamlStep::Multicast(MulticastStep { multicast }) => {
            let aggregation = match multicast.aggregation.as_str() {
                "last_wins" => MulticastAggregationDef::LastWins,
                "collect_all" => MulticastAggregationDef::CollectAll,
                "original" => MulticastAggregationDef::Original,
                other => {
                    return Err(CamelError::RouteError(format!(
                        "unsupported multicast.aggregation `{other}`"
                    )));
                }
            };

            let steps = multicast
                .steps
                .into_iter()
                .map(yaml_step_to_declarative_step)
                .collect::<Result<Vec<_>, _>>()?;

            Ok(DeclarativeStep::Multicast(MulticastStepDef {
                steps,
                parallel: multicast.parallel,
                parallel_limit: multicast.parallel_limit,
                stop_on_exception: multicast.stop_on_exception,
                timeout_ms: multicast.timeout_ms,
                aggregation,
            }))
        }
        YamlStep::ConvertBodyTo(step) => {
            let def = match step.convert_body_to.to_lowercase().as_str() {
                "text" => BodyTypeDef::Text,
                "json" => BodyTypeDef::Json,
                "bytes" => BodyTypeDef::Bytes,
                "xml" => BodyTypeDef::Xml,
                "empty" => BodyTypeDef::Empty,
                other => {
                    return Err(CamelError::RouteError(format!(
                        "unknown convert_body_to target: '{}'. Expected: text, json, bytes, xml, empty",
                        other
                    )));
                }
            };
            Ok(DeclarativeStep::ConvertBodyTo(def))
        }
    }
}

fn parse_predicate_block(
    block: &PredicateBlock,
    context: &str,
) -> Result<LanguageExpressionDef, CamelError> {
    let mut selected = Vec::new();

    if let (Some(language), Some(source)) = (block.language.as_ref(), block.source.as_ref()) {
        selected.push((language.clone(), source.clone()));
    } else if block.language.is_some() || block.source.is_some() {
        return Err(CamelError::RouteError(format!(
            "{context}: `language` and `source` must be set together"
        )));
    }

    if let Some(source) = block.simple.as_ref() {
        selected.push(("simple".to_string(), source.clone()));
    }
    if let Some(source) = block.rhai.as_ref() {
        selected.push(("rhai".to_string(), source.clone()));
    }

    if selected.len() != 1 {
        return Err(CamelError::RouteError(format!(
            "{context} must define exactly one predicate source: `language+source`, `simple`, or `rhai`"
        )));
    }

    let (language, source) = selected.remove(0);
    Ok(LanguageExpressionDef { language, source })
}

fn parse_value_source(
    literal: Option<serde_json::Value>,
    language: Option<String>,
    source: Option<String>,
    simple: Option<String>,
    rhai: Option<String>,
    context: &str,
) -> Result<ValueSourceDef, CamelError> {
    let mut count = 0;
    if literal.is_some() {
        count += 1;
    }
    if language.is_some() || source.is_some() {
        if language.is_some() && source.is_some() {
            count += 1;
        } else {
            return Err(CamelError::RouteError(format!(
                "{context}: `language` and `source` must be set together"
            )));
        }
    }
    if simple.is_some() {
        count += 1;
    }
    if rhai.is_some() {
        count += 1;
    }

    if count != 1 {
        return Err(CamelError::RouteError(format!(
            "{context} must define exactly one value source: `value`, `language+source`, `simple`, or `rhai`"
        )));
    }

    if let Some(value) = literal {
        return Ok(ValueSourceDef::Literal(value));
    }
    if let (Some(language), Some(source)) = (language, source) {
        return Ok(ValueSourceDef::Expression(LanguageExpressionDef {
            language,
            source,
        }));
    }
    if let Some(source) = simple {
        return Ok(ValueSourceDef::Expression(LanguageExpressionDef {
            language: "simple".to_string(),
            source,
        }));
    }
    if let Some(source) = rhai {
        return Ok(ValueSourceDef::Expression(LanguageExpressionDef {
            language: "rhai".to_string(),
            source,
        }));
    }

    Err(CamelError::RouteError(format!(
        "{context}: missing value source"
    )))
}

fn parse_language_expression(
    language: Option<String>,
    source: Option<String>,
    simple: Option<String>,
    rhai: Option<String>,
    context: &str,
) -> Result<LanguageExpressionDef, CamelError> {
    let mut count = 0;
    if language.is_some() || source.is_some() {
        if language.is_some() && source.is_some() {
            count += 1;
        } else {
            return Err(CamelError::RouteError(format!(
                "{context}: `language` and `source` must be set together"
            )));
        }
    }
    if simple.is_some() {
        count += 1;
    }
    if rhai.is_some() {
        count += 1;
    }

    if count != 1 {
        return Err(CamelError::RouteError(format!(
            "{context} must define exactly one language source: `language+source`, `simple`, or `rhai`"
        )));
    }

    if let (Some(language), Some(source)) = (language, source) {
        return Ok(LanguageExpressionDef { language, source });
    }
    if let Some(source) = simple {
        return Ok(LanguageExpressionDef {
            language: "simple".to_string(),
            source,
        });
    }
    if let Some(source) = rhai {
        return Ok(LanguageExpressionDef {
            language: "rhai".to_string(),
            source,
        });
    }

    Err(CamelError::RouteError(format!(
        "{context}: missing language source"
    )))
}

pub fn load_from_file(path: &Path) -> Result<Vec<RouteDefinition>, CamelError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| CamelError::Io(format!("Failed to read {}: {e}", path.display())))?;
    parse_yaml(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_yaml() {
        let yaml = r#"
routes:
  - id: "test-route"
    from: "timer:tick?period=1000"
    steps:
      - set_header:
          key: "source"
          value: "timer"
      - to: "log:info"
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "test-route");
        assert_eq!(defs[0].from_uri(), "timer:tick?period=1000");
    }

    #[test]
    fn test_parse_missing_id_fails() {
        let yaml = r#"
routes:
  - from: "timer:tick"
    steps:
      - to: "log:info"
"#;
        let result = parse_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_id_fails() {
        let yaml = r#"
routes:
  - id: ""
    from: "timer:tick"
"#;
        let result = parse_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_multiple_routes() {
        let yaml = r#"
routes:
  - id: "route-a"
    from: "timer:tick"
    steps:
      - to: "log:info"
  - id: "route-b"
    from: "timer:tock"
    auto_startup: false
    startup_order: 10
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[1].route_id(), "route-b");
    }

    #[test]
    fn test_parse_defaults() {
        let yaml = r#"
routes:
  - id: "default-route"
    from: "timer:tick"
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert!(defs[0].auto_startup());
        assert_eq!(defs[0].startup_order(), 1000);
    }

    #[test]
    fn test_parse_yaml_to_declarative_preserves_route_metadata() {
        let yaml = r#"
routes:
  - id: "declarative-route"
    from: "timer:tick"
    auto_startup: false
    startup_order: 7
    steps:
      - log: "hello"
      - to: "mock:out"
"#;

        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id, "declarative-route");
        assert_eq!(routes[0].from, "timer:tick");
        assert!(!routes[0].auto_startup);
        assert_eq!(routes[0].startup_order, 7);
        assert_eq!(routes[0].steps.len(), 2);
    }

    #[test]
    fn test_parse_yaml_to_declarative_parses_on_exceptions() {
        let yaml = r#"
routes:
  - id: "eh-route"
    from: "direct:start"
    error_handler:
      dead_letter_channel: "log:dlc"
      on_exceptions:
        - kind: "Io"
          retry:
            max_attempts: 3
            handled_by: "log:io"
"#;

        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let eh = routes[0]
            .error_handler
            .as_ref()
            .expect("error handler should be present");
        let clauses = eh
            .on_exceptions
            .as_ref()
            .expect("on_exceptions should be present");
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].kind.as_deref(), Some("Io"));
        assert!(clauses[0].message_contains.is_none());
        let retry = clauses[0].retry.as_ref().expect("retry should be present");
        assert_eq!(retry.max_attempts, 3);
        assert_eq!(retry.handled_by.as_deref(), Some("log:io"));
    }

    #[test]
    fn test_parse_yaml_to_canonical_supports_to_log_stop_subset() {
        let yaml = r#"
routes:
  - id: "canonical-v1"
    from: "direct:start"
    steps:
      - to: "mock:out"
      - log:
          message: "hello"
      - stop: true
"#;
        let routes = parse_yaml_to_canonical(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id, "canonical-v1");
        assert_eq!(routes[0].from, "direct:start");
        assert_eq!(routes[0].version, 1);
        assert_eq!(routes[0].steps.len(), 3);
    }

    #[test]
    fn test_parse_yaml_to_canonical_supports_advanced_declarative_steps() {
        let yaml = r#"
routes:
  - id: "canonical-v1-advanced"
    from: "direct:start"
    circuit_breaker:
      failure_threshold: 4
      open_duration_ms: 750
    steps:
      - filter:
          simple: "${header.kind} == 'A'"
          steps:
            - to: "mock:filtered"
      - choice:
          when:
            - simple: "${header.kind} == 'A'"
              steps:
                - to: "mock:a"
          otherwise:
            - to: "mock:other"
      - split:
          expression: body_lines
          aggregation: collect_all
          steps:
            - to: "mock:split"
      - aggregate:
          header: "orderId"
          completion_size: 2
      - wire_tap: "mock:tap"
      - script:
          language: "simple"
          source: "${body}"
"#;

        let routes = parse_yaml_to_canonical(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id, "canonical-v1-advanced");
        assert_eq!(routes[0].steps.len(), 6);
        let cb = routes[0]
            .circuit_breaker
            .as_ref()
            .expect("circuit breaker should be present");
        assert_eq!(cb.failure_threshold, 4);
        assert_eq!(cb.open_duration_ms, 750);
    }

    #[test]
    fn test_parse_yaml_to_canonical_rejects_unsupported_steps() {
        let yaml = r#"
routes:
  - id: "canonical-v1-unsupported"
    from: "direct:start"
    steps:
      - set_header:
          key: "k"
          value: "v"
"#;
        let err = parse_yaml_to_canonical(yaml).unwrap_err().to_string();
        assert!(
            err.contains("canonical v1 does not support step `set_header`"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains("out-of-scope"),
            "expected explicit canonical subset reason, got: {err}"
        );
    }

    #[test]
    fn test_parse_yaml_supports_all_declarative_step_kinds() {
        let yaml = r#"
routes:
  - id: "all-steps"
    from: "direct:start"
    sequential: true
    error_handler:
      dead_letter_channel: "log:dlc"
      retry:
        max_attempts: 2
        handled_by: "log:handled"
    circuit_breaker:
      failure_threshold: 3
      open_duration_ms: 500
    steps:
      - log:
          message: "hello"
          level: "debug"
      - set_header:
          key: "kind"
          value: "A"
      - set_body:
          value: "payload"
      - filter:
          simple: "${header.kind} == 'A'"
          steps:
            - to: "mock:filtered"
      - choice:
          when:
            - simple: "${header.kind} == 'A'"
              steps:
                - to: "mock:a"
          otherwise:
            - to: "mock:other"
      - split:
          expression: body_lines
          aggregation: collect_all
          steps:
            - to: "mock:split"
      - aggregate:
          header: "orderId"
          completion_size: 2
      - wire_tap: "mock:tap"
      - multicast:
          steps:
            - to: "mock:left"
            - to: "mock:right"
      - script:
          language: "simple"
          source: "${body}"
      - stop: true
"#;

        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].steps.len(), 11);

        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "all-steps");
    }

    #[test]
    fn test_load_from_file() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join("test_routes.yaml");

        let yaml_content = r#"
routes:
  - id: "file-route"
    from: "timer:tick"
    steps:
      - to: "log:info"
"#;

        let mut file = std::fs::File::create(&file_path).unwrap();
        file.write_all(yaml_content.as_bytes()).unwrap();

        let defs = load_from_file(&file_path).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "file-route");

        std::fs::remove_file(&file_path).ok();
    }

    /// Verifies that `log:` with a Simple Language expression is parsed into a
    /// `DeclarativeStep::Log` that carries a `ValueSourceDef::Expression`, not a bare String.
    /// This test drives the requirement that `LogStepDef.message` becomes a `ValueSourceDef`.
    #[test]
    fn test_log_step_with_simple_expression_parses_as_expression() {
        let yaml = r#"
routes:
  - id: "log-expr"
    from: "timer:tick"
    steps:
      - log:
          message:
            simple: "${header.CamelTimerCounter} World"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        let step = &routes[0].steps[0];
        match step {
            DeclarativeStep::Log(def) => match &def.message {
                ValueSourceDef::Expression(expr) => {
                    assert_eq!(expr.language, "simple");
                    assert_eq!(expr.source, "${header.CamelTimerCounter} World");
                }
                ValueSourceDef::Literal(_) => {
                    panic!("expected Expression, got Literal")
                }
            },
            _ => panic!("expected Log step, got {:?}", step),
        }
    }

    #[test]
    fn test_load_from_nonexistent_file() {
        let result = load_from_file(Path::new("/nonexistent/path/routes.yaml"));
        assert!(result.is_err());
    }
}
