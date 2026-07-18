//! YAML route definition parser.

// serde_yml migrated to noyalib (compat-serde-yaml shim) — closes RUSTSEC-2025-0068.
// Module alias preserves call-site paths byte-for-byte.
use noyalib::compat::serde_yaml as serde_yml;

use std::path::Path;

use tracing::{debug, error, info};

use camel_api::error_handler::ExceptionDisposition;
use camel_api::{
    CamelError, CanonicalLossReport, CanonicalRouteSpec, StreamSplitConfig, StreamSplitFormat,
};
use camel_core::route::RouteDefinition;

use crate::compile::{
    compile_declarative_route, compile_declarative_route_to_canonical,
    compile_declarative_route_with_stream_cache_threshold,
};
use crate::contract::{DeclarativeStepKind, assert_contract_coverage};
use crate::input_format::{InputFormat, annotate_format};
use crate::model::{
    AggregateStepDef, AggregateStrategyDef, BeanStepDef, BodyTypeDef, ChoiceStepDef,
    ClaimCheckStepDef, DataFormatDef, DeclarativeCircuitBreaker, DeclarativeConcurrency,
    DeclarativeErrorHandler, DeclarativeOnException, DeclarativeRedeliveryPolicy, DeclarativeRoute,
    DeclarativeSecurityPolicy, DeclarativeStep, DelayStepDef, DoTryCatchClauseDef, DoTryFinallyDef,
    DynamicRouterStepDef, EnrichStepDef, IdempotentConsumerStepDef, LanguageExpressionDef,
    LoadBalanceStepDef, LoadBalanceStrategyDef, LogLevelDef, LogStepDef, LoopStepDef,
    MulticastAggregationDef, MulticastStepDef, RecipientListStepDef, ResequenceModeDef,
    ResequenceStepDef, RoutingSlipStepDef, SamplingStepDef, ScriptStepDef, SecurityCompileContext,
    SetBodyStepDef, SetHeaderStepDef, SetPropertyStepDef, SortStepDef, SplitAggregationDef,
    SplitExpressionDef, SplitStepDef, StreamCacheStepDef, ThrottleStepDef, ThrottleStrategyDef,
    ToStepDef, ValidateStepDef, ValueSourceDef, WhenStepDef, WireTapStepDef,
};
pub use crate::route_ast::{
    AggregateData, AggregateStep, BeanStep, BeanStepData, ChoiceData, ChoiceStep, ClaimCheckBody,
    ClaimCheckStep, DelayBody, DelayStep, DynamicRouterData, DynamicRouterStep, EnrichBody,
    EnrichConfig, EnrichStep, FilterStep, FunctionStep, IdempotentConsumerBody,
    IdempotentConsumerStep, LoadBalanceData, LoadBalanceStep, LogConfig, LogMessageData,
    LogMessageExpr, LogStep, MarshalStep, MulticastData, MulticastStep, PollEnrichStep,
    PredicateBlock, RecipientListData, RecipientListStep, ResequenceBatchYaml, ResequenceData,
    ResequenceStep, ResequenceStreamYaml, RouteDslRest, RouteDslRoute, RouteDslRoutes,
    RouteDslStep, RoutingSlipData, RoutingSlipStep, SamplingBody, SamplingConfig, SamplingStep,
    ScatterGatherData, ScatterGatherStep, ScriptData, ScriptStep, SetBodyConfig, SetBodyData,
    SetBodyStep, SetHeaderData, SetHeaderStep, SetPropertyData, SetPropertyStep, SortBody,
    SortStep, SplitData, SplitExpressionConfig, SplitExpressionYaml, SplitStep, StopStep,
    StreamCacheBody, StreamCacheConfig, StreamCacheStep, StreamCapacityPolicyYaml,
    StreamGapPolicyYaml, ThrottleData, ThrottleStep, ToStep, TransformStep, UnmarshalStep,
    ValidateStep, WireTapStep,
};
use crate::route_ast::{DoTryStep, LoopData, LoopStep, LoopWhileExpr};

const YAML_IMPLEMENTED_MANDATORY_STEPS: [DeclarativeStepKind; 35] = [
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
    DeclarativeStepKind::Validate,
    DeclarativeStepKind::ConvertBodyTo,
    DeclarativeStepKind::Marshal,
    DeclarativeStepKind::Unmarshal,
    DeclarativeStepKind::Bean,
    DeclarativeStepKind::DynamicRouter,
    DeclarativeStepKind::LoadBalance,
    DeclarativeStepKind::RoutingSlip,
    DeclarativeStepKind::Throttle,
    DeclarativeStepKind::RecipientList,
    DeclarativeStepKind::Delay,
    DeclarativeStepKind::Loop,
    DeclarativeStepKind::Enrich,
    DeclarativeStepKind::PollEnrich,
    DeclarativeStepKind::DoTry,
    DeclarativeStepKind::IdempotentConsumer,
    DeclarativeStepKind::ClaimCheck,
    DeclarativeStepKind::Sampling,
    DeclarativeStepKind::Sort,
    DeclarativeStepKind::Resequence,
];

const _: () = assert_contract_coverage(&YAML_IMPLEMENTED_MANDATORY_STEPS);

pub fn parse_yaml_to_declarative(yaml: &str) -> Result<Vec<DeclarativeRoute>, CamelError> {
    annotate_format(InputFormat::Yaml, parse_yaml_to_declarative_inner(yaml))
}

fn parse_yaml_to_declarative_inner(yaml: &str) -> Result<Vec<DeclarativeRoute>, CamelError> {
    let mut dsl: RouteDslRoutes = serde_yml::from_str(yaml).map_err(|e| {
        // log-policy: system-broken
        error!(error = %e, "yaml parse failed");
        CamelError::RouteError(format!("YAML parse error: {e}"))
    })?;
    debug!(route_count = %dsl.routes.len(), rest_count = %dsl.rest.len(), "yaml routes parsed successfully");

    // Expand REST blocks into RouteDslRoute entries. Shared with the JSON
    // parser (review I2) — the helper performs cross-block validation
    // (duplicate method+path tuples, ambiguous templates per spec §6.3/§7.2).
    let prior_count = dsl.routes.len();
    crate::rest::expand_rest_into(&mut dsl.routes, &dsl.rest)?;
    if prior_count != dsl.routes.len() {
        debug!(
            routes_before = prior_count,
            rest_blocks = dsl.rest.len(),
            routes_after = dsl.routes.len(),
            "rest blocks expanded into route entries"
        );
    }

    // Check for duplicate route IDs across all routes (including REST-expanded).
    crate::rest::check_duplicate_route_ids(&dsl.routes)?;

    dsl.routes
        .into_iter()
        .map(route_dsl_to_declarative_route)
        .collect()
}

/// Extract `rest:` blocks from YAML **without** lowering them to `http:` routes.
///
/// Used by the OpenAPI generator (Phase 3) which needs the original AST,
/// not the lowered route definitions. **Input is unvalidated** — callers
/// should run `lower_all_rest_to_routes` to validate before generation
/// (catches duplicate operation IDs, ambiguous templates, etc.).
pub fn extract_rest_blocks(yaml: &str) -> Result<Vec<RouteDslRest>, CamelError> {
    let dsl: RouteDslRoutes = serde_yml::from_str(yaml).map_err(|e| {
        // log-policy: system-broken
        error!(error = %e, "yaml parse failed");
        CamelError::RouteError(format!("YAML parse error: {e}"))
    })?;
    Ok(dsl.rest)
}

pub fn parse_yaml(yaml: &str) -> Result<Vec<RouteDefinition>, CamelError> {
    annotate_format(InputFormat::Yaml, parse_yaml_inner(yaml))
}

fn parse_yaml_inner(yaml: &str) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_yaml_to_declarative_inner(yaml)?
        .into_iter()
        .map(compile_declarative_route)
        .collect()
}

pub fn parse_yaml_with_threshold(
    yaml: &str,
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, CamelError> {
    annotate_format(
        InputFormat::Yaml,
        parse_yaml_with_threshold_inner(yaml, stream_cache_threshold),
    )
}

fn parse_yaml_with_threshold_inner(
    yaml: &str,
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_yaml_with_threshold_and_security_inner(
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
    annotate_format(
        InputFormat::Yaml,
        parse_yaml_with_threshold_and_security_inner(yaml, stream_cache_threshold, security_ctx),
    )
}

fn parse_yaml_with_threshold_and_security_inner(
    yaml: &str,
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_yaml_to_declarative_inner(yaml)?
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

pub fn parse_yaml_to_canonical(
    yaml: &str,
    allow_loss: bool,
) -> Result<Vec<(CanonicalRouteSpec, Option<CanonicalLossReport>)>, CamelError> {
    annotate_format(
        InputFormat::Yaml,
        parse_yaml_to_canonical_inner(yaml, allow_loss),
    )
}

fn parse_yaml_to_canonical_inner(
    yaml: &str,
    allow_loss: bool,
) -> Result<Vec<(CanonicalRouteSpec, Option<CanonicalLossReport>)>, CamelError> {
    let routes = parse_yaml_to_declarative_inner(yaml)?;
    for route in &routes {
        if route.security_policy.is_some() {
            return Err(CamelError::RouteError(
                "routes with security_policy cannot use the canonical/hot-reload path (not yet supported)".into(),
            ));
        }
    }
    routes
        .into_iter()
        .map(|r| compile_declarative_route_to_canonical(r, allow_loss))
        .collect()
}

fn yaml_source_to_value_source(
    yaml: crate::route_ast::RouteDslPermissionValueSource,
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

pub(crate) fn route_dsl_to_declarative_route(
    route: RouteDslRoute,
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
                                .map(route_step_to_declarative_step)
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
                                continued: clause.continued,
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
                use_original_message: eh.use_original_message,
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
                    trust_upstream_principal: sp.trust_upstream_principal.unwrap_or(false),
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
                    trust_upstream_principal: sp.trust_upstream_principal.unwrap_or(false),
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
        .map(route_step_to_declarative_step)
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

/// Parse a `ResequenceCompletionYaml` into a `BatchCompletion`.
/// Exactly one of `size`, `timeout`, or `size_or_timeout` must be set.
fn parse_resequence_completion(
    c: &crate::route_ast::ResequenceCompletionYaml,
) -> Result<camel_api::resequencer::BatchCompletion, CamelError> {
    use camel_api::resequencer::BatchCompletion;
    match (c.size, c.timeout, &c.size_or_timeout) {
        (Some(s), None, None) => {
            if s == 0 {
                return Err(CamelError::ValidationError(
                    "resequence: batch completion size must be > 0".into(),
                ));
            }
            Ok(BatchCompletion::Size(s))
        }
        (None, Some(t), None) => {
            if t == 0 {
                return Err(CamelError::ValidationError(
                    "resequence: batch completion timeout must be > 0".into(),
                ));
            }
            Ok(BatchCompletion::Timeout(t))
        }
        (None, None, Some(v)) => {
            if v.len() != 2 {
                return Err(CamelError::ValidationError(format!(
                    "resequence: size_or_timeout must be [size, timeout_ms], got {} elements",
                    v.len()
                )));
            }
            let size = v[0] as usize;
            let timeout = v[1];
            if size == 0 || timeout == 0 {
                return Err(CamelError::ValidationError(
                    "resequence: both size and timeout in size_or_timeout must be > 0".into(),
                ));
            }
            Ok(BatchCompletion::SizeOrTimeout(size, timeout))
        }
        _ => Err(CamelError::ValidationError(
            "resequence: completion must specify exactly one of 'size', 'timeout', or 'size_or_timeout'".into(),
        )),
    }
}

fn parse_multicast_aggregation(s: &str) -> Result<MulticastAggregationDef, CamelError> {
    match s {
        "last_wins" => Ok(MulticastAggregationDef::LastWins),
        "collect_all" => Ok(MulticastAggregationDef::CollectAll),
        "original" => Ok(MulticastAggregationDef::Original),
        other => Err(CamelError::ValidationError(format!(
            "unknown aggregation strategy: {other}"
        ))),
    }
}

pub(crate) fn route_step_to_declarative_step(
    step: RouteDslStep,
) -> Result<DeclarativeStep, CamelError> {
    match step {
        RouteDslStep::To(ToStep { to }) => {
            if to.trim().is_empty() {
                return Err(CamelError::RouteError("to: URI must not be empty".into()));
            }
            Ok(DeclarativeStep::To(ToStepDef::new(to)))
        }
        RouteDslStep::WireTap(WireTapStep { wire_tap }) => {
            if wire_tap.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "wire_tap: URI must not be empty".into(),
                ));
            }
            Ok(DeclarativeStep::WireTap(WireTapStepDef { uri: wire_tap }))
        }
        RouteDslStep::Stop(StopStep { stop }) => {
            if stop {
                Ok(DeclarativeStep::Stop)
            } else {
                Err(CamelError::RouteError(
                    "'stop: false' is invalid; remove the step or use 'stop: true'".into(),
                ))
            }
        }
        RouteDslStep::StreamCache(step) => {
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
        RouteDslStep::Log(LogStep { log }) => {
            let (message_data, level) = match log {
                crate::route_ast::LogBody::Message(message) => (
                    crate::route_ast::LogMessageData::Literal(message),
                    LogLevelDef::Info,
                ),
                crate::route_ast::LogBody::Config(config) => {
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
                crate::route_ast::LogMessageData::Literal(s) => {
                    ValueSourceDef::Expression(LanguageExpressionDef {
                        language: "simple".to_string(),
                        source: s,
                    })
                }
                crate::route_ast::LogMessageData::Expr(expr) => parse_value_source(
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
        RouteDslStep::SetHeader(SetHeaderStep { set_header }) => {
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
        RouteDslStep::SetProperty(SetPropertyStep { set_property }) => {
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
        RouteDslStep::SetBody(SetBodyStep { set_body }) => {
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
        RouteDslStep::Transform(TransformStep { transform }) => {
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
        RouteDslStep::Script(ScriptStep {
            script: ScriptData { language, source },
        }) => Ok(DeclarativeStep::Script(ScriptStepDef {
            expression: LanguageExpressionDef { language, source },
        })),
        RouteDslStep::Filter(FilterStep { filter }) => {
            let predicate = parse_predicate_block(&filter, "filter")?;
            let steps = filter
                .steps
                .into_iter()
                .map(route_step_to_declarative_step)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DeclarativeStep::Filter(crate::model::FilterStepDef {
                predicate,
                steps,
            }))
        }
        RouteDslStep::Function(FunctionStep { function: data }) => {
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
        RouteDslStep::Choice(ChoiceStep {
            choice: ChoiceData { when, otherwise },
        }) => {
            let whens = when
                .into_iter()
                .map(|block| {
                    let predicate = parse_predicate_block(&block, "choice.when")?;
                    let steps = block
                        .steps
                        .into_iter()
                        .map(route_step_to_declarative_step)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(WhenStepDef { predicate, steps })
                })
                .collect::<Result<Vec<_>, CamelError>>()?;

            let otherwise = match otherwise {
                Some(steps) => Some(
                    steps
                        .into_iter()
                        .map(route_step_to_declarative_step)
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                None => None,
            };

            Ok(DeclarativeStep::Choice(ChoiceStepDef { whens, otherwise }))
        }
        RouteDslStep::Split(SplitStep { split }) => {
            let expression = if split.streaming {
                let stream_cfg = split.stream.unwrap_or_default();
                let config = StreamSplitConfig {
                    format: match stream_cfg.format.as_deref() {
                        Some("ndjson") => StreamSplitFormat::Ndjson,
                        Some("lines") => StreamSplitFormat::Lines,
                        Some("chunks") => StreamSplitFormat::Chunks,
                        Some("zip") => StreamSplitFormat::Zip,
                        Some("auto") | None => StreamSplitFormat::Auto,
                        Some(other) => {
                            return Err(CamelError::RouteError(format!(
                                "unsupported stream.format '{other}'"
                            )));
                        }
                    },
                    max_record_bytes: stream_cfg.max_record_bytes.unwrap_or(1024 * 1024),
                    batch_size: stream_cfg.batch_size.unwrap_or(1),
                    chunk_size: stream_cfg.chunk_size,
                    include_origin: true,
                };
                config.validate()?;
                SplitExpressionDef::Stream(config)
            } else {
                match split.expression {
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
                .map(route_step_to_declarative_step)
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
        RouteDslStep::Aggregate(AggregateStep { aggregate }) => {
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
        RouteDslStep::Multicast(MulticastStep { multicast }) => {
            let aggregation = parse_multicast_aggregation(multicast.aggregation.as_str())?;

            let steps = multicast
                .steps
                .into_iter()
                .map(route_step_to_declarative_step)
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
        // ponytail: scatter_gather is pure DSL sugar over Multicast — lowers directly,
        // no new DeclarativeStep variant, no new processor.
        RouteDslStep::ScatterGather(step) => {
            let aggregation =
                parse_multicast_aggregation(step.scatter_gather.aggregation.as_str())?;

            if step.scatter_gather.endpoints.is_empty() {
                return Err(CamelError::ValidationError(
                    "scatter_gather requires at least one endpoint".into(),
                ));
            }

            let steps: Vec<DeclarativeStep> = step
                .scatter_gather
                .endpoints
                .into_iter()
                .map(|endpoint| {
                    if endpoint.trim().is_empty() {
                        return Err(CamelError::RouteError(
                            "scatter_gather: endpoint URI must not be empty".into(),
                        ));
                    }
                    Ok(DeclarativeStep::To(ToStepDef::new(endpoint)))
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok(DeclarativeStep::Multicast(MulticastStepDef {
                steps,
                parallel: true,
                parallel_limit: None,
                stop_on_exception: false,
                timeout_ms: None,
                aggregation,
            }))
        }
        RouteDslStep::ConvertBodyTo(step) => {
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
        RouteDslStep::Marshal(MarshalStep { marshal, config }) => {
            if marshal.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "marshal: format must not be empty".into(),
                ));
            }
            Ok(DeclarativeStep::Marshal(DataFormatDef {
                format: marshal,
                schema: None,
                config,
            }))
        }
        RouteDslStep::Unmarshal(UnmarshalStep {
            unmarshal,
            schema,
            config,
        }) => {
            if unmarshal.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "unmarshal: format must not be empty".into(),
                ));
            }
            Ok(DeclarativeStep::Unmarshal(DataFormatDef {
                format: unmarshal,
                schema,
                config,
            }))
        }
        RouteDslStep::Bean(BeanStep {
            bean: BeanStepData { name, method },
        }) => {
            if name.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "bean: name must not be empty".into(),
                ));
            }
            Ok(DeclarativeStep::Bean(BeanStepDef::new(name, method)))
        }
        RouteDslStep::DynamicRouter(DynamicRouterStep {
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
        RouteDslStep::LoadBalance(LoadBalanceStep {
            load_balance:
                LoadBalanceData {
                    strategy,
                    distribution_ratio,
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
                .map(route_step_to_declarative_step)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DeclarativeStep::LoadBalance(LoadBalanceStepDef {
                strategy,
                steps,
            }))
        }
        RouteDslStep::RoutingSlip(RoutingSlipStep {
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
        RouteDslStep::RecipientList(RecipientListStep {
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
                None => MulticastAggregationDef::LastWins,
                Some(s) => parse_multicast_aggregation(s)?,
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
        RouteDslStep::Throttle(ThrottleStep {
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
                .map(route_step_to_declarative_step)
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
        RouteDslStep::Delay(DelayStep { delay }) => {
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
        RouteDslStep::Loop(LoopStep { loop_data }) => {
            let (count, while_predicate, steps, max_iterations) = match loop_data {
                LoopData::Count(n) => (Some(n), None, vec![], None),
                LoopData::Full(cfg) => {
                    let predicate = match &cfg.while_expr {
                        Some(expr) => Some(parse_loop_while_expr(expr, "loop.while")?),
                        None => None,
                    };
                    let sub_steps = cfg
                        .steps
                        .into_iter()
                        .map(route_step_to_declarative_step)
                        .collect::<Result<Vec<_>, _>>()?;
                    (cfg.count, predicate, sub_steps, cfg.max_iterations)
                }
            };
            Ok(DeclarativeStep::Loop(LoopStepDef {
                count,
                while_predicate,
                steps,
                max_iterations,
            }))
        }
        RouteDslStep::Validate(ValidateStep { validate }) => {
            let expr = LanguageExpressionDef {
                language: "simple".into(),
                source: validate,
            };
            Ok(DeclarativeStep::Validate(ValidateStepDef {
                predicate: expr,
            }))
        }
        RouteDslStep::Enrich(EnrichStep { enrich }) => {
            let (uri, strategy) = unpack_enrich_body(enrich)?;
            Ok(DeclarativeStep::Enrich(EnrichStepDef {
                uri,
                strategy,
                timeout_ms: None,
            }))
        }
        RouteDslStep::PollEnrich(PollEnrichStep { poll_enrich }) => {
            let (uri, strategy, timeout) = unpack_poll_enrich_body(poll_enrich)?;
            Ok(DeclarativeStep::PollEnrich(EnrichStepDef {
                uri,
                strategy,
                timeout_ms: timeout,
            }))
        }
        RouteDslStep::IdempotentConsumer(IdempotentConsumerStep {
            idempotent_consumer: body,
        }) => {
            if body.repository.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "idempotent_consumer: 'repository' must not be empty".into(),
                ));
            }
            if body.expression.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "idempotent_consumer: 'expression' must not be empty".into(),
                ));
            }
            let steps = body
                .steps
                .into_iter()
                .map(route_step_to_declarative_step)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DeclarativeStep::IdempotentConsumer(
                IdempotentConsumerStepDef {
                    repository: body.repository,
                    expression: LanguageExpressionDef {
                        language: "simple".into(),
                        source: body.expression,
                    },
                    steps,
                    eager: body.eager,
                    remove_on_failure: body.remove_on_failure,
                },
            ))
        }
        RouteDslStep::ClaimCheck(ClaimCheckStep {
            claim_check:
                ClaimCheckBody {
                    repository,
                    operation,
                    key,
                    filter,
                },
        }) => {
            if repository.trim().is_empty() {
                return Err(CamelError::ValidationError(
                    "claim_check: repository is required".into(),
                ));
            }
            if operation.trim().is_empty() {
                return Err(CamelError::ValidationError(
                    "claim_check: operation is required".into(),
                ));
            }
            if key.trim().is_empty() {
                return Err(CamelError::ValidationError(
                    "claim_check: key is required".into(),
                ));
            }
            match operation.as_str() {
                "set" | "get" | "get_and_remove" | "push" | "pop" => {}
                other => {
                    return Err(CamelError::ValidationError(format!(
                        "claim_check: unknown operation '{other}'"
                    )));
                }
            }
            Ok(DeclarativeStep::ClaimCheck(ClaimCheckStepDef {
                repository,
                operation,
                key: LanguageExpressionDef {
                    language: "simple".into(),
                    source: key,
                },
                filter,
            }))
        }
        RouteDslStep::Sampling(SamplingStep { sampling }) => {
            let period = match sampling {
                SamplingBody::Short(n) => n,
                SamplingBody::Full(cfg) => cfg.period,
            };
            if period == 0 {
                return Err(CamelError::ValidationError(
                    "sampling: period must be > 0".into(),
                ));
            }
            Ok(DeclarativeStep::Sampling(SamplingStepDef { period }))
        }
        RouteDslStep::Sort(SortStep {
            sort:
                SortBody {
                    expression,
                    reverse,
                    language,
                },
        }) => {
            let lang = language.unwrap_or_else(|| "simple".to_string());
            if expression.trim().is_empty() {
                return Err(CamelError::ValidationError(
                    "sort: expression is required".into(),
                ));
            }
            Ok(DeclarativeStep::Sort(SortStepDef {
                expression: LanguageExpressionDef {
                    language: lang,
                    source: expression,
                },
                reverse,
            }))
        }
        RouteDslStep::Resequence(ResequenceStep {
            resequence: ResequenceData { batch, stream },
        }) => {
            let mode = match (batch, stream) {
                (Some(b), None) => {
                    let completion = parse_resequence_completion(&b.completion)?;
                    ResequenceModeDef::Batch {
                        correlation: b.correlation,
                        sort: b.sort,
                        completion,
                    }
                }
                (None, Some(s)) => ResequenceModeDef::Stream {
                    sequence: s.sequence,
                    capacity: s.capacity,
                    gap_timeout: s.gap_timeout,
                    on_gap: match s.on_gap {
                        StreamGapPolicyYaml::EmitPartial => camel_api::GapPolicy::EmitPartial,
                        StreamGapPolicyYaml::DropAndLog => camel_api::GapPolicy::DropAndLog,
                    },
                    on_capacity_exceeded: match s.on_capacity_exceeded {
                        StreamCapacityPolicyYaml::LogAndDrop => {
                            camel_api::CapacityPolicy::LogAndDrop
                        }
                        StreamCapacityPolicyYaml::DropOldest => {
                            camel_api::CapacityPolicy::DropOldest
                        }
                    },
                    dedup: s.dedup,
                },
                (Some(_), Some(_)) => {
                    return Err(CamelError::ValidationError(
                        "resequence: must specify exactly one of 'batch' or 'stream'".into(),
                    ));
                }
                (None, None) => {
                    return Err(CamelError::ValidationError(
                        "resequence: must specify 'batch' or 'stream'".into(),
                    ));
                }
            };
            Ok(DeclarativeStep::Resequence(ResequenceStepDef { mode }))
        }
        RouteDslStep::SetHeaderIfAbsent(SetHeaderStep { set_header }) => {
            if set_header.key.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "set_header_if_absent: key must not be empty".into(),
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
                "set_header_if_absent",
            )?;
            Ok(DeclarativeStep::SetHeaderIfAbsent(SetHeaderStepDef {
                key: set_header.key,
                value,
            }))
        }
        RouteDslStep::DoTry(DoTryStep { do_try: data }) => {
            // Spec §7.2 Rule 4: try steps must be non-empty.
            if data.steps.is_empty() {
                return Err(CamelError::Config("doTry `steps` cannot be empty".into()));
            }

            let steps = data
                .steps
                .into_iter()
                .map(route_step_to_declarative_step)
                .collect::<Result<Vec<_>, _>>()?;
            let catch = data
                .catch
                .into_iter()
                .map(|c| {
                    // Reject unsupported Continued disposition at parse time (spec §3 Non-Goal).
                    if matches!(c.disposition, ExceptionDisposition::Continued) {
                        return Err(CamelError::Config(
                            "doTry catch clause uses unsupported disposition `Continued`; \
                             only Handled and Propagate are supported in MVP"
                                .into(),
                        ));
                    }
                    // Spec §7.2 Rule 1: matcher exclusivity — must have exactly one main matcher.
                    if c.exception.is_some() && c.when.is_some() {
                        return Err(CamelError::Config(
                            "doTry catch clause cannot specify both `exception` and `when`".into(),
                        ));
                    }
                    if c.exception.is_none() && c.when.is_none() {
                        return Err(CamelError::Config(
                            "doTry catch clause must specify exactly one of `exception` or `when`"
                                .into(),
                        ));
                    }
                    // Spec §7.2 Rule 2: `on_when` is allowed ONLY when `exception` is the main
                    // matcher. `on_when` alongside `when` (alone) is a hard error.
                    if c.on_when.is_some() && c.exception.is_none() {
                        return Err(CamelError::Config(
                            "doTry catch clause `on_when` requires `exception` as the main matcher \
                             (not `when`)"
                                .into(),
                        ));
                    }
                    // Spec §7.2 Rule 3: `"*"` isolation — cannot mix with other variant names.
                    if let Some(ref names) = c.exception
                        && names.iter().any(|n| n == "*") && names.len() > 1
                    {
                        return Err(CamelError::Config(
                            "doTry catch clause `exception: ['*']` cannot mix with other variant names"
                                .into(),
                        ));
                    }
                    // Reject empty/blank exception variant names — silent never-matches otherwise.
                    if let Some(ref names) = c.exception {
                        if names.is_empty() {
                            return Err(CamelError::Config(
                                "doTry catch clause `exception` cannot be an empty list".into(),
                            ));
                        }
                        if names.iter().any(|n| n.trim().is_empty()) {
                            return Err(CamelError::Config(
                                "doTry catch clause `exception` cannot contain blank variant names"
                                    .into(),
                            ));
                        }
                    }
                    // Spec §7.2 Rule 4: catch clause steps must be non-empty.
                    if c.steps.is_empty() {
                        return Err(CamelError::Config(
                            "doTry catch clause `steps` cannot be empty".into(),
                        ));
                    }

                    let exception = c.exception;
                    let when = c.when.map(|s| LanguageExpressionDef {
                        language: "simple".into(),
                        source: s,
                    });
                    let on_when = c.on_when.map(|s| LanguageExpressionDef {
                        language: "simple".into(),
                        source: s,
                    });
                    let clause_steps = c
                        .steps
                        .into_iter()
                        .map(route_step_to_declarative_step)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(DoTryCatchClauseDef {
                        exception,
                        when,
                        on_when,
                        disposition: c.disposition,
                        steps: clause_steps,
                    })
                })
                .collect::<Result<Vec<_>, CamelError>>()?;
            let finally = if let Some(f) = data.finally {
                // Spec §7.2 Rule 4: finally steps must be non-empty.
                if f.steps.is_empty() {
                    return Err(CamelError::Config(
                        "doTry `finally.steps` cannot be empty".into(),
                    ));
                }
                let on_when = f.on_when.map(|s| LanguageExpressionDef {
                    language: "simple".into(),
                    source: s,
                });
                let fsteps = f
                    .steps
                    .into_iter()
                    .map(route_step_to_declarative_step)
                    .collect::<Result<Vec<_>, _>>()?;
                Some(DoTryFinallyDef {
                    on_when,
                    steps: fsteps,
                })
            } else {
                None
            };
            Ok(DeclarativeStep::DoTry {
                steps,
                catch,
                finally,
            })
        }
    }
}

fn unpack_enrich_body(body: EnrichBody) -> Result<(String, Option<String>), CamelError> {
    match body {
        EnrichBody::Uri(uri) => {
            if uri.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "enrich: URI must not be empty".into(),
                ));
            }
            Ok((uri, None))
        }
        EnrichBody::Full(config) => {
            if config.uri.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "enrich: URI must not be empty".into(),
                ));
            }
            Ok((config.uri, config.strategy))
        }
    }
}

fn unpack_poll_enrich_body(
    body: EnrichBody,
) -> Result<(String, Option<String>, Option<u64>), CamelError> {
    match body {
        EnrichBody::Uri(uri) => {
            if uri.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "poll_enrich: URI must not be empty".into(),
                ));
            }
            Ok((uri, None, None))
        }
        EnrichBody::Full(config) => {
            if config.uri.trim().is_empty() {
                return Err(CamelError::RouteError(
                    "poll_enrich: URI must not be empty".into(),
                ));
            }
            Ok((config.uri, config.strategy, config.timeout))
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

use crate::util::read_route_file_capped;

pub fn load_from_file(path: &Path) -> Result<Vec<RouteDefinition>, CamelError> {
    info!(path = %path.display(), "loading routes from file");
    let content = read_route_file_capped(path).map_err(|e| {
        // log-policy: system-broken
        error!(path = %path.display(), error = %e, "failed to load routes from file");
        e
    })?;
    let annotated = annotate_format(InputFormat::Yaml, parse_yaml_inner(&content));
    annotated.map_err(|e| match e {
        CamelError::RouteError(msg) => {
            CamelError::RouteError(format!("{msg} (in {})", path.display()))
        }
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

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
    fn test_parse_yaml_continued_true() {
        let yaml = r#"
routes:
  - id: "continued-route"
    from: "direct:start"
    steps:
      - to: "log:out"
    error_handler:
      dead_letter_channel: "log:dlq"
      on_exceptions:
        - kind: "ProcessorError"
          continued: true
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let eh = routes[0]
            .error_handler
            .as_ref()
            .expect("error handler should be present");
        let on_ex = eh
            .on_exceptions
            .as_ref()
            .expect("on_exceptions should be present");
        assert_eq!(on_ex[0].continued, Some(true));
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
        let routes = parse_yaml_to_canonical(yaml, false).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].0.route_id, "canonical-v1");
        assert_eq!(routes[0].0.from, "direct:start");
        assert_eq!(routes[0].0.version, 2);
        assert_eq!(routes[0].0.steps.len(), 3);
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

        let routes = parse_yaml_to_canonical(yaml, false).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].0.route_id, "canonical-v1-advanced");
        assert_eq!(routes[0].0.steps.len(), 6);
        let cb = routes[0]
            .0
            .circuit_breaker
            .as_ref()
            .expect("circuit breaker should be present");
        assert_eq!(cb.failure_threshold, 4);
        assert_eq!(cb.open_duration_ms, 750);
    }

    #[test]
    fn test_parse_yaml_to_canonical_aggregate_completion_predicate() {
        let yaml = r#"
routes:
  - id: "canonical-aggregate-predicate"
    from: "direct:start"
    steps:
      - aggregate:
          header: orderId
          correlation_key: "${header.orderId}"
          completion_predicate:
            simple: "${body} == 'DONE'"
"#;
        let routes = parse_yaml_to_canonical(yaml, false).unwrap();
        assert_eq!(routes.len(), 1);
        let (route, _loss) = &routes[0];
        let agg = route
            .steps
            .iter()
            .find_map(|step| {
                if let camel_api::runtime::CanonicalStepSpec::Aggregate(spec) = step {
                    Some(spec)
                } else {
                    None
                }
            })
            .expect("expected an Aggregate step");
        assert!(
            agg.completion_predicate.is_some(),
            "completion_predicate must survive YAML -> canonical round-trip"
        );
        let pred = agg
            .completion_predicate
            .as_ref()
            .expect("predicate present");
        assert_eq!(pred.language, "simple");
        assert_eq!(pred.source, "${body} == 'DONE'");
    }

    #[test]
    fn test_parse_yaml_to_canonical_aggregate_no_completion_predicate() {
        let yaml = r#"
routes:
  - id: "canonical-aggregate-no-predicate"
    from: "direct:start"
    steps:
      - aggregate:
          header: orderId
          correlation_key: "${header.orderId}"
          completion_size: 2
"#;
        let routes = parse_yaml_to_canonical(yaml, false).unwrap();
        assert_eq!(routes.len(), 1);
        let (route, _loss) = &routes[0];
        let agg = route
            .steps
            .iter()
            .find_map(|step| {
                if let camel_api::runtime::CanonicalStepSpec::Aggregate(spec) = step {
                    Some(spec)
                } else {
                    None
                }
            })
            .expect("expected an Aggregate step");
        assert!(
            agg.completion_predicate.is_none(),
            "aggregate without completion_predicate must yield None"
        );
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
        let err = parse_yaml_to_canonical(yaml, false)
            .unwrap_err()
            .to_string();
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
                trust_upstream_principal,
            } => {
                assert_eq!(roles, &vec!["admin".to_string(), "superuser".to_string()]);
                assert!(!all_required);
                assert!(!trust_upstream_principal);
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
                trust_upstream_principal,
            } => {
                assert_eq!(scopes, &vec!["read:api".to_string()]);
                assert!(*all_required);
                assert!(!trust_upstream_principal);
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
    fn parse_security_policy_roles_trust_upstream_principal_default_false() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      roles: ["admin"]
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let sp = routes[0].security_policy.as_ref().unwrap();
        match sp {
            DeclarativeSecurityPolicy::Roles {
                trust_upstream_principal,
                ..
            } => {
                assert!(
                    !trust_upstream_principal,
                    "trust_upstream_principal must default to false (fail-closed)"
                );
            }
            _ => panic!("expected Roles"),
        }
    }

    #[test]
    fn parse_security_policy_roles_trust_upstream_principal_opt_in() {
        let yaml = r#"
routes:
  - id: r-sec
    from: direct:start
    security_policy:
      roles: ["admin"]
      trust_upstream_principal: true
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let sp = routes[0].security_policy.as_ref().unwrap();
        match sp {
            DeclarativeSecurityPolicy::Roles {
                trust_upstream_principal,
                ..
            } => {
                assert!(
                    *trust_upstream_principal,
                    "trust_upstream_principal must be true when set in YAML"
                );
            }
            _ => panic!("expected Roles"),
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
    steps:
      - to: log:info
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let sp = routes[0].security_policy.as_ref().unwrap();
        match sp {
            DeclarativeSecurityPolicy::Wasm { path, config } => {
                assert_eq!(path, "plugins/my-auth-policy.wasm");
                assert!(
                    config.is_empty(),
                    "per-route config must be empty; use Camel.toml"
                );
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
        let result = parse_yaml_to_canonical(yaml, false);
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
    fn test_parse_yaml_claim_check() {
        let yaml = r#"
routes:
  - id: "cc-test"
    from: "direct:in"
    steps:
      - claim_check:
          repository: memory
          operation: set
          key: "${header.claimKey}"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        let step = &routes[0].steps[0];
        assert!(
            matches!(step, DeclarativeStep::ClaimCheck(ClaimCheckStepDef {
            repository,
            operation,
            ..
        }) if repository == "memory" && operation == "set")
        );
    }

    #[test]
    fn test_parse_yaml_claim_check_filter_accepted() {
        let yaml = r#"
routes:
  - id: "cc-filter"
    from: "direct:in"
    steps:
      - claim_check:
          repository: memory
          operation: set
          key: "${header.claimKey}"
          filter: "someFilter"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let step = &routes[0].steps[0];
        match step {
            DeclarativeStep::ClaimCheck(def) => {
                assert_eq!(def.filter.as_deref(), Some("someFilter"));
            }
            other => panic!("expected ClaimCheck, got {other:?}"),
        }
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
            matches!(&steps[0], DeclarativeStep::Marshal(DataFormatDef { format, .. }) if format == "json")
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
            matches!(&steps[0], DeclarativeStep::Unmarshal(DataFormatDef { format, .. }) if format == "xml")
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
    fn validate_step_parses_to_validate_step_def() {
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
            matches!(step, DeclarativeStep::Validate(ValidateStepDef { predicate }) if predicate.language == "simple" && predicate.source == "schemas/order.xsd"),
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
        assert!(matches!(routes[0].steps[1], DeclarativeStep::Validate(_)));
    }

    #[test]
    fn idempotent_consumer_parses_to_step_def_with_defaults() {
        let yaml = r#"
routes:
  - id: idem-test
    from: "direct:in"
    steps:
      - idempotent_consumer:
          repository: memory
          expression: "${header.messageId}"
          steps:
            - to: "log:info"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let step = &routes[0].steps[0];
        match step {
            DeclarativeStep::IdempotentConsumer(def) => {
                assert_eq!(def.repository, "memory");
                assert_eq!(def.expression.language, "simple");
                assert_eq!(def.expression.source, "${header.messageId}");
                assert_eq!(def.steps.len(), 1);
                assert!(matches!(def.steps[0], DeclarativeStep::To(_)));
                assert_eq!(def.eager, None, "eager default is None");
                assert_eq!(
                    def.remove_on_failure, None,
                    "remove_on_failure default is None"
                );
            }
            other => panic!("expected IdempotentConsumer, got {other:?}"),
        }
    }

    #[test]
    fn idempotent_consumer_parses_eager_and_remove_on_failure_flags() {
        let yaml = r#"
routes:
  - id: idem-eager
    from: "direct:in"
    steps:
      - idempotent_consumer:
          repository: memory
          expression: "${header.messageId}"
          eager: true
          remove_on_failure: true
          steps:
            - to: "log:info"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        match &routes[0].steps[0] {
            DeclarativeStep::IdempotentConsumer(def) => {
                assert_eq!(def.eager, Some(true));
                assert_eq!(def.remove_on_failure, Some(true));
            }
            other => panic!("expected IdempotentConsumer, got {other:?}"),
        }
    }

    #[test]
    fn idempotent_consumer_rejects_empty_repository() {
        let yaml = r#"
routes:
  - id: idem-bad
    from: "direct:in"
    steps:
      - idempotent_consumer:
          repository: ""
          expression: "${header.messageId}"
          steps: []
"#;
        let err = parse_yaml_to_declarative(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("repository"),
            "expected error about repository, got: {msg}"
        );
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
        let err = parse_yaml_to_canonical(yaml, false)
            .unwrap_err()
            .to_string();
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

    // --- Streaming split tests ---

    #[test]
    fn test_streaming_true_with_format_produces_stream_def() {
        let yaml = r#"
routes:
  - id: "stream-split"
    from: "direct:start"
    steps:
      - split:
          streaming: true
          stream:
            format: ndjson
          steps:
            - to: "mock:out"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let step = &routes[0].steps[0];
        match step {
            DeclarativeStep::Split(def) => match &def.expression {
                SplitExpressionDef::Stream(config) => {
                    assert_eq!(
                        config.format,
                        camel_api::StreamSplitFormat::Ndjson,
                        "expected Ndjson format"
                    );
                    assert_eq!(config.max_record_bytes, 1024 * 1024);
                    assert_eq!(config.batch_size, 1);
                    assert_eq!(config.chunk_size, None);
                    assert!(config.include_origin);
                }
                other => panic!("expected Stream expression, got {other:?}"),
            },
            other => panic!("expected Split step, got {other:?}"),
        }
    }

    #[test]
    fn test_streaming_true_without_stream_uses_defaults() {
        let yaml = r#"
routes:
  - id: "stream-split-defaults"
    from: "direct:start"
    steps:
      - split:
          streaming: true
          steps:
            - to: "mock:out"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let step = &routes[0].steps[0];
        match step {
            DeclarativeStep::Split(def) => match &def.expression {
                SplitExpressionDef::Stream(config) => {
                    assert_eq!(
                        config.format,
                        camel_api::StreamSplitFormat::Auto,
                        "expected Auto format"
                    );
                    assert_eq!(config.max_record_bytes, 1024 * 1024);
                    assert_eq!(config.batch_size, 1);
                    assert!(config.include_origin);
                }
                other => panic!("expected Stream expression, got {other:?}"),
            },
            other => panic!("expected Split step, got {other:?}"),
        }
    }

    #[test]
    fn test_missing_streaming_defaults_false() {
        let yaml = r#"
routes:
  - id: "no-streaming"
    from: "direct:start"
    steps:
      - split:
          expression: body_lines
          steps:
            - to: "mock:out"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let step = &routes[0].steps[0];
        match step {
            DeclarativeStep::Split(def) => {
                assert_eq!(
                    def.expression,
                    SplitExpressionDef::BodyLines,
                    "expected BodyLines when streaming is not set"
                );
            }
            other => panic!("expected Split step, got {other:?}"),
        }
    }

    #[test]
    fn test_streaming_true_with_format_zip_produces_zip_def() {
        let yaml = r#"
routes:
  - id: "zip-split-test"
    from: "direct:start"
    steps:
      - split:
          streaming: true
          stream:
            format: zip
          steps:
            - to: "mock:out"
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let step = &routes[0].steps[0];
        match step {
            DeclarativeStep::Split(def) => match &def.expression {
                SplitExpressionDef::Stream(config) => {
                    assert_eq!(
                        config.format,
                        camel_api::StreamSplitFormat::Zip,
                        "expected Zip format"
                    );
                    assert_eq!(config.max_record_bytes, 1024 * 1024);
                    assert_eq!(config.batch_size, 1);
                    assert_eq!(config.chunk_size, None);
                    assert!(config.include_origin);
                }
                other => panic!("expected Stream expression, got {other:?}"),
            },
            other => panic!("expected Split step, got {other:?}"),
        }
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
        ratio: 3.5
        enabled: true
        nothing: null
"#;
        let specs = parse_yaml_templates(yaml).unwrap();
        assert_eq!(specs[0].routes[0]["count"], 42);
        assert_eq!(specs[0].routes[0]["ratio"], 3.5);
        assert_eq!(specs[0].routes[0]["enabled"], true);
        assert_eq!(specs[0].routes[0]["nothing"], serde_json::Value::Null);
    }

    // --- doTry parse tests ---

    #[test]
    fn do_try_parses_minimal_form() {
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        assert_eq!(steps.len(), 1);
        match &steps[0] {
            RouteDslStep::DoTry(d) => {
                assert_eq!(d.do_try.steps.len(), 1);
                assert!(d.do_try.catch.is_empty());
                assert!(d.do_try.finally.is_none());
            }
            other => panic!("expected DoTry, got {:?}", other),
        }
    }

    #[test]
    fn do_try_parses_full_form_with_catch_and_finally() {
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    catch:
      - exception: ["ProcessorError"]
        on_when: "${body} contains 'x'"
        disposition: propagate
        steps:
          - to: "log:caught"
      - when: "${exchangeProperty.CamelExceptionKind} == 'Io'"
        steps:
          - to: "log:io"
      - exception: ["*"]
        steps:
          - to: "log:all"
    finally:
      on_when: "${header.cleanup} == 'true'"
      steps:
        - to: "log:fin"
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        match &steps[0] {
            RouteDslStep::DoTry(d) => {
                assert_eq!(d.do_try.catch.len(), 3);
                let c0 = &d.do_try.catch[0];
                assert_eq!(
                    c0.exception.as_deref().unwrap(),
                    &["ProcessorError".to_string()][..]
                );
                assert!(c0.on_when.is_some());
                assert_eq!(c0.steps.len(), 1, "catch clause must have steps field");
                let c1 = &d.do_try.catch[1];
                assert!(c1.when.is_some());
                let c2 = &d.do_try.catch[2];
                assert_eq!(c2.exception.as_deref().unwrap(), &["*".to_string()][..]);
                let f = d.do_try.finally.as_ref().unwrap();
                assert!(f.on_when.is_some());
                assert_eq!(f.steps.len(), 1);
            }
            other => panic!("expected DoTry, got {:?}", other),
        }
    }

    #[test]
    fn do_try_rejects_continued_disposition_at_yaml_level() {
        let yaml = r#"
routes:
  - id: "test-continued-reject"
    from: "direct:start"
    steps:
      - do_try:
          steps:
            - to: "log:try"
          catch:
            - exception: ["ProcessorError"]
              disposition: continued
              steps:
                - to: "log:catch"
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err(), "Continued disposition should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unsupported disposition `Continued`"),
            "error should mention Continued rejection: {err}"
        );
    }

    #[test]
    fn do_try_defaults_to_handled_disposition() {
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    catch:
      - exception: ["*"]
        steps:
          - to: "log:catch"
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        match &steps[0] {
            RouteDslStep::DoTry(d) => {
                let c0 = &d.do_try.catch[0];
                assert_eq!(
                    c0.disposition,
                    camel_api::error_handler::ExceptionDisposition::Handled,
                    "default disposition should be Handled"
                );
            }
            other => panic!("expected DoTry, got {:?}", other),
        }
    }

    // ── Negative tests for spec §7.2 parse-time validation rules ──

    #[test]
    fn do_try_rejects_both_exception_and_when() {
        // Spec §7.2 Rule 1: matcher exclusivity — cannot have both `exception` and `when`.
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    catch:
      - exception: ["ProcessorError"]
        when: "${body} == 'x'"
        steps:
          - to: "log:caught"
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        let decl = route_step_to_declarative_step(steps.into_iter().next().unwrap());
        assert!(
            decl.is_err(),
            "must reject catch clause with both `exception` and `when`"
        );
    }

    #[test]
    fn do_try_rejects_on_when_without_matcher() {
        // Spec §7.2 Rule 1 + Rule 2: `on_when` requires `exception` as the main matcher;
        // no matcher at all is rejected by Rule 1.
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    catch:
      - on_when: "${body} == 'x'"
        steps:
          - to: "log:caught"
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        let decl = route_step_to_declarative_step(steps.into_iter().next().unwrap());
        assert!(
            decl.is_err(),
            "must reject catch clause with `on_when` but no matcher"
        );
    }

    #[test]
    fn do_try_rejects_star_mixed_with_other_variants() {
        // Spec §7.2 Rule 3: `"*"` isolation — cannot mix with other variant names.
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    catch:
      - exception: ["*", "ProcessorError"]
        steps:
          - to: "log:caught"
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        let decl = route_step_to_declarative_step(steps.into_iter().next().unwrap());
        assert!(
            decl.is_err(),
            "must reject catch clause with `*` mixed with other variants"
        );
    }

    #[test]
    fn do_try_rejects_empty_try_steps() {
        // Spec §7.2 Rule 4: try steps must be non-empty.
        let yaml = r#"
- do_try:
    steps: []
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        let decl = route_step_to_declarative_step(steps.into_iter().next().unwrap());
        assert!(decl.is_err(), "must reject doTry with empty `steps`");
    }

    #[test]
    fn do_try_rejects_empty_catch_steps() {
        // Spec §7.2 Rule 4: catch clause steps must be non-empty.
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    catch:
      - exception: ["*"]
        steps: []
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        let decl = route_step_to_declarative_step(steps.into_iter().next().unwrap());
        assert!(decl.is_err(), "must reject catch clause with empty `steps`");
    }

    #[test]
    fn do_try_rejects_empty_finally_steps() {
        // Spec §7.2 Rule 4: finally steps must be non-empty.
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    finally:
      steps: []
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        let decl = route_step_to_declarative_step(steps.into_iter().next().unwrap());
        assert!(decl.is_err(), "must reject finally with empty `steps`");
    }

    #[test]
    fn do_try_rejects_on_when_with_when_only() {
        // Spec §7.2 Rule 2: `on_when` allowed ONLY when `exception` is the main matcher.
        // `on_when` alongside `when` (alone) is a hard error.
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    catch:
      - when: "${body} == 'x'"
        on_when: "${header.flag} == true"
        steps:
          - to: "log:caught"
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        let decl = route_step_to_declarative_step(steps.into_iter().next().unwrap());
        assert!(
            decl.is_err(),
            "must reject catch clause with `on_when` when `when` (not `exception`) is the matcher"
        );
    }

    #[test]
    fn do_try_rejects_catch_clause_without_matcher() {
        // Spec §7.2 Rule 1: catch clause must have exactly one matcher (`exception` OR `when`).
        // Having neither is a hard error.
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    catch:
      - steps:
          - to: "log:caught"
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        let decl = route_step_to_declarative_step(steps.into_iter().next().unwrap());
        assert!(
            decl.is_err(),
            "must reject catch clause with neither `exception` nor `when`"
        );
    }

    #[test]
    fn do_try_rejects_empty_exception_list() {
        // An empty `exception: []` is a silent never-match; reject explicitly.
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    catch:
      - exception: []
        steps:
          - to: "log:caught"
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        let decl = route_step_to_declarative_step(steps.into_iter().next().unwrap());
        assert!(
            decl.is_err(),
            "must reject catch clause with empty `exception` list"
        );
    }

    #[test]
    fn do_try_rejects_blank_exception_variant_name() {
        // Blank variant names like `""` or `"  "` are silent never-matches; reject.
        let yaml = r#"
- do_try:
    steps:
      - to: "log:try"
    catch:
      - exception: [""]
        steps:
          - to: "log:caught"
"#;
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        let decl = route_step_to_declarative_step(steps.into_iter().next().unwrap());
        assert!(
            decl.is_err(),
            "must reject catch clause with blank variant name in `exception`"
        );
    }

    // ── YAML format annotation tests ──

    #[test]
    fn semantic_error_carries_yaml_format_prefix() {
        // A semantic error (empty route id) must be prefixed with "YAML DSL error:".
        let yaml = r#"
routes:
  - id: ""
    from: "timer:tick"
"#;
        let err = parse_yaml_to_declarative(yaml).unwrap_err().to_string();
        assert!(
            err.contains("YAML DSL error:"),
            "expected YAML DSL error prefix, got: {err}"
        );
        assert!(
            err.contains("must not be empty"),
            "expected original error message, got: {err}"
        );
    }

    #[test]
    fn yaml_parse_error_carries_format_prefix() {
        // A parse error from serde_yml must be wrapped with "YAML DSL error:".
        let yaml = r#"
routes:
  - id: [invalid
    from: "timer:tick"
"#;
        let err = match parse_yaml(yaml) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected parse error"),
        };
        assert!(
            err.contains("YAML DSL error:"),
            "expected YAML DSL error prefix, got: {err}"
        );
        assert!(
            err.contains("YAML parse error"),
            "expected original parse error message, got: {err}"
        );
    }

    #[test]
    fn scatter_gather_lowers_to_multicast() {
        let yaml = r#"
routes:
  - id: sg-route
    from: direct:start
    steps:
      - scatter_gather:
          endpoints:
            - direct:a
            - direct:b
            - direct:c
          aggregation: collect_all
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].steps.len(), 1);

        match &routes[0].steps[0] {
            DeclarativeStep::Multicast(def) => {
                assert_eq!(def.steps.len(), 3);
                assert!(matches!(def.steps[0], DeclarativeStep::To(_)));
                assert!(matches!(def.steps[1], DeclarativeStep::To(_)));
                assert!(matches!(def.steps[2], DeclarativeStep::To(_)));
                assert_eq!(def.aggregation, MulticastAggregationDef::CollectAll);
                assert!(def.parallel);
                assert!(def.parallel_limit.is_none());
                assert!(!def.stop_on_exception);
            }
            other => panic!("expected Multicast, got {:?}", other.kind()),
        }
    }

    #[test]
    fn scatter_gather_default_aggregation_is_last_wins() {
        let yaml = r#"
routes:
  - id: sg-route
    from: direct:start
    steps:
      - scatter_gather:
          endpoints:
            - direct:a
            - direct:b
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        match &routes[0].steps[0] {
            DeclarativeStep::Multicast(def) => {
                assert_eq!(def.aggregation, MulticastAggregationDef::LastWins);
            }
            other => panic!("expected Multicast, got {:?}", other.kind()),
        }
    }

    #[test]
    fn scatter_gather_rejects_empty_endpoint() {
        let yaml = r#"
routes:
  - id: sg-route
    from: direct:start
    steps:
      - scatter_gather:
          endpoints:
            - "  "
          aggregation: collect_all
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must not be empty")
        );
    }

    #[test]
    fn scatter_gather_rejects_bad_aggregation() {
        let yaml = r#"
routes:
  - id: sg-route
    from: direct:start
    steps:
      - scatter_gather:
          endpoints:
            - direct:a
          aggregation: bad_strategy
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unknown aggregation strategy")
        );
    }

    // ── REST expansion tests (Phase 1 Task 3) ──

    #[test]
    fn rest_block_expands_to_routes_in_parse_yaml_to_declarative() {
        let yaml = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /users
    operations:
      - method: GET
        path: /{id}
        operation_id: getUser
        to: bean:userService
      - method: POST
        operation_id: createUser
        to: bean:createUser
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 2);

        // First route: GET /users/{id}
        assert_eq!(routes[0].route_id, "getUser");
        assert!(routes[0].from.contains("httpMethod=GET"));
        assert!(routes[0].from.contains("0.0.0.0:8080"));
        assert!(routes[0].from.contains("/users/{id}"));

        // Second route: POST /users
        assert_eq!(routes[1].route_id, "createUser");
        assert!(routes[1].from.contains("httpMethod=POST"));
        assert!(routes[1].from.contains("0.0.0.0:8080"));
    }

    #[test]
    fn rest_block_expands_to_routes_in_parse_yaml() {
        let yaml = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /api
    operations:
      - method: GET
        path: /items
        operation_id: listItems
        to: bean:listHandler
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "listItems");
        assert!(defs[0].from_uri().contains("httpMethod=GET"));
    }

    #[test]
    fn rest_and_routes_together_expands_both() {
        let yaml = r#"
routes:
  - id: normal-route
    from: direct:start
    steps:
      - to: log:info
rest:
  - path: /api
    operations:
      - method: GET
        operation_id: getApi
        to: bean:handler
      - method: POST
        operation_id: postApi
        to: bean:creator
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        // 1 normal route + 2 REST-expanded routes = 3 total
        assert_eq!(routes.len(), 3);

        // First is the normal route (preserves order from YAML)
        assert_eq!(routes[0].route_id, "normal-route");
        assert_eq!(routes[0].from, "direct:start");

        // Then REST-expanded routes
        assert_eq!(routes[1].route_id, "getApi");
        assert_eq!(routes[2].route_id, "postApi");
    }

    #[test]
    fn rest_duplicate_operation_id_is_rejected() {
        let yaml = r#"
rest:
  - path: /users
    operations:
      - method: GET
        path: /{id}
        operation_id: duplicateOp
        to: bean:svc
  - path: /orders
    operations:
      - method: GET
        path: /{id}
        operation_id: duplicateOp
        to: bean:other
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("duplicate route id"),
            "expected duplicate route id error, got: {err}"
        );
        assert!(
            err.contains("duplicateOp"),
            "expected error to mention the duplicate id, got: {err}"
        );
    }

    #[test]
    fn rest_route_id_conflict_with_normal_route_is_rejected() {
        let yaml = r#"
routes:
  - id: conflictRoute
    from: direct:start
rest:
  - path: /api
    operations:
      - method: GET
        operation_id: conflictRoute
        to: bean:handler
"#;
        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("duplicate route id"),
            "expected duplicate route id error, got: {err}"
        );
    }

    #[test]
    fn rest_with_multiple_verbs_all_expanded() {
        let yaml = r#"
rest:
  - path: /items
    operations:
      - method: GET
        to: bean:list
      - method: POST
        operation_id: createItem
        to: bean:create
      - method: PUT
        operation_id: updateItem
        to: bean:update
      - method: DELETE
        operation_id: deleteItem
        to: bean:delete
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        assert_eq!(routes.len(), 4);
        // Declaration order (Vec), not alphabetical
        assert!(routes[0].from.contains("httpMethod=GET"));
        assert!(routes[1].from.contains("httpMethod=POST"));
        assert!(routes[2].from.contains("httpMethod=PUT"));
        assert!(routes[3].from.contains("httpMethod=DELETE"));
    }

    #[test]
    fn rest_op_without_method_fails_to_parse() {
        let yaml = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /api
    operations:
      - path: /health
        to: direct:a
"#;
        let err = parse_yaml_to_declarative(yaml).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("method"),
            "expected method-related parse error, got: {err}"
        );
    }

    #[test]
    fn extract_rest_blocks_returns_ast_without_lowering() {
        let yaml = r#"
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
routes:
  - id: dummy
    from: timer:tick
    steps:
      - to: log:info
"#;
        let blocks = extract_rest_blocks(yaml).expect("YAML should parse");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].path, "/api/users");
        assert_eq!(blocks[0].operations.len(), 1);
        assert_eq!(blocks[0].operations[0].method, "GET");
    }

    #[test]
    fn extract_rest_blocks_empty_when_no_rest() {
        let yaml = r#"
routes:
  - id: dummy
    from: timer:tick
    steps:
      - to: log:info
"#;
        let blocks = extract_rest_blocks(yaml).expect("YAML should parse");
        assert!(blocks.is_empty());
    }

    #[test]
    fn load_from_file_rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.yaml");
        std::fs::write(&path, "routes: []").unwrap();
        // Extend the file past the 16 MiB cap (sparse file, no actual disk usage)
        let file = File::create(&path).unwrap();
        file.set_len(16 * 1024 * 1024 + 1).unwrap();
        drop(file);

        let err = match load_from_file(&path) {
            Ok(_) => panic!("expected oversized file error"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("exceeds max"),
            "expected size cap error, got: {err}"
        );
    }

    #[test]
    fn parse_unmarshal_with_config() {
        let yaml = r#"
routes:
  - id: test-config
    from: "direct:start"
    steps:
      - unmarshal: json
        config:
          max_bytes: 67108864
"#;
        let routes = parse_yaml_to_declarative(yaml).unwrap();
        let steps = &routes[0].steps;
        match &steps[0] {
            DeclarativeStep::Unmarshal(def) => {
                assert_eq!(def.format, "json");
                assert!(def.config.is_some());
                assert_eq!(def.config.as_ref().unwrap()["max_bytes"], 67108864);
            }
            _ => panic!("expected Unmarshal step"),
        }
    }

    #[test]
    fn set_header_if_absent_not_deserializable() {
        // A set_header-shaped input MUST deserialize to SetHeader, never
        // SetHeaderIfAbsent. Since RouteDslStep is #[serde(untagged)] and
        // SetHeaderIfAbsent has #[serde(skip_deserializing)], it is excluded
        // from the candidate list — this input can only match SetHeader.
        let yaml = "- set_header:\n    key: test\n    value: 200\n";
        let steps: Vec<RouteDslStep> = serde_yml::from_str(yaml).unwrap();
        assert!(
            matches!(steps[0], RouteDslStep::SetHeader(_)),
            "set_header input must deserialize to SetHeader, not SetHeaderIfAbsent"
        );
    }

    #[test]
    fn set_header_if_absent_yaml_key_rejected() {
        // A YAML input using the literal key `set_header_if_absent` must
        // fail deserialisation — the variant is skip_deserializing on an
        // untagged enum, so no candidate matches this shape.
        let yaml = "- set_header_if_absent:\n    name: CamelHttpResponseCode\n    value: 200\n";
        let result: Result<Vec<RouteDslStep>, _> = serde_yml::from_str(yaml);
        assert!(
            result.is_err(),
            "set_header_if_absent YAML key must be rejected at deserialization"
        );
    }
}
