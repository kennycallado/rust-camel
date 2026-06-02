//! YAML route definition parser.

use std::path::Path;

use tracing::{debug, error, info};

use camel_api::{CamelError, CanonicalRouteSpec};
use camel_core::route::RouteDefinition;

use crate::compile::{
    compile_declarative_route, compile_declarative_route_to_canonical,
    compile_declarative_route_with_stream_cache_threshold,
};
use crate::contract::{DeclarativeStepKind, assert_contract_coverage};
use crate::model::{
    AggregateStepDef, AggregateStrategyDef, BeanStepDef, BodyTypeDef, ChoiceStepDef, DataFormatDef,
    DeclarativeCircuitBreaker, DeclarativeConcurrency, DeclarativeErrorHandler,
    DeclarativeOnException, DeclarativeRedeliveryPolicy, DeclarativeRoute,
    DeclarativeSecurityPolicy, DeclarativeStep, DelayStepDef, DynamicRouterStepDef,
    LanguageExpressionDef, LoadBalanceStepDef, LoadBalanceStrategyDef, LogLevelDef, LogStepDef,
    LoopStepDef, MulticastAggregationDef, MulticastStepDef, RecipientListStepDef,
    RoutingSlipStepDef, ScriptStepDef, SecurityCompileContext, SetBodyStepDef, SetHeaderStepDef,
    SetPropertyStepDef, SplitAggregationDef, SplitExpressionDef, SplitStepDef, StreamCacheStepDef,
    ThrottleStepDef, ThrottleStrategyDef, ToStepDef, ValueSourceDef, WhenStepDef, WireTapStepDef,
};
pub use crate::yaml_ast::{
    AggregateData, AggregateStep, BeanStep, BeanStepData, ChoiceData, ChoiceStep, DelayBody,
    DelayStep, DynamicRouterData, DynamicRouterStep, FilterStep, FunctionStep, LoadBalanceData,
    LoadBalanceStep, LogConfig, LogMessageData, LogMessageExpr, LogStep, MarshalStep,
    MulticastData, MulticastStep, PredicateBlock, RecipientListData, RecipientListStep,
    RoutingSlipData, RoutingSlipStep, ScriptData, ScriptStep, SetBodyConfig, SetBodyData,
    SetBodyStep, SetHeaderData, SetHeaderStep, SetPropertyData, SetPropertyStep, SplitData,
    SplitExpressionConfig, SplitExpressionYaml, SplitStep, StopStep, StreamCacheBody,
    StreamCacheConfig, StreamCacheStep, ThrottleData, ThrottleStep, ToStep, TransformStep,
    UnmarshalStep, ValidateStep, WireTapStep, YamlRoute, YamlRoutes, YamlStep,
};
use crate::yaml_ast::{LoopData, LoopStep, LoopWhileExpr};

const YAML_IMPLEMENTED_MANDATORY_STEPS: [DeclarativeStepKind; 24] = [
    DeclarativeStepKind::To,
    DeclarativeStepKind::Log,
    DeclarativeStepKind::SetHeader,
    DeclarativeStepKind::SetProperty,
    DeclarativeStepKind::SetBody,
    DeclarativeStepKind::Filter,
    DeclarativeStepKind::Function,
    DeclarativeStepKind::Choice,
    DeclarativeStepKind::Split,
    DeclarativeStepKind::Aggregate,
    DeclarativeStepKind::WireTap,
    DeclarativeStepKind::Multicast,
    DeclarativeStepKind::Stop,
    DeclarativeStepKind::Script,
    DeclarativeStepKind::StreamCache,
    DeclarativeStepKind::ConvertBodyTo,
    DeclarativeStepKind::Marshal,
    DeclarativeStepKind::Unmarshal,
    DeclarativeStepKind::Bean,
    DeclarativeStepKind::DynamicRouter,
    DeclarativeStepKind::LoadBalance,
    DeclarativeStepKind::RoutingSlip,
    DeclarativeStepKind::Throttle,
    DeclarativeStepKind::RecipientList,
];

const _: () = assert_contract_coverage(&YAML_IMPLEMENTED_MANDATORY_STEPS);

pub fn parse_yaml_to_declarative(yaml: &str) -> Result<Vec<DeclarativeRoute>, CamelError> {
    let routes: YamlRoutes = serde_yml::from_str(yaml).map_err(|e| {
        error!(error = %e, "yaml parse failed");
        CamelError::RouteError(format!("YAML parse error: {e}"))
    })?;
    debug!(route_count = %routes.routes.len(), "yaml routes parsed successfully");

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

pub fn parse_yaml_with_threshold(
    yaml: &str,
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_yaml_with_threshold_and_security(
        yaml,
        stream_cache_threshold,
        SecurityCompileContext::default(),
    )
}

pub fn parse_yaml_with_threshold_and_security(
    yaml: &str,
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_yaml_to_declarative(yaml)?
        .into_iter()
        .map(|route| {
            compile_declarative_route_with_stream_cache_threshold(
                route,
                stream_cache_threshold,
                security_ctx.clone(),
            )
        })
        .collect()
}

pub fn parse_yaml_to_canonical(yaml: &str) -> Result<Vec<CanonicalRouteSpec>, CamelError> {
    let routes = parse_yaml_to_declarative(yaml)?;
    for route in &routes {
        if route.security_policy.is_some() {
            return Err(CamelError::RouteError(
                "routes with security_policy cannot use the canonical/hot-reload path (not yet supported)".into(),
            ));
        }
    }
    routes
        .into_iter()
        .map(compile_declarative_route_to_canonical)
        .collect()
}

fn yaml_source_to_value_source(
    yaml: crate::yaml_ast::YamlPermissionValueSource,
) -> Result<camel_auth::PermissionValueSource, CamelError> {
    if let Some(s) = yaml.literal {
        Ok(camel_auth::PermissionValueSource::Literal(s))
    } else if let Some(h) = yaml.header {
        Ok(camel_auth::PermissionValueSource::Header(h))
    } else if let Some(p) = yaml.property {
        Ok(camel_auth::PermissionValueSource::Property(p))
    } else {
        Err(CamelError::RouteError(
            "security_policy permission resource/action must specify exactly one of: literal, header, or property".into(),
        ))
    }
}

pub(crate) fn yaml_route_to_declarative_route(
    route: YamlRoute,
) -> Result<DeclarativeRoute, CamelError> {
    if route.id.is_empty() {
        return Err(CamelError::RouteError(
            "route 'id' must not be empty".into(),
        ));
    }

    if route.sequential && route.concurrent.is_some() {
        return Err(CamelError::RouteError(format!(
            "route '{}': cannot set both 'sequential' and 'concurrent'",
            route.id
        )));
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

    let error_handler = route
        .error_handler
        .map(|eh| -> Result<DeclarativeErrorHandler, CamelError> {
            let on_exceptions = eh
                .on_exceptions
                .map(|clauses| {
                    clauses
                        .into_iter()
                        .map(|clause| {
                            let steps = clause
                                .steps
                                .into_iter()
                                .map(yaml_step_to_declarative_step)
                                .collect::<Result<Vec<_>, _>>()?;
                            Ok::<_, CamelError>(DeclarativeOnException {
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
                                steps,
                                handled: clause.handled,
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?;
            Ok(DeclarativeErrorHandler {
                dead_letter_channel: eh.dead_letter_channel,
                retry: eh.retry.map(|retry| DeclarativeRedeliveryPolicy {
                    max_attempts: retry.max_attempts,
                    initial_delay_ms: retry.initial_delay_ms,
                    multiplier: retry.multiplier,
                    max_delay_ms: retry.max_delay_ms,
                    jitter_factor: retry.jitter_factor,
                    handled_by: retry.handled_by,
                }),
                on_exceptions,
            })
        })
        .transpose()?;

    let circuit_breaker = route
        .circuit_breaker
        .map(|cb| {
            if cb.failure_threshold == 0 {
                return Err(CamelError::RouteError(
                    "circuit_breaker: failure_threshold must be > 0".into(),
                ));
            }
            Ok(DeclarativeCircuitBreaker {
                failure_threshold: cb.failure_threshold,
                open_duration_ms: cb.open_duration_ms,
            })
        })
        .transpose()?;

    let security_policy = route
        .security_policy
        .map(|sp| {
            let forms = [
                sp.roles.is_some(),
                sp.scopes.is_some(),
                sp.r#ref.is_some(),
                sp.wasm.is_some(),
                sp.permission.is_some(),
            ]
            .iter()
            .filter(|&&f| f)
            .count();
            if forms == 0 {
                return Err(CamelError::RouteError(
                    "security_policy must specify exactly one of: roles, scopes, ref, wasm, permission".into(),
                ));
            }
            if forms > 1 {
                return Err(CamelError::RouteError(
                    "security_policy must specify exactly one of: roles, scopes, ref, wasm, permission (multiple forms found)".into(),
                ));
            }
            if let Some(roles) = sp.roles {
                if roles.is_empty() {
                    return Err(CamelError::RouteError(
                        "security_policy roles must not be empty".into(),
                    ));
                }
                Ok(DeclarativeSecurityPolicy::Roles {
                    roles,
                    all_required: sp.all_required.unwrap_or(true),
                })
            } else if let Some(scopes) = sp.scopes {
                if scopes.is_empty() {
                    return Err(CamelError::RouteError(
                        "security_policy scopes must not be empty".into(),
                    ));
                }
                Ok(DeclarativeSecurityPolicy::Scopes {
                    scopes,
                    all_required: sp.all_required.unwrap_or(true),
                })
            } else if let Some(name) = sp.r#ref {
                if sp.all_required.is_some() {
                    return Err(CamelError::RouteError(
                        "security_policy all_required is not valid with ref form".into(),
                    ));
                }
                Ok(DeclarativeSecurityPolicy::Ref { name })
            } else if let Some(path) = sp.wasm {
                if sp.all_required.is_some() {
                    return Err(CamelError::RouteError(
                        "security_policy all_required is not valid with wasm form".into(),
                    ));
                }
                Ok(DeclarativeSecurityPolicy::Wasm {
                    path,
                    config: sp.config.unwrap_or_default(),
                })
            } else if let Some(perm) = sp.permission {
                if perm.policy.trim().is_empty() {
                    return Err(CamelError::RouteError(
                        "security_policy permission policy must not be empty".into(),
                    ));
                }
                if sp.all_required.is_some() {
                    return Err(CamelError::RouteError(
                        "security_policy all_required is not valid with permission form".into(),
                    ));
                }
                Ok(DeclarativeSecurityPolicy::Permission {
                    policy: perm.policy,
                    resource: perm.resource.map(yaml_source_to_value_source).transpose()?.unwrap_or(
                        camel_auth::PermissionValueSource::Header("x-resource".into()),
                    ),
                    action: perm.action.map(yaml_source_to_value_source).transpose()?.unwrap_or(
                        camel_auth::PermissionValueSource::Header("x-action".into()),
                    ),
                    scopes: perm.scopes.unwrap_or_default(),
                    context: perm
                        .context
                        .map(|ctx| camel_auth::PermissionContextConfig {
                            include_headers: ctx.headers,
                            include_properties: ctx.properties,
                        })
                        .unwrap_or_default(),
                    cache_ttl_secs: perm.cache_ttl_secs,
                    cache_negative_ttl_secs: perm.cache_negative_ttl_secs,
                })
            } else {
                unreachable!("validated forms count above")
            }
        })
        .transpose()?;

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
        security_policy,
        unit_of_work,
        steps,
    })
}

pub(crate) fn yaml_step_to_declarative_step(step: YamlStep) -> Result<DeclarativeStep, CamelError> {
    match step {
        YamlStep::To(ToStep { to }) => {
            if to.trim().is_empty() {
                return Err(CamelError::RouteError("to: URI must not be empty".into()));
            }
            Ok(DeclarativeStep::To(ToStepDef::new(to)))
        }
        YamlStep::WireTap(WireTapStep { wire_tap }) => {
            if wire_tap.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "wire_tap: URI must not be empty".into(),
                ));
            }
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
        YamlStep::StreamCache(step) => {
            let threshold = match step.stream_cache {
                StreamCacheBody::Enabled(true) => None,
                StreamCacheBody::Enabled(false) => {
                    return Err(CamelError::RouteError(
                        "'stream_cache: false' is invalid; remove the step or use 'stream_cache: true'"
                            .into(),
                    ));
                }
                StreamCacheBody::Config(config) => config.threshold,
            };
            Ok(DeclarativeStep::StreamCache(StreamCacheStepDef {
                threshold,
            }))
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
                    expr.jsonpath,
                    expr.xpath,
                    "log.message",
                )?,
            };
            Ok(DeclarativeStep::Log(LogStepDef { message, level }))
        }
        YamlStep::SetHeader(SetHeaderStep { set_header }) => {
            if set_header.key.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "set_header: key must not be empty".into(),
                ));
            }
            let value = parse_value_source(
                set_header.value,
                set_header.language,
                set_header.source,
                set_header.simple,
                set_header.rhai,
                set_header.jsonpath,
                set_header.xpath,
                "set_header",
            )?;
            Ok(DeclarativeStep::SetHeader(SetHeaderStepDef {
                key: set_header.key,
                value,
            }))
        }
        YamlStep::SetProperty(SetPropertyStep { set_property }) => {
            if set_property.name.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "set_property: key must not be empty".into(),
                ));
            }
            let value = parse_value_source(
                set_property.value,
                set_property.language,
                set_property.source,
                set_property.simple,
                set_property.rhai,
                set_property.jsonpath,
                set_property.xpath,
                "set_property",
            )?;
            Ok(DeclarativeStep::SetProperty(SetPropertyStepDef {
                key: set_property.name,
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
                    jsonpath,
                    xpath,
                }) => parse_value_source(
                    value, language, source, simple, rhai, jsonpath, xpath, "set_body",
                )?,
            };
            Ok(DeclarativeStep::SetBody(SetBodyStepDef { value }))
        }
        YamlStep::Transform(TransformStep { transform }) => {
            let value = match transform {
                SetBodyData::Literal(value) => ValueSourceDef::Literal(value),
                SetBodyData::Config(SetBodyConfig {
                    value,
                    language,
                    source,
                    simple,
                    rhai,
                    jsonpath,
                    xpath,
                }) => parse_value_source(
                    value,
                    language,
                    source,
                    simple,
                    rhai,
                    jsonpath,
                    xpath,
                    "transform",
                )?,
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
        YamlStep::Function(FunctionStep { function: data }) => {
            if data.runtime.is_empty() {
                return Err(CamelError::RouteError(
                    "function: 'runtime' must not be empty".into(),
                ));
            }
            if data.source.is_empty() {
                return Err(CamelError::RouteError(
                    "function: 'source' must not be empty".into(),
                ));
            }
            if let Some(t) = data.timeout_ms
                && t == 0
            {
                return Err(CamelError::RouteError(
                    "function: 'timeout_ms' must be greater than 0".into(),
                ));
            }
            if data.runtime != "deno" {
                return Err(CamelError::RouteError(format!(
                    "function: unsupported runtime '{}'. Supported runtimes: [\"deno\"]",
                    data.runtime
                )));
            }
            Ok(DeclarativeStep::Function(crate::model::FunctionStepDef {
                runtime: data.runtime,
                source: data.source,
                timeout_ms: data.timeout_ms,
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
                    jsonpath,
                    xpath,
                })) => {
                    let expr = parse_language_expression(
                        language,
                        source,
                        simple,
                        rhai,
                        jsonpath,
                        xpath,
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
                correlation_key: aggregate.correlation_key,
                completion_size: aggregate.completion_size,
                completion_timeout_ms: aggregate.completion_timeout_ms,
                completion_predicate,
                strategy,
                max_buckets: aggregate.max_buckets,
                bucket_ttl_ms: aggregate.bucket_ttl_ms,
                force_completion_on_stop: aggregate.force_completion_on_stop,
                discard_on_timeout: aggregate.discard_on_timeout,
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
        YamlStep::Marshal(MarshalStep { marshal }) => {
            if marshal.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "marshal: format must not be empty".into(),
                ));
            }
            Ok(DeclarativeStep::Marshal(DataFormatDef { format: marshal }))
        }
        YamlStep::Unmarshal(UnmarshalStep { unmarshal }) => {
            if unmarshal.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "unmarshal: format must not be empty".into(),
                ));
            }
            Ok(DeclarativeStep::Unmarshal(DataFormatDef {
                format: unmarshal,
            }))
        }
        YamlStep::Bean(BeanStep {
            bean: BeanStepData { name, method },
        }) => {
            if name.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "bean: name must not be empty".into(),
                ));
            }
            Ok(DeclarativeStep::Bean(BeanStepDef::new(name, method)))
        }
        YamlStep::DynamicRouter(DynamicRouterStep {
            dynamic_router:
                DynamicRouterData {
                    language,
                    source,
                    simple,
                    rhai,
                    uri_delimiter,
                    cache_size,
                    ignore_invalid_endpoints,
                    max_iterations,
                },
        }) => {
            let expression = parse_language_expression(
                language,
                source,
                simple,
                rhai,
                None,
                None,
                "dynamic_router",
            )?;
            Ok(DeclarativeStep::DynamicRouter(DynamicRouterStepDef {
                expression,
                uri_delimiter,
                cache_size,
                ignore_invalid_endpoints,
                max_iterations,
            }))
        }
        YamlStep::LoadBalance(LoadBalanceStep {
            load_balance:
                LoadBalanceData {
                    strategy,
                    distribution_ratio,
                    parallel,
                    steps,
                },
        }) => {
            let strategy = match strategy.as_str() {
                "round_robin" => LoadBalanceStrategyDef::RoundRobin,
                "random" => LoadBalanceStrategyDef::Random,
                "failover" => LoadBalanceStrategyDef::Failover,
                "weighted" => {
                    let ratio = distribution_ratio.ok_or_else(|| {
                        CamelError::RouteError(
                            "weighted strategy requires distribution_ratio (e.g. \"4,2,1\")".into(),
                        )
                    })?;
                    LoadBalanceStrategyDef::Weighted {
                        distribution_ratio: ratio,
                    }
                }
                other => {
                    return Err(CamelError::RouteError(format!(
                        "unsupported load_balance.strategy `{other}`"
                    )));
                }
            };
            let steps = steps
                .into_iter()
                .map(yaml_step_to_declarative_step)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DeclarativeStep::LoadBalance(LoadBalanceStepDef {
                strategy,
                parallel,
                steps,
            }))
        }
        YamlStep::RoutingSlip(RoutingSlipStep {
            routing_slip:
                RoutingSlipData {
                    language,
                    source,
                    simple,
                    rhai,
                    uri_delimiter,
                    cache_size,
                    ignore_invalid_endpoints,
                },
        }) => {
            let expression = parse_language_expression(
                language,
                source,
                simple,
                rhai,
                None,
                None,
                "routing_slip",
            )?;
            Ok(DeclarativeStep::RoutingSlip(RoutingSlipStepDef {
                expression,
                uri_delimiter,
                cache_size,
                ignore_invalid_endpoints,
            }))
        }
        YamlStep::RecipientList(RecipientListStep {
            recipient_list:
                RecipientListData {
                    language,
                    source,
                    simple,
                    rhai,
                    delimiter,
                    parallel,
                    parallel_limit,
                    stop_on_exception,
                    strategy,
                },
        }) => {
            let expression = parse_language_expression(
                language,
                source,
                simple,
                rhai,
                None,
                None,
                "recipient_list",
            )?;
            let aggregation = match strategy.as_deref() {
                None | Some("last_wins") => MulticastAggregationDef::LastWins,
                Some("collect_all") => MulticastAggregationDef::CollectAll,
                Some("original") => MulticastAggregationDef::Original,
                Some(other) => {
                    return Err(CamelError::RouteError(format!(
                        "unsupported recipient_list.strategy `{other}`"
                    )));
                }
            };
            Ok(DeclarativeStep::RecipientList(RecipientListStepDef {
                expression,
                delimiter,
                parallel,
                parallel_limit,
                stop_on_exception,
                aggregation,
            }))
        }
        YamlStep::Throttle(ThrottleStep {
            throttle:
                ThrottleData {
                    max_requests,
                    period_secs,
                    strategy,
                    steps,
                },
        }) => {
            let strategy = match strategy.as_deref() {
                None | Some("delay") => ThrottleStrategyDef::Delay,
                Some("reject") => ThrottleStrategyDef::Reject,
                Some("drop") => ThrottleStrategyDef::Drop,
                Some(other) => {
                    return Err(CamelError::RouteError(format!(
                        "unsupported throttle.strategy `{other}`"
                    )));
                }
            };
            let steps = steps
                .into_iter()
                .map(yaml_step_to_declarative_step)
                .collect::<Result<Vec<_>, _>>()?;
            if max_requests == 0 {
                return Err(CamelError::RouteError(
                    "throttle: max_requests must be > 0".into(),
                ));
            }
            Ok(DeclarativeStep::Throttle(ThrottleStepDef {
                max_requests,
                period_ms: period_secs.saturating_mul(1000),
                strategy,
                steps,
            }))
        }
        YamlStep::Delay(DelayStep { delay }) => {
            let (delay_ms, dynamic_header) = match delay {
                DelayBody::Short(ms) => (ms, None),
                DelayBody::Full(cfg) => (cfg.delay_ms, cfg.dynamic_header),
            };
            if delay_ms == 0 {
                return Err(CamelError::RouteError("delay: delay_ms must be > 0".into()));
            }
            Ok(DeclarativeStep::Delay(DelayStepDef {
                delay_ms,
                dynamic_header,
            }))
        }
        YamlStep::Loop(LoopStep { loop_data }) => {
            let (count, while_predicate, steps) = match loop_data {
                LoopData::Count(n) => (Some(n), None, vec![]),
                LoopData::Full(cfg) => {
                    let predicate = match &cfg.while_expr {
                        Some(expr) => Some(parse_loop_while_expr(expr, "loop.while")?),
                        None => None,
                    };
                    let sub_steps = cfg
                        .steps
                        .into_iter()
                        .map(yaml_step_to_declarative_step)
                        .collect::<Result<Vec<_>, _>>()?;
                    (cfg.count, predicate, sub_steps)
                }
            };
            Ok(DeclarativeStep::Loop(LoopStepDef {
                count,
                while_predicate,
                steps,
            }))
        }
        YamlStep::Validate(ValidateStep { validate }) => {
            let uri = if validate.starts_with("validator:") {
                validate
            } else {
                format!("validator:{validate}")
            };
            Ok(DeclarativeStep::To(ToStepDef::new(uri)))
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
    if let Some(source) = block.jsonpath.as_ref() {
        selected.push(("jsonpath".to_string(), source.clone()));
    }
    if let Some(source) = block.xpath.as_ref() {
        selected.push(("xpath".to_string(), source.clone()));
    }

    if selected.len() != 1 {
        return Err(CamelError::RouteError(format!(
            "{context} must define exactly one predicate source: `language+source`, `simple`, `rhai`, `jsonpath`, or `xpath`"
        )));
    }

    let (language, source) = selected.remove(0);
    Ok(LanguageExpressionDef { language, source })
}

fn parse_loop_while_expr(
    expr: &LoopWhileExpr,
    context: &str,
) -> Result<LanguageExpressionDef, CamelError> {
    let mut selected = Vec::new();

    if let (Some(language), Some(source)) = (expr.language.as_ref(), expr.source.as_ref()) {
        selected.push((language.clone(), source.clone()));
    } else if expr.language.is_some() || expr.source.is_some() {
        return Err(CamelError::RouteError(format!(
            "{context}: `language` and `source` must be set together"
        )));
    }

    if let Some(source) = expr.simple.as_ref() {
        selected.push(("simple".to_string(), source.clone()));
    }
    if let Some(source) = expr.rhai.as_ref() {
        selected.push(("rhai".to_string(), source.clone()));
    }
    if let Some(source) = expr.jsonpath.as_ref() {
        selected.push(("jsonpath".to_string(), source.clone()));
    }
    if let Some(source) = expr.xpath.as_ref() {
        selected.push(("xpath".to_string(), source.clone()));
    }

    if selected.len() != 1 {
        return Err(CamelError::RouteError(format!(
            "{context} must define exactly one predicate source: `language+source`, `simple`, `rhai`, `jsonpath`, or `xpath`"
        )));
    }

    let (language, source) = selected.remove(0);
    Ok(LanguageExpressionDef { language, source })
}

#[allow(clippy::too_many_arguments)]
fn parse_value_source(
    literal: Option<serde_json::Value>,
    language: Option<String>,
    source: Option<String>,
    simple: Option<String>,
    rhai: Option<String>,
    jsonpath: Option<String>,
    xpath: Option<String>,
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
    if jsonpath.is_some() {
        count += 1;
    }
    if xpath.is_some() {
        count += 1;
    }

    if count != 1 {
        return Err(CamelError::RouteError(format!(
            "{context} must define exactly one value source: `value`, `language+source`, `simple`, `rhai`, `jsonpath`, or `xpath`"
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
    if let Some(source) = jsonpath {
        return Ok(ValueSourceDef::Expression(LanguageExpressionDef {
            language: "jsonpath".to_string(),
            source,
        }));
    }
    if let Some(source) = xpath {
        return Ok(ValueSourceDef::Expression(LanguageExpressionDef {
            language: "xpath".to_string(),
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
    jsonpath: Option<String>,
    xpath: Option<String>,
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
    if jsonpath.is_some() {
        count += 1;
    }
    if xpath.is_some() {
        count += 1;
    }

    if count != 1 {
        return Err(CamelError::RouteError(format!(
            "{context} must define exactly one language source: `language+source`, `simple`, `rhai`, `jsonpath`, or `xpath`"
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
    if let Some(source) = jsonpath {
        return Ok(LanguageExpressionDef {
            language: "jsonpath".to_string(),
            source,
        });
    }
    if let Some(source) = xpath {
        return Ok(LanguageExpressionDef {
            language: "xpath".to_string(),
            source,
        });
    }

    Err(CamelError::RouteError(format!(
        "{context}: missing language source"
    )))
}

pub fn load_from_file(path: &Path) -> Result<Vec<RouteDefinition>, CamelError> {
    info!(path = %path.display(), "loading routes from file");
    let content = std::fs::read_to_string(path).map_err(|e| {
        error!(path = %path.display(), error = %e, "failed to load routes from file");
        CamelError::Io(format!("Failed to read {}: {e}", path.display()))
    })?;
    parse_yaml(&content).map_err(|e| CamelError::RouteError(format!("{e} (in {})", path.display())))
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
    fn test_parse_invalid_yaml_produces_parse_error() {
        let yaml = r#"
routes:
  - id: [invalid
    from: "timer:tick"
"#;
        let err = match parse_yaml(yaml) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected parse error for invalid YAML"),
        };
        assert!(
            err.contains("YAML parse error"),
            "expected 'YAML parse error' in message, got: {err}"
        );
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
          correlation_key: "${header.orderId}"
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
    fn parse_security_policy_roles() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      roles: ["admin", "superuser"]
      all_required: false
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        let sp = routes[0].security_policy.as_ref().unwrap();
        match sp {
            DeclarativeSecurityPolicy::Roles {
                roles,
                all_required,
            } => {
                assert_eq!(roles, &vec!["admin".to_string(), "superuser".to_string()]);
                assert!(!all_required);
            }
            _ => panic!("expected Roles"),
        }
    }

    #[test]
    fn parse_security_policy_scopes() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      scopes: ["read:api"]
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let sp = routes[0].security_policy.as_ref().unwrap();
        match sp {
            DeclarativeSecurityPolicy::Scopes {
                scopes,
                all_required,
            } => {
                assert_eq!(scopes, &vec!["read:api".to_string()]);
                assert!(*all_required);
            }
            _ => panic!("expected Scopes"),
        }
    }

    #[test]
    fn parse_security_policy_ref() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      ref: "my-custom-policy"
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let sp = routes[0].security_policy.as_ref().unwrap();
        match sp {
            DeclarativeSecurityPolicy::Ref { name } => {
                assert_eq!(name, "my-custom-policy");
            }
            _ => panic!("expected Ref"),
        }
    }

    #[test]
    fn parse_security_policy_wasm() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      wasm: "plugins/my-auth-policy.wasm"
      config:
        ldap_url: "ldap://corp"
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let sp = routes[0].security_policy.as_ref().unwrap();
        match sp {
            DeclarativeSecurityPolicy::Wasm { path, config } => {
                assert_eq!(path, "plugins/my-auth-policy.wasm");
                assert_eq!(config.get("ldap_url").unwrap(), "ldap://corp");
            }
            _ => panic!("expected Wasm"),
        }
    }

    #[test]
    fn parse_security_policy_permission() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      permission:
        policy: "keycloak-uma"
        cache_ttl_secs: 60
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let sp = routes[0].security_policy.as_ref().unwrap();
        match sp {
            DeclarativeSecurityPolicy::Permission {
                policy,
                resource,
                action,
                scopes,
                context,
                cache_ttl_secs,
                cache_negative_ttl_secs,
            } => {
                assert_eq!(policy, "keycloak-uma");
                assert_eq!(
                    resource,
                    &camel_auth::PermissionValueSource::Header("x-resource".into())
                );
                assert_eq!(
                    action,
                    &camel_auth::PermissionValueSource::Header("x-action".into())
                );
                assert!(scopes.is_empty());
                assert_eq!(context, &camel_auth::PermissionContextConfig::default());
                assert_eq!(*cache_ttl_secs, Some(60));
                assert_eq!(*cache_negative_ttl_secs, None);
            }
            _ => panic!("expected Permission"),
        }
    }

    #[test]
    fn parse_security_policy_permission_full_shape() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      permission:
        policy: "invoice-policy"
        resource:
          header: "CamelResourceId"
        action:
          literal: "read"
        scopes: ["read", "write"]
        context:
          headers: ["CamelTenantId"]
          properties: ["camel.auth.subject"]
        cache_ttl_secs: 60
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let sp = routes[0].security_policy.as_ref().unwrap();
        match sp {
            DeclarativeSecurityPolicy::Permission {
                policy,
                resource,
                action,
                scopes,
                context,
                cache_ttl_secs,
                cache_negative_ttl_secs,
            } => {
                assert_eq!(policy, "invoice-policy");
                assert_eq!(
                    resource,
                    &camel_auth::PermissionValueSource::Header("CamelResourceId".to_string())
                );
                assert_eq!(
                    action,
                    &camel_auth::PermissionValueSource::Literal("read".to_string())
                );
                assert_eq!(scopes, &vec!["read".to_string(), "write".to_string()]);
                assert_eq!(context.include_headers, vec!["CamelTenantId".to_string()]);
                assert_eq!(
                    context.include_properties,
                    vec!["camel.auth.subject".to_string()]
                );
                assert_eq!(*cache_ttl_secs, Some(60));
                assert_eq!(*cache_negative_ttl_secs, None);
            }
            _ => panic!("expected Permission"),
        }
    }

    #[test]
    fn parse_security_policy_permission_with_all_ttl() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      permission:
        policy: "keycloak-uma"
        cache_ttl_secs: 120
        cache_negative_ttl_secs: 30
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let sp = routes[0].security_policy.as_ref().unwrap();
        match sp {
            DeclarativeSecurityPolicy::Permission {
                policy,
                resource,
                action,
                scopes,
                context,
                cache_ttl_secs,
                cache_negative_ttl_secs,
            } => {
                assert_eq!(policy, "keycloak-uma");
                assert_eq!(
                    resource,
                    &camel_auth::PermissionValueSource::Header("x-resource".into())
                );
                assert_eq!(
                    action,
                    &camel_auth::PermissionValueSource::Header("x-action".into())
                );
                assert!(scopes.is_empty());
                assert_eq!(context, &camel_auth::PermissionContextConfig::default());
                assert_eq!(*cache_ttl_secs, Some(120));
                assert_eq!(*cache_negative_ttl_secs, Some(30));
            }
            _ => panic!("expected Permission"),
        }
    }

    #[test]
    fn parse_security_policy_permission_empty_policy_rejected() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      permission:
        policy: ""
    steps:
      - to: log:info
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_security_policy_permission_with_all_required_rejected() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      permission:
        policy: "keycloak-uma"
      all_required: true
    steps:
      - to: log:info
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_security_policy_empty_rejected() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy: {}
    steps:
      - to: log:info
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_security_policy_multiple_forms_rejected() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      roles: ["admin"]
      scopes: ["read"]
    steps:
      - to: log:info
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_security_policy_empty_roles_rejected() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      roles: []
    steps:
      - to: log:info
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_security_policy_ref_with_all_required_rejected() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      ref: "my-policy"
      all_required: true
    steps:
      - to: log:info
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_security_policy_empty_scopes_rejected() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      scopes: []
    steps:
      - to: log:info
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_security_policy_wasm_with_all_required_rejected() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      wasm: "plugin.wasm"
      all_required: true
    steps:
      - to: log:info
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_security_policy_none_is_ok() {
        let yaml = r#"
routes:
  - id: r-plain
    from: direct:start
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert!(routes[0].security_policy.is_none());
    }

    #[test]
    fn parse_yaml_to_canonical_rejects_security_policy() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      roles: ["admin"]
    steps:
      - to: log:info
"#;
        let result = parse_yaml_to_canonical(yaml);
        assert!(result.is_err());
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
          correlation_key: "${header.orderId}"
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
    fn transform_step_sets_body() {
        let yaml = r#"
routes:
  - id: "transform-step"
    from: "timer:tick"
    steps:
      - transform:
          simple: "hello"
      - to: "log:out"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        let steps = &routes[0].steps;
        assert_eq!(steps.len(), 2);
        assert!(matches!(&steps[0], DeclarativeStep::SetBody(_)));
    }

    #[test]
    fn transform_same_output_as_set_body() {
        let yaml_transform = r#"
routes:
  - id: "transform-eq"
    from: "timer:tick"
    steps:
      - transform:
          simple: "world"
"#;
        let yaml_set_body = r#"
routes:
  - id: "transform-eq"
    from: "timer:tick"
    steps:
      - set_body:
          simple: "world"
"#;
        let routes_t = parse_yaml_to_declarative(yaml_transform).unwrap();
        let routes_s = parse_yaml_to_declarative(yaml_set_body).unwrap();
        assert_eq!(
            format!("{:?}", routes_t[0].steps[0]),
            format!("{:?}", routes_s[0].steps[0])
        );
    }

    #[test]
    fn test_parse_loop_shorthand() {
        let yaml = r#"
routes:
  - id: "loop-short"
    from: "direct:start"
    steps:
      - loop: 3
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn test_parse_loop_count_with_steps() {
        let yaml = r#"
routes:
  - id: "loop-count"
    from: "direct:start"
    steps:
      - loop:
          count: 3
          steps:
            - to: "mock:result"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn test_parse_loop_while_simple() {
        let yaml = r#"
routes:
  - id: "loop-while"
    from: "direct:start"
    steps:
      - loop:
          while:
            simple: "${body} contains 'retry'"
          steps:
            - to: "mock:retry"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn test_parse_loop_both_count_and_while_ok_at_yaml_level() {
        let yaml = r#"
routes:
  - id: "loop-both"
    from: "direct:start"
    steps:
      - loop:
          count: 3
          while:
            simple: "${body}"
          steps:
            - to: "mock:result"
"#;
        // Both fields can coexist in YAML AST — validation happens in step_resolution
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_from_nonexistent_file() {
        let result = load_from_file(Path::new("/nonexistent/path/routes.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn parse_marshal_step() {
        let yaml = r#"
routes:
  - id: test
    from: "direct:in"
    steps:
      - marshal: json
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let steps = &routes[0].steps;
        assert!(
            matches!(&steps[0], DeclarativeStep::Marshal(DataFormatDef { format }) if format == "json")
        );
    }

    #[test]
    fn parse_unmarshal_step() {
        let yaml = r#"
routes:
  - id: test
    from: "direct:in"
    steps:
      - unmarshal: xml
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let steps = &routes[0].steps;
        assert!(
            matches!(&steps[0], DeclarativeStep::Unmarshal(DataFormatDef { format }) if format == "xml")
        );
    }

    #[test]
    fn test_parse_filter_jsonpath() {
        let yaml = r#"
routes:
  - id: "jsonpath-filter"
    from: "direct:start"
    steps:
      - filter:
          jsonpath: "$.active"
          steps:
            - to: "log:info"
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn test_parse_set_header_jsonpath() {
        let yaml = r#"
routes:
  - id: "jsonpath-header"
    from: "direct:start"
    steps:
      - set_header:
          key: "orderId"
          jsonpath: "$.order.id"
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn test_parse_set_property_literal() {
        let yaml = r#"
routes:
  - id: "property-route"
    from: "direct:start"
    steps:
      - set_property:
          name: "traceId"
          value: "abc-123"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        match &routes[0].steps[0] {
            DeclarativeStep::SetProperty(def) => {
                assert_eq!(def.key, "traceId");
                assert_eq!(
                    def.value,
                    ValueSourceDef::Literal(serde_json::Value::String("abc-123".into()))
                );
            }
            other => panic!("expected SetProperty step, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_set_body_jsonpath() {
        let yaml = r#"
routes:
  - id: "jsonpath-body"
    from: "direct:start"
    steps:
      - set_body:
          jsonpath: "$.items[0]"
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn test_parse_log_jsonpath() {
        let yaml = r#"
routes:
  - id: "jsonpath-log"
    from: "direct:start"
    steps:
      - log:
          message:
            jsonpath: "$.message"
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn test_parse_split_jsonpath() {
        let yaml = r#"
routes:
  - id: "jsonpath-split"
    from: "direct:start"
    steps:
      - split:
          expression:
            jsonpath: "$.items[*]"
          steps:
            - to: "log:info"
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn test_parse_choice_jsonpath() {
        let yaml = r#"
routes:
  - id: "jsonpath-choice"
    from: "direct:start"
    steps:
      - choice:
          when:
            - jsonpath: "$.priority == 'high'"
              steps:
                - to: "log:high"
          otherwise:
            - to: "log:low"
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn test_parse_filter_jsonpath_and_simple_fails() {
        let yaml = r#"
routes:
  - id: "jsonpath-conflict"
    from: "direct:start"
    steps:
      - filter:
          jsonpath: "$.active"
          simple: "${body} == 'test'"
          steps:
            - to: "log:info"
"#;
        let result = parse_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_filter_xpath() {
        let yaml = r#"
routes:
  - id: "xpath-filter"
    from: "direct:start"
    steps:
      - filter:
          xpath: "/orders/order[@status='pending']"
          steps:
            - to: "log:filtered"
"#;
        let routes = parse_yaml(yaml).expect("parse failed");
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn test_parse_set_header_xpath() {
        let yaml = r#"
routes:
  - id: "xpath-header"
    from: "direct:start"
    steps:
      - set_header:
          key: "title"
          xpath: "/books/book[1]/title"
      - to: "log:out"
"#;
        let routes = parse_yaml(yaml).expect("parse failed");
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn test_parse_set_body_xpath() {
        let yaml = r#"
routes:
  - id: "xpath-body"
    from: "direct:start"
    steps:
      - set_body:
          xpath: "//item[name='widget']/price"
      - to: "log:out"
"#;
        let routes = parse_yaml(yaml).expect("parse failed");
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn test_parse_log_xpath() {
        let yaml = r#"
routes:
  - id: "xpath-log"
    from: "direct:start"
    steps:
      - log:
          message:
            xpath: "/root/message"
"#;
        let routes = parse_yaml(yaml).expect("parse failed");
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn test_parse_split_xpath() {
        let yaml = r#"
routes:
  - id: "xpath-split"
    from: "direct:start"
    steps:
      - split:
          expression:
            xpath: "/catalog/book"
"#;
        let routes = parse_yaml(yaml).expect("parse failed");
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn test_parse_choice_xpath() {
        let yaml = r#"
routes:
  - id: "xpath-choice"
    from: "direct:start"
    steps:
      - choice:
          when:
            - xpath: "count(/errors/error) > 0"
              steps:
                - to: "log:error"
"#;
        let routes = parse_yaml(yaml).expect("parse failed");
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn test_parse_filter_xpath_and_simple_fails() {
        let yaml = r#"
routes:
  - id: "xpath-and-simple"
    from: "direct:start"
    steps:
      - filter:
          xpath: "/root/item"
          simple: "${header.type}"
          steps:
            - to: "log:out"
"#;
        let result = parse_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_filter_xpath_and_jsonpath_fails() {
        let yaml = r#"
routes:
  - id: "xpath-and-jsonpath"
    from: "direct:start"
    steps:
      - filter:
          xpath: "/root/item"
          jsonpath: "$.item"
          steps:
            - to: "log:out"
"#;
        let result = parse_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn validate_step_parses_to_validator_uri() {
        let yaml = r#"
routes:
  - id: test
    from: "direct:in"
    steps:
      - validate: "schemas/order.xsd"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let step = &routes[0].steps[0];
        assert!(
            matches!(step, DeclarativeStep::To(uri) if uri.uri == "validator:schemas/order.xsd"),
            "got: {step:?}"
        );
    }

    #[test]
    fn validate_step_does_not_break_untagged_resolution_order() {
        let yaml = r#"
routes:
  - id: test
    from: "direct:in"
    steps:
      - to: "direct:next"
      - validate: "schemas/order.xsd"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert!(matches!(routes[0].steps[0], DeclarativeStep::To(_)));
        assert!(matches!(routes[0].steps[1], DeclarativeStep::To(_)));
    }

    #[test]
    fn test_parse_stream_cache_shorthand() {
        let yaml = r#"
routes:
  - id: "sc-short"
    from: "direct:start"
    steps:
      - stream_cache: true
      - to: "log:out"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(&routes[0].steps[0], DeclarativeStep::StreamCache(def) if def.threshold.is_none())
        );
    }

    #[test]
    fn test_parse_stream_cache_with_threshold() {
        let yaml = r#"
routes:
  - id: "sc-thresh"
    from: "direct:start"
    steps:
      - stream_cache: { threshold: 65536 }
      - to: "log:out"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        match &routes[0].steps[0] {
            DeclarativeStep::StreamCache(def) => assert_eq!(def.threshold, Some(65536)),
            other => panic!("expected StreamCache, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_stream_cache_false_rejected() {
        let yaml = r#"
routes:
  - id: "sc-false"
    from: "direct:start"
    steps:
      - stream_cache: false
"#;
        let result = parse_yaml_to_declarative(yaml);
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("stream_cache: false"),
            "expected rejection message, got: {msg}"
        );
    }

    #[test]
    fn test_parse_function_step_happy_path() {
        let yaml = r#"
routes:
  - id: "fn-ok"
    from: "direct:start"
    steps:
      - function:
          runtime: deno
          source: "return { body: \"ok\" };"
          timeout_ms: 5000
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        match &routes[0].steps[0] {
            DeclarativeStep::Function(def) => {
                assert_eq!(def.runtime, "deno");
                assert_eq!(def.source, "return { body: \"ok\" };");
                assert_eq!(def.timeout_ms, Some(5000));
            }
            other => panic!("expected Function, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_function_step_rejects_bad_runtime() {
        let yaml = r#"
routes:
  - id: "fn-bad-runtime"
    from: "direct:start"
    steps:
      - function:
          runtime: node
          source: "return {};"
"#;
        let err = parse_yaml_to_declarative(yaml).unwrap_err().to_string();
        assert!(err.contains("unsupported runtime 'node'"));
        assert!(err.contains("[\"deno\"]"));
    }

    #[test]
    fn test_parse_function_step_rejects_empty_runtime_and_source() {
        let empty_runtime = r#"
routes:
  - id: "fn-empty-runtime"
    from: "direct:start"
    steps:
      - function:
          runtime: ""
          source: "return {};"
"#;
        let err = parse_yaml_to_declarative(empty_runtime)
            .unwrap_err()
            .to_string();
        assert!(err.contains("function: 'runtime' must not be empty"));

        let empty_source = r#"
routes:
  - id: "fn-empty-source"
    from: "direct:start"
    steps:
      - function:
          runtime: deno
          source: ""
"#;
        let err = parse_yaml_to_declarative(empty_source)
            .unwrap_err()
            .to_string();
        assert!(err.contains("function: 'source' must not be empty"));
    }

    #[test]
    fn test_parse_function_step_rejects_unknown_v1_keys() {
        let yaml = r#"
routes:
  - id: "fn-unknown"
    from: "direct:start"
    steps:
      - function:
          runtime: deno
          source: "return {};"
          provider: docker
"#;
        assert!(parse_yaml_to_declarative(yaml).is_err());
    }

    #[test]
    fn test_function_step_rejected_by_canonical_yaml() {
        let yaml = r#"
routes:
  - id: "fn-canonical-reject"
    from: "direct:start"
    steps:
      - function:
          runtime: deno
          source: "return {};"
          timeout_ms: 1000
"#;
        let err = parse_yaml_to_canonical(yaml).unwrap_err().to_string();
        assert!(
            err.contains("canonical v1 does not support step `function`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_yaml_empty_to_uri_rejected() {
        let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    steps:
      - to: ""
"#;
        let err = match parse_yaml(yaml) {
            Ok(_) => panic!("expected error, got success"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("URI must not be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_yaml_empty_wire_tap_uri_rejected() {
        let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    steps:
      - wire_tap: ""
"#;
        let err = match parse_yaml(yaml) {
            Ok(_) => panic!("expected error, got success"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("URI must not be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_yaml_empty_header_key_rejected() {
        let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    steps:
      - set_header:
          key: ""
          value: "test"
"#;
        let err = match parse_yaml(yaml) {
            Ok(_) => panic!("expected error, got success"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("key must not be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_yaml_empty_property_key_rejected() {
        let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    steps:
      - set_property:
          name: ""
          value: "test"
"#;
        let err = match parse_yaml(yaml) {
            Ok(_) => panic!("expected error, got success"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("key must not be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_yaml_zero_delay_rejected() {
        let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    steps:
      - delay: 0
"#;
        let err = match parse_yaml(yaml) {
            Ok(_) => panic!("expected error, got success"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("delay_ms must be > 0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_yaml_zero_throttle_rejected() {
        let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    steps:
      - throttle:
          max_requests: 0
"#;
        let err = match parse_yaml(yaml) {
            Ok(_) => panic!("expected error, got success"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("max_requests must be > 0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_yaml_zero_failure_threshold_rejected() {
        let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    circuit_breaker:
      failure_threshold: 0
      open_duration_ms: 5000
    steps:
      - to: "log:info"
"#;
        let err = match parse_yaml(yaml) {
            Ok(_) => panic!("expected error, got success"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("failure_threshold must be > 0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_yaml_empty_marshal_format_rejected() {
        let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    steps:
      - marshal: ""
"#;
        let err = match parse_yaml(yaml) {
            Ok(_) => panic!("expected error, got success"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("format must not be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_yaml_empty_bean_name_rejected() {
        let yaml = r#"
routes:
  - id: test
    from: "timer:tick"
    steps:
      - bean:
          name: ""
          method: "process"
"#;
        let err = match parse_yaml(yaml) {
            Ok(_) => panic!("expected error, got success"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("name must not be empty"),
            "unexpected error: {err}"
        );
    }

    // --- Template parsing tests (Phase 4) ---

    #[test]
    fn test_parse_yaml_templates_basic() {
        use crate::template::yaml::parse_yaml_templates;
        let yaml = r#"
routes: []
templates:
  - id: http-route
    parameters:
      - name: path
        default_value: /api
    routes:
      - id: "my-route"
        from: "rest:{{path}}"
        steps:
          - to: "log:info"
"#;
        let specs = parse_yaml_templates(yaml).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "http-route");
        assert_eq!(specs[0].parameters.len(), 1);
        assert_eq!(specs[0].parameters[0].name, "path");
        assert_eq!(specs[0].routes[0]["id"], "my-route");
    }

    #[test]
    fn test_parse_yaml_templated_routes_basic() {
        use crate::template::yaml::parse_yaml_templated_routes;
        let yaml = r#"
routes: []
templated_routes:
  - route_template_ref: http-route
    route_id: my-http-route
    parameters:
      path: /users
"#;
        let specs = parse_yaml_templated_routes(yaml).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].route_template_ref, "http-route");
        assert_eq!(specs[0].route_id.as_deref(), Some("my-http-route"));
        assert_eq!(specs[0].parameters["path"], "/users");
    }

    #[test]
    fn test_parse_yaml_backward_compat_no_templates() {
        use crate::template::yaml::{parse_yaml_templated_routes, parse_yaml_templates};
        let yaml = r#"
routes:
  - id: r1
    from: direct:start
"#;
        assert!(parse_yaml_templates(yaml).unwrap().is_empty());
        assert!(parse_yaml_templated_routes(yaml).unwrap().is_empty());
    }

    #[test]
    fn test_parse_json_templates_basic() {
        use crate::template::json::parse_json_templates;
        let json = r#"
{
    "routes": [],
    "templates": [
        {
            "id": "http-route",
            "parameters": [{"name": "path", "default_value": "/api"}],
            "routes": [
                {
                    "id": "my-route",
                    "from": "rest:{{path}}",
                    "steps": [{"to": "log:info"}]
                }
            ]
        }
    ]
}"#;
        let specs = parse_json_templates(json).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "http-route");
        assert_eq!(specs[0].routes[0]["id"], "my-route");
    }

    #[test]
    fn test_parse_json_templated_routes_basic() {
        use crate::template::json::parse_json_templated_routes;
        let json = r#"
{
    "routes": [],
    "templated_routes": [
        {
            "route_template_ref": "http-route",
            "route_id": "my-route",
            "parameters": {"path": "/users"}
        }
    ]
}"#;
        let specs = parse_json_templated_routes(json).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].route_template_ref, "http-route");
        assert_eq!(specs[0].parameters["path"], "/users");
    }

    #[test]
    fn test_parse_json_backward_compat_no_templates() {
        use crate::template::json::{parse_json_templated_routes, parse_json_templates};
        let json = r#"
{
    "routes": [{"id": "r1", "from": "direct:start"}]
}"#;
        assert!(parse_json_templates(json).unwrap().is_empty());
        assert!(parse_json_templated_routes(json).unwrap().is_empty());
    }

    #[test]
    fn test_yaml_to_json_value_conversion() {
        use crate::template::yaml::parse_yaml_templates;
        let yaml = r#"
routes: []
templates:
  - id: num-tpl
    routes:
      - id: num-route
        count: 42
        ratio: 3.14
        enabled: true
        nothing: null
"#;
        let specs = parse_yaml_templates(yaml).unwrap();
        assert_eq!(specs[0].routes[0]["count"], 42);
        assert_eq!(specs[0].routes[0]["ratio"], 3.14);
        assert_eq!(specs[0].routes[0]["enabled"], true);
        assert_eq!(specs[0].routes[0]["nothing"], serde_json::Value::Null);
    }
}
