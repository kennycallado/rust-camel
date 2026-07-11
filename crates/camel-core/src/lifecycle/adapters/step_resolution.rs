use std::sync::Arc;

pub(crate) enum FunctionStagingMode {
    DirectAdd,
    DryCompile,
    HotReload { generation: u64 },
}

use crate::lifecycle::adapters::step_compilers::CompiledStep;
use crate::{ClaimCheckRegistry, IdempotentRegistry};
use camel_api::{CamelError, Exchange, FilterPredicate, FunctionInvoker, ProducerContext, Value};
use camel_bean::BeanRegistry;
use camel_component_api::{ComponentContext, RuntimeObservability};
use camel_language_api::{Expression, Language, Predicate};
use camel_processor::{EnrichmentStrategy, ThrowOnNoPoll, UseEnrichedBody};

/// Helper to evaluate an async expression from a sync closure context.
///
/// These closures are stored in sync callback types (FilterPredicate, SetBody callback, etc.)
/// but are only executed at runtime inside Tower services (async context). We use
/// `tokio::task::block_in_place` + `Handle::block_on` to bridge the sync→async gap.
pub(crate) fn await_eval(expr: &Arc<dyn Expression>, exchange: &Exchange) -> Value {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::try_current()
            .expect("await_eval: must be called from within a tokio runtime") // allow-unwrap
            .block_on(expr.evaluate(exchange))
    })
    .unwrap_or(Value::Null)
}

/// Helper to evaluate an async predicate from a sync closure context.
pub(crate) fn await_matches(pred: &Arc<dyn Predicate>, exchange: &Exchange) -> bool {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::try_current()
            .expect("await_matches: must be called from within a tokio runtime") // allow-unwrap
            .block_on(pred.matches(exchange))
    })
    .unwrap_or(false)
}

use crate::lifecycle::adapters::route_controller::SharedLanguageRegistry;
use crate::lifecycle::application::route_definition::{BuilderStep, LanguageExpressionDef};
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

/// Compile a `LanguageExpressionDef` into a `SortExpression` closure.
/// Uses `await_eval` to bridge the sync→async gap (same pattern as `compile_filter_predicate`).
/// Returns a `SortKey` for each element's value.
pub(crate) fn compile_sort_expression(
    languages: &SharedLanguageRegistry,
    expression: &LanguageExpressionDef,
) -> Result<camel_processor::SortExpression, CamelError> {
    let expr = compile_language_expression(languages, expression)?;
    Ok(std::sync::Arc::new(move |value: &serde_json::Value| {
        // Create a minimal exchange containing the element as body,
        // evaluate the language expression against it, then convert result to SortKey.
        let msg = camel_api::Message::new(camel_api::body::Body::Json(value.clone()));
        let exchange = camel_api::Exchange::new(msg);
        let result = await_eval(&expr, &exchange);
        // Reject non-scalar keys (Array/Object)
        if result.is_array() || result.is_object() {
            return Err(CamelError::ProcessorError(
                "sort expression returned a non-scalar value (array/object); expected null/bool/number/string"
                    .into(),
            ));
        }
        Ok(camel_processor::SortKey(result))
    }))
}

/// Compile a `LanguageExpressionDef` into a synchronous `MessageIdExpression`
/// closure for the Idempotent Consumer EIP. Uses `await_eval` to bridge the
/// sync→async gap (same pattern as `compile_filter_predicate`).
pub(crate) fn compile_message_id_expression(
    languages: &SharedLanguageRegistry,
    expression: &LanguageExpressionDef,
) -> Result<camel_processor::MessageIdExpression, CamelError> {
    let expr = compile_language_expression(languages, expression)?;
    Ok(Arc::new(move |exchange: &Exchange| {
        let value = await_eval(&expr, exchange);
        match value {
            Value::Null => None,
            Value::String(s) if s.is_empty() => None,
            Value::String(s) => Some(s),
            other => {
                // Non-string, non-null: stringify. Matches Apache Camel's
                // coercion of expression results to message-id strings.
                let s = other.to_string();
                if s.is_empty() { None } else { Some(s) }
            }
        }
    }))
}

/// Compile a `LanguageExpressionDef` into a `KeyExpression` closure for the
/// Claim Check EIP. Uses `await_eval` to bridge sync→async (same pattern as
/// `compile_message_id_expression`). Returns a `CamelError::ValidationError`
/// if the expression evaluates to null or empty — Claim Check keys are
/// REQUIRED non-empty strings, otherwise it's a user error.
pub(crate) fn compile_key_expression(
    languages: &SharedLanguageRegistry,
    expression: &LanguageExpressionDef,
) -> Result<camel_processor::KeyExpression, CamelError> {
    let expr = compile_language_expression(languages, expression)?;
    Ok(std::sync::Arc::new(
        move |exchange: &camel_api::Exchange| {
            let value = await_eval(&expr, exchange);
            match value {
                Value::Null => Err(CamelError::ValidationError(
                    "claim_check key expression evaluated to null or empty".into(),
                )),
                Value::String(s) if s.is_empty() => Err(CamelError::ValidationError(
                    "claim_check key expression evaluated to null or empty".into(),
                )),
                Value::String(s) => Ok(s),
                other => Ok(other.to_string()),
            }
        },
    ))
}

pub(crate) fn resolve_enrichment_strategy(
    name: Option<String>,
) -> Result<Arc<dyn EnrichmentStrategy>, CamelError> {
    match name.as_deref() {
        None | Some("useEnrichedBody") => Ok(Arc::new(UseEnrichedBody)),
        Some("throwOnNoPoll") => Ok(Arc::new(ThrowOnNoPoll::new(Arc::new(UseEnrichedBody)))),
        Some(other) => Err(CamelError::ProcessorError(format!(
            "unknown EnrichmentStrategy `{}`; supported: useEnrichedBody (default), throwOnNoPoll",
            other
        ))),
    }
}

/// Thin dispatcher: builds the step compiler registry, creates a compilation context,
/// and compiles all steps.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_steps(
    steps: Vec<BuilderStep>,
    producer_ctx: &ProducerContext,
    rt: Arc<dyn RuntimeObservability>,
    _registry: &Arc<std::sync::Mutex<Registry>>,
    languages: &SharedLanguageRegistry,
    beans: &Arc<std::sync::Mutex<BeanRegistry>>,
    function_invoker: Option<Arc<dyn FunctionInvoker>>,
    component_ctx: Arc<dyn ComponentContext>,
    route_id: Option<&str>,
    staging_mode: &FunctionStagingMode,
    idempotent_repositories: &IdempotentRegistry,
    claim_check_repositories: &ClaimCheckRegistry,
) -> Result<Vec<CompiledStep>, CamelError> {
    use crate::lifecycle::adapters::step_compilers::{CompilationContext, build_registry};

    let compiler_registry = build_registry();
    let ctx = CompilationContext {
        producer_ctx,
        rt,
        languages,
        beans,
        function_invoker,
        component_ctx,
        route_id,
        staging_mode,
        idempotent_repositories,
        claim_check_repositories,
    };
    compiler_registry.compile_steps(steps, &ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::application::route_definition::{LanguageExpressionDef, ValueSourceDef};
    use camel_api::BoxProcessor;
    use camel_api::IdentityProcessor;
    use camel_api::body::Body;

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

    fn local_value_to_body(value: Value) -> Body {
        match value {
            Value::Null => Body::Empty,
            Value::String(text) => Body::Text(text),
            other => Body::Json(other),
        }
    }

    #[tokio::test]
    async fn value_to_body_covers_null_string_and_json() {
        assert!(matches!(local_value_to_body(Value::Null), Body::Empty));
        assert!(matches!(
            local_value_to_body(Value::String("x".into())),
            Body::Text(ref s) if s == "x"
        ));
        assert!(matches!(
            local_value_to_body(Value::Number(serde_json::Number::from(7))),
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
                max_iterations: None,
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
            &crate::IdempotentRegistry::new(),
            &crate::ClaimCheckRegistry::new(),
        )
        .unwrap_err();
        assert!(
            matches!(
                both,
                CamelError::ConfigValidation(
                    camel_api::ConfigValidationError::LoopConflictingCountAndWhile,
                )
            ),
            "expected ConfigValidation(LoopConflictingCountAndWhile), got: {both}"
        );

        let neither = resolve_steps(
            vec![BuilderStep::DeclarativeLoop {
                count: None,
                while_predicate: None,
                steps: vec![],
                max_iterations: None,
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
            &crate::IdempotentRegistry::new(),
            &crate::ClaimCheckRegistry::new(),
        )
        .unwrap_err();
        assert!(
            matches!(
                neither,
                CamelError::ConfigValidation(
                    camel_api::ConfigValidationError::LoopMissingCountOrWhile,
                )
            ),
            "expected ConfigValidation(LoopMissingCountOrWhile), got: {neither}"
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
            &crate::IdempotentRegistry::new(),
            &crate::ClaimCheckRegistry::new(),
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
                max_iterations: None,
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
            &crate::IdempotentRegistry::new(),
            &crate::ClaimCheckRegistry::new(),
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
            &crate::IdempotentRegistry::new(),
            &crate::ClaimCheckRegistry::new(),
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

    #[tokio::test]
    async fn resolve_enrichment_strategy_none_returns_default_passthrough() {
        let strategy = resolve_enrichment_strategy(None).unwrap();
        let exchange = Exchange::new(camel_api::Message::new("test"));
        let result = strategy.on_no_poll(exchange).await.unwrap();
        match &result.input.body {
            Body::Text(s) => assert_eq!(s, "test"),
            other => panic!("expected Text body, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_enrichment_strategy_use_enriched_body_returns_default_passthrough() {
        let strategy = resolve_enrichment_strategy(Some("useEnrichedBody".into())).unwrap();
        let exchange = Exchange::new(camel_api::Message::new("test"));
        let result = strategy.on_no_poll(exchange).await.unwrap();
        match &result.input.body {
            Body::Text(s) => assert_eq!(s, "test"),
            other => panic!("expected Text body, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_enrichment_strategy_throw_on_no_poll_returns_error() {
        let strategy = resolve_enrichment_strategy(Some("throwOnNoPoll".into())).unwrap();
        let exchange = Exchange::new(camel_api::Message::new("test"));
        let err = strategy.on_no_poll(exchange).await.unwrap_err();
        assert!(
            err.to_string().contains("no message available"),
            "throwOnNoPoll should return error on no poll, got: {err}"
        );
    }

    #[test]
    fn resolve_enrichment_strategy_unknown_name_returns_error() {
        let result = resolve_enrichment_strategy(Some("bogus".into()));
        match result {
            Ok(_) => panic!("expected error for unknown strategy name"),
            Err(err) => assert!(
                err.to_string().contains("unknown EnrichmentStrategy"),
                "unexpected error message: {err}"
            ),
        }
    }
}
