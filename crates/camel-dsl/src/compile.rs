use std::time::Duration;

const DEFAULT_FUNCTION_TIMEOUT_MS: u64 = 5000;

use camel_api::aggregator::{AggregationStrategy as AggregatorStrategy, AggregatorConfig};
use camel_api::body_converter::BodyType;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::multicast::{MulticastConfig, MulticastStrategy};
use camel_api::splitter::{
    AggregationStrategy as SplitAggregation, SplitterConfig, split_body_json_array,
    split_body_lines,
};
use camel_api::{
    CamelError, CanonicalConcurrencySpec, CanonicalFieldLoss, CanonicalLossReport,
    CanonicalRouteSpec, CircuitBreakerConfig, DelayConfig, IdentityProcessor, LoadBalanceStrategy,
    LoadBalancerConfig, ThrottleStrategy, ThrottlerConfig, canonical_contract_rejection_reason,
    runtime::{
        CanonicalAggregateSpec, CanonicalAggregateStrategySpec, CanonicalCircuitBreakerSpec,
        CanonicalSplitAggregationSpec, CanonicalSplitExpressionSpec, CanonicalStepSpec,
        CanonicalWhenSpec,
    },
};
use camel_component_api::ConcurrencyModel;
use camel_core::route::{
    BuilderStep, CompiledStep, DeclarativeWhenStep, DoTryCatchClauseBuilder, DoTryFinallyBuilder,
    RouteDefinition, compose_pipeline,
};
use camel_processor::{
    ConvertBodyTo, LogLevel, MarshalService, StopService, StreamCacheService, UnmarshalService,
    builtin_data_format,
};

use crate::model::{
    AggregateStepDef, AggregateStrategyDef, BeanStepDef, BodyTypeDef, ChoiceStepDef, DataFormatDef,
    DeclarativeCircuitBreaker, DeclarativeConcurrency, DeclarativeErrorHandler,
    DeclarativeRedeliveryPolicy, DeclarativeRoute, DeclarativeStep, DelayStepDef,
    DynamicRouterStepDef, FunctionStepDef, LanguageExpressionDef, LoadBalanceStepDef,
    LoadBalanceStrategyDef, LogLevelDef, LogStepDef, LoopStepDef, MulticastAggregationDef,
    MulticastStepDef, RecipientListStepDef, RoutingSlipStepDef, ScriptStepDef,
    SecurityCompileContext, SetBodyStepDef, SetHeaderStepDef, SetPropertyStepDef,
    SplitAggregationDef, SplitExpressionDef, SplitStepDef, ThrottleStepDef, ThrottleStrategyDef,
    ToStepDef, ValueSourceDef, WireTapStepDef,
};

fn require_authenticator(
    ctx: &SecurityCompileContext,
    mode: &str,
) -> Result<std::sync::Arc<dyn camel_auth::TokenAuthenticator>, CamelError> {
    ctx.authenticator.clone().ok_or_else(|| {
        CamelError::RouteError(format!(
            "security_policy with {} requires a JWT authenticator (configure [security] in Camel.toml)",
            mode
        ))
    })
}

fn compile_security_policy(
    def: crate::model::DeclarativeSecurityPolicy,
    ctx: &SecurityCompileContext,
) -> Result<
    (
        camel_api::security_policy::SecurityPolicyConfig,
        Option<std::sync::Arc<dyn camel_auth::TokenAuthenticator>>,
    ),
    CamelError,
> {
    match def {
        crate::model::DeclarativeSecurityPolicy::Roles {
            roles,
            all_required,
        } => {
            let auth = require_authenticator(ctx, "roles")?;
            let policy =
                camel_auth::RolePolicy::new(roles, all_required, std::sync::Arc::clone(&auth));
            Ok((
                camel_api::security_policy::SecurityPolicyConfig::new(policy),
                Some(auth),
            ))
        }
        crate::model::DeclarativeSecurityPolicy::Scopes {
            scopes,
            all_required,
        } => {
            let auth = require_authenticator(ctx, "scopes")?;
            let policy =
                camel_auth::ScopePolicy::new(scopes, all_required, std::sync::Arc::clone(&auth));
            Ok((
                camel_api::security_policy::SecurityPolicyConfig::new(policy),
                Some(auth),
            ))
        }
        crate::model::DeclarativeSecurityPolicy::Ref { name } => {
            let registry = ctx.registry.as_ref().ok_or_else(|| {
                CamelError::RouteError(
                    "security_policy with ref requires a SecurityPolicyRegistry".into(),
                )
            })?;
            let policy = registry.get(&name).ok_or_else(|| {
                CamelError::RouteError(format!(
                    "security_policy ref '{}' not found in SecurityPolicyRegistry",
                    name
                ))
            })?;
            Ok((
                camel_api::security_policy::SecurityPolicyConfig::from_arc(policy),
                None,
            ))
        }
        crate::model::DeclarativeSecurityPolicy::Wasm { path, config } => {
            if !config.is_empty() {
                return Err(CamelError::RouteError(
                    "security_policy.wasm.config cannot be set per-route; \
                     configure it in [security.policies.wasm.<name>.config] \
                     in Camel.toml — ADR-0014 §4"
                        .into(),
                ));
            }
            let registry = ctx.registry.as_ref().ok_or_else(|| {
                CamelError::RouteError(
                    "security_policy with wasm requires a SecurityPolicyRegistry".into(),
                )
            })?;
            let policy = registry.get(&path).ok_or_else(|| {
                CamelError::RouteError(format!(
                    "security_policy with wasm: '{}' not found in registry",
                    path
                ))
            })?;
            Ok((
                camel_api::security_policy::SecurityPolicyConfig::from_arc(policy),
                None,
            ))
        }
        crate::model::DeclarativeSecurityPolicy::Permission {
            policy,
            resource,
            action,
            scopes,
            context,
            cache_ttl_secs,
            cache_negative_ttl_secs,
        } => {
            let eval_reg = ctx.evaluator_registry.as_ref().ok_or_else(|| {
                CamelError::RouteError(
                    "security_policy with permission requires a PermissionEvaluatorRegistry".into(),
                )
            })?;
            let evaluator_arc = eval_reg.get(&policy).ok_or_else(|| {
                CamelError::RouteError(format!(
                    "security_policy permission policy '{}' not found in PermissionEvaluatorRegistry",
                    policy
                ))
            })?;
            let mut cache_opts = camel_auth::PermissionCacheOptions::default();
            if let Some(ttl) = cache_ttl_secs {
                cache_opts.positive_ttl = Duration::from_secs(ttl);
            }
            if let Some(ttl) = cache_negative_ttl_secs {
                cache_opts.negative_ttl = Duration::from_secs(ttl);
            }
            let cached = camel_auth::CachingPermissionEvaluator::new(evaluator_arc, cache_opts);
            let policy = camel_auth::PermissionPolicy::new(
                std::sync::Arc::new(cached),
                resource,
                action,
                scopes,
                context,
            );
            Ok((
                camel_api::security_policy::SecurityPolicyConfig::from_arc(std::sync::Arc::new(
                    policy,
                )),
                None,
            ))
        }
    }
}

pub fn compile_declarative_route(route: DeclarativeRoute) -> Result<RouteDefinition, CamelError> {
    compile_declarative_route_with_stream_cache_threshold(
        route,
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    )
}

pub fn compile_declarative_route_with_stream_cache_threshold(
    route: DeclarativeRoute,
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
) -> Result<RouteDefinition, CamelError> {
    validate_route(&route)?;
    let steps = compile_declarative_steps(route.steps, stream_cache_threshold)?;

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

    if let Some(sp) = route.security_policy {
        let (config, authenticator) = compile_security_policy(sp, &security_ctx)?;
        definition = definition.with_security_policy(config);
        if let Some(auth) = authenticator {
            definition = definition.with_security_authenticator(auth);
        }
    }

    if let Some(uow) = route.unit_of_work {
        definition = definition.with_unit_of_work(uow);
    }

    Ok(definition)
}

pub fn compile_declarative_route_to_canonical(
    route: DeclarativeRoute,
    allow_loss: bool,
) -> Result<(CanonicalRouteSpec, Option<CanonicalLossReport>), CamelError> {
    validate_route(&route)?;
    if route.security_policy.is_some() {
        return Err(CamelError::RouteError(
            "routes with security_policy cannot use the canonical/hot-reload path (not yet supported)".into(),
        ));
    }
    if !allow_loss {
        if route.error_handler.is_some() {
            return Err(CamelError::RouteError(
                "routes with error_handler cannot use the canonical/hot-reload path (not yet supported)".into(),
            ));
        }
        if route.unit_of_work.is_some() {
            return Err(CamelError::RouteError(
                "routes with unit_of_work cannot use the canonical/hot-reload path (not yet supported)".into(),
            ));
        }
    }

    let mut report = CanonicalLossReport::default();
    if allow_loss {
        if route.error_handler.is_some() {
            report.dropped_fields.push(CanonicalFieldLoss {
                field: "error_handler",
                reason: "not supported by CanonicalRouteSpec v2".to_string(),
                target_version: camel_api::CANONICAL_CONTRACT_VERSION,
            });
        }
        if route.unit_of_work.is_some() {
            report.dropped_fields.push(CanonicalFieldLoss {
                field: "unit_of_work",
                reason: "not supported by CanonicalRouteSpec v2".to_string(),
                target_version: camel_api::CANONICAL_CONTRACT_VERSION,
            });
        }
    }

    let circuit_breaker = route.circuit_breaker.map(|cb| CanonicalCircuitBreakerSpec {
        failure_threshold: cb.failure_threshold,
        open_duration_ms: cb.open_duration_ms,
    });

    let concurrency = match route.concurrency {
        Some(DeclarativeConcurrency::Sequential) => Some(CanonicalConcurrencySpec::Sequential),
        Some(DeclarativeConcurrency::Concurrent { max: Some(m) }) => {
            Some(CanonicalConcurrencySpec::Concurrent { max: m })
        }
        Some(DeclarativeConcurrency::Concurrent { max: None }) => {
            if allow_loss {
                report.dropped_fields.push(CanonicalFieldLoss {
                    field: "concurrency.max",
                    reason: "unbounded concurrency (max=None) not supported by CanonicalRouteSpec v2, dropped".to_string(),
                    target_version: camel_api::CANONICAL_CONTRACT_VERSION,
                });
                None
            } else {
                return Err(CamelError::RouteError(
                    "concurrent routes with unbounded max (max=None) cannot use the canonical path. Specify an explicit max or use the full DSL path.".into(),
                ));
            }
        }
        None => None,
    };

    let steps = route
        .steps
        .into_iter()
        .map(compile_declarative_step_to_canonical)
        .collect::<Result<Vec<_>, _>>()?;

    let spec = CanonicalRouteSpec {
        route_id: route.route_id,
        from: route.from,
        steps,
        circuit_breaker,
        auto_startup: Some(route.auto_startup),
        startup_order: Some(route.startup_order),
        concurrency,
        version: camel_api::CANONICAL_CONTRACT_VERSION,
    };
    spec.validate_contract()?;

    let loss_report = if report.is_empty() {
        None
    } else {
        Some(report)
    };
    Ok((spec, loss_report))
}

pub fn compile_canonical_route(
    spec: CanonicalRouteSpec,
    stream_cache_threshold: usize,
) -> Result<RouteDefinition, CamelError> {
    spec.validate_contract()?;
    let steps = compile_canonical_steps(spec.steps, stream_cache_threshold)?;

    let mut definition = RouteDefinition::new(spec.from, steps)
        .with_route_id(spec.route_id)
        .with_auto_startup(spec.auto_startup.unwrap_or(true));

    if let Some(order) = spec.startup_order {
        definition = definition.with_startup_order(order);
    }

    if let Some(concurrency) = spec.concurrency {
        definition = definition.with_concurrency(match concurrency {
            CanonicalConcurrencySpec::Sequential => ConcurrencyModel::Sequential,
            CanonicalConcurrencySpec::Concurrent { max } => {
                ConcurrencyModel::Concurrent { max: Some(max) }
            }
        });
    }

    if let Some(cb) = spec.circuit_breaker {
        definition = definition.with_circuit_breaker(
            CircuitBreakerConfig::new()
                .failure_threshold(cb.failure_threshold)
                .open_duration(Duration::from_millis(cb.open_duration_ms)),
        );
    }

    Ok(definition)
}

pub fn compile_canonical_step(
    step: CanonicalStepSpec,
    stream_cache_threshold: usize,
) -> Result<BuilderStep, CamelError> {
    match step {
        CanonicalStepSpec::To { uri } => Ok(BuilderStep::To(uri)),
        CanonicalStepSpec::Log { message } => Ok(BuilderStep::Log {
            level: LogLevel::Info,
            message,
        }),
        CanonicalStepSpec::WireTap { uri } => Ok(BuilderStep::WireTap { uri }),
        CanonicalStepSpec::Stop => Ok(BuilderStep::Stop),
        CanonicalStepSpec::Script { expression } => {
            Ok(BuilderStep::DeclarativeScript { expression })
        }
        CanonicalStepSpec::Delay {
            delay_ms,
            dynamic_header,
        } => Ok(BuilderStep::Delay {
            config: DelayConfig {
                delay_ms,
                dynamic_header,
            },
        }),
        CanonicalStepSpec::Filter { predicate, steps } => Ok(BuilderStep::DeclarativeFilter {
            predicate,
            steps: compile_canonical_steps(steps, stream_cache_threshold)?,
        }),
        CanonicalStepSpec::Choice { whens, otherwise } => {
            let mut compiled_whens = Vec::with_capacity(whens.len());
            for when in whens {
                compiled_whens.push(DeclarativeWhenStep {
                    predicate: when.predicate,
                    steps: compile_canonical_steps(when.steps, stream_cache_threshold)?,
                });
            }
            let compiled_otherwise = match otherwise {
                Some(steps) => Some(compile_canonical_steps(steps, stream_cache_threshold)?),
                None => None,
            };
            Ok(BuilderStep::DeclarativeChoice {
                whens: compiled_whens,
                otherwise: compiled_otherwise,
            })
        }
        CanonicalStepSpec::Split {
            expression,
            aggregation,
            parallel,
            parallel_limit,
            stop_on_exception,
            steps,
        } => compile_canonical_split(
            expression,
            aggregation,
            parallel,
            parallel_limit,
            stop_on_exception,
            steps,
            stream_cache_threshold,
        ),
        CanonicalStepSpec::Aggregate(config) => compile_canonical_aggregate(config),
    }
}

fn compile_canonical_steps(
    steps: Vec<CanonicalStepSpec>,
    stream_cache_threshold: usize,
) -> Result<Vec<BuilderStep>, CamelError> {
    steps
        .into_iter()
        .map(|step| compile_canonical_step(step, stream_cache_threshold))
        .collect()
}

fn compile_canonical_split(
    expression: CanonicalSplitExpressionSpec,
    aggregation: CanonicalSplitAggregationSpec,
    parallel: bool,
    parallel_limit: Option<usize>,
    stop_on_exception: bool,
    steps: Vec<CanonicalStepSpec>,
    stream_cache_threshold: usize,
) -> Result<BuilderStep, CamelError> {
    let aggregation = match aggregation {
        CanonicalSplitAggregationSpec::LastWins => SplitAggregation::LastWins,
        CanonicalSplitAggregationSpec::CollectAll => SplitAggregation::CollectAll,
        CanonicalSplitAggregationSpec::Original => SplitAggregation::Original,
    };
    let compiled_steps = compile_canonical_steps(steps, stream_cache_threshold)?;
    match expression {
        CanonicalSplitExpressionSpec::BodyLines => {
            let config = SplitterConfig::new(split_body_lines())
                .aggregation(aggregation)
                .parallel(parallel)
                .stop_on_exception(stop_on_exception);
            let config = if let Some(limit) = parallel_limit {
                config.parallel_limit(limit)
            } else {
                config
            };
            Ok(BuilderStep::Split {
                config,
                steps: compiled_steps,
            })
        }
        CanonicalSplitExpressionSpec::BodyJsonArray => {
            let config = SplitterConfig::new(split_body_json_array())
                .aggregation(aggregation)
                .parallel(parallel)
                .stop_on_exception(stop_on_exception);
            let config = if let Some(limit) = parallel_limit {
                config.parallel_limit(limit)
            } else {
                config
            };
            Ok(BuilderStep::Split {
                config,
                steps: compiled_steps,
            })
        }
        CanonicalSplitExpressionSpec::Language(expression) => Ok(BuilderStep::DeclarativeSplit {
            expression,
            aggregation,
            parallel,
            parallel_limit,
            stop_on_exception,
            steps: compiled_steps,
        }),
        CanonicalSplitExpressionSpec::Stream(stream_config) => {
            Ok(BuilderStep::DeclarativeStreamSplit {
                stream_config,
                aggregation,
                stop_on_exception,
                steps: compiled_steps,
            })
        }
    }
}

fn compile_canonical_aggregate(config: CanonicalAggregateSpec) -> Result<BuilderStep, CamelError> {
    let completion_size = config.completion_size.unwrap_or(1);
    let mut builder = AggregatorConfig::correlate_by(&config.header);

    match (config.completion_timeout_ms, completion_size) {
        (Some(timeout_ms), size) if timeout_ms > 0 && size > 1 => {
            builder = builder.complete_on_size_or_timeout(size, Duration::from_millis(timeout_ms));
        }
        (Some(timeout_ms), _) if timeout_ms > 0 => {
            builder = builder.complete_on_timeout(Duration::from_millis(timeout_ms));
        }
        (_, size) => {
            builder = builder.complete_when_size(size);
        }
    }

    builder = match config.strategy {
        CanonicalAggregateStrategySpec::CollectAll => {
            builder.strategy(AggregatorStrategy::CollectAll)
        }
    };
    if let Some(max_buckets) = config.max_buckets {
        builder = builder.max_buckets(max_buckets);
    }
    if let Some(ttl_ms) = config.bucket_ttl_ms {
        builder = builder.bucket_ttl(Duration::from_millis(ttl_ms));
    }
    if let Some(force) = config.force_completion_on_stop {
        builder = builder.force_completion_on_stop(force);
    }
    if let Some(discard) = config.discard_on_timeout {
        builder = builder.discard_on_timeout(discard);
    }

    let mut agg_config = builder.build()?;
    if let Some(expr) = config.correlation_key {
        use camel_api::aggregator::CorrelationStrategy;
        agg_config.correlation = CorrelationStrategy::Expression {
            expr,
            language: "simple".to_string(),
        };
    }

    Ok(BuilderStep::Aggregate { config: agg_config })
}

fn compile_error_handler(def: DeclarativeErrorHandler) -> Result<ErrorHandlerConfig, CamelError> {
    let mut config = if let Some(uri) = def.dead_letter_channel {
        ErrorHandlerConfig::dead_letter_channel(uri)
    } else {
        ErrorHandlerConfig::log_only()
    };

    if let Some(on_exceptions) = def.on_exceptions {
        for clause in on_exceptions {
            if clause.kind.is_none() && clause.message_contains.is_none() {
                return Err(CamelError::Config(
                    "error_handler.on_exceptions clause must set `kind` or `message_contains`"
                        .into(),
                ));
            }

            if let Some(ref kind) = clause.kind {
                ensure_known_exception_kind(kind)?;
            }

            let kind = clause.kind;
            let message_contains = clause.message_contains;
            let handled = clause.handled.unwrap_or(false);
            let continued = clause.continued.unwrap_or(false);

            if handled && continued {
                return Err(CamelError::Config(
                    "on_exceptions: handled=true and continued=true are mutually exclusive".into(),
                ));
            }

            let mut builder = config.on_exception(move |e| {
                let kind_ok = kind
                    .as_deref()
                    .is_none_or(|expected| exception_kind_matches(expected, e));
                let message_ok = message_contains
                    .as_ref()
                    .is_none_or(|needle| e.to_string().contains(needle));
                kind_ok && message_ok
            });

            if let Some(retry) = clause.retry {
                builder = builder.retry(retry.max_attempts).with_backoff(
                    Duration::from_millis(retry.initial_delay_ms),
                    retry.multiplier,
                    Duration::from_millis(retry.max_delay_ms),
                );
                if retry.jitter_factor > 0.0 {
                    builder = builder.with_jitter(retry.jitter_factor);
                }
                if let Some(uri) = retry.handled_by {
                    builder = builder.handled_by(uri);
                }
            }

            let has_steps = !clause.steps.is_empty();
            if has_steps {
                let compiled_steps = compile_declarative_steps(clause.steps, 0)?;
                let total = compiled_steps.len();
                let processors: Vec<camel_api::BoxProcessor> = compiled_steps
                    .into_iter()
                    .filter_map(|step| match step {
                        camel_core::route::BuilderStep::Processor(p) => Some(p),
                        _ => None,
                    })
                    .collect();
                let filtered = total - processors.len();
                if filtered > 0 {
                    tracing::warn!(
                        filtered,
                        total,
                        "on_exceptions: {filtered}/{total} steps require registry resolution \
                         and are not supported inline — use handled_by instead"
                    );
                }
                if processors.is_empty() {
                    if handled {
                        return Err(CamelError::RouteError(
                            "error_handler.on_exceptions: handled=true but no executable steps remain \
                             (all steps require registry resolution — use handled_by instead)"
                                .to_string(),
                        ));
                    }
                    tracing::warn!(
                        total,
                        "on_exceptions: all {total} steps require registry resolution and \
                         cannot be executed inline — use handled_by instead"
                    );
                } else {
                    builder = builder.on_steps(compose_pipeline(
                        processors
                            .into_iter()
                            .map(|p| CompiledStep::Process {
                                processor: p,
                                body_contract: None,
                            })
                            .collect(),
                    ));
                }
            }
            // Boolean precedence: continued=true → Continued; else handled=true → Handled; else Propagate
            if continued {
                builder = builder.continued(true);
            } else if handled {
                builder = builder.handled(true);
            }

            config = builder.build();
        }
    } else if let Some(retry) = def.retry {
        let mut builder = config.on_exception(|_e| true).retry(retry.max_attempts);
        builder = builder.with_backoff(
            Duration::from_millis(retry.initial_delay_ms),
            retry.multiplier,
            Duration::from_millis(retry.max_delay_ms),
        );
        if retry.jitter_factor > 0.0 {
            builder = builder.with_jitter(retry.jitter_factor);
        }
        if let Some(uri) = retry.handled_by {
            builder = builder.handled_by(uri);
        }
        config = builder.build();
    }

    Ok(config)
}

fn ensure_known_exception_kind(kind: &str) -> Result<(), CamelError> {
    if supported_exception_kinds().contains(&kind) {
        Ok(())
    } else {
        Err(CamelError::Config(format!(
            "unknown exception kind '{kind}'. supported kinds: {}",
            supported_exception_kinds().join(", ")
        )))
    }
}

fn supported_exception_kinds() -> Vec<&'static str> {
    vec![
        "ComponentNotFound",
        "EndpointCreationFailed",
        "ProcessorError",
        "TypeConversionFailed",
        "InvalidUri",
        "ChannelClosed",
        "RouteError",
        "Io",
        "DeadLetterChannelFailed",
        "CircuitOpen",
        "HttpOperationFailed",
        "ConsumerStopping",
        "Config",
        "AlreadyConsumed",
        "StreamLimitExceeded",
    ]
}

fn exception_kind_matches(kind: &str, err: &CamelError) -> bool {
    match kind {
        "ComponentNotFound" => matches!(err, CamelError::ComponentNotFound(_)),
        "EndpointCreationFailed" => matches!(err, CamelError::EndpointCreationFailed(_)),
        "ProcessorError" => matches!(err, CamelError::ProcessorError(_)),
        "TypeConversionFailed" => matches!(err, CamelError::TypeConversionFailed(_)),
        "InvalidUri" => matches!(err, CamelError::InvalidUri(_)),
        "ChannelClosed" => matches!(err, CamelError::ChannelClosed),
        "RouteError" => matches!(err, CamelError::RouteError(_)),
        "Io" => matches!(err, CamelError::Io(_)),
        "DeadLetterChannelFailed" => matches!(err, CamelError::DeadLetterChannelFailed(_)),
        "CircuitOpen" => matches!(err, CamelError::CircuitOpen(_)),
        "HttpOperationFailed" => matches!(err, CamelError::HttpOperationFailed { .. }),
        // "Stopped" arm REMOVED (ADR-0024 + user directive 2026-06-20):
        // Stop EIP no longer produces an error at the top-level (CompiledStep::Stop
        // → PipelineOutcome::Stopped → Ok(ex)). Sub-pipeline Stop propagates via
        // Err(Stopped) but is bypassed in run_steps before reaching the handler.
        // So onException: [Stopped] would never fire — arm removed.
        "ConsumerStopping" => matches!(err, CamelError::ConsumerStopping),
        "Config" => matches!(err, CamelError::Config(_)),
        "AlreadyConsumed" => matches!(err, CamelError::AlreadyConsumed),
        "StreamLimitExceeded" => matches!(err, CamelError::StreamLimitExceeded(_)),
        _ => false,
    }
}

fn compile_circuit_breaker(def: DeclarativeCircuitBreaker) -> CircuitBreakerConfig {
    CircuitBreakerConfig::new()
        .failure_threshold(def.failure_threshold)
        .open_duration(Duration::from_millis(def.open_duration_ms))
}

fn compile_declarative_steps(
    steps: Vec<DeclarativeStep>,
    stream_cache_threshold: usize,
) -> Result<Vec<BuilderStep>, CamelError> {
    steps
        .into_iter()
        .map(|step| compile_declarative_step_with_threshold(step, stream_cache_threshold))
        .collect()
}

pub fn compile_declarative_step(step: DeclarativeStep) -> Result<BuilderStep, CamelError> {
    compile_declarative_step_with_threshold(
        step,
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
    )
}

fn compile_declarative_step_with_threshold(
    step: DeclarativeStep,
    stream_cache_threshold: usize,
) -> Result<BuilderStep, CamelError> {
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
        DeclarativeStep::SetProperty(SetPropertyStepDef { key, value }) => {
            compile_set_property_step(key, value)
        }
        DeclarativeStep::SetBody(SetBodyStepDef { value }) => compile_set_body_step(value),
        DeclarativeStep::Script(ScriptStepDef { expression }) => {
            Ok(BuilderStep::DeclarativeScript { expression })
        }
        DeclarativeStep::StreamCache(def) => {
            let config = stream_cache_config(def.threshold, stream_cache_threshold);
            Ok(BuilderStep::Processor(camel_api::BoxProcessor::new(
                StreamCacheService::new(camel_api::IdentityProcessor, config),
            )))
        }
        DeclarativeStep::Stop => Ok(BuilderStep::Processor(camel_api::BoxProcessor::new(
            StopService,
        ))),
        DeclarativeStep::Filter(def) => {
            compile_filter_step(def.predicate, def.steps, stream_cache_threshold)
        }
        DeclarativeStep::Function(FunctionStepDef {
            runtime,
            source,
            timeout_ms,
        }) => {
            let timeout_ms = timeout_ms.unwrap_or(DEFAULT_FUNCTION_TIMEOUT_MS);
            let definition = camel_api::FunctionDefinition {
                id: camel_api::FunctionId::compute(&runtime, &source, timeout_ms),
                runtime,
                source,
                timeout_ms,
                route_id: None,
                step_index: None,
            };
            Ok(BuilderStep::DeclarativeFunction { definition })
        }
        DeclarativeStep::Choice(ChoiceStepDef { whens, otherwise }) => {
            let mut compiled_whens = Vec::with_capacity(whens.len());
            for when in whens {
                let predicate = when.predicate;
                let steps = compile_declarative_steps(when.steps, stream_cache_threshold)?;
                compiled_whens.push(DeclarativeWhenStep { predicate, steps });
            }

            let compiled_otherwise = match otherwise {
                Some(steps) => Some(compile_declarative_steps(steps, stream_cache_threshold)?),
                None => None,
            };

            Ok(BuilderStep::DeclarativeChoice {
                whens: compiled_whens,
                otherwise: compiled_otherwise,
            })
        }
        DeclarativeStep::Split(def) => compile_split_step(def, stream_cache_threshold),
        DeclarativeStep::Aggregate(def) => compile_aggregate_step(def),
        DeclarativeStep::Throttle(ThrottleStepDef {
            max_requests,
            period_ms,
            strategy,
            steps,
        }) => {
            let strategy = match strategy {
                ThrottleStrategyDef::Delay => ThrottleStrategy::Delay,
                ThrottleStrategyDef::Reject => ThrottleStrategy::Reject,
                ThrottleStrategyDef::Drop => ThrottleStrategy::Drop,
            };
            let config = ThrottlerConfig::new(max_requests, Duration::from_millis(period_ms))
                .strategy(strategy);
            let compiled_steps = compile_declarative_steps(steps, stream_cache_threshold)?;
            Ok(BuilderStep::Throttle {
                config,
                steps: compiled_steps,
            })
        }
        DeclarativeStep::LoadBalance(LoadBalanceStepDef {
            strategy,
            parallel,
            steps,
        }) => {
            let compiled_steps = compile_declarative_steps(steps, stream_cache_threshold)?;
            let strategy = match strategy {
                LoadBalanceStrategyDef::RoundRobin => LoadBalanceStrategy::RoundRobin,
                LoadBalanceStrategyDef::Random => LoadBalanceStrategy::Random,
                LoadBalanceStrategyDef::Failover => LoadBalanceStrategy::Failover,
                LoadBalanceStrategyDef::Weighted { distribution_ratio } => {
                    let weights: Vec<u32> = distribution_ratio
                        .split(',')
                        .map(|s| s.trim().parse::<u32>())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| {
                            CamelError::RouteError(format!(
                                "weighted distribution_ratio contains invalid value: {e}"
                            ))
                        })?;
                    if weights.len() != compiled_steps.len() {
                        return Err(CamelError::RouteError(format!(
                            "weighted distribution_ratio has {} weights but {} steps",
                            weights.len(),
                            compiled_steps.len()
                        )));
                    }
                    let weighted: Vec<(String, u32)> = weights
                        .into_iter()
                        .enumerate()
                        .map(|(i, w)| (format!("endpoint-{i}"), w))
                        .collect();
                    LoadBalanceStrategy::Weighted(weighted)
                }
            };
            let config = LoadBalancerConfig { strategy, parallel };
            Ok(BuilderStep::LoadBalance {
                config,
                steps: compiled_steps,
            })
        }
        DeclarativeStep::Multicast(def) => compile_multicast_step(def, stream_cache_threshold),
        DeclarativeStep::DynamicRouter(DynamicRouterStepDef {
            expression,
            uri_delimiter,
            cache_size,
            ignore_invalid_endpoints,
            max_iterations,
        }) => Ok(BuilderStep::DeclarativeDynamicRouter {
            expression,
            uri_delimiter,
            cache_size,
            ignore_invalid_endpoints,
            max_iterations,
        }),
        DeclarativeStep::RoutingSlip(RoutingSlipStepDef {
            expression,
            uri_delimiter,
            cache_size,
            ignore_invalid_endpoints,
        }) => Ok(BuilderStep::DeclarativeRoutingSlip {
            expression,
            uri_delimiter,
            cache_size,
            ignore_invalid_endpoints,
        }),
        DeclarativeStep::RecipientList(RecipientListStepDef {
            expression,
            delimiter,
            parallel,
            parallel_limit,
            stop_on_exception,
            aggregation,
        }) => {
            let agg_str = match aggregation {
                MulticastAggregationDef::LastWins => "last_wins".to_string(),
                MulticastAggregationDef::CollectAll => "collect_all".to_string(),
                MulticastAggregationDef::Original => "original".to_string(),
            };
            Ok(BuilderStep::DeclarativeRecipientList {
                expression,
                delimiter,
                parallel,
                parallel_limit,
                stop_on_exception,
                aggregation: agg_str,
            })
        }
        DeclarativeStep::ConvertBodyTo(def) => {
            let target = match def {
                BodyTypeDef::Text => BodyType::Text,
                BodyTypeDef::Json => BodyType::Json,
                BodyTypeDef::Bytes => BodyType::Bytes,
                BodyTypeDef::Xml => BodyType::Xml,
                BodyTypeDef::Empty => BodyType::Empty,
            };
            Ok(BuilderStep::Processor(camel_api::BoxProcessor::new(
                StreamCacheService::new(
                    ConvertBodyTo::new(IdentityProcessor, target),
                    camel_api::stream_cache::StreamCacheConfig::new(stream_cache_threshold),
                ),
            )))
        }
        DeclarativeStep::Bean(BeanStepDef { name, method }) => {
            Ok(BuilderStep::Bean { name, method })
        }
        DeclarativeStep::Marshal(DataFormatDef { format }) => {
            let df = if format.strip_prefix("protobuf:").is_some() {
                #[cfg(feature = "protobuf")]
                {
                    let rest = format.strip_prefix("protobuf:").expect("checked prefix"); // allow-unwrap
                    resolve_protobuf_dataformat(rest)?
                }
                #[cfg(not(feature = "protobuf"))]
                {
                    return Err(CamelError::RouteError(
                        "protobuf data format requires the 'protobuf' feature flag".to_string(),
                    ));
                }
            } else {
                builtin_data_format(&format).ok_or_else(|| {
                    CamelError::RouteError(format!(
                        "unknown data format: '{}'. Expected: json, xml, zip, protobuf:<path>#<Message>",
                        format
                    ))
                })?
            };
            Ok(BuilderStep::Processor(camel_api::BoxProcessor::new(
                MarshalService::new(camel_api::IdentityProcessor, df),
            )))
        }
        DeclarativeStep::Unmarshal(DataFormatDef { format }) => {
            let df = if format.strip_prefix("protobuf:").is_some() {
                #[cfg(feature = "protobuf")]
                {
                    let rest = format.strip_prefix("protobuf:").expect("checked prefix"); // allow-unwrap
                    resolve_protobuf_dataformat(rest)?
                }
                #[cfg(not(feature = "protobuf"))]
                {
                    return Err(CamelError::RouteError(
                        "protobuf data format requires the 'protobuf' feature flag".to_string(),
                    ));
                }
            } else {
                builtin_data_format(&format).ok_or_else(|| {
                    CamelError::RouteError(format!(
                        "unknown data format: '{}'. Expected: json, xml, zip, protobuf:<path>#<Message>",
                        format
                    ))
                })?
            };
            Ok(BuilderStep::Processor(camel_api::BoxProcessor::new(
                StreamCacheService::new(
                    UnmarshalService::new(camel_api::IdentityProcessor, df),
                    camel_api::stream_cache::StreamCacheConfig::new(stream_cache_threshold),
                ),
            )))
        }
        DeclarativeStep::Delay(DelayStepDef {
            delay_ms,
            dynamic_header,
        }) => {
            let config = DelayConfig::new(delay_ms);
            let config = match dynamic_header {
                Some(h) => config.with_dynamic_header(h),
                None => config,
            };
            Ok(BuilderStep::Delay { config })
        }
        DeclarativeStep::Loop(def) => compile_loop_step(def, stream_cache_threshold),
        DeclarativeStep::Enrich(def) => Ok(BuilderStep::Enrich {
            uri: def.uri,
            strategy: def.strategy,
            timeout_ms: def.timeout_ms,
        }),
        DeclarativeStep::PollEnrich(def) => Ok(BuilderStep::PollEnrich {
            uri: def.uri,
            strategy: def.strategy,
            timeout_ms: def.timeout_ms,
        }),
        DeclarativeStep::DoTry {
            steps,
            catch,
            finally,
        } => {
            let try_steps = compile_declarative_steps(steps, stream_cache_threshold)?;
            let catch_clauses = catch
                .into_iter()
                .map(|c| {
                    let clause_steps = compile_declarative_steps(c.steps, stream_cache_threshold)?;
                    Ok(DoTryCatchClauseBuilder {
                        exception: c.exception,
                        when: c.when,
                        on_when: c.on_when,
                        disposition: c.disposition,
                        steps: clause_steps,
                    })
                })
                .collect::<Result<Vec<_>, CamelError>>()?;
            let finally = match finally {
                Some(f) => {
                    let fsteps = compile_declarative_steps(f.steps, stream_cache_threshold)?;
                    Some(DoTryFinallyBuilder {
                        on_when: f.on_when,
                        steps: fsteps,
                    })
                }
                None => None,
            };
            Ok(BuilderStep::DeclarativeDoTry {
                try_steps,
                catch: catch_clauses,
                finally,
            })
        }
    }
}

fn stream_cache_config(
    step_threshold: Option<usize>,
    stream_cache_threshold: usize,
) -> camel_api::stream_cache::StreamCacheConfig {
    camel_api::stream_cache::StreamCacheConfig::new(
        step_threshold.unwrap_or(stream_cache_threshold),
    )
}

fn compile_loop_step(
    def: LoopStepDef,
    stream_cache_threshold: usize,
) -> Result<BuilderStep, CamelError> {
    let sub_steps = compile_declarative_steps(def.steps, stream_cache_threshold)?;
    Ok(BuilderStep::DeclarativeLoop {
        count: def.count,
        while_predicate: def.while_predicate,
        steps: sub_steps,
    })
}

#[cfg(feature = "protobuf")]
static PROTO_CACHE: std::sync::OnceLock<camel_proto_compiler::ProtoCache> =
    std::sync::OnceLock::new();

#[cfg(feature = "protobuf")]
fn resolve_protobuf_dataformat(
    spec: &str,
) -> Result<std::sync::Arc<dyn camel_api::DataFormat>, CamelError> {
    let (proto_path, message_name) = spec.split_once('#').ok_or_else(|| {
        CamelError::RouteError(format!(
            "invalid protobuf format: 'protobuf:{}'. Expected: protobuf:<proto_path>#<MessageName>",
            spec
        ))
    })?;
    if proto_path.starts_with('/') || proto_path.contains("..") {
        return Err(CamelError::RouteError(format!(
            "proto path '{}' must be relative and cannot contain '..'",
            proto_path
        )));
    }
    let cache = PROTO_CACHE.get_or_init(camel_proto_compiler::ProtoCache::new);
    let df = camel_dataformat_protobuf::ProtobufDataFormat::new_with_cache(
        proto_path,
        message_name,
        cache,
    )?;
    Ok(std::sync::Arc::new(df))
}

fn compile_declarative_step_to_canonical(
    step: DeclarativeStep,
) -> Result<CanonicalStepSpec, CamelError> {
    match step {
        DeclarativeStep::To(ToStepDef { uri }) => Ok(CanonicalStepSpec::To { uri }),
        DeclarativeStep::Stop => Ok(CanonicalStepSpec::Stop),
        DeclarativeStep::Log(LogStepDef { message, .. }) => Ok(CanonicalStepSpec::Log {
            message: compile_log_message(message)?,
        }),
        DeclarativeStep::WireTap(WireTapStepDef { uri }) => Ok(CanonicalStepSpec::WireTap { uri }),
        DeclarativeStep::Script(ScriptStepDef { expression }) => {
            Ok(CanonicalStepSpec::Script { expression })
        }
        DeclarativeStep::Filter(def) => Ok(CanonicalStepSpec::Filter {
            predicate: def.predicate,
            steps: compile_declarative_steps_to_canonical(def.steps)?,
        }),
        DeclarativeStep::Choice(ChoiceStepDef { whens, otherwise }) => {
            let mut canonical_whens = Vec::with_capacity(whens.len());
            for when in whens {
                canonical_whens.push(CanonicalWhenSpec {
                    predicate: when.predicate,
                    steps: compile_declarative_steps_to_canonical(when.steps)?,
                });
            }
            let otherwise = match otherwise {
                Some(steps) => Some(compile_declarative_steps_to_canonical(steps)?),
                None => None,
            };
            Ok(CanonicalStepSpec::Choice {
                whens: canonical_whens,
                otherwise,
            })
        }
        DeclarativeStep::Split(def) => compile_split_step_to_canonical(def),
        DeclarativeStep::Aggregate(def) => compile_aggregate_step_to_canonical(def),
        DeclarativeStep::Delay(DelayStepDef {
            delay_ms,
            dynamic_header,
        }) => Ok(CanonicalStepSpec::Delay {
            delay_ms,
            dynamic_header,
        }),
        DeclarativeStep::Loop(_) => {
            let detail = canonical_contract_rejection_reason("loop")
                .unwrap_or("not included in canonical v1");
            Err(CamelError::RouteError(format!(
                "canonical v1 does not support step `loop`: {detail}"
            )))
        }
        other => {
            let step_name = declarative_step_name(&other);
            let detail = canonical_contract_rejection_reason(step_name)
                .unwrap_or("not included in canonical v1");
            Err(CamelError::RouteError(format!(
                "canonical v1 does not support step `{step_name}`: {detail}"
            )))
        }
    }
}

fn compile_declarative_steps_to_canonical(
    steps: Vec<DeclarativeStep>,
) -> Result<Vec<CanonicalStepSpec>, CamelError> {
    steps
        .into_iter()
        .map(compile_declarative_step_to_canonical)
        .collect()
}

fn compile_split_step_to_canonical(def: SplitStepDef) -> Result<CanonicalStepSpec, CamelError> {
    let expression = match def.expression {
        SplitExpressionDef::BodyLines => CanonicalSplitExpressionSpec::BodyLines,
        SplitExpressionDef::BodyJsonArray => CanonicalSplitExpressionSpec::BodyJsonArray,
        SplitExpressionDef::Language(expr) => CanonicalSplitExpressionSpec::Language(expr),
        SplitExpressionDef::Stream(config) => CanonicalSplitExpressionSpec::Stream(config),
    };
    let aggregation = match def.aggregation {
        SplitAggregationDef::LastWins => CanonicalSplitAggregationSpec::LastWins,
        SplitAggregationDef::CollectAll => CanonicalSplitAggregationSpec::CollectAll,
        SplitAggregationDef::Original => CanonicalSplitAggregationSpec::Original,
    };
    Ok(CanonicalStepSpec::Split {
        expression,
        aggregation,
        parallel: def.parallel,
        parallel_limit: def.parallel_limit,
        stop_on_exception: def.stop_on_exception,
        steps: compile_declarative_steps_to_canonical(def.steps)?,
    })
}

fn compile_aggregate_step_to_canonical(
    def: AggregateStepDef,
) -> Result<CanonicalStepSpec, CamelError> {
    if def.completion_predicate.is_some() {
        return Err(CamelError::RouteError(
            "aggregate.completion_predicate is not yet implemented".to_string(),
        ));
    }

    let strategy = match def.strategy {
        AggregateStrategyDef::CollectAll => CanonicalAggregateStrategySpec::CollectAll,
    };

    Ok(CanonicalStepSpec::Aggregate(CanonicalAggregateSpec {
        header: def.header,
        completion_size: def.completion_size,
        completion_timeout_ms: def.completion_timeout_ms,
        correlation_key: def.correlation_key,
        force_completion_on_stop: def.force_completion_on_stop,
        discard_on_timeout: def.discard_on_timeout,
        strategy,
        max_buckets: def.max_buckets,
        bucket_ttl_ms: def.bucket_ttl_ms,
    }))
}

fn compile_log_message(message: ValueSourceDef) -> Result<String, CamelError> {
    match message {
        ValueSourceDef::Literal(value) => Ok(match value {
            serde_json::Value::String(text) => text,
            other => other.to_string(),
        }),
        ValueSourceDef::Expression(LanguageExpressionDef { language, source }) => {
            if language != "simple" {
                return Err(CamelError::RouteError(format!(
                    "canonical v1 only supports log expressions in simple language; got `{language}`"
                )));
            }
            Ok(source)
        }
    }
}

fn declarative_step_name(step: &DeclarativeStep) -> &'static str {
    match step {
        DeclarativeStep::To(_) => "to",
        DeclarativeStep::Log(_) => "log",
        DeclarativeStep::SetHeader(_) => "set_header",
        DeclarativeStep::SetProperty(_) => "set_property",
        DeclarativeStep::SetBody(_) => "set_body",
        DeclarativeStep::Filter(_) => "filter",
        DeclarativeStep::Choice(_) => "choice",
        DeclarativeStep::Split(_) => "split",
        DeclarativeStep::Aggregate(_) => "aggregate",
        DeclarativeStep::WireTap(_) => "wire_tap",
        DeclarativeStep::DynamicRouter(_) => "dynamic_router",
        DeclarativeStep::LoadBalance(_) => "load_balance",
        DeclarativeStep::Multicast(_) => "multicast",
        DeclarativeStep::RoutingSlip(_) => "routing_slip",
        DeclarativeStep::RecipientList(_) => "recipient_list",
        DeclarativeStep::Stop => "stop",
        DeclarativeStep::Throttle(_) => "throttle",
        DeclarativeStep::Script(_) => "script",
        DeclarativeStep::StreamCache(_) => "stream_cache",
        DeclarativeStep::ConvertBodyTo(_) => "convert_body_to",
        DeclarativeStep::Bean(_) => "bean",
        DeclarativeStep::Marshal(_) => "marshal",
        DeclarativeStep::Unmarshal(_) => "unmarshal",
        DeclarativeStep::Delay(_) => "delay",
        DeclarativeStep::Loop(_) => "loop",
        DeclarativeStep::Function(_) => "function",
        DeclarativeStep::Enrich(_) => "enrich",
        DeclarativeStep::PollEnrich(_) => "poll_enrich",
        DeclarativeStep::DoTry { .. } => "do_try",
    }
}

fn compile_split_step(
    def: SplitStepDef,
    stream_cache_threshold: usize,
) -> Result<BuilderStep, CamelError> {
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
                steps: compile_declarative_steps(def.steps, stream_cache_threshold)?,
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
                steps: compile_declarative_steps(def.steps, stream_cache_threshold)?,
            })
        }
        SplitExpressionDef::Language(expression) => Ok(BuilderStep::DeclarativeSplit {
            expression,
            aggregation,
            parallel: def.parallel,
            parallel_limit: def.parallel_limit,
            stop_on_exception: def.stop_on_exception,
            steps: compile_declarative_steps(def.steps, stream_cache_threshold)?,
        }),
        SplitExpressionDef::Stream(stream_config) => Ok(BuilderStep::DeclarativeStreamSplit {
            stream_config,
            aggregation,
            stop_on_exception: def.stop_on_exception,
            steps: compile_declarative_steps(def.steps, stream_cache_threshold)?,
        }),
    }
}

fn compile_aggregate_step(def: AggregateStepDef) -> Result<BuilderStep, CamelError> {
    let completion_size = def.completion_size.unwrap_or(1);

    if def.completion_predicate.is_some() {
        return Err(CamelError::RouteError(
            "aggregate.completion_predicate is not yet implemented".to_string(),
        ));
    }

    // NOTE: def.correlation_key is intentionally not wired here — the builder path
    // lacks correlate_by_expr(). Expression correlation is resolved at runtime by
    // the processor via CanonicalAggregateSpec.correlation_key (canonical path).
    let mut builder = AggregatorConfig::correlate_by(&def.header);

    match (def.completion_timeout_ms, completion_size) {
        (Some(timeout_ms), size) if timeout_ms > 0 && size > 1 => {
            builder = builder.complete_on_size_or_timeout(size, Duration::from_millis(timeout_ms));
        }
        (Some(timeout_ms), _) if timeout_ms > 0 => {
            builder = builder.complete_on_timeout(Duration::from_millis(timeout_ms));
        }
        (_, size) => {
            builder = builder.complete_when_size(size);
        }
    }

    builder = match def.strategy {
        AggregateStrategyDef::CollectAll => builder.strategy(AggregatorStrategy::CollectAll),
    };
    if let Some(max_buckets) = def.max_buckets {
        builder = builder.max_buckets(max_buckets);
    }
    if let Some(ttl_ms) = def.bucket_ttl_ms {
        builder = builder.bucket_ttl(Duration::from_millis(ttl_ms));
    }
    if let Some(force) = def.force_completion_on_stop {
        builder = builder.force_completion_on_stop(force);
    }
    if let Some(discard) = def.discard_on_timeout {
        builder = builder.discard_on_timeout(discard);
    }

    Ok(BuilderStep::Aggregate {
        config: builder.build()?,
    })
}

fn compile_multicast_step(
    def: MulticastStepDef,
    stream_cache_threshold: usize,
) -> Result<BuilderStep, CamelError> {
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
        steps: compile_declarative_steps(def.steps, stream_cache_threshold)?,
        config,
    })
}

fn compile_filter_step(
    predicate: LanguageExpressionDef,
    steps: Vec<DeclarativeStep>,
    stream_cache_threshold: usize,
) -> Result<BuilderStep, CamelError> {
    Ok(BuilderStep::DeclarativeFilter {
        predicate,
        steps: compile_declarative_steps(steps, stream_cache_threshold)?,
    })
}

fn compile_set_header_step(key: String, value: ValueSourceDef) -> Result<BuilderStep, CamelError> {
    Ok(BuilderStep::DeclarativeSetHeader { key, value })
}

fn compile_set_property_step(
    key: String,
    value: ValueSourceDef,
) -> Result<BuilderStep, CamelError> {
    Ok(BuilderStep::DeclarativeSetProperty {
        key,
        value_source: value,
    })
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

fn validate_route(route: &DeclarativeRoute) -> Result<(), CamelError> {
    if route.from.trim().is_empty() {
        return Err(CamelError::Config(
            "route 'from' URI must not be empty".to_string(),
        ));
    }
    for step in &route.steps {
        validate_step(step)?;
    }
    if let Some(ref cb) = route.circuit_breaker {
        if cb.failure_threshold == 0 {
            return Err(CamelError::Config(
                "circuit_breaker failure_threshold must be > 0".to_string(),
            ));
        }
        if cb.open_duration_ms == 0 {
            return Err(CamelError::Config(
                "circuit_breaker open_duration_ms must be > 0".to_string(),
            ));
        }
    }
    if let Some(ref eh) = route.error_handler {
        validate_error_handler(eh)?;
    }
    Ok(())
}

fn validate_step(step: &DeclarativeStep) -> Result<(), CamelError> {
    match step {
        DeclarativeStep::To(ToStepDef { uri }) => {
            if uri.trim().is_empty() {
                return Err(CamelError::Config(
                    "step 'to' URI must not be empty".to_string(),
                ));
            }
        }
        DeclarativeStep::WireTap(WireTapStepDef { uri }) => {
            if uri.trim().is_empty() {
                return Err(CamelError::Config(
                    "step 'wire_tap' URI must not be empty".to_string(),
                ));
            }
        }
        DeclarativeStep::SetHeader(SetHeaderStepDef { key, .. }) => {
            if key.trim().is_empty() {
                return Err(CamelError::Config(
                    "set_header key must not be empty".to_string(),
                ));
            }
        }
        DeclarativeStep::SetProperty(SetPropertyStepDef { key, .. }) => {
            if key.trim().is_empty() {
                return Err(CamelError::Config(
                    "set_property key must not be empty".to_string(),
                ));
            }
        }
        DeclarativeStep::Throttle(ThrottleStepDef {
            max_requests,
            period_ms,
            steps,
            ..
        }) => {
            if *max_requests == 0 {
                return Err(CamelError::Config(
                    "throttle max_requests must be > 0".to_string(),
                ));
            }
            if *period_ms == 0 {
                return Err(CamelError::Config(
                    "throttle period_ms must be > 0".to_string(),
                ));
            }
            for s in steps {
                validate_step(s)?;
            }
        }
        DeclarativeStep::Delay(DelayStepDef { delay_ms, .. }) => {
            if *delay_ms == 0 {
                return Err(CamelError::Config("delay delay_ms must be > 0".to_string()));
            }
        }
        DeclarativeStep::Filter(def) => {
            for s in &def.steps {
                validate_step(s)?;
            }
        }
        DeclarativeStep::Choice(ChoiceStepDef { whens, otherwise }) => {
            for when in whens {
                for s in &when.steps {
                    validate_step(s)?;
                }
            }
            if let Some(steps) = otherwise {
                for s in steps {
                    validate_step(s)?;
                }
            }
        }
        DeclarativeStep::Split(def) => {
            for s in &def.steps {
                validate_step(s)?;
            }
        }
        DeclarativeStep::Multicast(def) => {
            if def.timeout_ms.is_some_and(|t| t == 0) {
                return Err(CamelError::Config(
                    "multicast timeout_ms must be > 0".to_string(),
                ));
            }
            for s in &def.steps {
                validate_step(s)?;
            }
        }
        DeclarativeStep::LoadBalance(def) => {
            for s in &def.steps {
                validate_step(s)?;
            }
        }
        DeclarativeStep::Loop(def) => {
            if def.count.is_some_and(|c| c == 0) {
                return Err(CamelError::Config("loop count must be > 0".to_string()));
            }
            if def.count.is_some_and(|c| c > 0) && def.while_predicate.is_some() {
                return Err(CamelError::Config(
                    "loop cannot have both count and predicate".to_string(),
                ));
            }
            for s in &def.steps {
                validate_step(s)?;
            }
        }
        DeclarativeStep::Aggregate(def) => {
            if def.correlation_key.is_none() {
                return Err(CamelError::Config(
                    "aggregate requires a correlation_key".to_string(),
                ));
            }
        }
        // Steps without validation-relevant fields
        DeclarativeStep::Log(_)
        | DeclarativeStep::SetBody(_)
        | DeclarativeStep::Script(_)
        | DeclarativeStep::StreamCache(_)
        | DeclarativeStep::Stop
        | DeclarativeStep::ConvertBodyTo(_)
        | DeclarativeStep::Bean(_)
        | DeclarativeStep::Marshal(_)
        | DeclarativeStep::Unmarshal(_)
        | DeclarativeStep::Function(_)
        | DeclarativeStep::DynamicRouter(_)
        | DeclarativeStep::RoutingSlip(_)
        | DeclarativeStep::RecipientList(_)
        | DeclarativeStep::Enrich(_)
        | DeclarativeStep::PollEnrich(_) => {}
        DeclarativeStep::DoTry {
            steps,
            catch,
            finally,
        } => {
            for s in steps {
                validate_step(s)?;
            }
            for c in catch {
                for s in &c.steps {
                    validate_step(s)?;
                }
            }
            if let Some(f) = finally {
                for s in &f.steps {
                    validate_step(s)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_error_handler(eh: &DeclarativeErrorHandler) -> Result<(), CamelError> {
    if let Some(ref retry) = eh.retry {
        validate_redelivery_policy(retry)?;
    }
    if let Some(ref exceptions) = eh.on_exceptions {
        for clause in exceptions {
            if let Some(ref retry) = clause.retry {
                validate_redelivery_policy(retry)?;
            }
        }
    }
    Ok(())
}

fn validate_redelivery_policy(policy: &DeclarativeRedeliveryPolicy) -> Result<(), CamelError> {
    if policy.max_attempts == 0 {
        return Err(CamelError::Config(
            "redelivery max_attempts must be > 0".to_string(),
        ));
    }
    if policy.initial_delay_ms == 0 {
        return Err(CamelError::Config(
            "redelivery initial_delay_ms must be > 0".to_string(),
        ));
    }
    if policy.max_delay_ms == 0 {
        return Err(CamelError::Config(
            "redelivery max_delay_ms must be > 0".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AggregateStrategyDef, BeanStepDef, BodyTypeDef, ChoiceStepDef, DataFormatDef,
        DeclarativeCircuitBreaker, DeclarativeConcurrency, DeclarativeErrorHandler,
        DeclarativeOnException, DeclarativeRedeliveryPolicy, DeclarativeRoute,
        DeclarativeSecurityPolicy, DelayStepDef, DynamicRouterStepDef, FilterStepDef,
        LanguageExpressionDef, LoadBalanceStepDef, LoadBalanceStrategyDef, LogLevelDef, LogStepDef,
        LoopStepDef, MulticastAggregationDef, MulticastStepDef, RecipientListStepDef,
        RoutingSlipStepDef, SetBodyStepDef, SetHeaderStepDef, SetPropertyStepDef,
        SplitAggregationDef, SplitExpressionDef, SplitStepDef, StreamCacheStepDef, ThrottleStepDef,
        ThrottleStrategyDef, ToStepDef, ValueSourceDef, WhenStepDef, WireTapStepDef,
    };
    use async_trait::async_trait;
    use serde_json::Value;

    fn make_predicate(simple_src: &str) -> LanguageExpressionDef {
        LanguageExpressionDef {
            language: "simple".into(),
            source: simple_src.into(),
        }
    }

    #[test]
    fn test_compile_error_handler_on_exceptions_order_preserved() {
        let config = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: Some("log:dlc".into()),
            retry: None,
            on_exceptions: Some(vec![
                DeclarativeOnException {
                    kind: Some("Io".into()),
                    message_contains: None,
                    retry: Some(DeclarativeRedeliveryPolicy {
                        max_attempts: 3,
                        initial_delay_ms: 10,
                        multiplier: 2.0,
                        max_delay_ms: 100,
                        jitter_factor: 0.0,
                        handled_by: Some("log:io".into()),
                    }),
                    steps: vec![],
                    handled: None,
                    continued: None,
                },
                DeclarativeOnException {
                    kind: Some("ProcessorError".into()),
                    message_contains: Some("validation".into()),
                    retry: Some(DeclarativeRedeliveryPolicy {
                        max_attempts: 1,
                        initial_delay_ms: 5,
                        multiplier: 2.0,
                        max_delay_ms: 50,
                        jitter_factor: 0.0,
                        handled_by: None,
                    }),
                    steps: vec![],
                    handled: None,
                    continued: None,
                },
            ]),
        })
        .expect("compile should succeed");

        assert_eq!(config.policies.len(), 2);
        assert_eq!(
            config.policies[0].retry.as_ref().map(|p| p.max_attempts),
            Some(3)
        );
        assert_eq!(
            config.policies[1].retry.as_ref().map(|p| p.max_attempts),
            Some(1)
        );
    }

    #[test]
    fn test_compile_error_handler_unknown_kind_returns_config_error() {
        let err = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: None,
            retry: None,
            on_exceptions: Some(vec![DeclarativeOnException {
                kind: Some("NotRealKind".into()),
                message_contains: None,
                retry: None,
                steps: vec![],
                handled: None,
                continued: None,
            }]),
        })
        .err()
        .expect("should fail");

        assert!(matches!(err, CamelError::Config(_)));
    }

    #[test]
    fn test_compile_error_handler_invalid_clause_without_matcher() {
        let err = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: None,
            retry: None,
            on_exceptions: Some(vec![DeclarativeOnException {
                kind: None,
                message_contains: None,
                retry: None,
                steps: vec![],
                handled: None,
                continued: None,
            }]),
        })
        .err()
        .expect("should fail");

        assert!(matches!(err, CamelError::Config(_)));
    }

    #[test]
    fn test_compile_error_handler_legacy_retry_still_supported() {
        let config = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: Some("log:dlc".into()),
            retry: Some(DeclarativeRedeliveryPolicy {
                max_attempts: 2,
                initial_delay_ms: 100,
                multiplier: 2.0,
                max_delay_ms: 1000,
                jitter_factor: 0.0,
                handled_by: None,
            }),
            on_exceptions: None,
        })
        .expect("compile should succeed");

        assert_eq!(config.policies.len(), 1);
        assert_eq!(
            config.policies[0].retry.as_ref().map(|p| p.max_attempts),
            Some(2)
        );
    }

    #[test]
    fn test_compile_error_handler_kind_list_guard() {
        let expected = vec![
            "ComponentNotFound",
            "EndpointCreationFailed",
            "ProcessorError",
            "TypeConversionFailed",
            "InvalidUri",
            "ChannelClosed",
            "RouteError",
            "Io",
            "DeadLetterChannelFailed",
            "CircuitOpen",
            "HttpOperationFailed",
            "ConsumerStopping",
            "Config",
            "AlreadyConsumed",
            "StreamLimitExceeded",
        ];

        assert_eq!(supported_exception_kinds(), expected);
    }

    #[test]
    fn test_compile_error_handler_message_contains_refines_kind_matching() {
        let config = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: Some("log:dlc".into()),
            retry: None,
            on_exceptions: Some(vec![
                DeclarativeOnException {
                    kind: Some("Io".into()),
                    message_contains: Some("validation".into()),
                    retry: Some(DeclarativeRedeliveryPolicy {
                        max_attempts: 1,
                        initial_delay_ms: 10,
                        multiplier: 2.0,
                        max_delay_ms: 100,
                        jitter_factor: 0.0,
                        handled_by: Some("log:validation".into()),
                    }),
                    steps: vec![],
                    handled: None,
                    continued: None,
                },
                DeclarativeOnException {
                    kind: Some("Io".into()),
                    message_contains: None,
                    retry: Some(DeclarativeRedeliveryPolicy {
                        max_attempts: 2,
                        initial_delay_ms: 10,
                        multiplier: 2.0,
                        max_delay_ms: 100,
                        jitter_factor: 0.0,
                        handled_by: Some("log:io".into()),
                    }),
                    steps: vec![],
                    handled: None,
                    continued: None,
                },
            ]),
        })
        .expect("compile should succeed");

        let err = CamelError::Io("network reset".into());
        let first_matches = (config.policies[0].matches)(&err);
        let second_matches = (config.policies[1].matches)(&err);

        assert!(!first_matches);
        assert!(second_matches);
        assert_eq!(
            config.policies[0].handled_by.as_deref(),
            Some("log:validation")
        );
        assert_eq!(config.policies[1].handled_by.as_deref(), Some("log:io"));
        assert_eq!(config.dlc_uri.as_deref(), Some("log:dlc"));
    }

    #[test]
    fn test_compile_error_handler_message_contains_only_clause() {
        let config = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: None,
            retry: None,
            on_exceptions: Some(vec![DeclarativeOnException {
                kind: None,
                message_contains: Some("validation".into()),
                retry: None,
                steps: vec![],
                handled: None,
                continued: None,
            }]),
        })
        .expect("compile should succeed");

        assert_eq!(config.policies.len(), 1);
        assert!((config.policies[0].matches)(&CamelError::ProcessorError(
            "validation failed".into()
        )));
        assert!(!(config.policies[0].matches)(&CamelError::ProcessorError(
            "other error".into()
        )));
    }

    #[test]
    fn test_compile_error_handler_on_exceptions_without_retry_builds_policy() {
        let config = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: Some("log:dlc".into()),
            retry: None,
            on_exceptions: Some(vec![DeclarativeOnException {
                kind: Some("ConsumerStopping".into()),
                message_contains: None,
                retry: None,
                steps: vec![],
                handled: None,
                continued: None,
            }]),
        })
        .expect("compile should succeed");

        assert_eq!(config.policies.len(), 1);
        assert!(config.policies[0].retry.is_none());
        assert!((config.policies[0].matches)(&CamelError::ConsumerStopping));
    }

    #[test]
    fn test_compile_error_handler_on_exception_with_steps() {
        let def = DeclarativeErrorHandler {
            dead_letter_channel: None,
            retry: None,
            on_exceptions: Some(vec![DeclarativeOnException {
                kind: Some("RouteError".to_string()),
                message_contains: None,
                retry: None,
                steps: vec![DeclarativeStep::Log(LogStepDef {
                    message: ValueSourceDef::Literal(Value::String("error".to_string())),
                    level: LogLevelDef::Info,
                })],
                handled: Some(true),
                continued: None,
            }]),
        };
        let config = compile_error_handler(def).unwrap();
        assert_eq!(config.policies.len(), 1);
        assert!(config.policies[0].on_steps.is_some());
        assert_eq!(
            config.policies[0].disposition,
            camel_api::error_handler::ExceptionDisposition::Handled
        );
    }

    #[test]
    fn test_compile_error_handler_handled_without_steps_maps_to_handled() {
        let def = DeclarativeErrorHandler {
            dead_letter_channel: None,
            retry: None,
            on_exceptions: Some(vec![DeclarativeOnException {
                kind: Some("RouteError".to_string()),
                message_contains: None,
                retry: None,
                steps: vec![],
                handled: Some(true),
                continued: None,
            }]),
        };
        let config = compile_error_handler(def).expect("compile should succeed");
        assert_eq!(
            config.policies[0].disposition,
            camel_api::error_handler::ExceptionDisposition::Handled
        );
    }

    #[test]
    fn test_compile_continued_true_maps_to_continued_disposition() {
        let config = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: None,
            retry: None,
            on_exceptions: Some(vec![DeclarativeOnException {
                kind: Some("ProcessorError".into()),
                message_contains: None,
                retry: None,
                steps: vec![],
                handled: None,
                continued: Some(true),
            }]),
        })
        .expect("compile should succeed");
        assert_eq!(
            config.policies[0].disposition,
            camel_api::error_handler::ExceptionDisposition::Continued
        );
    }

    #[test]
    fn test_compile_handled_and_continued_both_true_errors() {
        let result = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: None,
            retry: None,
            on_exceptions: Some(vec![DeclarativeOnException {
                kind: Some("ProcessorError".into()),
                message_contains: None,
                retry: None,
                steps: vec![],
                handled: Some(true),
                continued: Some(true),
            }]),
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_compile_handled_true_with_continued_false_maps_to_handled() {
        let config = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: None,
            retry: None,
            on_exceptions: Some(vec![DeclarativeOnException {
                kind: Some("ProcessorError".into()),
                message_contains: None,
                retry: None,
                steps: vec![],
                handled: Some(true),
                continued: Some(false),
            }]),
        })
        .expect("compile should succeed");
        assert_eq!(
            config.policies[0].disposition,
            camel_api::error_handler::ExceptionDisposition::Handled
        );
    }

    #[test]
    fn test_compile_neither_handled_nor_continued_maps_to_propagate() {
        let config = compile_error_handler(DeclarativeErrorHandler {
            dead_letter_channel: None,
            retry: None,
            on_exceptions: Some(vec![DeclarativeOnException {
                kind: Some("ProcessorError".into()),
                message_contains: None,
                retry: None,
                steps: vec![],
                handled: None,
                continued: None,
            }]),
        })
        .expect("compile should succeed");
        assert_eq!(
            config.policies[0].disposition,
            camel_api::error_handler::ExceptionDisposition::Propagate
        );
    }

    #[test]
    fn test_exception_kind_matches_all_supported_variants() {
        assert!(exception_kind_matches(
            "ComponentNotFound",
            &CamelError::ComponentNotFound("x".into())
        ));
        assert!(exception_kind_matches(
            "EndpointCreationFailed",
            &CamelError::EndpointCreationFailed("x".into())
        ));
        assert!(exception_kind_matches(
            "ProcessorError",
            &CamelError::ProcessorError("x".into())
        ));
        assert!(exception_kind_matches(
            "TypeConversionFailed",
            &CamelError::TypeConversionFailed("x".into())
        ));
        assert!(exception_kind_matches(
            "InvalidUri",
            &CamelError::InvalidUri("x".into())
        ));
        assert!(exception_kind_matches(
            "ChannelClosed",
            &CamelError::ChannelClosed
        ));
        assert!(exception_kind_matches(
            "RouteError",
            &CamelError::RouteError("x".into())
        ));
        assert!(exception_kind_matches("Io", &CamelError::Io("x".into())));
        assert!(exception_kind_matches(
            "DeadLetterChannelFailed",
            &CamelError::DeadLetterChannelFailed("x".into())
        ));
        assert!(exception_kind_matches(
            "CircuitOpen",
            &CamelError::CircuitOpen("x".into())
        ));
        assert!(exception_kind_matches(
            "HttpOperationFailed",
            &CamelError::HttpOperationFailed {
                method: "GET".into(),
                url: "https://example.com".into(),
                status_code: 500,
                status_text: "boom".into(),
                response_body: None,
            }
        ));
        assert!(exception_kind_matches(
            "Config",
            &CamelError::Config("x".into())
        ));
        assert!(exception_kind_matches(
            "AlreadyConsumed",
            &CamelError::AlreadyConsumed
        ));
        assert!(exception_kind_matches(
            "StreamLimitExceeded",
            &CamelError::StreamLimitExceeded(1)
        ));
        assert!(!exception_kind_matches("NoSuchKind", &CamelError::Stopped));
    }

    #[test]
    fn compile_marshal_json_to_processor() {
        let step = DeclarativeStep::Marshal(DataFormatDef {
            format: "json".to_string(),
        });
        let result = compile_declarative_step(step);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), BuilderStep::Processor(_)));
    }

    #[test]
    fn compile_unmarshal_xml_to_processor() {
        let step = DeclarativeStep::Unmarshal(DataFormatDef {
            format: "xml".to_string(),
        });
        let result = compile_declarative_step(step);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), BuilderStep::Processor(_)));
    }

    #[test]
    fn compile_marshal_unknown_format_returns_error() {
        let step = DeclarativeStep::Marshal(DataFormatDef {
            format: "avro".to_string(),
        });
        let result = compile_declarative_step(step);
        assert!(result.is_err());
    }

    #[test]
    fn compile_unmarshal_unknown_format_returns_error() {
        let step = DeclarativeStep::Unmarshal(DataFormatDef {
            format: "protobuf".to_string(),
        });
        let result = compile_declarative_step(step);
        assert!(result.is_err());
    }

    #[cfg(feature = "protobuf")]
    fn proto_fixture_path() -> String {
        "tests/helloworld.proto".to_string()
    }

    #[cfg(feature = "protobuf")]
    #[test]
    fn compile_marshal_protobuf_to_processor() {
        let step = DeclarativeStep::Marshal(DataFormatDef {
            format: format!("protobuf:{}#helloworld.HelloRequest", proto_fixture_path()),
        });
        let result = compile_declarative_step(step);
        assert!(
            result.is_ok(),
            "protobuf marshal should resolve: {:?}",
            result
        );
    }

    #[cfg(feature = "protobuf")]
    #[test]
    fn compile_unmarshal_protobuf_to_processor() {
        let step = DeclarativeStep::Unmarshal(DataFormatDef {
            format: format!("protobuf:{}#helloworld.HelloReply", proto_fixture_path()),
        });
        let result = compile_declarative_step(step);
        assert!(
            result.is_ok(),
            "protobuf unmarshal should resolve: {:?}",
            result
        );
    }

    #[cfg(feature = "protobuf")]
    #[test]
    fn compile_marshal_protobuf_bad_format() {
        let step = DeclarativeStep::Marshal(DataFormatDef {
            format: "protobuf:missing.proto#nonexistent".to_string(),
        });
        let result = compile_declarative_step(step);
        assert!(result.is_err());
    }

    #[test]
    fn declarative_step_name_marshal() {
        let step = DeclarativeStep::Marshal(DataFormatDef {
            format: "json".to_string(),
        });
        assert_eq!(declarative_step_name(&step), "marshal");
    }

    #[test]
    fn declarative_step_name_unmarshal() {
        let step = DeclarativeStep::Unmarshal(DataFormatDef {
            format: "xml".to_string(),
        });
        assert_eq!(declarative_step_name(&step), "unmarshal");
    }

    #[test]
    fn compile_stream_cache_default() {
        let step = DeclarativeStep::StreamCache(StreamCacheStepDef { threshold: None });
        let result = compile_declarative_step(step);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), BuilderStep::Processor(_)));
        assert_eq!(
            stream_cache_config(
                None,
                camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD
            )
            .threshold,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD
        );
    }

    #[test]
    fn compile_stream_cache_with_threshold() {
        let threshold = 65536;
        let step = DeclarativeStep::StreamCache(StreamCacheStepDef {
            threshold: Some(threshold),
        });
        let result = compile_declarative_step(step);
        assert!(result.is_ok());
        assert_eq!(
            stream_cache_config(
                Some(threshold),
                camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD
            )
            .threshold,
            threshold
        );
    }

    #[test]
    fn declarative_step_name_stream_cache() {
        let step = DeclarativeStep::StreamCache(StreamCacheStepDef { threshold: None });
        assert_eq!(declarative_step_name(&step), "stream_cache");
    }

    #[test]
    fn compile_log_level_all_variants() {
        assert_eq!(compile_log_level(LogLevelDef::Trace), LogLevel::Trace);
        assert_eq!(compile_log_level(LogLevelDef::Debug), LogLevel::Debug);
        assert_eq!(compile_log_level(LogLevelDef::Info), LogLevel::Info);
        assert_eq!(compile_log_level(LogLevelDef::Warn), LogLevel::Warn);
        assert_eq!(compile_log_level(LogLevelDef::Error), LogLevel::Error);
    }

    #[test]
    fn compile_log_message_literal_string() {
        let msg = ValueSourceDef::Literal(serde_json::Value::String("hello".into()));
        assert_eq!(compile_log_message(msg).unwrap(), "hello");
    }

    #[test]
    fn compile_log_message_literal_number() {
        let msg = ValueSourceDef::Literal(serde_json::json!(42));
        assert_eq!(compile_log_message(msg).unwrap(), "42");
    }

    #[test]
    fn compile_log_message_expression_simple() {
        let msg = ValueSourceDef::Expression(LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        });
        assert_eq!(compile_log_message(msg).unwrap(), "${body}");
    }

    #[test]
    fn compile_log_message_expression_non_simple_rejected() {
        let msg = ValueSourceDef::Expression(LanguageExpressionDef {
            language: "rhai".into(),
            source: "1+1".into(),
        });
        assert!(compile_log_message(msg).is_err());
    }

    #[test]
    fn declarative_step_name_all_variants() {
        assert_eq!(
            declarative_step_name(&DeclarativeStep::To(ToStepDef::new("x"))),
            "to"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Log(LogStepDef::info("x"))),
            "log"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::SetHeader(SetHeaderStepDef::literal(
                "k", "v"
            ))),
            "set_header"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::SetBody(SetBodyStepDef {
                value: ValueSourceDef::Literal(serde_json::json!("x"))
            })),
            "set_body"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Filter(FilterStepDef {
                predicate: LanguageExpressionDef {
                    language: "simple".into(),
                    source: "true".into()
                },
                steps: vec![]
            })),
            "filter"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Choice(ChoiceStepDef {
                whens: vec![],
                otherwise: None
            })),
            "choice"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Split(SplitStepDef {
                expression: SplitExpressionDef::BodyLines,
                aggregation: SplitAggregationDef::LastWins,
                parallel: false,
                parallel_limit: None,
                stop_on_exception: false,
                steps: vec![]
            })),
            "split"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Aggregate(AggregateStepDef {
                header: "h".into(),
                correlation_key: None,
                completion_size: None,
                completion_timeout_ms: None,
                completion_predicate: None,
                strategy: AggregateStrategyDef::CollectAll,
                max_buckets: None,
                bucket_ttl_ms: None,
                force_completion_on_stop: None,
                discard_on_timeout: None
            })),
            "aggregate"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::WireTap(WireTapStepDef {
                uri: "x".into()
            })),
            "wire_tap"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::DynamicRouter(DynamicRouterStepDef {
                expression: LanguageExpressionDef {
                    language: "simple".into(),
                    source: "x".into()
                },
                uri_delimiter: ",".into(),
                cache_size: 1000,
                ignore_invalid_endpoints: false,
                max_iterations: 100
            })),
            "dynamic_router"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::LoadBalance(LoadBalanceStepDef {
                strategy: LoadBalanceStrategyDef::RoundRobin,
                parallel: false,
                steps: vec![]
            })),
            "load_balance"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Multicast(MulticastStepDef {
                steps: vec![],
                parallel: false,
                parallel_limit: None,
                stop_on_exception: false,
                timeout_ms: None,
                aggregation: MulticastAggregationDef::LastWins
            })),
            "multicast"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::RoutingSlip(RoutingSlipStepDef {
                expression: LanguageExpressionDef {
                    language: "simple".into(),
                    source: "x".into()
                },
                uri_delimiter: ",".into(),
                cache_size: 1000,
                ignore_invalid_endpoints: false
            })),
            "routing_slip"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::RecipientList(RecipientListStepDef {
                expression: LanguageExpressionDef {
                    language: "simple".into(),
                    source: "x".into()
                },
                delimiter: ",".into(),
                parallel: false,
                parallel_limit: None,
                stop_on_exception: false,
                aggregation: MulticastAggregationDef::LastWins
            })),
            "recipient_list"
        );
        assert_eq!(declarative_step_name(&DeclarativeStep::Stop), "stop");
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Throttle(ThrottleStepDef {
                max_requests: 10,
                period_ms: 1000,
                strategy: ThrottleStrategyDef::Delay,
                steps: vec![]
            })),
            "throttle"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Script(ScriptStepDef {
                expression: LanguageExpressionDef {
                    language: "rhai".into(),
                    source: "1".into()
                }
            })),
            "script"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::ConvertBodyTo(BodyTypeDef::Json)),
            "convert_body_to"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Bean(BeanStepDef::new("b", "m"))),
            "bean"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Delay(DelayStepDef {
                delay_ms: 100,
                dynamic_header: None
            })),
            "delay"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Loop(LoopStepDef {
                count: Some(3),
                while_predicate: None,
                steps: vec![]
            })),
            "loop"
        );
        assert_eq!(
            declarative_step_name(&DeclarativeStep::Function(FunctionStepDef {
                runtime: "deno".into(),
                source: "return {};".into(),
                timeout_ms: None,
            })),
            "function"
        );
    }

    #[test]
    fn compile_function_step_default_timeout() {
        let step = DeclarativeStep::Function(FunctionStepDef {
            runtime: "deno".into(),
            source: "return {};".into(),
            timeout_ms: None,
        });
        let compiled = compile_declarative_step(step).unwrap();
        match compiled {
            BuilderStep::DeclarativeFunction { definition } => {
                assert_eq!(definition.timeout_ms, 5000);
                assert_eq!(definition.runtime, "deno");
            }
            other => panic!("expected DeclarativeFunction, got {other:?}"),
        }
    }

    #[test]
    fn compile_circuit_breaker_def() {
        let def = DeclarativeCircuitBreaker {
            failure_threshold: 3,
            open_duration_ms: 5000,
        };
        let config = compile_circuit_breaker(def);
        assert_eq!(config.failure_threshold, 3);
        assert_eq!(config.open_duration, Duration::from_millis(5000));
    }

    #[test]
    fn ensure_known_exception_kind_valid() {
        assert!(ensure_known_exception_kind("Io").is_ok());
        assert!(ensure_known_exception_kind("ProcessorError").is_ok());
        assert!(ensure_known_exception_kind("ConsumerStopping").is_ok());
    }

    #[test]
    fn ensure_known_exception_kind_invalid() {
        assert!(ensure_known_exception_kind("NoSuchError").is_err());
    }

    #[test]
    fn exception_kind_matches_consumer_stopping() {
        assert!(exception_kind_matches(
            "ConsumerStopping",
            &CamelError::ConsumerStopping
        ));
        assert!(!exception_kind_matches(
            "ConsumerStopping",
            &CamelError::ProcessorError("x".into())
        ));
    }

    #[test]
    fn exception_kind_matches_stopped_arm_removed() {
        // ADR-0024 + user directive 2026-06-20 (no deprecation paths):
        // The "Stopped" arm was removed because it would never fire — Stop EIP
        // is no longer an error at the top-level (CompiledStep::Stop → Stopped → Ok).
        // Sub-pipeline Stop propagation is bypassed in run_steps before the handler.
        assert!(!exception_kind_matches(
            "Stopped",
            &CamelError::ConsumerStopping
        ));
        // Note: CamelError::Stopped variant still exists (deferred to bd rc-5uv
        // for full removal) but the DSL arm is gone because the arm would never fire.
    }

    #[test]
    fn compile_aggregate_step_timeout_and_size() {
        let def = AggregateStepDef {
            header: "corr".into(),
            correlation_key: None,
            completion_size: Some(5),
            completion_timeout_ms: Some(2000),
            completion_predicate: None,
            strategy: AggregateStrategyDef::CollectAll,
            max_buckets: None,
            bucket_ttl_ms: None,
            force_completion_on_stop: None,
            discard_on_timeout: None,
        };
        let result = compile_aggregate_step(def);
        assert!(result.is_ok());
    }

    #[test]
    fn compile_aggregate_step_timeout_only() {
        let def = AggregateStepDef {
            header: "corr".into(),
            correlation_key: None,
            completion_size: None,
            completion_timeout_ms: Some(1000),
            completion_predicate: None,
            strategy: AggregateStrategyDef::CollectAll,
            max_buckets: None,
            bucket_ttl_ms: None,
            force_completion_on_stop: None,
            discard_on_timeout: None,
        };
        let result = compile_aggregate_step(def);
        assert!(result.is_ok());
    }

    #[test]
    fn compile_aggregate_step_rejects_predicate() {
        let def = AggregateStepDef {
            header: "corr".into(),
            correlation_key: None,
            completion_size: None,
            completion_timeout_ms: None,
            completion_predicate: Some(make_predicate("true")),
            strategy: AggregateStrategyDef::CollectAll,
            max_buckets: None,
            bucket_ttl_ms: None,
            force_completion_on_stop: None,
            discard_on_timeout: None,
        };
        assert!(compile_aggregate_step(def).is_err());
    }

    #[test]
    fn compile_aggregate_step_with_extras() {
        let def = AggregateStepDef {
            header: "corr".into(),
            correlation_key: None,
            completion_size: Some(3),
            completion_timeout_ms: None,
            completion_predicate: None,
            strategy: AggregateStrategyDef::CollectAll,
            max_buckets: Some(100),
            bucket_ttl_ms: Some(60000),
            force_completion_on_stop: Some(true),
            discard_on_timeout: Some(true),
        };
        let result = compile_aggregate_step(def);
        assert!(result.is_ok());
    }

    #[test]
    fn compile_step_to() {
        let step = DeclarativeStep::To(ToStepDef::new("direct:a"));
        let result = compile_declarative_step(step);
        assert!(matches!(result.unwrap(), BuilderStep::To(u) if u == "direct:a"));
    }

    #[test]
    fn compile_step_wire_tap() {
        let step = DeclarativeStep::WireTap(WireTapStepDef {
            uri: "log:tap".into(),
        });
        let result = compile_declarative_step(step);
        assert!(matches!(result.unwrap(), BuilderStep::WireTap { uri } if uri == "log:tap"));
    }

    #[test]
    fn compile_step_stop() {
        let step = DeclarativeStep::Stop;
        let result = compile_declarative_step(step);
        assert!(result.is_ok());
    }

    #[test]
    fn compile_step_bean() {
        let step = DeclarativeStep::Bean(BeanStepDef::new("myBean", "process"));
        let result = compile_declarative_step(step);
        match result.unwrap() {
            BuilderStep::Bean { name, method } => {
                assert_eq!(name, "myBean");
                assert_eq!(method, "process");
            }
            other => panic!("expected Bean, got {other:?}"),
        }
    }

    #[test]
    fn compile_step_delay() {
        let step = DeclarativeStep::Delay(DelayStepDef {
            delay_ms: 500,
            dynamic_header: None,
        });
        let result = compile_declarative_step(step);
        assert!(matches!(result.unwrap(), BuilderStep::Delay { .. }));
    }

    #[test]
    fn compile_step_delay_with_header() {
        let step = DeclarativeStep::Delay(DelayStepDef {
            delay_ms: 200,
            dynamic_header: Some("X-D".into()),
        });
        let result = compile_declarative_step(step);
        assert!(result.is_ok());
    }

    #[test]
    fn compile_step_dynamic_router() {
        let step = DeclarativeStep::DynamicRouter(DynamicRouterStepDef {
            expression: make_predicate("x"),
            uri_delimiter: ",".into(),
            cache_size: 500,
            ignore_invalid_endpoints: true,
            max_iterations: 100,
        });
        let result = compile_declarative_step(step);
        assert!(matches!(
            result.unwrap(),
            BuilderStep::DeclarativeDynamicRouter { .. }
        ));
    }

    #[test]
    fn compile_step_routing_slip() {
        let step = DeclarativeStep::RoutingSlip(RoutingSlipStepDef {
            expression: make_predicate("x"),
            uri_delimiter: ",".into(),
            cache_size: 500,
            ignore_invalid_endpoints: false,
        });
        let result = compile_declarative_step(step);
        assert!(matches!(
            result.unwrap(),
            BuilderStep::DeclarativeRoutingSlip { .. }
        ));
    }

    #[test]
    fn compile_step_recipient_list() {
        let step = DeclarativeStep::RecipientList(RecipientListStepDef {
            expression: make_predicate("x"),
            delimiter: ",".into(),
            parallel: true,
            parallel_limit: Some(4),
            stop_on_exception: false,
            aggregation: MulticastAggregationDef::CollectAll,
        });
        let result = compile_declarative_step(step);
        assert!(matches!(
            result.unwrap(),
            BuilderStep::DeclarativeRecipientList { .. }
        ));
    }

    #[test]
    fn compile_step_script() {
        let step = DeclarativeStep::Script(ScriptStepDef {
            expression: make_predicate("1+1"),
        });
        let result = compile_declarative_step(step);
        assert!(matches!(
            result.unwrap(),
            BuilderStep::DeclarativeScript { .. }
        ));
    }

    #[test]
    fn compile_step_throttle() {
        let step = DeclarativeStep::Throttle(ThrottleStepDef {
            max_requests: 10,
            period_ms: 1000,
            strategy: ThrottleStrategyDef::Reject,
            steps: vec![],
        });
        let result = compile_declarative_step(step);
        assert!(matches!(result.unwrap(), BuilderStep::Throttle { .. }));
    }

    #[test]
    fn compile_step_load_balance_round_robin() {
        let step = DeclarativeStep::LoadBalance(LoadBalanceStepDef {
            strategy: LoadBalanceStrategyDef::RoundRobin,
            parallel: false,
            steps: vec![DeclarativeStep::To(ToStepDef::new("direct:a"))],
        });
        let result = compile_declarative_step(step);
        assert!(matches!(result.unwrap(), BuilderStep::LoadBalance { .. }));
    }

    #[test]
    fn compile_step_load_balance_weighted() {
        let step = DeclarativeStep::LoadBalance(LoadBalanceStepDef {
            strategy: LoadBalanceStrategyDef::Weighted {
                distribution_ratio: "3,1".into(),
            },
            parallel: false,
            steps: vec![
                DeclarativeStep::To(ToStepDef::new("direct:a")),
                DeclarativeStep::To(ToStepDef::new("direct:b")),
            ],
        });
        let result = compile_declarative_step(step);
        assert!(result.is_ok());
    }

    #[test]
    fn compile_step_load_balance_weighted_bad_ratio() {
        let step = DeclarativeStep::LoadBalance(LoadBalanceStepDef {
            strategy: LoadBalanceStrategyDef::Weighted {
                distribution_ratio: "abc".into(),
            },
            parallel: false,
            steps: vec![DeclarativeStep::To(ToStepDef::new("direct:a"))],
        });
        assert!(compile_declarative_step(step).is_err());
    }

    #[test]
    fn compile_step_load_balance_weighted_mismatched_count() {
        let step = DeclarativeStep::LoadBalance(LoadBalanceStepDef {
            strategy: LoadBalanceStrategyDef::Weighted {
                distribution_ratio: "3,1".into(),
            },
            parallel: false,
            steps: vec![DeclarativeStep::To(ToStepDef::new("direct:a"))],
        });
        assert!(compile_declarative_step(step).is_err());
    }

    #[test]
    fn compile_step_convert_body_to_text() {
        let step = DeclarativeStep::ConvertBodyTo(BodyTypeDef::Text);
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_convert_body_to_json() {
        let step = DeclarativeStep::ConvertBodyTo(BodyTypeDef::Json);
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_convert_body_to_bytes() {
        let step = DeclarativeStep::ConvertBodyTo(BodyTypeDef::Bytes);
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_convert_body_to_xml() {
        let step = DeclarativeStep::ConvertBodyTo(BodyTypeDef::Xml);
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_convert_body_to_empty() {
        let step = DeclarativeStep::ConvertBodyTo(BodyTypeDef::Empty);
        assert!(compile_declarative_step(step).is_ok());
    }

    fn make_when(simple_src: &str) -> WhenStepDef {
        WhenStepDef {
            predicate: make_predicate(simple_src),
            steps: vec![],
        }
    }

    #[test]
    fn compile_step_choice_with_otherwise() {
        let step = DeclarativeStep::Choice(ChoiceStepDef {
            whens: vec![make_when("true")],
            otherwise: Some(vec![DeclarativeStep::Stop]),
        });
        let result = compile_declarative_step(step);
        assert!(matches!(
            result.unwrap(),
            BuilderStep::DeclarativeChoice { .. }
        ));
    }

    #[test]
    fn compile_step_choice_without_otherwise() {
        let step = DeclarativeStep::Choice(ChoiceStepDef {
            whens: vec![],
            otherwise: None,
        });
        let result = compile_declarative_step(step);
        assert!(matches!(
            result.unwrap(),
            BuilderStep::DeclarativeChoice { .. }
        ));
    }

    #[test]
    fn compile_step_filter() {
        let step = DeclarativeStep::Filter(FilterStepDef {
            predicate: make_predicate("true"),
            steps: vec![DeclarativeStep::Stop],
        });
        let result = compile_declarative_step(step);
        assert!(matches!(
            result.unwrap(),
            BuilderStep::DeclarativeFilter { .. }
        ));
    }

    #[test]
    fn compile_step_multicast() {
        let step = DeclarativeStep::Multicast(MulticastStepDef {
            steps: vec![DeclarativeStep::To(ToStepDef::new("direct:a"))],
            parallel: true,
            parallel_limit: Some(2),
            stop_on_exception: false,
            timeout_ms: Some(5000),
            aggregation: MulticastAggregationDef::CollectAll,
        });
        let result = compile_declarative_step(step);
        assert!(matches!(result.unwrap(), BuilderStep::Multicast { .. }));
    }

    #[test]
    fn compile_step_split_body_lines() {
        let step = DeclarativeStep::Split(SplitStepDef {
            expression: SplitExpressionDef::BodyLines,
            aggregation: SplitAggregationDef::LastWins,
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            steps: vec![],
        });
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_split_body_json_array() {
        let step = DeclarativeStep::Split(SplitStepDef {
            expression: SplitExpressionDef::BodyJsonArray,
            aggregation: SplitAggregationDef::CollectAll,
            parallel: true,
            parallel_limit: Some(4),
            stop_on_exception: true,
            steps: vec![],
        });
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_split_language() {
        let step = DeclarativeStep::Split(SplitStepDef {
            expression: SplitExpressionDef::Language(make_predicate("x")),
            aggregation: SplitAggregationDef::Original,
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            steps: vec![],
        });
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_log_expression() {
        let step = DeclarativeStep::Log(LogStepDef {
            message: ValueSourceDef::Expression(make_predicate("${body}")),
            level: LogLevelDef::Info,
        });
        let result = compile_declarative_step(step);
        assert!(matches!(
            result.unwrap(),
            BuilderStep::DeclarativeLog { .. }
        ));
    }

    #[test]
    fn compile_step_set_header() {
        let step = DeclarativeStep::SetHeader(SetHeaderStepDef {
            key: "myKey".into(),
            value: ValueSourceDef::Literal(serde_json::json!("myValue")),
        });
        let result = compile_declarative_step(step);
        assert!(matches!(
            result.unwrap(),
            BuilderStep::DeclarativeSetHeader { .. }
        ));
    }

    #[test]
    fn compile_step_set_body() {
        let step = DeclarativeStep::SetBody(SetBodyStepDef {
            value: ValueSourceDef::Literal(serde_json::json!("hello")),
        });
        let result = compile_declarative_step(step);
        assert!(matches!(
            result.unwrap(),
            BuilderStep::DeclarativeSetBody { .. }
        ));
    }

    #[test]
    fn compile_declarative_route_minimal() {
        let route = DeclarativeRoute {
            from: "direct:start".into(),
            route_id: "test-route".into(),
            auto_startup: true,
            startup_order: 1000,
            concurrency: None,
            error_handler: None,
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: None,
            steps: vec![DeclarativeStep::Stop],
        };
        let result = compile_declarative_route(route);
        assert!(result.is_ok());
    }

    #[test]
    fn compile_declarative_route_with_error_handler() {
        let route = DeclarativeRoute {
            from: "direct:start".into(),
            route_id: "test-route".into(),
            auto_startup: true,
            startup_order: 1000,
            concurrency: None,
            error_handler: Some(DeclarativeErrorHandler {
                dead_letter_channel: Some("log:dlq".into()),
                retry: None,
                on_exceptions: None,
            }),
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: None,
            steps: vec![],
        };
        let result = compile_declarative_route(route);
        assert!(result.is_ok());
    }

    #[test]
    fn compile_declarative_route_with_circuit_breaker() {
        let route = DeclarativeRoute {
            from: "direct:start".into(),
            route_id: "test-route".into(),
            auto_startup: false,
            startup_order: 500,
            concurrency: Some(DeclarativeConcurrency::Concurrent { max: Some(4) }),
            error_handler: None,
            circuit_breaker: Some(DeclarativeCircuitBreaker {
                failure_threshold: 3,
                open_duration_ms: 5000,
            }),
            security_policy: None,
            unit_of_work: None,
            steps: vec![DeclarativeStep::To(ToStepDef::new("log:out"))],
        };
        let result = compile_declarative_route(route);
        assert!(result.is_ok());
    }

    #[test]
    fn compile_step_load_balance_failover() {
        let step = DeclarativeStep::LoadBalance(LoadBalanceStepDef {
            strategy: LoadBalanceStrategyDef::Failover,
            parallel: true,
            steps: vec![DeclarativeStep::To(ToStepDef::new("direct:a"))],
        });
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_load_balance_random() {
        let step = DeclarativeStep::LoadBalance(LoadBalanceStepDef {
            strategy: LoadBalanceStrategyDef::Random,
            parallel: false,
            steps: vec![DeclarativeStep::To(ToStepDef::new("direct:a"))],
        });
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_throttle_drop() {
        let step = DeclarativeStep::Throttle(ThrottleStepDef {
            max_requests: 5,
            period_ms: 500,
            strategy: ThrottleStrategyDef::Drop,
            steps: vec![DeclarativeStep::To(ToStepDef::new("direct:a"))],
        });
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_throttle_delay() {
        let step = DeclarativeStep::Throttle(ThrottleStepDef {
            max_requests: 100,
            period_ms: 2000,
            strategy: ThrottleStrategyDef::Delay,
            steps: vec![],
        });
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_log_literal_number() {
        let step = DeclarativeStep::Log(LogStepDef {
            message: ValueSourceDef::Literal(serde_json::json!(42)),
            level: LogLevelDef::Debug,
        });
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_step_log_literal_string() {
        let step = DeclarativeStep::Log(LogStepDef {
            message: ValueSourceDef::Literal(serde_json::Value::String("hello".into())),
            level: LogLevelDef::Warn,
        });
        assert!(compile_declarative_step(step).is_ok());
    }

    #[test]
    fn compile_canonical_step_covers_all_variants() {
        use camel_api::LanguageExpressionDef;
        use camel_api::runtime::{
            CanonicalAggregateSpec, CanonicalAggregateStrategySpec, CanonicalSplitAggregationSpec,
            CanonicalSplitExpressionSpec, CanonicalStepSpec, CanonicalWhenSpec,
        };

        let threshold = camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD;

        let step = compile_canonical_step(
            CanonicalStepSpec::To {
                uri: "log:out".into(),
            },
            threshold,
        )
        .unwrap();
        assert!(matches!(step, BuilderStep::To(_)));

        let step = compile_canonical_step(
            CanonicalStepSpec::Log {
                message: "hello".into(),
            },
            threshold,
        )
        .unwrap();
        assert!(matches!(step, BuilderStep::Log { .. }));

        let step = compile_canonical_step(
            CanonicalStepSpec::WireTap {
                uri: "direct:audit".into(),
            },
            threshold,
        )
        .unwrap();
        assert!(matches!(step, BuilderStep::WireTap { .. }));

        let step = compile_canonical_step(CanonicalStepSpec::Stop, threshold).unwrap();
        assert!(matches!(step, BuilderStep::Stop));

        let step = compile_canonical_step(
            CanonicalStepSpec::Script {
                expression: LanguageExpressionDef {
                    language: "simple".into(),
                    source: "${body}".into(),
                },
            },
            threshold,
        )
        .unwrap();
        assert!(matches!(step, BuilderStep::DeclarativeScript { .. }));

        let step = compile_canonical_step(
            CanonicalStepSpec::Delay {
                delay_ms: 500,
                dynamic_header: None,
            },
            threshold,
        )
        .unwrap();
        assert!(matches!(step, BuilderStep::Delay { .. }));

        let step = compile_canonical_step(
            CanonicalStepSpec::Filter {
                predicate: LanguageExpressionDef {
                    language: "simple".into(),
                    source: "${body} != null".into(),
                },
                steps: vec![CanonicalStepSpec::Stop],
            },
            threshold,
        )
        .unwrap();
        assert!(matches!(step, BuilderStep::DeclarativeFilter { .. }));

        let step = compile_canonical_step(
            CanonicalStepSpec::Choice {
                whens: vec![CanonicalWhenSpec {
                    predicate: LanguageExpressionDef {
                        language: "simple".into(),
                        source: "${body} == 1".into(),
                    },
                    steps: vec![CanonicalStepSpec::To {
                        uri: "mock:a".into(),
                    }],
                }],
                otherwise: Some(vec![CanonicalStepSpec::To {
                    uri: "mock:b".into(),
                }]),
            },
            threshold,
        )
        .unwrap();
        assert!(matches!(step, BuilderStep::DeclarativeChoice { .. }));

        let step = compile_canonical_step(
            CanonicalStepSpec::Split {
                expression: CanonicalSplitExpressionSpec::BodyLines,
                aggregation: CanonicalSplitAggregationSpec::CollectAll,
                parallel: false,
                parallel_limit: None,
                stop_on_exception: false,
                steps: vec![CanonicalStepSpec::To {
                    uri: "log:line".into(),
                }],
            },
            threshold,
        )
        .unwrap();
        assert!(matches!(step, BuilderStep::Split { .. }));

        let step = compile_canonical_step(
            CanonicalStepSpec::Split {
                expression: CanonicalSplitExpressionSpec::Language(LanguageExpressionDef {
                    language: "simple".into(),
                    source: "${body.items}".into(),
                }),
                aggregation: CanonicalSplitAggregationSpec::Original,
                parallel: false,
                parallel_limit: None,
                stop_on_exception: false,
                steps: vec![CanonicalStepSpec::Stop],
            },
            threshold,
        )
        .unwrap();
        assert!(matches!(step, BuilderStep::DeclarativeSplit { .. }));

        let step = compile_canonical_step(
            CanonicalStepSpec::Aggregate(CanonicalAggregateSpec {
                header: "corr-id".into(),
                completion_size: Some(5),
                completion_timeout_ms: Some(1000),
                correlation_key: Some("simple:${header.id}".into()),
                force_completion_on_stop: Some(true),
                discard_on_timeout: Some(false),
                strategy: CanonicalAggregateStrategySpec::CollectAll,
                max_buckets: Some(100),
                bucket_ttl_ms: Some(60000),
            }),
            threshold,
        )
        .unwrap();
        assert!(matches!(step, BuilderStep::Aggregate { .. }));
    }

    // --- DSL-level validation tests (DSL-001, DSL-002, DSL-004) ---

    fn make_basic_route(from: &str, steps: Vec<DeclarativeStep>) -> DeclarativeRoute {
        DeclarativeRoute {
            from: from.into(),
            route_id: "test-route".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: None,
            error_handler: None,
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: None,
            steps,
        }
    }

    fn assert_config_error(result: Result<RouteDefinition, CamelError>, needle: &str) {
        let err = result.err().expect("expected Config error");
        assert!(
            matches!(err, CamelError::Config(ref s) if s.contains(needle)),
            "expected Config error containing '{needle}', got: {err:?}"
        );
    }

    #[test]
    fn test_empty_from_uri_rejected() {
        let route = make_basic_route("", vec![]);
        assert_config_error(compile_declarative_route(route), "'from'");
    }

    #[test]
    fn test_whitespace_from_uri_rejected() {
        let route = make_basic_route("   ", vec![]);
        assert_config_error(compile_declarative_route(route), "'from'");
    }

    #[test]
    fn test_empty_to_uri_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::To(ToStepDef::new(""))],
        );
        assert_config_error(compile_declarative_route(route), "'to'");
    }

    #[test]
    fn test_whitespace_to_uri_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::To(ToStepDef::new("  "))],
        );
        assert_config_error(compile_declarative_route(route), "'to'");
    }

    #[test]
    fn test_empty_wire_tap_uri_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::WireTap(WireTapStepDef {
                uri: "  ".into(),
            })],
        );
        assert_config_error(compile_declarative_route(route), "'wire_tap'");
    }

    #[test]
    fn test_empty_set_header_key_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::SetHeader(SetHeaderStepDef::literal(
                "", "value",
            ))],
        );
        assert_config_error(compile_declarative_route(route), "set_header");
    }

    #[test]
    fn test_whitespace_set_header_key_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::SetHeader(SetHeaderStepDef::literal(
                "  ", "value",
            ))],
        );
        assert_config_error(compile_declarative_route(route), "set_header");
    }

    #[test]
    fn test_empty_set_property_key_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::SetProperty(SetPropertyStepDef::literal(
                "", "value",
            ))],
        );
        assert_config_error(compile_declarative_route(route), "set_property");
    }

    #[test]
    fn test_zero_throttle_max_requests_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::Throttle(ThrottleStepDef {
                max_requests: 0,
                period_ms: 1000,
                strategy: ThrottleStrategyDef::Reject,
                steps: vec![],
            })],
        );
        assert_config_error(compile_declarative_route(route), "max_requests");
    }

    #[test]
    fn test_zero_throttle_period_ms_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::Throttle(ThrottleStepDef {
                max_requests: 10,
                period_ms: 0,
                strategy: ThrottleStrategyDef::Reject,
                steps: vec![],
            })],
        );
        assert_config_error(compile_declarative_route(route), "period_ms");
    }

    #[test]
    fn test_zero_delay_ms_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::Delay(DelayStepDef {
                delay_ms: 0,
                dynamic_header: None,
            })],
        );
        assert_config_error(compile_declarative_route(route), "delay_ms");
    }

    #[test]
    fn test_zero_loop_count_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::Loop(LoopStepDef {
                count: Some(0),
                while_predicate: None,
                steps: vec![],
            })],
        );
        assert_config_error(compile_declarative_route(route), "loop count");
    }

    #[test]
    fn test_loop_rejects_count_and_predicate_together() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::Loop(LoopStepDef {
                count: Some(2),
                while_predicate: Some(make_predicate("${header.go} == true")),
                steps: vec![],
            })],
        );
        assert_config_error(
            compile_declarative_route(route),
            "loop cannot have both count and predicate",
        );
    }

    #[test]
    fn test_aggregate_requires_correlation_key() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::Aggregate(AggregateStepDef {
                header: "orderId".to_string(),
                correlation_key: None,
                completion_size: Some(2),
                completion_timeout_ms: None,
                completion_predicate: None,
                strategy: AggregateStrategyDef::CollectAll,
                max_buckets: None,
                bucket_ttl_ms: None,
                force_completion_on_stop: None,
                discard_on_timeout: None,
            })],
        );
        assert_config_error(
            compile_declarative_route(route),
            "aggregate requires a correlation_key",
        );
    }

    #[test]
    fn test_zero_multicast_timeout_rejected() {
        let route = make_basic_route(
            "direct:start",
            vec![DeclarativeStep::Multicast(MulticastStepDef {
                steps: vec![],
                parallel: false,
                parallel_limit: None,
                stop_on_exception: false,
                timeout_ms: Some(0),
                aggregation: MulticastAggregationDef::LastWins,
            })],
        );
        assert_config_error(compile_declarative_route(route), "multicast");
    }

    #[test]
    fn test_zero_circuit_breaker_failure_threshold_rejected() {
        let mut route = make_basic_route("direct:start", vec![]);
        route.circuit_breaker = Some(DeclarativeCircuitBreaker {
            failure_threshold: 0,
            open_duration_ms: 5000,
        });
        assert_config_error(compile_declarative_route(route), "failure_threshold");
    }

    #[test]
    fn test_zero_circuit_breaker_open_duration_rejected() {
        let mut route = make_basic_route("direct:start", vec![]);
        route.circuit_breaker = Some(DeclarativeCircuitBreaker {
            failure_threshold: 3,
            open_duration_ms: 0,
        });
        assert_config_error(compile_declarative_route(route), "open_duration_ms");
    }

    #[test]
    fn test_zero_redelivery_max_attempts_rejected() {
        let mut route = make_basic_route("direct:start", vec![]);
        route.error_handler = Some(DeclarativeErrorHandler {
            dead_letter_channel: None,
            retry: Some(DeclarativeRedeliveryPolicy {
                max_attempts: 0,
                initial_delay_ms: 100,
                multiplier: 2.0,
                max_delay_ms: 1000,
                jitter_factor: 0.0,
                handled_by: None,
            }),
            on_exceptions: None,
        });
        assert_config_error(compile_declarative_route(route), "max_attempts");
    }

    #[test]
    fn test_valid_route_passes_validation() {
        let route = make_basic_route(
            "direct:start",
            vec![
                DeclarativeStep::SetHeader(SetHeaderStepDef::literal("X-Trace", "123")),
                DeclarativeStep::To(ToStepDef::new("log:output")),
            ],
        );
        assert!(compile_declarative_route(route).is_ok());
    }

    struct TestAuthenticator;

    #[async_trait]
    impl camel_auth::TokenAuthenticator for TestAuthenticator {
        async fn authenticate_bearer(
            &self,
            _token: &str,
        ) -> Result<camel_api::security_policy::Principal, CamelError> {
            Ok(camel_api::security_policy::Principal {
                subject: "test-user".into(),
                issuer: "test-issuer".into(),
                audience: vec![],
                scopes: vec!["read:api".into()],
                roles: vec!["admin".into()],
                claims: serde_json::Value::Null,
            })
        }
    }

    struct TestPolicy;

    #[async_trait]
    impl camel_api::security_policy::SecurityPolicy for TestPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut camel_api::Exchange,
        ) -> Result<camel_api::security_policy::AuthorizationDecision, CamelError> {
            Ok(camel_api::security_policy::AuthorizationDecision::Granted {
                principal: camel_api::security_policy::Principal {
                    subject: "test".into(),
                    issuer: "test".into(),
                    audience: vec![],
                    scopes: vec![],
                    roles: vec![],
                    claims: serde_json::Value::Null,
                },
            })
        }
    }

    #[test]
    fn compile_security_policy_roles() {
        let auth = std::sync::Arc::new(TestAuthenticator);
        let ctx = SecurityCompileContext::new(Some(auth), None);
        let def = DeclarativeSecurityPolicy::Roles {
            roles: vec!["admin".into()],
            all_required: true,
        };
        let (_config, returned_auth) = compile_security_policy(def, &ctx).unwrap();
        assert!(returned_auth.is_some());
    }

    #[test]
    fn compile_security_policy_scopes() {
        let auth = std::sync::Arc::new(TestAuthenticator);
        let ctx = SecurityCompileContext::new(Some(auth), None);
        let def = DeclarativeSecurityPolicy::Scopes {
            scopes: vec!["read".into()],
            all_required: false,
        };
        let (_config, returned_auth) = compile_security_policy(def, &ctx).unwrap();
        assert!(returned_auth.is_some());
    }

    #[test]
    fn compile_security_policy_roles_without_authenticator_fails() {
        let ctx = SecurityCompileContext::default();
        let def = DeclarativeSecurityPolicy::Roles {
            roles: vec!["admin".into()],
            all_required: true,
        };
        let result = compile_security_policy(def, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn compile_security_policy_ref_from_registry() {
        let registry = std::sync::Arc::new(camel_auth::SecurityPolicyRegistry::new());
        registry.register("my-policy", std::sync::Arc::new(TestPolicy));
        let ctx = SecurityCompileContext::new(None, Some(registry));
        let def = DeclarativeSecurityPolicy::Ref {
            name: "my-policy".into(),
        };
        let (_config, returned_auth) = compile_security_policy(def, &ctx).unwrap();
        assert!(returned_auth.is_none());
    }

    #[test]
    fn compile_security_policy_ref_missing_from_registry() {
        let registry = std::sync::Arc::new(camel_auth::SecurityPolicyRegistry::new());
        let ctx = SecurityCompileContext::new(None, Some(registry));
        let def = DeclarativeSecurityPolicy::Ref {
            name: "nonexistent".into(),
        };
        let result = compile_security_policy(def, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn compile_security_policy_ref_without_registry_fails() {
        let ctx = SecurityCompileContext::default();
        let def = DeclarativeSecurityPolicy::Ref {
            name: "my-policy".into(),
        };
        let result = compile_security_policy(def, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn compile_security_policy_wasm_resolves_from_registry() {
        let registry = std::sync::Arc::new(camel_auth::SecurityPolicyRegistry::new());
        registry.register("plugin.wasm", std::sync::Arc::new(TestPolicy));
        let ctx = SecurityCompileContext::new(None, Some(registry));
        let def = DeclarativeSecurityPolicy::Wasm {
            path: "plugin.wasm".into(),
            config: Default::default(),
        };
        let (_config, returned_auth) = compile_security_policy(def, &ctx).unwrap();
        assert!(returned_auth.is_none());
    }

    #[test]
    fn compile_security_policy_wasm_with_yaml_config_rejected() {
        let registry = std::sync::Arc::new(camel_auth::SecurityPolicyRegistry::new());
        registry.register("plugin.wasm", std::sync::Arc::new(TestPolicy));
        let ctx = SecurityCompileContext::new(None, Some(registry));
        let mut config: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        config.insert("ldap_url".to_string(), "ldap://corp".to_string());
        let def = DeclarativeSecurityPolicy::Wasm {
            path: "plugin.wasm".into(),
            config,
        };
        let result = compile_security_policy(def, &ctx);
        assert!(result.is_err(), "expected error for non-empty config");
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => unreachable!(),
        };
        assert!(
            err_msg.contains("ADR-0014 §4"),
            "error must cite ADR-0014 §4, got: {err_msg}"
        );
        assert!(
            err_msg.contains("Camel.toml"),
            "error must point to Camel.toml, got: {err_msg}"
        );
    }

    #[test]
    fn compile_security_policy_wasm_missing_registry_returns_error() {
        let ctx = SecurityCompileContext::default();
        let def = DeclarativeSecurityPolicy::Wasm {
            path: "plugin.wasm".into(),
            config: Default::default(),
        };
        let result = compile_security_policy(def, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn compile_security_policy_wasm_not_in_registry_returns_error() {
        let registry = std::sync::Arc::new(camel_auth::SecurityPolicyRegistry::new());
        let ctx = SecurityCompileContext::new(None, Some(registry));
        let def = DeclarativeSecurityPolicy::Wasm {
            path: "missing.wasm".into(),
            config: Default::default(),
        };
        let result = compile_security_policy(def, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn compile_security_policy_permission_from_registry() {
        use camel_auth::{PermissionDecision, PermissionEvaluator, PermissionRequest};

        struct GrantEvaluator;
        #[async_trait::async_trait]
        impl PermissionEvaluator for GrantEvaluator {
            async fn evaluate(
                &self,
                _request: PermissionRequest,
            ) -> Result<PermissionDecision, camel_auth::AuthError> {
                Ok(PermissionDecision::Granted)
            }
        }

        let eval_reg = std::sync::Arc::new(camel_auth::PermissionEvaluatorRegistry::new());
        eval_reg.register("keycloak-uma", std::sync::Arc::new(GrantEvaluator));
        let ctx = SecurityCompileContext::default().with_evaluator_registry(eval_reg);
        let def = DeclarativeSecurityPolicy::Permission {
            policy: "keycloak-uma".into(),
            resource: camel_auth::PermissionValueSource::Header("x-resource".into()),
            action: camel_auth::PermissionValueSource::Header("x-action".into()),
            scopes: vec![],
            context: camel_auth::PermissionContextConfig::default(),
            cache_ttl_secs: Some(60),
            cache_negative_ttl_secs: None,
        };
        let (_config, returned_auth) = compile_security_policy(def, &ctx).unwrap();
        assert!(returned_auth.is_none());
    }

    #[test]
    fn compile_security_policy_permission_missing_from_registry() {
        let eval_reg = std::sync::Arc::new(camel_auth::PermissionEvaluatorRegistry::new());
        let ctx = SecurityCompileContext::default().with_evaluator_registry(eval_reg);
        let def = DeclarativeSecurityPolicy::Permission {
            policy: "nonexistent".into(),
            resource: camel_auth::PermissionValueSource::Header("x-resource".into()),
            action: camel_auth::PermissionValueSource::Header("x-action".into()),
            scopes: vec![],
            context: camel_auth::PermissionContextConfig::default(),
            cache_ttl_secs: None,
            cache_negative_ttl_secs: None,
        };
        let result = compile_security_policy(def, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn compile_security_policy_permission_without_registry_fails() {
        let ctx = SecurityCompileContext::default();
        let def = DeclarativeSecurityPolicy::Permission {
            policy: "keycloak-uma".into(),
            resource: camel_auth::PermissionValueSource::Header("x-resource".into()),
            action: camel_auth::PermissionValueSource::Header("x-action".into()),
            scopes: vec![],
            context: camel_auth::PermissionContextConfig::default(),
            cache_ttl_secs: None,
            cache_negative_ttl_secs: None,
        };
        let result = compile_security_policy(def, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn compile_security_policy_sets_both_policy_and_authenticator() {
        let auth = std::sync::Arc::new(TestAuthenticator);
        let ctx = SecurityCompileContext::new(Some(auth), None);
        let route = DeclarativeRoute {
            from: "direct:start".into(),
            route_id: "test".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: None,
            error_handler: None,
            circuit_breaker: None,
            security_policy: Some(DeclarativeSecurityPolicy::Roles {
                roles: vec!["admin".into()],
                all_required: true,
            }),
            unit_of_work: None,
            steps: vec![DeclarativeStep::To(ToStepDef {
                uri: "log:info".into(),
            })],
        };
        let def = compile_declarative_route_with_stream_cache_threshold(route, 1024, ctx).unwrap();
        assert!(def.security_policy_config().is_some());
        assert!(def.security_authenticator().is_some());
    }

    #[test]
    fn compile_declarative_route_to_canonical_rejects_security_policy() {
        let route = DeclarativeRoute {
            from: "direct:start".into(),
            route_id: "test".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: None,
            error_handler: None,
            circuit_breaker: None,
            security_policy: Some(DeclarativeSecurityPolicy::Roles {
                roles: vec!["admin".into()],
                all_required: true,
            }),
            unit_of_work: None,
            steps: vec![DeclarativeStep::To(ToStepDef {
                uri: "log:info".into(),
            })],
        };
        let result = compile_declarative_route_to_canonical(route, false);
        assert!(result.is_err());
    }

    #[test]
    fn compile_declarative_route_to_canonical_rejects_error_handler() {
        let route = DeclarativeRoute {
            from: "direct:start".into(),
            route_id: "test".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: None,
            error_handler: Some(DeclarativeErrorHandler {
                dead_letter_channel: Some("log:dlq".into()),
                retry: None,
                on_exceptions: None,
            }),
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: None,
            steps: vec![DeclarativeStep::To(ToStepDef {
                uri: "log:info".into(),
            })],
        };
        let err = compile_declarative_route_to_canonical(route, false)
            .err()
            .expect("expected error");
        assert!(err.to_string().contains("error_handler"));
    }

    #[test]
    fn compile_declarative_route_to_canonical_rejects_unit_of_work() {
        let route = DeclarativeRoute {
            from: "direct:start".into(),
            route_id: "test".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: None,
            error_handler: None,
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: Some(camel_api::UnitOfWorkConfig {
                on_complete: Some("log:done".into()),
                on_failure: None,
            }),
            steps: vec![DeclarativeStep::To(ToStepDef {
                uri: "log:info".into(),
            })],
        };
        let err = compile_declarative_route_to_canonical(route, false)
            .err()
            .expect("expected error");
        assert!(err.to_string().contains("unit_of_work"));
    }

    #[test]
    fn compile_declarative_route_to_canonical_propagates_v2_fields() {
        let route = DeclarativeRoute {
            from: "direct:start".into(),
            route_id: "test".into(),
            auto_startup: false,
            startup_order: 42,
            concurrency: Some(DeclarativeConcurrency::Concurrent { max: Some(4) }),
            error_handler: None,
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: None,
            steps: vec![DeclarativeStep::To(ToStepDef {
                uri: "log:info".into(),
            })],
        };
        let (spec, loss_report) =
            compile_declarative_route_to_canonical(route, false).expect("should succeed");
        assert!(loss_report.is_none());
        assert_eq!(spec.auto_startup, Some(false));
        assert_eq!(spec.startup_order, Some(42));
        assert_eq!(
            spec.concurrency,
            Some(CanonicalConcurrencySpec::Concurrent { max: 4 })
        );
    }

    #[test]
    fn compile_declarative_route_to_canonical_lossy_drops_error_handler() {
        let route = DeclarativeRoute {
            from: "direct:start".into(),
            route_id: "test".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: None,
            error_handler: Some(DeclarativeErrorHandler {
                dead_letter_channel: Some("log:dlq".into()),
                retry: None,
                on_exceptions: None,
            }),
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: None,
            steps: vec![DeclarativeStep::To(ToStepDef {
                uri: "log:info".into(),
            })],
        };
        let (spec, loss_report) =
            compile_declarative_route_to_canonical(route, true).expect("should succeed");
        assert!(spec.auto_startup.is_some());
        let report = loss_report.expect("expected loss report");
        assert_eq!(report.dropped_fields.len(), 1);
        assert_eq!(report.dropped_fields[0].field, "error_handler");
    }

    #[test]
    fn compile_canonical_route_respects_auto_startup_false() {
        use camel_api::runtime::CanonicalStepSpec;
        let spec = CanonicalRouteSpec {
            route_id: "test".into(),
            from: "direct:start".into(),
            steps: vec![CanonicalStepSpec::Stop],
            circuit_breaker: None,
            auto_startup: Some(false),
            startup_order: None,
            concurrency: None,
            version: camel_api::CANONICAL_CONTRACT_VERSION,
        };
        let def = compile_canonical_route(spec, 1024).expect("should succeed");
        assert!(!def.auto_startup());
    }

    #[test]
    fn compile_canonical_route_respects_startup_order() {
        use camel_api::runtime::CanonicalStepSpec;
        let spec = CanonicalRouteSpec {
            route_id: "test".into(),
            from: "direct:start".into(),
            steps: vec![CanonicalStepSpec::Stop],
            circuit_breaker: None,
            auto_startup: None,
            startup_order: Some(42),
            concurrency: None,
            version: camel_api::CANONICAL_CONTRACT_VERSION,
        };
        let def = compile_canonical_route(spec, 1024).expect("should succeed");
        assert_eq!(def.startup_order(), 42);
    }

    #[test]
    fn compile_canonical_route_respects_concurrency() {
        use camel_api::runtime::CanonicalStepSpec;
        let spec = CanonicalRouteSpec {
            route_id: "test".into(),
            from: "direct:start".into(),
            steps: vec![CanonicalStepSpec::Stop],
            circuit_breaker: None,
            auto_startup: None,
            startup_order: None,
            concurrency: Some(CanonicalConcurrencySpec::Concurrent { max: 4 }),
            version: camel_api::CANONICAL_CONTRACT_VERSION,
        };
        let def = compile_canonical_route(spec, 1024).expect("should succeed");
        assert_eq!(
            def.concurrency_override(),
            Some(&ConcurrencyModel::Concurrent { max: Some(4) })
        );
    }

    #[test]
    fn compile_canonical_route_defaults_auto_startup_true() {
        use camel_api::runtime::CanonicalStepSpec;
        let spec = CanonicalRouteSpec {
            route_id: "test".into(),
            from: "direct:start".into(),
            steps: vec![CanonicalStepSpec::Stop],
            circuit_breaker: None,
            auto_startup: None,
            startup_order: None,
            concurrency: None,
            version: camel_api::CANONICAL_CONTRACT_VERSION,
        };
        let def = compile_canonical_route(spec, 1024).expect("should succeed");
        assert!(def.auto_startup());
    }

    #[test]
    fn compile_declarative_route_to_canonical_lossy_drops_unit_of_work() {
        let route = DeclarativeRoute {
            from: "timer:tick".into(),
            route_id: "r1".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: None,
            error_handler: None,
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: Some(camel_api::UnitOfWorkConfig {
                on_complete: Some("log:done".into()),
                on_failure: None,
            }),
            steps: vec![],
        };
        let (spec, report) = compile_declarative_route_to_canonical(route, true).unwrap();
        assert_eq!(spec.route_id, "r1");
        let report = report.unwrap();
        assert!(
            report
                .dropped_fields
                .iter()
                .any(|f| f.field == "unit_of_work")
        );
    }

    #[test]
    fn compile_declarative_route_to_canonical_rejects_unbounded_concurrency() {
        let route = DeclarativeRoute {
            from: "timer:tick".into(),
            route_id: "r1".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: Some(DeclarativeConcurrency::Concurrent { max: None }),
            error_handler: None,
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: None,
            steps: vec![],
        };
        let err = compile_declarative_route_to_canonical(route, false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unbounded") || err.contains("max=None"),
            "{err}"
        );
    }

    #[test]
    fn compile_declarative_route_to_canonical_lossy_drops_multiple_fields() {
        let route = DeclarativeRoute {
            from: "timer:tick".into(),
            route_id: "r1".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: None,
            error_handler: Some(DeclarativeErrorHandler {
                dead_letter_channel: Some("log:dlq".into()),
                retry: None,
                on_exceptions: None,
            }),
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: Some(camel_api::UnitOfWorkConfig {
                on_complete: Some("log:done".into()),
                on_failure: None,
            }),
            steps: vec![],
        };
        let (spec, report) = compile_declarative_route_to_canonical(route, true).unwrap();
        assert_eq!(spec.route_id, "r1");
        let report = report.unwrap();
        assert_eq!(report.dropped_fields.len(), 2);
        let fields: Vec<&str> = report.dropped_fields.iter().map(|f| f.field).collect();
        assert!(fields.contains(&"error_handler"));
        assert!(fields.contains(&"unit_of_work"));
    }

    #[test]
    fn compile_declarative_route_to_canonical_lossy_drops_unbounded_concurrency() {
        let route = DeclarativeRoute {
            from: "timer:tick".into(),
            route_id: "r1".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: Some(DeclarativeConcurrency::Concurrent { max: None }),
            error_handler: None,
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: None,
            steps: vec![],
        };
        let (spec, report) = compile_declarative_route_to_canonical(route, true).unwrap();
        assert_eq!(spec.route_id, "r1");
        assert!(spec.concurrency.is_none());
        let report = report.unwrap();
        assert!(
            report
                .dropped_fields
                .iter()
                .any(|f| f.field == "concurrency.max")
        );
    }
}
