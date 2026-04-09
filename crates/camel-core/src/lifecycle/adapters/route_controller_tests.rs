use super::*;
use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinition};
use crate::shared::components::domain::Registry;
use camel_api::{Value, ValueSourceDef};

fn build_controller() -> DefaultRouteController {
    DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new())))
}

fn build_controller_with_components() -> DefaultRouteController {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(camel_component_timer::TimerComponent::new());
        guard.register(camel_component_mock::MockComponent::new());
        guard.register(camel_component_log::LogComponent::new());
    }
    DefaultRouteController::new(registry)
}

fn register_simple_language(controller: &mut DefaultRouteController) {
    controller.languages.lock().expect("languages lock").insert(
        "simple".into(),
        Arc::new(camel_language_simple::SimpleLanguage),
    );
}

#[test]
fn helper_functions_cover_non_async_branches() {
    let managed = ManagedRoute {
        definition: RouteDefinition::new("timer:a", vec![])
            .with_route_id("r")
            .to_info(),
        from_uri: "timer:a".into(),
        pipeline: Arc::new(ArcSwap::from_pointee(SyncBoxProcessor(BoxProcessor::new(
            IdentityProcessor,
        )))),
        concurrency: None,
        consumer_handle: None,
        pipeline_handle: None,
        consumer_cancel_token: CancellationToken::new(),
        pipeline_cancel_token: CancellationToken::new(),
        channel_sender: None,
        in_flight: None,
        aggregate_split: None,
        agg_service: None,
    };

    assert_eq!(inferred_lifecycle_label(&managed), "Stopped");
    assert!(!handle_is_running(&managed.consumer_handle));

    let cmd = runtime_failure_command("route-x", "boom");
    match cmd {
        RuntimeCommand::FailRoute {
            route_id, error, ..
        } => {
            assert_eq!(route_id, "route-x");
            assert_eq!(error, "boom");
        }
        _ => panic!("expected FailRoute command"),
    }
}

#[test]
fn add_route_detects_duplicates() {
    let mut controller = build_controller();

    controller
        .add_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("r1"))
        .expect("add route");

    let dup_err = controller
        .add_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("r1"))
        .expect_err("duplicate must fail");
    assert!(dup_err.to_string().contains("already exists"));
}

#[test]
fn route_introspection_and_ordering_helpers_work() {
    let mut controller = build_controller();

    controller
        .add_route(
            RouteDefinition::new("timer:a", vec![])
                .with_route_id("a")
                .with_startup_order(20),
        )
        .unwrap();
    controller
        .add_route(
            RouteDefinition::new("timer:b", vec![])
                .with_route_id("b")
                .with_startup_order(10),
        )
        .unwrap();
    controller
        .add_route(
            RouteDefinition::new("timer:c", vec![])
                .with_route_id("c")
                .with_auto_startup(false)
                .with_startup_order(5),
        )
        .unwrap();

    assert_eq!(controller.route_count(), 3);
    assert_eq!(controller.route_from_uri("a"), Some("timer:a".into()));
    assert!(controller.route_ids().contains(&"a".to_string()));
    assert_eq!(
        controller.auto_startup_route_ids(),
        vec!["b".to_string(), "a".to_string()]
    );
    assert_eq!(
        controller.shutdown_route_ids(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[test]
fn swap_pipeline_and_remove_route_behaviors() {
    let mut controller = build_controller();

    controller
        .add_route(RouteDefinition::new("timer:a", vec![]).with_route_id("swap"))
        .unwrap();

    controller
        .swap_pipeline("swap", BoxProcessor::new(IdentityProcessor))
        .unwrap();
    assert!(controller.get_pipeline("swap").is_some());

    controller.remove_route("swap").unwrap();
    assert_eq!(controller.route_count(), 0);

    let err = controller
        .remove_route("swap")
        .expect_err("missing route must fail");
    assert!(err.to_string().contains("not found"));
}

#[test]
fn resolve_steps_covers_declarative_and_eip_variants() {
    use camel_api::LanguageExpressionDef;
    use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_lines};

    let mut controller = build_controller_with_components();
    register_simple_language(&mut controller);

    let expr = |source: &str| LanguageExpressionDef {
        language: "simple".into(),
        source: source.into(),
    };

    let steps = vec![
        BuilderStep::To("mock:out".into()),
        BuilderStep::Stop,
        BuilderStep::Log {
            level: camel_processor::LogLevel::Info,
            message: "log".into(),
        },
        BuilderStep::DeclarativeSetHeader {
            key: "k".into(),
            value: ValueSourceDef::Literal(Value::String("v".into())),
        },
        BuilderStep::DeclarativeSetHeader {
            key: "k2".into(),
            value: ValueSourceDef::Expression(expr("${body}")),
        },
        BuilderStep::DeclarativeSetBody {
            value: ValueSourceDef::Expression(expr("${body}")),
        },
        BuilderStep::DeclarativeFilter {
            predicate: expr("${body} != null"),
            steps: vec![BuilderStep::Stop],
        },
        BuilderStep::DeclarativeChoice {
            whens: vec![
                crate::lifecycle::application::route_definition::DeclarativeWhenStep {
                    predicate: expr("${body} == 'x'"),
                    steps: vec![BuilderStep::Stop],
                },
            ],
            otherwise: Some(vec![BuilderStep::Stop]),
        },
        BuilderStep::DeclarativeScript {
            expression: expr("${body}"),
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
                .build(),
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
        BuilderStep::WireTap {
            uri: "mock:tap".into(),
        },
        BuilderStep::Multicast {
            steps: vec![
                BuilderStep::To("mock:m1".into()),
                BuilderStep::To("mock:m2".into()),
            ],
            config: camel_api::MulticastConfig::new(),
        },
        BuilderStep::DeclarativeLog {
            level: camel_processor::LogLevel::Info,
            message: ValueSourceDef::Expression(expr("${body}")),
        },
        BuilderStep::Throttle {
            config: camel_api::ThrottlerConfig::new(10, Duration::from_millis(100)),
            steps: vec![BuilderStep::To("mock:t".into())],
        },
        BuilderStep::LoadBalance {
            config: camel_api::LoadBalancerConfig::round_robin(),
            steps: vec![
                BuilderStep::To("mock:l1".into()),
                BuilderStep::To("mock:l2".into()),
            ],
        },
        BuilderStep::DynamicRouter {
            config: camel_api::DynamicRouterConfig::new(Arc::new(|_| Some("mock:dr".into()))),
        },
        BuilderStep::RoutingSlip {
            config: camel_api::RoutingSlipConfig::new(Arc::new(|_| Some("mock:rs".into()))),
        },
    ];

    let producer_ctx = ProducerContext::new();
    let resolved = controller
        .resolve_steps(steps, &producer_ctx, &controller.registry)
        .expect("resolve should succeed");
    assert!(!resolved.is_empty());
}

#[test]
fn resolve_steps_script_requires_mutating_language_support() {
    use camel_api::LanguageExpressionDef;

    let mut controller = build_controller_with_components();
    register_simple_language(&mut controller);

    let steps = vec![BuilderStep::Script {
        language: "simple".into(),
        script: "${body}".into(),
    }];

    let err = controller
        .resolve_steps(steps, &ProducerContext::new(), &controller.registry)
        .expect_err("simple script should fail for mutating expression");
    assert!(err.to_string().contains("does not support"));

    let bean_missing = vec![BuilderStep::Bean {
        name: "unknown".into(),
        method: "run".into(),
    }];
    let bean_err = controller
        .resolve_steps(bean_missing, &ProducerContext::new(), &controller.registry)
        .expect_err("missing bean must fail");
    assert!(bean_err.to_string().contains("Bean not found"));

    let bad_declarative = vec![BuilderStep::DeclarativeScript {
        expression: LanguageExpressionDef {
            language: "unknown".into(),
            source: "x".into(),
        },
    }];
    let lang_err = controller
        .resolve_steps(
            bad_declarative,
            &ProducerContext::new(),
            &controller.registry,
        )
        .expect_err("unknown language must fail");
    assert!(lang_err.to_string().contains("not registered"));
}

#[tokio::test]
async fn lifecycle_methods_report_missing_routes() {
    let mut controller = build_controller();

    assert!(controller.start_route("missing").await.is_err());
    assert!(controller.stop_route("missing").await.is_err());
    assert!(controller.suspend_route("missing").await.is_err());
    assert!(controller.resume_route("missing").await.is_err());
}

#[tokio::test]
async fn start_stop_route_happy_path_with_timer_and_mock() {
    let mut controller = build_controller_with_components();

    let route = RouteDefinition::new(
        "timer:tick?period=10&repeatCount=1",
        vec![BuilderStep::To("mock:out".into())],
    )
    .with_route_id("rt-1");
    controller.add_route(route).unwrap();

    controller.start_route("rt-1").await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    controller.stop_route("rt-1").await.unwrap();

    controller.remove_route("rt-1").unwrap();
}

#[tokio::test]
async fn suspend_resume_and_restart_cover_execution_transitions() {
    let mut controller = build_controller_with_components();

    let route = RouteDefinition::new(
        "timer:tick?period=30",
        vec![BuilderStep::To("mock:out".into())],
    )
    .with_route_id("rt-2");
    controller.add_route(route).unwrap();

    controller.start_route("rt-2").await.unwrap();
    controller.suspend_route("rt-2").await.unwrap();
    controller.resume_route("rt-2").await.unwrap();
    controller.restart_route("rt-2").await.unwrap();
    controller.stop_route("rt-2").await.unwrap();
}

#[tokio::test]
async fn remove_route_rejects_running_route() {
    let mut controller = build_controller_with_components();

    let route = RouteDefinition::new(
        "timer:tick?period=25",
        vec![BuilderStep::To("mock:out".into())],
    )
    .with_route_id("rt-running");
    controller.add_route(route).unwrap();
    controller.start_route("rt-running").await.unwrap();

    let err = controller
        .remove_route("rt-running")
        .expect_err("running route removal must fail");
    assert!(err.to_string().contains("must be stopped before removal"));

    controller.stop_route("rt-running").await.unwrap();
    controller.remove_route("rt-running").unwrap();
}

#[tokio::test]
async fn start_route_on_suspended_state_returns_guidance_error() {
    let mut controller = build_controller_with_components();

    let route = RouteDefinition::new(
        "timer:tick?period=40",
        vec![BuilderStep::To("mock:out".into())],
    )
    .with_route_id("rt-suspend");
    controller.add_route(route).unwrap();

    controller.start_route("rt-suspend").await.unwrap();
    controller.suspend_route("rt-suspend").await.unwrap();

    let err = controller
        .start_route("rt-suspend")
        .await
        .expect_err("start from suspended must fail");
    assert!(err.to_string().contains("use resume_route"));

    controller.resume_route("rt-suspend").await.unwrap();
    controller.stop_route("rt-suspend").await.unwrap();
}

#[tokio::test]
async fn suspend_and_resume_validate_execution_state() {
    let mut controller = build_controller_with_components();

    controller
        .add_route(RouteDefinition::new("timer:tick?period=50", vec![]).with_route_id("rt-state"))
        .unwrap();

    let suspend_err = controller
        .suspend_route("rt-state")
        .await
        .expect_err("suspend before start must fail");
    assert!(suspend_err.to_string().contains("Cannot suspend route"));

    controller.start_route("rt-state").await.unwrap();
    let resume_err = controller
        .resume_route("rt-state")
        .await
        .expect_err("resume while started must fail");
    assert!(resume_err.to_string().contains("Cannot resume route"));

    controller.stop_route("rt-state").await.unwrap();
}

#[tokio::test]
async fn concurrent_concurrency_override_path_executes() {
    let mut controller = build_controller_with_components();

    let route = RouteDefinition::new(
        "timer:tick?period=10&repeatCount=2",
        vec![BuilderStep::To("mock:out".into())],
    )
    .with_route_id("rt-concurrent")
    .with_concurrency(ConcurrencyModel::Concurrent { max: Some(2) });

    controller.add_route(route).unwrap();
    controller.start_route("rt-concurrent").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    controller.stop_route("rt-concurrent").await.unwrap();
}

#[tokio::test]
async fn add_route_with_circuit_breaker_and_error_handler_compiles() {
    use camel_api::circuit_breaker::CircuitBreakerConfig;
    use camel_api::error_handler::ErrorHandlerConfig;

    let mut controller = build_controller_with_components();

    let route = RouteDefinition::new("timer:tick?period=25", vec![BuilderStep::Stop])
        .with_route_id("rt-eh")
        .with_circuit_breaker(CircuitBreakerConfig::new())
        .with_error_handler(ErrorHandlerConfig::dead_letter_channel("log:dlq"));

    controller
        .add_route(route)
        .expect("route with layers should compile");
    controller.start_route("rt-eh").await.unwrap();
    controller.stop_route("rt-eh").await.unwrap();
}

#[tokio::test]
async fn compile_and_swap_errors_for_missing_route() {
    let mut controller = build_controller_with_components();

    let compiled = controller
        .compile_route_definition(
            RouteDefinition::new("timer:tick?period=10", vec![BuilderStep::Stop])
                .with_route_id("compiled"),
        )
        .expect("compile should work");

    let err = controller
        .swap_pipeline("nope", compiled)
        .expect_err("missing route swap must fail");
    assert!(err.to_string().contains("not found"));
}
