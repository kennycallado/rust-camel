use super::*;
use crate::lifecycle::application::route_definition::{
    BuilderStep, LanguageExpressionDef, RouteDefinition,
};
use crate::lifecycle::domain::{RouteRuntimeAggregate, RouteRuntimeState};
use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::{
    CanonicalRouteSpec, RuntimeCommand, RuntimeCommandResult, RuntimeQuery, RuntimeQueryResult,
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

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(HoldConsumer))
    }

    fn create_producer(
        &self,
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
    assert!(pred.matches(&ex).unwrap());
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
    impl Expression for DummyExpression {
        fn evaluate(
            &self,
            _exchange: &camel_api::Exchange,
        ) -> Result<camel_api::Value, LanguageError> {
            Ok(camel_api::Value::String("ok".into()))
        }
    }

    struct DummyPredicate;
    impl Predicate for DummyPredicate {
        fn matches(&self, _exchange: &camel_api::Exchange) -> Result<bool, LanguageError> {
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
    let report = ctx.health_check();

    assert_eq!(report.status, HealthStatus::Healthy);
    assert!(report.services.is_empty());
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

        fn create_consumer(&self) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
            Err(CamelError::RouteError("no consumer".to_string()))
        }

        fn create_producer(
            &self,
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
