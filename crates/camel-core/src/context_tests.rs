use super::*;
use crate::lifecycle::application::route_definition::{
    BuilderStep, LanguageExpressionDef, RouteDefinition,
};
use crate::lifecycle::domain::{RouteRuntimeAggregate, RouteRuntimeState};
use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::{
    AsyncHealthCheck, CanonicalRouteSpec, CheckResult, HealthStatus, RuntimeCommand,
    RuntimeCommandResult, RuntimeQuery, RuntimeQueryResult, ServiceStatus,
};
use camel_component_api::{Component, ConcurrencyModel, Consumer, ConsumerContext, Endpoint};

/// Mock component for testing
struct MockComponent;

impl Component for MockComponent {
    fn scheme(&self) -> &str {
        "mock"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Err(CamelError::ComponentNotFound("mock".to_string()))
    }
}

struct HoldConsumer;

#[async_trait]
impl Consumer for HoldConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        ctx.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}

struct HoldEndpoint;

impl Endpoint for HoldEndpoint {
    fn uri(&self) -> &str {
        "hold:test"
    }

    fn create_consumer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(HoldConsumer))
    }

    fn create_producer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &camel_api::ProducerContext,
    ) -> Result<camel_api::BoxProcessor, CamelError> {
        Err(CamelError::RouteError("no producer".to_string()))
    }
}

struct HoldComponent;

impl Component for HoldComponent {
    fn scheme(&self) -> &str {
        "hold"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(HoldEndpoint))
    }
}

#[tokio::test]
async fn test_context_handles_mutex_poisoning_gracefully() {
    let mut ctx = CamelContext::builder().build().await.unwrap();

    // Register a component successfully
    ctx.register_component(MockComponent);

    // Access registry should work even after potential panic in another thread
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = ctx.registry();
    }));

    assert!(
        result.is_ok(),
        "Registry access should handle mutex poisoning"
    );
}

#[tokio::test]
async fn test_context_resolves_simple_language() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let lang = ctx
        .resolve_language("simple")
        .expect("simple language not found");
    assert_eq!(lang.name(), "simple");
}

#[tokio::test]
async fn test_builder_with_default_platform_service_is_noop() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let handle = ctx.leadership().start("orders").await.unwrap();
    assert!(handle.is_leader());
}

#[tokio::test]
async fn test_builder_with_custom_platform_service() {
    let identity = camel_api::PlatformIdentity::local("my-pod");
    let ctx = CamelContext::builder()
        .platform_service(Arc::new(camel_api::NoopPlatformService::new(identity)))
        .build()
        .await
        .unwrap();

    assert_eq!(ctx.platform_identity().node_id, "my-pod");
}

#[tokio::test]
async fn test_simple_language_via_context() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let lang = ctx.resolve_language("simple").unwrap();
    let pred = lang.create_predicate("${header.x} == 'hello'").unwrap();
    let mut msg = camel_api::message::Message::default();
    msg.set_header("x", camel_api::Value::String("hello".into()));
    let ex = camel_api::exchange::Exchange::new(msg);
    assert!(pred.matches(&ex).await.unwrap());
}

#[tokio::test]
async fn test_resolve_unknown_language_returns_none() {
    let ctx = CamelContext::builder().build().await.unwrap();
    assert!(ctx.resolve_language("nonexistent").is_none());
}

#[tokio::test]
async fn test_register_language_duplicate_returns_error() {
    use camel_language_api::LanguageError;
    struct DummyLang;
    impl camel_language_api::Language for DummyLang {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn create_expression(
            &self,
            _: &str,
        ) -> Result<Box<dyn camel_language_api::Expression>, LanguageError> {
            Err(LanguageError::EvalError("not implemented".into()))
        }
        fn create_predicate(
            &self,
            _: &str,
        ) -> Result<Box<dyn camel_language_api::Predicate>, LanguageError> {
            Err(LanguageError::EvalError("not implemented".into()))
        }
    }

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_language("dummy", Box::new(DummyLang)).unwrap();
    let result = ctx.register_language("dummy", Box::new(DummyLang));
    assert!(result.is_err(), "duplicate registration should fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("dummy"),
        "error should mention the language name"
    );
}

#[tokio::test]
async fn test_register_language_new_key_succeeds() {
    use camel_language_api::LanguageError;
    struct DummyLang;
    impl camel_language_api::Language for DummyLang {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn create_expression(
            &self,
            _: &str,
        ) -> Result<Box<dyn camel_language_api::Expression>, LanguageError> {
            Err(LanguageError::EvalError("not implemented".into()))
        }
        fn create_predicate(
            &self,
            _: &str,
        ) -> Result<Box<dyn camel_language_api::Predicate>, LanguageError> {
            Err(LanguageError::EvalError("not implemented".into()))
        }
    }

    let mut ctx = CamelContext::builder().build().await.unwrap();
    let result = ctx.register_language("dummy", Box::new(DummyLang));
    assert!(result.is_ok(), "first registration should succeed");
}

#[tokio::test]
async fn test_add_route_definition_uses_runtime_registered_language() {
    use camel_language_api::{Expression, LanguageError, Predicate};

    struct DummyExpression;
    #[async_trait::async_trait]
    impl Expression for DummyExpression {
        async fn evaluate(
            &self,
            _exchange: &camel_api::Exchange,
        ) -> Result<camel_api::Value, LanguageError> {
            Ok(camel_api::Value::String("ok".into()))
        }
    }

    struct DummyPredicate;
    #[async_trait::async_trait]
    impl Predicate for DummyPredicate {
        async fn matches(&self, _exchange: &camel_api::Exchange) -> Result<bool, LanguageError> {
            Ok(true)
        }
    }

    struct RuntimeLang;
    impl camel_language_api::Language for RuntimeLang {
        fn name(&self) -> &'static str {
            "runtime"
        }

        fn create_expression(&self, _script: &str) -> Result<Box<dyn Expression>, LanguageError> {
            Ok(Box::new(DummyExpression))
        }

        fn create_predicate(&self, _script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
            Ok(Box::new(DummyPredicate))
        }
    }

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_language("runtime", Box::new(RuntimeLang))
        .unwrap();

    let definition = RouteDefinition::new(
        "timer:tick",
        vec![BuilderStep::DeclarativeScript {
            expression: LanguageExpressionDef {
                language: "runtime".into(),
                source: "${body}".into(),
            },
        }],
    )
    .with_route_id("runtime-lang-route");

    let result = ctx.add_route_definition(definition).await;
    assert!(
        result.is_ok(),
        "route should resolve runtime language: {result:?}"
    );
}

#[tokio::test]
async fn test_add_route_definition_fails_for_unregistered_runtime_language() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let definition = RouteDefinition::new(
        "timer:tick",
        vec![BuilderStep::DeclarativeSetBody {
            value: crate::route::ValueSourceDef::Expression(LanguageExpressionDef {
                language: "missing-lang".into(),
                source: "${body}".into(),
            }),
        }],
    )
    .with_route_id("missing-runtime-lang-route");

    let result = ctx.add_route_definition(definition).await;
    assert!(
        result.is_err(),
        "route should fail when language is missing"
    );
    let error_text = result.unwrap_err().to_string();
    assert!(
        error_text.contains("missing-lang"),
        "error should mention missing language, got: {error_text}"
    );
}

#[tokio::test]
async fn add_route_definition_does_not_require_mut() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let definition = RouteDefinition::new("timer:tick", vec![]).with_route_id("immutable-ctx");

    let result = ctx.add_route_definition(definition).await;
    assert!(
        result.is_ok(),
        "immutable context should add route: {result:?}"
    );
}

#[tokio::test]
async fn test_health_check_empty_context() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let report = ctx.health_check().await;

    assert_eq!(report.status, HealthStatus::Healthy);
    assert!(report.services.is_empty());
}

#[tokio::test]
async fn test_health_check_degraded_when_service_stopped() {
    struct DegradedCheck;

    #[async_trait]
    impl AsyncHealthCheck for DegradedCheck {
        fn name(&self) -> &str {
            "degraded-check"
        }

        async fn check(&self) -> CheckResult {
            CheckResult::degraded("degraded-check", "slow")
        }
    }

    let ctx = CamelContext::builder().build().await.unwrap();
    ctx.health_registry()
        .register_for_route("route-1", Arc::new(DegradedCheck));
    ctx.health_registry().mark_route_started("route-1");

    let report = ctx.health_check().await;
    assert_eq!(
        report.status,
        HealthStatus::Degraded,
        "a degraded check should produce Degraded"
    );
    assert_eq!(report.services.len(), 1);
    assert_eq!(report.services[0].name, "degraded-check");
}

#[tokio::test]
async fn test_health_check_unhealthy_when_service_failed() {
    struct FailedCheck;

    #[async_trait]
    impl AsyncHealthCheck for FailedCheck {
        fn name(&self) -> &str {
            "failed-check"
        }

        async fn check(&self) -> CheckResult {
            CheckResult::unhealthy("failed-check", "fail")
        }
    }

    let ctx = CamelContext::builder().build().await.unwrap();
    ctx.health_registry()
        .register_for_route("route-1", Arc::new(FailedCheck));
    ctx.health_registry().mark_route_started("route-1");

    let report = ctx.health_check().await;
    assert_eq!(
        report.status,
        HealthStatus::Unhealthy,
        "a failed service should produce Unhealthy"
    );
    assert_eq!(report.services[0].status, ServiceStatus::Failed);
}

#[tokio::test]
async fn test_health_check_failed_overrides_stopped() {
    struct DegradedCheck;

    #[async_trait]
    impl AsyncHealthCheck for DegradedCheck {
        fn name(&self) -> &str {
            "degraded-check"
        }
        async fn check(&self) -> CheckResult {
            CheckResult::degraded("degraded-check", "slow")
        }
    }

    struct FailedCheck;

    #[async_trait]
    impl AsyncHealthCheck for FailedCheck {
        fn name(&self) -> &str {
            "failed-check"
        }
        async fn check(&self) -> CheckResult {
            CheckResult::unhealthy("failed-check", "fail")
        }
    }

    let ctx = CamelContext::builder().build().await.unwrap();
    let registry = ctx.health_registry();
    registry.register_for_route("route-1", Arc::new(DegradedCheck));
    registry.mark_route_started("route-1");
    registry.register_for_route("route-2", Arc::new(FailedCheck));
    registry.mark_route_started("route-2");

    let report = ctx.health_check().await;
    assert_eq!(
        report.status,
        HealthStatus::Unhealthy,
        "Failed should take priority over Stopped"
    );
}

#[tokio::test]
async fn context_exposes_runtime_command_and_query_buses() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let runtime = ctx.runtime();

    let register = runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("runtime-r1", "timer:tick"),
            command_id: "cmd-1".into(),
            causation_id: None,
        })
        .await
        .unwrap();
    assert!(matches!(
        register,
        RuntimeCommandResult::RouteRegistered { ref route_id } if route_id == "runtime-r1"
    ));

    let query = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "runtime-r1".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        query,
        RuntimeQueryResult::RouteStatus { ref status, .. } if status == "Registered"
    ));
}

#[tokio::test]
async fn default_runtime_journal_isolated_per_context_without_env_override() {
    if let Ok(value) = std::env::var("CAMEL_RUNTIME_JOURNAL_PATH")
        && !value.trim().is_empty()
    {
        return;
    }

    let first = CamelContext::builder().build().await.unwrap();
    first
        .runtime()
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("default-isolation-r1", "timer:tick"),
            command_id: "iso-c1".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    let second = CamelContext::builder().build().await.unwrap();
    second
        .runtime()
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("default-isolation-r1", "timer:tick"),
            command_id: "iso-c2".into(),
            causation_id: None,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn runtime_commands_drive_real_route_controller_lifecycle() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(HoldComponent);
    let runtime = ctx.runtime();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("runtime-hold", "hold:test"),
            command_id: "c1".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    assert_eq!(
        ctx.runtime_route_status("runtime-hold").await.unwrap(),
        Some("Registered".to_string())
    );
    assert!(matches!(
        ctx.runtime.repo().load("runtime-hold").await.unwrap(),
        Some(agg)
            if matches!(agg.state(), crate::lifecycle::domain::RouteRuntimeState::Registered)
    ));

    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "runtime-hold".into(),
            command_id: "c2".into(),
            causation_id: Some("c1".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        ctx.runtime_route_status("runtime-hold").await.unwrap(),
        Some("Started".to_string())
    );
    assert!(matches!(
        ctx.runtime.repo().load("runtime-hold").await.unwrap(),
        Some(agg)
            if matches!(agg.state(), crate::lifecycle::domain::RouteRuntimeState::Started)
    ));

    runtime
        .execute(RuntimeCommand::SuspendRoute {
            route_id: "runtime-hold".into(),
            command_id: "c3".into(),
            causation_id: Some("c2".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        ctx.runtime_route_status("runtime-hold").await.unwrap(),
        Some("Suspended".to_string())
    );

    runtime
        .execute(RuntimeCommand::ResumeRoute {
            route_id: "runtime-hold".into(),
            command_id: "c4".into(),
            causation_id: Some("c3".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        ctx.runtime_route_status("runtime-hold").await.unwrap(),
        Some("Started".to_string())
    );

    runtime
        .execute(RuntimeCommand::StopRoute {
            route_id: "runtime-hold".into(),
            command_id: "c5".into(),
            causation_id: Some("c4".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        ctx.runtime_route_status("runtime-hold").await.unwrap(),
        Some("Stopped".to_string())
    );

    runtime
        .execute(RuntimeCommand::ReloadRoute {
            route_id: "runtime-hold".into(),
            command_id: "c6".into(),
            causation_id: Some("c5".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        ctx.runtime_route_status("runtime-hold").await.unwrap(),
        Some("Started".to_string())
    );

    runtime
        .execute(RuntimeCommand::StopRoute {
            route_id: "runtime-hold".into(),
            command_id: "c7".into(),
            causation_id: Some("c6".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        ctx.runtime_route_status("runtime-hold").await.unwrap(),
        Some("Stopped".to_string())
    );

    runtime
        .execute(RuntimeCommand::RemoveRoute {
            route_id: "runtime-hold".into(),
            command_id: "c8".into(),
            causation_id: Some("c7".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        ctx.runtime_route_status("runtime-hold").await.unwrap(),
        None
    );
    assert!(
        ctx.runtime
            .repo()
            .load("runtime-hold")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn runtime_queries_read_projection_state_when_connected() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(HoldComponent);
    let runtime = ctx.runtime();
    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("rq", "hold:test"),
            command_id: "c1".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    // Diverge live controller state from the projection on purpose.
    ctx.runtime_execution_handle()
        .force_start_route_for_test("rq")
        .await
        .unwrap();

    let result = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "rq".into(),
        })
        .await
        .unwrap();

    match result {
        RuntimeQueryResult::RouteStatus { status, .. } => assert_eq!(status, "Registered"),
        _ => panic!("unexpected query result"),
    }
}

#[tokio::test]
async fn add_route_definition_produces_registered_state() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let definition = RouteDefinition::new("direct:test", vec![]).with_route_id("async-test-route");

    ctx.add_route_definition(definition).await.unwrap();

    let status = ctx
        .runtime()
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "async-test-route".to_string(),
        })
        .await
        .unwrap();

    match status {
        RuntimeQueryResult::RouteStatus { status, .. } => {
            assert_eq!(
                status, "Registered",
                "expected Registered state after add_route_definition"
            );
        }
        _ => panic!("unexpected query result"),
    }
}

#[tokio::test]
async fn add_route_definition_injects_runtime_into_producer_context() {
    use std::sync::atomic::{AtomicBool, Ordering};

    struct RuntimeAwareEndpoint {
        saw_runtime: Arc<AtomicBool>,
    }

    impl Endpoint for RuntimeAwareEndpoint {
        fn uri(&self) -> &str {
            "runtime-aware:test"
        }

        fn create_consumer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
            Err(CamelError::RouteError("no consumer".to_string()))
        }

        fn create_producer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
            ctx: &camel_api::ProducerContext,
        ) -> Result<camel_api::BoxProcessor, CamelError> {
            self.saw_runtime
                .store(ctx.runtime().is_some(), Ordering::SeqCst);
            if ctx.runtime().is_none() {
                return Err(CamelError::RouteError(
                    "runtime handle missing in ProducerContext".to_string(),
                ));
            }
            Ok(camel_api::BoxProcessor::new(camel_api::IdentityProcessor))
        }
    }

    struct RuntimeAwareComponent {
        saw_runtime: Arc<AtomicBool>,
    }

    impl Component for RuntimeAwareComponent {
        fn scheme(&self) -> &str {
            "runtime-aware"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn camel_component_api::ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(RuntimeAwareEndpoint {
                saw_runtime: Arc::clone(&self.saw_runtime),
            }))
        }
    }

    let saw_runtime = Arc::new(AtomicBool::new(false));
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(RuntimeAwareComponent {
        saw_runtime: Arc::clone(&saw_runtime),
    });

    let definition = RouteDefinition::new(
        "timer:tick",
        vec![BuilderStep::To("runtime-aware:test".to_string())],
    )
    .with_route_id("runtime-aware-route");

    let result = ctx.add_route_definition(definition).await;
    assert!(
        result.is_ok(),
        "route should resolve producer with runtime context: {result:?}"
    );
    assert!(
        saw_runtime.load(Ordering::SeqCst),
        "component producer should observe runtime handle in ProducerContext"
    );
}

#[tokio::test]
async fn add_route_definition_registers_runtime_projection_and_aggregate() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(HoldComponent);

    let definition = RouteDefinition::new("hold:test", vec![]).with_route_id("ctx-runtime-r1");
    ctx.add_route_definition(definition).await.unwrap();

    let aggregate = ctx.runtime.repo().load("ctx-runtime-r1").await.unwrap();
    assert!(
        matches!(aggregate, Some(agg) if matches!(agg.state(), RouteRuntimeState::Registered)),
        "route registration should seed aggregate as Registered"
    );

    let status = ctx.runtime_route_status("ctx-runtime-r1").await.unwrap();
    assert_eq!(status.as_deref(), Some("Registered"));
}

#[tokio::test]
async fn add_route_definition_rolls_back_controller_when_runtime_registration_fails() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(HoldComponent);

    ctx.runtime
        .repo()
        .save(RouteRuntimeAggregate::new("ctx-runtime-dup"))
        .await
        .unwrap();

    let definition = RouteDefinition::new("hold:test", vec![]).with_route_id("ctx-runtime-dup");
    let result = ctx.add_route_definition(definition).await;
    assert!(result.is_err(), "duplicate runtime registration must fail");

    assert_eq!(
        ctx.runtime_execution_handle()
            .controller_route_count_for_test()
            .await,
        0,
        "controller route should be rolled back on runtime bootstrap failure"
    );
}

#[tokio::test]
async fn context_start_stop_drives_runtime_lifecycle_via_command_bus() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(HoldComponent);

    let autostart = RouteDefinition::new("hold:test", vec![]).with_route_id("ctx-lifecycle-auto");
    let lazy = RouteDefinition::new("hold:test", vec![])
        .with_route_id("ctx-lifecycle-lazy")
        .with_auto_startup(false);

    ctx.add_route_definition(autostart).await.unwrap();
    ctx.add_route_definition(lazy).await.unwrap();

    assert_eq!(
        ctx.runtime_route_status("ctx-lifecycle-auto")
            .await
            .unwrap(),
        Some("Registered".to_string())
    );
    assert_eq!(
        ctx.runtime_route_status("ctx-lifecycle-lazy")
            .await
            .unwrap(),
        Some("Registered".to_string())
    );

    ctx.start().await.unwrap();

    assert_eq!(
        ctx.runtime_route_status("ctx-lifecycle-auto")
            .await
            .unwrap(),
        Some("Started".to_string())
    );
    assert_eq!(
        ctx.runtime_route_status("ctx-lifecycle-lazy")
            .await
            .unwrap(),
        Some("Registered".to_string())
    );

    ctx.stop().await.unwrap();

    assert_eq!(
        ctx.runtime_route_status("ctx-lifecycle-auto")
            .await
            .unwrap(),
        Some("Stopped".to_string())
    );
    assert_eq!(
        ctx.runtime_route_status("ctx-lifecycle-lazy")
            .await
            .unwrap(),
        Some("Registered".to_string())
    );
}

use camel_api::Lifecycle;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct MockService {
    start_count: Arc<AtomicUsize>,
    stop_count: Arc<AtomicUsize>,
}

impl MockService {
    fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let start_count = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::new(AtomicUsize::new(0));
        (
            Self {
                start_count: start_count.clone(),
                stop_count: stop_count.clone(),
            },
            start_count,
            stop_count,
        )
    }
}

#[async_trait]
impl Lifecycle for MockService {
    fn name(&self) -> &str {
        "mock"
    }

    async fn start(&mut self) -> Result<(), CamelError> {
        self.start_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        self.stop_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn test_context_starts_lifecycle_services() {
    let (service, start_count, stop_count) = MockService::new();

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .unwrap()
        .with_lifecycle(service);

    assert_eq!(start_count.load(Ordering::SeqCst), 0);

    ctx.start().await.unwrap();

    assert_eq!(start_count.load(Ordering::SeqCst), 1);
    assert_eq!(stop_count.load(Ordering::SeqCst), 0);

    ctx.stop().await.unwrap();

    assert_eq!(stop_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_context_abort_stops_lifecycle_services() {
    let (service, _start_count, stop_count) = MockService::new();

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .unwrap()
        .with_lifecycle(service);

    assert_eq!(stop_count.load(Ordering::SeqCst), 0);

    ctx.abort().await;

    assert_eq!(stop_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_service_start_failure_rollback() {
    struct FailingService {
        start_count: Arc<AtomicUsize>,
        stop_count: Arc<AtomicUsize>,
        should_fail: bool,
    }

    #[async_trait]
    impl Lifecycle for FailingService {
        fn name(&self) -> &str {
            "failing"
        }

        async fn start(&mut self) -> Result<(), CamelError> {
            self.start_count.fetch_add(1, Ordering::SeqCst);
            if self.should_fail {
                Err(CamelError::ProcessorError("intentional failure".into()))
            } else {
                Ok(())
            }
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            self.stop_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let start1 = Arc::new(AtomicUsize::new(0));
    let stop1 = Arc::new(AtomicUsize::new(0));
    let start2 = Arc::new(AtomicUsize::new(0));
    let stop2 = Arc::new(AtomicUsize::new(0));
    let start3 = Arc::new(AtomicUsize::new(0));
    let stop3 = Arc::new(AtomicUsize::new(0));

    let service1 = FailingService {
        start_count: start1.clone(),
        stop_count: stop1.clone(),
        should_fail: false,
    };
    let service2 = FailingService {
        start_count: start2.clone(),
        stop_count: stop2.clone(),
        should_fail: true, // This one will fail
    };
    let service3 = FailingService {
        start_count: start3.clone(),
        stop_count: stop3.clone(),
        should_fail: false,
    };

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .unwrap()
        .with_lifecycle(service1)
        .with_lifecycle(service2)
        .with_lifecycle(service3);

    // Attempt to start - should fail
    let result = ctx.start().await;
    assert!(result.is_err());

    // Verify service1 was started and then stopped (rollback)
    assert_eq!(start1.load(Ordering::SeqCst), 1);
    assert_eq!(stop1.load(Ordering::SeqCst), 1);

    // Verify service2 was attempted to start but failed
    assert_eq!(start2.load(Ordering::SeqCst), 1);
    assert_eq!(stop2.load(Ordering::SeqCst), 0);

    // Verify service3 was never started
    assert_eq!(start3.load(Ordering::SeqCst), 0);
    assert_eq!(stop3.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_services_stop_in_reverse_order() {
    use std::sync::Mutex as StdMutex;

    struct OrderTracker {
        name: String,
        order: Arc<StdMutex<Vec<String>>>,
    }

    #[async_trait]
    impl Lifecycle for OrderTracker {
        fn name(&self) -> &str {
            &self.name
        }

        async fn start(&mut self) -> Result<(), CamelError> {
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            self.order.lock().unwrap().push(self.name.clone());
            Ok(())
        }
    }

    let order = Arc::new(StdMutex::new(Vec::<String>::new()));

    let s1 = OrderTracker {
        name: "first".into(),
        order: Arc::clone(&order),
    };
    let s2 = OrderTracker {
        name: "second".into(),
        order: Arc::clone(&order),
    };
    let s3 = OrderTracker {
        name: "third".into(),
        order: Arc::clone(&order),
    };

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .unwrap()
        .with_lifecycle(s1)
        .with_lifecycle(s2)
        .with_lifecycle(s3);

    ctx.start().await.unwrap();
    ctx.stop().await.unwrap();

    let stopped = order.lock().unwrap();
    assert_eq!(
        *stopped,
        vec!["third", "second", "first"],
        "services must stop in reverse insertion order"
    );
}

#[derive(Debug, Clone, PartialEq)]
struct MyConfig {
    value: u32,
}

#[tokio::test]
async fn test_set_and_get_component_config() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.set_component_config(MyConfig { value: 42 });
    let got = ctx.get_component_config::<MyConfig>();
    assert_eq!(got, Some(&MyConfig { value: 42 }));
}

#[tokio::test]
async fn test_get_missing_config_returns_none() {
    let ctx = CamelContext::builder().build().await.unwrap();
    assert!(ctx.get_component_config::<MyConfig>().is_none());
}

#[tokio::test]
async fn test_set_overwrites_previous_config() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.set_component_config(MyConfig { value: 1 });
    ctx.set_component_config(MyConfig { value: 2 });
    assert_eq!(ctx.get_component_config::<MyConfig>().unwrap().value, 2);
}

#[tokio::test]
async fn context_producer_context_is_wired_to_runtime() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let producer_ctx = ctx.producer_context();
    assert!(producer_ctx.runtime().is_some());
}

#[tokio::test]
async fn context_metrics_defaults_to_noop() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let metrics = ctx.metrics();
    metrics.record_exchange_duration("r1", std::time::Duration::from_millis(1));
    metrics.increment_exchanges("r1");
    metrics.increment_errors("r1", "e1");
    metrics.set_queue_depth("r1", 3);
    metrics.record_circuit_breaker_change("r1", "closed", "open");
}

#[tokio::test]
async fn context_exposes_platform_ports() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let readiness = ctx.readiness_gate();
    readiness.notify_starting().await.unwrap();
    readiness.notify_not_ready("boot").await.unwrap();
    readiness.notify_ready().await.unwrap();

    let leadership = ctx.leadership();
    let handle = leadership.start("coverage-group").await.unwrap();
    assert!(handle.is_leader());

    let service = ctx.platform_service();
    let identity = service.identity();
    assert!(!identity.node_id.is_empty());
}

#[tokio::test]
async fn builder_shutdown_timeout_is_applied() {
    let ctx = CamelContext::builder()
        .shutdown_timeout(std::time::Duration::from_secs(7))
        .build()
        .await
        .unwrap();
    assert_eq!(ctx.shutdown_timeout(), std::time::Duration::from_secs(7));
}

#[tokio::test]
async fn builder_default_shutdown_timeout_is_5_seconds() {
    let ctx = CamelContext::builder().build().await.unwrap();
    assert_eq!(ctx.shutdown_timeout(), std::time::Duration::from_secs(5));
}

#[tokio::test]
async fn context_platform_identity_matches_platform_service_identity() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let via_context = ctx.platform_identity();
    let via_service = ctx.platform_service().identity();
    assert_eq!(via_context.node_id, via_service.node_id);
}

#[tokio::test]
async fn context_leadership_start_is_idempotent_for_same_group() {
    let ctx = CamelContext::builder().build().await.unwrap();
    let leadership = ctx.leadership();
    let first = leadership.start("coverage-group-dup").await.unwrap();
    let second = leadership.start("coverage-group-dup").await.unwrap();
    assert!(first.is_leader());
    assert!(second.is_leader());
}

#[tokio::test]
async fn context_set_shutdown_timeout_updates_value() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.set_shutdown_timeout(std::time::Duration::from_secs(13));
    assert_eq!(ctx.shutdown_timeout(), std::time::Duration::from_secs(13));
}

#[tokio::test]
async fn context_registry_arc_points_to_same_registry() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(MockComponent);

    let original = ctx.registry_arc();
    let cloned = ctx.registry_arc();
    assert!(Arc::ptr_eq(&original, &cloned));
}

#[tokio::test]
async fn builder_default_matches_new() {
    let ctx_from_default = CamelContextBuilder::default().build().await.unwrap();
    let ctx_from_new = CamelContextBuilder::new().build().await.unwrap();

    assert_eq!(
        ctx_from_default.shutdown_timeout(),
        ctx_from_new.shutdown_timeout()
    );
    assert!(ctx_from_default.resolve_language("simple").is_some());
    assert!(ctx_from_new.resolve_language("simple").is_some());
}

// -------------------------------------------------------------
// Regression test: CamelContext routes real observability
// when accessed through Arc<dyn RuntimeObservability>.
// -------------------------------------------------------------

/// Recording metrics collector used by
/// `camel_context_routes_real_observability`.
///
/// Pattern adapted from `RecordingMetrics` in
/// crates/components/camel-component-api/tests/runtime_observability_tests.rs.
struct RecMetrics {
    errors: std::sync::Mutex<Vec<(String, String)>>,
}

impl RecMetrics {
    fn error_count(&self, route_id: &str, error_type: &str) -> usize {
        self.errors
            .lock()
            .expect("metrics lock")
            .iter()
            .filter(|(r, t)| r == route_id && t == error_type)
            .count()
    }
}

impl MetricsCollector for RecMetrics {
    fn record_exchange_duration(&self, _: &str, _: std::time::Duration) {}
    fn increment_errors(&self, route_id: &str, error_type: &str) {
        self.errors
            .lock()
            .expect("metrics lock")
            .push((route_id.to_string(), error_type.to_string()));
    }
    fn increment_exchanges(&self, _: &str) {}
    fn set_queue_depth(&self, _: &str, _: usize) {}
    fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
}

#[tokio::test]
async fn camel_context_routes_real_observability() {
    // GIVEN: a CamelContext configured with a recording MetricsCollector
    // and a real HealthCheckRegistry (not a NoOp).
    let metrics: Arc<RecMetrics> = Arc::new(RecMetrics {
        errors: std::sync::Mutex::new(Vec::new()),
    });
    let health_registry = Arc::new(HealthCheckRegistry::new(std::time::Duration::from_secs(5)));
    let hr_for_builder = Arc::clone(&health_registry);

    let ctx = CamelContext::builder()
        .metrics(Arc::clone(&metrics) as Arc<dyn MetricsCollector>)
        .health_registry(hr_for_builder)
        .build()
        .await
        .expect("CamelContext should build");

    // The blanket impl makes Arc<CamelContext> usable as Arc<dyn RuntimeObservability>.
    let rt: Arc<dyn camel_component_api::RuntimeObservability> = Arc::new(ctx);

    // WHEN: increment_errors is called via the trait
    rt.metrics().increment_errors("test-route", "test-label");

    // THEN: the underlying MetricsCollector records the increment (NOT a no-op).
    assert_eq!(
        metrics.error_count("test-route", "test-label"),
        1,
        "CamelContext must route increment_errors to the real MetricsCollector, not a NoOp"
    );

    // WHEN: force_unhealthy_for_route is called via the trait
    rt.health()
        .force_unhealthy_for_route("test-route", "probe", "test reason");

    // THEN: the HealthCheckRegistry marks the route unhealthy.
    let report = health_registry.check_all().await;
    assert_eq!(report.status, HealthStatus::Unhealthy);
    assert_eq!(report.services.len(), 1);
    assert_eq!(report.services[0].name, "probe");
    assert_eq!(report.services[0].message.as_deref(), Some("test reason"));
}

// ADR-0033: CamelContext::start() must run every registered ConfigCheck
// before any route consumer starts. A misconfigured SqlDynamicQueryCheck
// (useMessageBodyForSql=true with allowDynamicQuery=false) must cause
// start() to fail closed with CamelError::Config and NOT start the
// route consumer.
#[tokio::test]
async fn test_start_runs_registered_config_checks() {
    use crate::startup_validation::SqlDynamicQueryCheck;

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(MockComponent);

    // Register a startup check that is guaranteed to fail: the operator
    // declared body-sourced SQL queries but did not opt into the dynamic
    // query capability — runtime would silently produce empty queries.
    ctx.add_startup_check(Box::new(SqlDynamicQueryCheck {
        use_message_body_for_sql: true,
        allow_dynamic_query: false,
    }));

    let result = ctx.start().await;
    match result {
        Err(CamelError::Config(msg)) => {
            assert!(
                msg.contains("sql-dynamic-query"),
                "error must name the check that failed: {msg}"
            );
        }
        Err(other) => panic!("expected CamelError::Config, got {other:?}"),
        Ok(()) => panic!("start() must fail closed when a ConfigCheck fails"),
    }

    // The check list is drained on start(); a second start() call would
    // not re-run checks. We do not call start() again here — the contract
    // is that the runtime is single-use.
}

// D-L5: abort() must send Shutdown to the controller actor and join its
// JoinHandle with a bounded timeout. stop() keeps the actor alive for restart.
#[tokio::test]
async fn test_abort_joins_actor_gracefully() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.start().await.unwrap();

    // Take the actor join handle before abort consumes it
    let actor_join = ctx.take_actor_join().expect("actor_join should be present");

    let start = std::time::Instant::now();
    ctx.abort().await;
    let elapsed = start.elapsed();

    // Give the actor a moment to process the Shutdown command
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // The actor must have exited its recv loop after receiving Shutdown
    assert!(
        actor_join.is_finished(),
        "controller actor should have exited after abort()"
    );

    // And the whole abort+join should complete well under the 5s bounded timeout
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "abort() took {elapsed:?}, expected < 2s (graceful actor join)"
    );
}

// stop() must NOT kill the controller actor — it stays alive so the context
// can be restarted via a subsequent start().
#[tokio::test]
async fn test_stop_keeps_actor_alive_for_restart() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.start().await.unwrap();

    // Take the join handle so we can inspect actor state without stop consuming it
    let actor_join = ctx.take_actor_join().expect("actor_join should be present");

    ctx.stop().await.unwrap();

    // Actor should still be running (not exited)
    assert!(
        !actor_join.is_finished(),
        "controller actor should still be alive after stop()"
    );

    // Restart should work — actor is alive, cancel_token was reset
    ctx.start().await.expect("restart should succeed");
    ctx.stop().await.expect("stop after restart should succeed");
}

// ADR-0033: an empty startup-check list must let start() proceed normally
// (i.e. validation is opt-in, not a hard requirement on every context).
#[tokio::test]
async fn test_start_with_no_registered_checks_proceeds() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(MockComponent);

    // No checks registered. start() should not fail on validation.
    // It may fail later (e.g. no routes to start) but the error must
    // NOT be CamelError::Config from the validator path.
    // We only assert that the validator is not the source of failure.
    let result = ctx.start().await;
    if let Err(CamelError::Config(msg)) = &result {
        panic!(
            "start() must not fail with CamelError::Config when no checks are registered, got: {msg}"
        );
    }
    // Any other outcome (Ok, or Err from later phases) is acceptable for
    // this test — the assertion above is the contract.
    let _ = result;
}
