use super::*;
use crate::lifecycle::adapters::pipeline_runtime::PipelineAssembly;
use crate::lifecycle::adapters::route_helpers::runtime_failure_command;
use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinition};
use crate::shared::components::domain::Registry;
use arc_swap::ArcSwap;
use camel_api::function::PrepareToken;
use camel_api::{
    BoxProcessor, BoxProcessorExt, ExchangePatch, FunctionDefinition, FunctionDiff, FunctionId,
    FunctionInvocationError, FunctionInvoker, FunctionInvokerSync, IdentityProcessor, Message,
    OpaqueProcessor, RuntimeCommand, StepLifecycle, StepShutdownReason, SyncBoxProcessor, Value,
    ValueSourceDef,
};
use camel_component_api::ConcurrencyModel;

struct NoopInvoker;

impl FunctionInvokerSync for NoopInvoker {
    fn stage_pending(&self, _def: FunctionDefinition, _route_id: Option<&str>, _generation: u64) {}
    fn discard_staging(&self, _generation: u64) {}
    fn begin_reload(&self) -> u64 {
        1
    }
    fn function_refs_for_route(&self, _route_id: &str) -> Vec<(FunctionId, Option<String>)> {
        vec![]
    }
    fn staged_refs_for_route(
        &self,
        _route_id: &str,
        _generation: u64,
    ) -> Vec<(FunctionId, Option<String>)> {
        vec![]
    }
    fn staged_defs_for_route(
        &self,
        _route_id: &str,
        _generation: u64,
    ) -> Vec<(FunctionDefinition, Option<String>)> {
        vec![]
    }
}

#[async_trait::async_trait]
impl FunctionInvoker for NoopInvoker {
    async fn register(
        &self,
        _def: FunctionDefinition,
        _route_id: Option<&str>,
    ) -> Result<(), FunctionInvocationError> {
        Ok(())
    }

    async fn unregister(
        &self,
        _id: &FunctionId,
        _route_id: Option<&str>,
    ) -> Result<(), FunctionInvocationError> {
        Ok(())
    }

    async fn invoke(
        &self,
        _id: &FunctionId,
        _exchange: &camel_api::Exchange,
    ) -> Result<ExchangePatch, FunctionInvocationError> {
        Ok(ExchangePatch::default())
    }

    async fn prepare_reload(
        &self,
        _diff: FunctionDiff,
        _generation: u64,
    ) -> Result<PrepareToken, FunctionInvocationError> {
        Ok(PrepareToken::default())
    }

    async fn finalize_reload(
        &self,
        _diff: &FunctionDiff,
        _generation: u64,
    ) -> Result<(), FunctionInvocationError> {
        Ok(())
    }

    async fn rollback_reload(
        &self,
        _token: PrepareToken,
        _generation: u64,
    ) -> Result<(), FunctionInvocationError> {
        Ok(())
    }

    async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
        Ok(())
    }
}

fn build_controller() -> DefaultRouteController {
    DefaultRouteController::new(
        Arc::new(std::sync::Mutex::new(Registry::new())),
        Arc::new(camel_api::NoopPlatformService::default()),
    )
}

fn build_controller_with_components() -> DefaultRouteController {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(std::sync::Arc::new(
            camel_component_timer::TimerComponent::new(),
        ));
        guard.register(std::sync::Arc::new(
            camel_component_mock::MockComponent::new(),
        ));
        guard.register(std::sync::Arc::new(camel_component_log::LogComponent::new()));
    }
    DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    )
}

fn register_simple_language(controller: &mut DefaultRouteController) {
    controller.languages.lock().expect("languages lock").insert(
        "simple".into(),
        Arc::new(camel_language_simple::SimpleLanguage::new()),
    );
}

#[test]
fn helper_functions_cover_non_async_branches() {
    let managed = ManagedRoute {
        definition: RouteDefinition::new("timer:a", vec![])
            .with_route_id("r")
            .to_info(),
        from_uri: "timer:a".into(),
        pipeline: Arc::new(ArcSwap::from_pointee(PipelineAssembly::new(
            SyncBoxProcessor::new(BoxProcessor::new(IdentityProcessor)),
            vec![],
        ))),
        concurrency: None,
        consumer_handle: None,
        pipeline_handle: None,
        consumer_cancel_token: CancellationToken::new(),
        pipeline_cancel_token: CancellationToken::new(),
        channel_sender: None,
        in_flight: None,
        drain_in_flight: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        aggregate_split: None,
        agg_service: None,
        compiled: route_runtime_state::CompiledRoute {
            security_policy: None,
            security_authenticator: None,
        },
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

#[tokio::test]
async fn add_route_detects_duplicates() {
    let mut controller = build_controller();

    controller
        .add_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("r1"))
        .await
        .expect("add route");

    let dup_err = controller
        .add_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("r1"))
        .await
        .expect_err("duplicate must fail");
    assert!(dup_err.to_string().contains("already exists"));
}

#[tokio::test]
async fn route_introspection_and_ordering_helpers_work() {
    let mut controller = build_controller();

    controller
        .add_route(
            RouteDefinition::new("timer:a", vec![])
                .with_route_id("a")
                .with_startup_order(20),
        )
        .await
        .unwrap();
    controller
        .add_route(
            RouteDefinition::new("timer:b", vec![])
                .with_route_id("b")
                .with_startup_order(10),
        )
        .await
        .unwrap();
    controller
        .add_route(
            RouteDefinition::new("timer:c", vec![])
                .with_route_id("c")
                .with_auto_startup(false)
                .with_startup_order(5),
        )
        .await
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

#[tokio::test]
async fn swap_pipeline_and_remove_route_behaviors() {
    let mut controller = build_controller();

    controller
        .add_route(RouteDefinition::new("timer:a", vec![]).with_route_id("swap"))
        .await
        .unwrap();

    controller
        .swap_pipeline("swap", BoxProcessor::new(IdentityProcessor))
        .unwrap();
    assert!(controller.get_pipeline("swap").is_some());

    controller.remove_route("swap").await.unwrap();
    assert_eq!(controller.route_count(), 0);

    let err = controller
        .remove_route("swap")
        .await
        .expect_err("missing route must fail");
    assert!(err.to_string().contains("not found"));
}

#[test]
fn resolve_steps_covers_declarative_and_eip_variants() {
    use camel_api::FilterPredicate;
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
                .build()
                .unwrap(),
        },
        BuilderStep::Filter {
            predicate: FilterPredicate::new(|_| true),
            steps: vec![BuilderStep::Stop],
        },
        BuilderStep::Choice {
            whens: vec![crate::lifecycle::application::route_definition::WhenStep {
                predicate: FilterPredicate::new(|_| true),
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
        .resolve_steps(
            steps,
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
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
        .resolve_steps(
            steps,
            &ProducerContext::new(),
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect_err("simple script should fail for mutating expression");
    assert!(err.to_string().contains("does not support"));

    let bean_missing = vec![BuilderStep::Bean {
        name: "unknown".into(),
        method: "run".into(),
    }];
    let bean_err = controller
        .resolve_steps(
            bean_missing,
            &ProducerContext::new(),
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
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
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
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
    controller.add_route(route).await.unwrap();

    controller.start_route("rt-1").await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    controller.stop_route("rt-1").await.unwrap();

    controller.remove_route("rt-1").await.unwrap();
}

#[tokio::test]
async fn start_route_spawns_pipeline_before_consumer_for_eager_consumers() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    set_start_route_event_hook(Some({
        let events = Arc::clone(&events);
        Arc::new(move |event| {
            events.lock().expect("events lock").push(event);
        })
    }));

    let mut controller = build_controller_with_components();
    controller
        .add_route(
            RouteDefinition::new(
                "timer:tick?period=10&repeatCount=1",
                vec![BuilderStep::To("mock:out".into())],
            )
            .with_route_id("startup-order"),
        )
        .await
        .unwrap();

    controller.start_route("startup-order").await.unwrap();
    set_start_route_event_hook(None);
    controller.stop_route("startup-order").await.unwrap();

    let events = events.lock().expect("events lock").clone();
    let pipeline_index = events
        .iter()
        .position(|event| *event == "pipeline_spawned")
        .expect("pipeline spawn event");
    let consumer_index = events
        .iter()
        .position(|event| *event == "consumer_spawned")
        .expect("consumer spawn event");

    assert!(
        pipeline_index < consumer_index,
        "expected pipeline task to spawn before consumer task, got {events:?}"
    );
}

#[tokio::test]
async fn suspend_resume_and_restart_cover_execution_transitions() {
    let mut controller = build_controller_with_components();

    let route = RouteDefinition::new(
        "timer:tick?period=30",
        vec![BuilderStep::To("mock:out".into())],
    )
    .with_route_id("rt-2");
    controller.add_route(route).await.unwrap();

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
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-running").await.unwrap();

    let err = controller
        .remove_route("rt-running")
        .await
        .expect_err("running route removal must fail");
    assert!(err.to_string().contains("must be stopped before removal"));

    controller.stop_route("rt-running").await.unwrap();
    controller.remove_route("rt-running").await.unwrap();
}

#[tokio::test]
async fn start_route_on_suspended_state_returns_guidance_error() {
    let mut controller = build_controller_with_components();

    let route = RouteDefinition::new(
        "timer:tick?period=40",
        vec![BuilderStep::To("mock:out".into())],
    )
    .with_route_id("rt-suspend");
    controller.add_route(route).await.unwrap();

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
        .await
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

    controller.add_route(route).await.unwrap();
    controller.start_route("rt-concurrent").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    controller.stop_route("rt-concurrent").await.unwrap();
}

#[tokio::test]
async fn concurrent_backpressure_blocks_processor_when_saturated() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    // This test verifies back-pressure at the processor level: with
    // Concurrent { max: 1 }, exchange B's processor is NOT invoked while
    // exchange A holds the sole semaphore permit.
    //
    // NOTE: This invariant holds under BOTH the old (permit-after-dequeue)
    // and new (B2 permit-before-dequeue, ADR-0044) admission patterns.
    // In the old code, B is dequeued into a spawned task that blocks on
    // semaphore acquire; in the new B2 code, B stays in the mpsc channel
    // because the outer loop blocks on acquire before rx.recv().  Neither
    // increments process_count for B.
    //
    // The distinguishing invariant (B stays in the mpsc channel under B2)
    // is not directly observable from outside the pipeline task.  B2
    // ordering correctness is verified by code inspection of
    // route_controller_trait.rs and documented in ADR-0044.
    //
    // If someone reverts to the old permit-after-dequeue loop:
    //   rx.recv() -> spawn { sem.acquire(); pipe.call() }
    // this test STILL PASSES — B's processor is not called in either case.
    let mut controller = build_controller_with_components();

    // Oneshot to block the first exchange's processor until test release
    let (block_tx, block_rx) = tokio::sync::oneshot::channel::<()>();
    let block_rx = Arc::new(tokio::sync::Mutex::new(Some(block_rx)));

    // Track how many times the processor was invoked
    let process_count = Arc::new(AtomicUsize::new(0));

    let processor = {
        let count = process_count.clone();
        let rx = block_rx.clone();
        BoxProcessor::from_fn(move |exchange: Exchange| {
            let count = count.clone();
            let rx = rx.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                // First exchange blocks waiting for test release
                let mut guard = rx.lock().await;
                if let Some(rx) = guard.take() {
                    let _ = rx.await;
                }
                Ok(exchange)
            })
        })
    };

    // period + delay prevent the timer's immediate first tick from
    // interfering — the consumer sleeps for 10s before producing any
    // exchanges, so only manually-injected exchanges go through the pipeline.
    let route = RouteDefinition::new(
        "timer:tick?period=10000&delay=10000",
        vec![BuilderStep::Processor(OpaqueProcessor(processor))],
    )
    .with_route_id("rt-b2")
    .with_concurrency(ConcurrencyModel::Concurrent { max: Some(1) });

    controller.add_route(route).await.unwrap();
    controller.start_route("rt-b2").await.unwrap();

    // Small yield for pipeline task to start polling
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Get channel sender to inject exchanges directly (bypass the timer
    // consumer, which never fires with period=10000 within the test window).
    let sender = controller
        .routes
        .get("rt-b2")
        .and_then(|r| r.channel_sender.clone())
        .expect("channel sender should exist after start");

    // Send exchange A — acquires the semaphore permit, blocks on oneshot
    sender
        .send(ExchangeEnvelope {
            exchange: Exchange::new(Message::new("A")),
            reply_tx: None,
        })
        .await
        .unwrap();

    // Allow the pipeline loop to dequeue A and spawn the processing task
    tokio::time::sleep(Duration::from_millis(30)).await;

    assert_eq!(
        process_count.load(Ordering::SeqCst),
        1,
        "exchange A should be processing (permit acquired)"
    );

    // Send exchange B — permit held by A, B blocks at semaphore acquisition
    sender
        .send(ExchangeEnvelope {
            exchange: Exchange::new(Message::new("B")),
            reply_tx: None,
        })
        .await
        .unwrap();

    // Allow time for B to attempt (and fail) to acquire the permit
    tokio::time::sleep(Duration::from_millis(30)).await;

    assert_eq!(
        process_count.load(Ordering::SeqCst),
        1,
        "exchange B should NOT be processing (permit held by A)"
    );

    // Release A — completion drops the permit, B acquires and processes
    block_tx.send(()).unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        process_count.load(Ordering::SeqCst),
        2,
        "exchange B should process after A releases the permit"
    );

    controller.stop_route("rt-b2").await.unwrap();
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
        .await
        .expect("route with layers should compile");
    controller.start_route("rt-eh").await.unwrap();
    controller.stop_route("rt-eh").await.unwrap();
}

#[tokio::test]
async fn compile_and_swap_errors_for_missing_route() {
    let controller = build_controller_with_components();

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

#[test]
fn resolve_steps_covers_remaining_builder_step_arms() {
    use camel_api::LanguageExpressionDef;

    let mut controller = build_controller_with_components();
    register_simple_language(&mut controller);
    let producer_ctx = ProducerContext::new();

    let expr = |source: &str| LanguageExpressionDef {
        language: "simple".into(),
        source: source.into(),
    };

    let resolved = controller
        .resolve_steps(
            vec![BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(
                IdentityProcessor,
            )))],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect("processor step should resolve");
    assert_eq!(resolved.len(), 1);

    let resolved = controller
        .resolve_steps(
            vec![BuilderStep::Delay {
                config: camel_api::DelayConfig::new(1),
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect("delay step should resolve");
    assert_eq!(resolved.len(), 1);

    let resolved = controller
        .resolve_steps(
            vec![BuilderStep::DeclarativeSetBody {
                value: ValueSourceDef::Literal(Value::Null),
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect("declarative set body null should resolve");
    assert_eq!(resolved.len(), 1);

    let resolved = controller
        .resolve_steps(
            vec![BuilderStep::DeclarativeSetBody {
                value: ValueSourceDef::Literal(Value::String("hello".into())),
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect("declarative set body string should resolve");
    assert_eq!(resolved.len(), 1);

    let resolved = controller
        .resolve_steps(
            vec![BuilderStep::DeclarativeSetBody {
                value: ValueSourceDef::Literal(Value::Bool(true)),
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect("declarative set body json should resolve");
    assert_eq!(resolved.len(), 1);

    let resolved = controller
        .resolve_steps(
            vec![BuilderStep::RoutingSlip {
                config: camel_api::RoutingSlipConfig::new(Arc::new(|_| Some("mock:rs".into()))),
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect("routing slip step should resolve");
    assert_eq!(resolved.len(), 1);

    let resolved = controller
        .resolve_steps(
            vec![BuilderStep::DeclarativeRoutingSlip {
                expression: expr("${body}"),
                uri_delimiter: ";".into(),
                cache_size: 16,
                ignore_invalid_endpoints: true,
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect("declarative routing slip step should resolve");
    assert_eq!(resolved.len(), 1);

    let resolved = controller
        .resolve_steps(
            vec![BuilderStep::RecipientList {
                config: camel_api::recipient_list::RecipientListConfig::new(Arc::new(|_| {
                    "mock:r1,mock:r2".into()
                })),
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect("recipient list step should resolve");
    assert_eq!(resolved.len(), 1);

    let resolved = controller
        .resolve_steps(
            vec![BuilderStep::DeclarativeRecipientList {
                expression: expr("${body}"),
                delimiter: ",".into(),
                parallel: true,
                parallel_limit: Some(2),
                stop_on_exception: false,
                aggregation: "collect".into(),
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect("declarative recipient list step should resolve");
    assert_eq!(resolved.len(), 1);

    let resolved = controller
        .resolve_steps(
            vec![BuilderStep::DeclarativeDynamicRouter {
                expression: expr("${body}"),
                uri_delimiter: ",".into(),
                cache_size: 8,
                ignore_invalid_endpoints: true,
                max_iterations: 3,
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect("declarative dynamic router step should resolve");
    assert_eq!(resolved.len(), 1);
}

#[test]
fn resolve_steps_error_paths_unknown_scheme_and_language() {
    use camel_api::LanguageExpressionDef;
    use camel_language_api::{Expression, Language, LanguageError, MutatingExpression, Predicate};

    struct ConstExpr;
    #[async_trait::async_trait]
    impl Expression for ConstExpr {
        async fn evaluate(&self, _exchange: &camel_api::Exchange) -> Result<Value, LanguageError> {
            Ok(Value::Null)
        }
    }

    struct ConstPred;
    #[async_trait::async_trait]
    impl Predicate for ConstPred {
        async fn matches(&self, _exchange: &camel_api::Exchange) -> Result<bool, LanguageError> {
            Ok(true)
        }
    }

    struct FailingMutatingExpr;
    #[async_trait::async_trait]
    impl MutatingExpression for FailingMutatingExpr {
        async fn evaluate(
            &self,
            _exchange: &mut camel_api::Exchange,
        ) -> Result<Value, LanguageError> {
            Ok(Value::Null)
        }
    }

    struct FailingMutatingLanguage;
    impl Language for FailingMutatingLanguage {
        fn name(&self) -> &'static str {
            "failing"
        }

        fn create_expression(&self, _script: &str) -> Result<Box<dyn Expression>, LanguageError> {
            Ok(Box::new(ConstExpr))
        }

        fn create_predicate(&self, _script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
            Ok(Box::new(ConstPred))
        }

        fn create_mutating_expression(
            &self,
            _script: &str,
        ) -> Result<Box<dyn MutatingExpression>, LanguageError> {
            let _ = FailingMutatingExpr;
            Err(LanguageError::EvalError("boom".into()))
        }
    }

    let mut controller = build_controller_with_components();
    register_simple_language(&mut controller);
    controller
        .languages
        .lock()
        .expect("languages lock")
        .insert("failing".into(), Arc::new(FailingMutatingLanguage));

    let producer_ctx = ProducerContext::new();

    let err = controller
        .resolve_steps(
            vec![BuilderStep::To("missing:out".into())],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect_err("unknown scheme in to should fail");
    assert!(err.to_string().contains("missing"));

    let err = controller
        .resolve_steps(
            vec![BuilderStep::WireTap {
                uri: "missing:tap".into(),
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect_err("unknown scheme in wiretap should fail");
    assert!(err.to_string().contains("missing"));

    let err = controller
        .resolve_steps(
            vec![BuilderStep::DeclarativeFilter {
                predicate: LanguageExpressionDef {
                    language: "unknown".into(),
                    source: "x".into(),
                },
                steps: vec![BuilderStep::Stop],
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect_err("unknown language in declarative filter should fail");
    assert!(err.to_string().contains("not registered"));

    let err = controller
        .resolve_steps(
            vec![BuilderStep::DeclarativeChoice {
                whens: vec![
                    crate::lifecycle::application::route_definition::DeclarativeWhenStep {
                        predicate: LanguageExpressionDef {
                            language: "unknown".into(),
                            source: "x".into(),
                        },
                        steps: vec![BuilderStep::Stop],
                    },
                ],
                otherwise: None,
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect_err("unknown language in declarative choice should fail");
    assert!(err.to_string().contains("not registered"));

    let err = controller
        .resolve_steps(
            vec![BuilderStep::DeclarativeLog {
                level: camel_processor::LogLevel::Info,
                message: ValueSourceDef::Expression(LanguageExpressionDef {
                    language: "unknown".into(),
                    source: "x".into(),
                }),
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect_err("unknown language in declarative log should fail");
    assert!(err.to_string().contains("not registered"));

    let err = controller
        .resolve_steps(
            vec![BuilderStep::DeclarativeScript {
                expression: LanguageExpressionDef {
                    language: "failing".into(),
                    source: "x".into(),
                },
            }],
            &producer_ctx,
            &controller.registry,
            None,
            &crate::lifecycle::adapters::step_resolution::FunctionStagingMode::DirectAdd,
        )
        .expect_err("declarative script generic language error should fail");
    assert!(
        err.to_string()
            .contains("Failed to create mutating expression for language 'failing'")
    );
    assert!(err.to_string().contains("boom"));

    let err = match crate::lifecycle::adapters::step_resolution::resolve_language(
        &controller.languages,
        "not-registered",
    ) {
        Ok(_) => panic!("resolve_language should fail for unknown language"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("not registered"));

    let err = match crate::lifecycle::adapters::step_resolution::compile_language_expression(
        &controller.languages,
        &LanguageExpressionDef {
            language: "simple".into(),
            source: "${unknown}".into(),
        },
    ) {
        Ok(_) => panic!("compile_language_expression should fail for invalid source"),
        Err(err) => err,
    };
    assert!(
        err.to_string()
            .contains("failed to compile simple expression `${unknown}`")
    );
}

#[tokio::test]
async fn add_route_with_generation_and_prepare_insert_behaviors() {
    let mut controller = build_controller_with_components();

    controller
        .add_route_with_generation(
            RouteDefinition::new("timer:tick?period=15", vec![BuilderStep::Stop])
                .with_route_id("g1"),
            7,
        )
        .await
        .expect("add with generation");

    let dup = controller
        .add_route_with_generation(
            RouteDefinition::new("timer:tick?period=15", vec![BuilderStep::Stop])
                .with_route_id("g1"),
            8,
        )
        .await
        .expect_err("duplicate add with generation should fail");
    assert!(dup.to_string().contains("already exists"));

    let prepared = controller
        .prepare_route_definition_with_generation(
            RouteDefinition::new("timer:tick?period=20", vec![BuilderStep::Stop])
                .with_route_id("g2"),
            9,
        )
        .expect("prepare route");

    controller
        .insert_prepared_route(prepared)
        .expect("insert prepared route");

    let prepared_dup = controller
        .prepare_route_definition_with_generation(
            RouteDefinition::new("timer:tick?period=21", vec![BuilderStep::Stop])
                .with_route_id("g2"),
            10,
        )
        .expect("prepare duplicate route");

    let err = controller
        .insert_prepared_route(prepared_dup)
        .expect_err("insert duplicate prepared route should fail");
    assert!(err.to_string().contains("already exists"));
}

#[test]
fn compile_route_definition_with_generation_and_global_error_handler_paths() {
    use camel_api::error_handler::ErrorHandlerConfig;

    let mut controller = build_controller_with_components();
    controller.set_error_handler(ErrorHandlerConfig::dead_letter_channel("log:dlq"));

    let _compiled = controller
        .compile_route_definition_with_generation(
            RouteDefinition::new("timer:tick?period=10", vec![BuilderStep::Stop])
                .with_route_id("cg"),
            11,
        )
        .expect("compile with generation should work");

    let mut failing = build_controller();
    failing.set_error_handler(ErrorHandlerConfig::dead_letter_channel("missing:dlq"));

    let err = failing
        .compile_route_definition(
            RouteDefinition::new("timer:tick?period=10", vec![BuilderStep::Stop])
                .with_route_id("fail-eh"),
        )
        .expect_err("missing dlc component should fail");
    assert!(err.to_string().contains("missing"));
}

#[tokio::test]
async fn start_route_state_guards_cover_already_started_and_inconsistent() {
    let mut controller = build_controller_with_components();

    controller
        .add_route(RouteDefinition::new("timer:tick?period=30", vec![]).with_route_id("guard"))
        .await
        .unwrap();

    controller.start_route("guard").await.unwrap();
    controller.start_route("guard").await.unwrap();
    controller.stop_route("guard").await.unwrap();

    let running = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let managed = controller.routes.get_mut("guard").expect("route exists");
    managed.consumer_handle = Some(running);
    managed.pipeline_handle = None;

    let err = controller
        .start_route("guard")
        .await
        .expect_err("consumer-running pipeline-stopped should fail");
    assert!(err.to_string().contains("inconsistent execution state"));

    if let Some(handle) = controller
        .routes
        .get_mut("guard")
        .expect("route exists")
        .consumer_handle
        .take()
    {
        let _ = handle.await;
    }
}

#[tokio::test]
async fn remove_route_preserving_functions_validates_states() {
    let mut controller = build_controller_with_components();

    controller
        .add_route(RouteDefinition::new("timer:tick?period=25", vec![]).with_route_id("preserve"))
        .await
        .unwrap();
    controller.start_route("preserve").await.unwrap();

    let err = controller
        .remove_route_preserving_functions("preserve")
        .await
        .expect_err("running route must fail");
    assert!(err.to_string().contains("must be stopped before removal"));

    controller.stop_route("preserve").await.unwrap();
    controller
        .remove_route_preserving_functions("preserve")
        .await
        .unwrap();

    let missing = controller
        .remove_route_preserving_functions("preserve")
        .await
        .expect_err("missing route should fail");
    assert!(missing.to_string().contains("not found"));
}

#[tokio::test]
async fn start_all_routes_reports_failures_and_stop_all_routes_succeeds() {
    let mut controller = build_controller_with_components();

    controller
        .add_route(
            RouteDefinition::new("timer:tick?period=10", vec![BuilderStep::Stop])
                .with_route_id("ok-a")
                .with_startup_order(2),
        )
        .await
        .unwrap();
    controller
        .add_route(
            RouteDefinition::new("missing:start", vec![BuilderStep::Stop])
                .with_route_id("bad-b")
                .with_startup_order(1),
        )
        .await
        .unwrap();

    let err = controller
        .start_all_routes()
        .await
        .expect_err("one bad route should aggregate error");
    assert!(err.to_string().contains("Failed to start routes"));
    assert!(err.to_string().contains("bad-b"));

    controller
        .remove_route("bad-b")
        .await
        .expect("failed route should remain stopped and removable");

    controller.start_all_routes().await.unwrap();
    controller.stop_all_routes().await.unwrap();
}

#[test]
fn constructors_and_reload_helpers_cover_accessors() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let langs: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let beans = Arc::new(std::sync::Mutex::new(camel_bean::BeanRegistry::new()));

    let mut with_beans =
        DefaultRouteController::with_beans(Arc::clone(&registry), Arc::clone(&beans));
    with_beans.set_function_invoker(Arc::new(NoopInvoker));

    let with_langs = DefaultRouteController::with_languages(
        Arc::clone(&registry),
        Arc::clone(&langs),
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    let _with_all = DefaultRouteController::with_languages_and_beans(
        Arc::clone(&registry),
        Arc::clone(&langs),
        Arc::new(camel_api::NoopPlatformService::default()),
        Arc::clone(&beans),
    )
    .with_function_invoker(Arc::new(NoopInvoker));

    assert_eq!(with_beans.route_count(), 0);
    assert_eq!(with_langs.route_ids().len(), 0);
}

#[tokio::test]
async fn aggregate_force_completion_on_stop_emits_pending_bucket_without_timeout() {
    let mock = Arc::new(camel_component_mock::MockComponent::new());
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(camel_component_timer::TimerComponent::new()));
        guard.register(Arc::clone(&mock) as Arc<dyn camel_component_api::Component>);
    }
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    let agg_config = camel_api::AggregatorConfig::correlate_by("key")
        .complete_when_size(10)
        .force_completion_on_stop(true)
        .build()
        .unwrap();

    let route = RouteDefinition::new(
        "timer:tick?period=10&repeatCount=1",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: camel_api::ValueSourceDef::Literal(camel_api::Value::String(
                    "order-1".into(),
                )),
            },
            BuilderStep::Aggregate { config: agg_config },
            BuilderStep::To("mock:sink".into()),
        ],
    )
    .with_route_id("force-agg");
    controller.add_route(route).await.unwrap();
    controller.start_route("force-agg").await.unwrap();

    tokio::time::sleep(Duration::from_millis(80)).await;
    controller.stop_route("force-agg").await.unwrap();

    let sink = mock.get_endpoint("sink").expect("mock sink endpoint");
    sink.await_exchanges(1, Duration::from_secs(2)).await;
    let received = sink.get_received_exchanges().await;
    assert_eq!(
        received.len(),
        1,
        "expected 1 force-completed exchange, got {}",
        received.len()
    );
    assert_eq!(
        received[0].property("CamelAggregatedCompletionReason"),
        Some(&serde_json::json!("stop"))
    );
}

#[tokio::test]
async fn aggregate_without_force_completion_on_stop_discards_pending_bucket() {
    let mock = Arc::new(camel_component_mock::MockComponent::new());
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(camel_component_timer::TimerComponent::new()));
        guard.register(Arc::clone(&mock) as Arc<dyn camel_component_api::Component>);
    }
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    let agg_config = camel_api::AggregatorConfig::correlate_by("key")
        .complete_when_size(10)
        .build()
        .unwrap();

    let route = RouteDefinition::new(
        "timer:tick?period=10&repeatCount=1",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: camel_api::ValueSourceDef::Literal(camel_api::Value::String(
                    "order-1".into(),
                )),
            },
            BuilderStep::Aggregate { config: agg_config },
            BuilderStep::To("mock:sink".into()),
        ],
    )
    .with_route_id("no-force-agg");
    controller.add_route(route).await.unwrap();
    controller.start_route("no-force-agg").await.unwrap();

    tokio::time::sleep(Duration::from_millis(80)).await;
    controller.stop_route("no-force-agg").await.unwrap();

    let sink = mock.get_endpoint("sink").expect("mock sink endpoint");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let received = sink.get_received_exchanges().await;

    let has_force_complete = received.iter().any(|ex| {
        ex.property("CamelAggregatedCompletionReason")
            .map(|v| v == &serde_json::json!("stop"))
            .unwrap_or(false)
    });
    assert!(
        !has_force_complete,
        "expected no force-completed exchange, but found one with CompletionReason=stop"
    );
}

#[tokio::test]
async fn syncbox_processor_concurrent_clone_inner_via_arcswap() {
    use crate::lifecycle::adapters::pipeline_runtime::PipelineAssembly;
    use arc_swap::ArcSwap;
    use camel_api::{BoxProcessor, IdentityProcessor, SyncBoxProcessor};
    use std::sync::Arc;
    use tower::ServiceExt;

    let assembly = PipelineAssembly::new(
        SyncBoxProcessor::new(BoxProcessor::new(IdentityProcessor)),
        vec![],
    );
    let shared: Arc<ArcSwap<PipelineAssembly>> = Arc::new(ArcSwap::from_pointee(assembly));

    let mut handles = vec![];
    for _ in 0..4 {
        let shared = shared.clone();
        handles.push(tokio::spawn(async move {
            let mut cloned = shared.load().processor.clone_inner();
            assert!(cloned.ready().await.is_ok());
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}

#[tokio::test]
async fn aggregate_force_completion_on_natural_consumer_completion_emits_pending_bucket() {
    let mock = Arc::new(camel_component_mock::MockComponent::new());
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(camel_component_timer::TimerComponent::new()));
        guard.register(Arc::clone(&mock) as Arc<dyn camel_component_api::Component>);
    }
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    let agg_config = camel_api::AggregatorConfig::correlate_by("key")
        .complete_when_size(10)
        .force_completion_on_stop(true)
        .build()
        .unwrap();

    let route = RouteDefinition::new(
        "timer:tick?period=10&repeatCount=1",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: camel_api::ValueSourceDef::Literal(camel_api::Value::String(
                    "order-1".into(),
                )),
            },
            BuilderStep::Aggregate { config: agg_config },
            BuilderStep::To("mock:natural-sink".into()),
        ],
    )
    .with_route_id("natural-force-agg");
    controller.add_route(route).await.unwrap();
    controller.start_route("natural-force-agg").await.unwrap();

    let sink = mock
        .get_endpoint("natural-sink")
        .expect("mock sink endpoint");
    sink.await_exchanges(1, Duration::from_secs(2)).await;
    let received = sink.get_received_exchanges().await;
    assert_eq!(
        received.len(),
        1,
        "expected natural consumer completion to force-complete 1 exchange, got {}",
        received.len()
    );
    assert_eq!(
        received[0].property("CamelAggregatedCompletionReason"),
        Some(&serde_json::json!("stop"))
    );
}

// ── Hot-swap rejection tests (Task 4) ──

#[derive(Debug)]
struct FakeStep;

#[async_trait::async_trait]
impl StepLifecycle for FakeStep {
    fn name(&self) -> &'static str {
        "fake"
    }
    async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
        Ok(())
    }
}

#[test]
fn swap_pipeline_rejects_lifecycle_bearing_route() {
    let mut controller = build_controller();

    let assembly = PipelineAssembly::new(
        SyncBoxProcessor::new(BoxProcessor::new(IdentityProcessor)),
        vec![Arc::new(FakeStep) as Arc<dyn StepLifecycle>],
    );

    let managed = ManagedRoute {
        definition: RouteDefinition::new("timer:test", vec![])
            .with_route_id("lifecycle-route")
            .to_info(),
        from_uri: "timer:test".into(),
        pipeline: Arc::new(ArcSwap::from_pointee(assembly)),
        concurrency: None,
        consumer_handle: None,
        pipeline_handle: None,
        consumer_cancel_token: CancellationToken::new(),
        pipeline_cancel_token: CancellationToken::new(),
        channel_sender: None,
        in_flight: None,
        drain_in_flight: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        aggregate_split: None,
        agg_service: None,
        compiled: route_runtime_state::CompiledRoute {
            security_policy: None,
            security_authenticator: None,
        },
    };

    controller.routes.insert("lifecycle-route".into(), managed);

    let result = controller.swap_pipeline("lifecycle-route", BoxProcessor::new(IdentityProcessor));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("lifecycle-bearing"),
        "expected 'lifecycle-bearing' error, got: {err}"
    );
}

#[test]
fn swap_pipeline_rejects_agg_service_route() {
    use camel_api::aggregator::AggregatorConfig;
    use camel_processor::aggregator::AggregatorService;

    let mut controller = build_controller();

    let (tx, _rx) = tokio::sync::mpsc::channel::<camel_api::Exchange>(64);
    let agg_config = AggregatorConfig::correlate_by("key")
        .complete_when_size(10)
        .build()
        .unwrap();
    let langs: SharedLanguageRegistry =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let cancel = CancellationToken::new();
    let svc = AggregatorService::new(agg_config, tx, langs, cancel);

    let assembly = PipelineAssembly::new(
        SyncBoxProcessor::new(BoxProcessor::new(IdentityProcessor)),
        vec![],
    );

    let managed = ManagedRoute {
        definition: RouteDefinition::new("timer:test", vec![])
            .with_route_id("agg-route")
            .to_info(),
        from_uri: "timer:test".into(),
        pipeline: Arc::new(ArcSwap::from_pointee(assembly)),
        concurrency: None,
        consumer_handle: None,
        pipeline_handle: None,
        consumer_cancel_token: CancellationToken::new(),
        pipeline_cancel_token: CancellationToken::new(),
        channel_sender: None,
        in_flight: None,
        drain_in_flight: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        aggregate_split: None,
        agg_service: Some(Arc::new(svc)),
        compiled: route_runtime_state::CompiledRoute {
            security_policy: None,
            security_authenticator: None,
        },
    };

    controller.routes.insert("agg-route".into(), managed);

    let result = controller.swap_pipeline("agg-route", BoxProcessor::new(IdentityProcessor));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("lifecycle-bearing"),
        "expected 'lifecycle-bearing' error, got: {err}"
    );
}

#[test]
fn swap_pipeline_raw_bypasses_lifecycle_rejection() {
    let mut controller = build_controller();

    let assembly = PipelineAssembly::new(
        SyncBoxProcessor::new(BoxProcessor::new(IdentityProcessor)),
        vec![Arc::new(FakeStep) as Arc<dyn StepLifecycle>],
    );

    let managed = ManagedRoute {
        definition: RouteDefinition::new("timer:test", vec![])
            .with_route_id("lifecycle-route")
            .to_info(),
        from_uri: "timer:test".into(),
        pipeline: Arc::new(ArcSwap::from_pointee(assembly)),
        concurrency: None,
        consumer_handle: None,
        pipeline_handle: None,
        consumer_cancel_token: CancellationToken::new(),
        pipeline_cancel_token: CancellationToken::new(),
        channel_sender: None,
        in_flight: None,
        drain_in_flight: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        aggregate_split: None,
        agg_service: None,
        compiled: route_runtime_state::CompiledRoute {
            security_policy: None,
            security_authenticator: None,
        },
    };

    controller.routes.insert("lifecycle-route".into(), managed);

    // swap_pipeline rejects lifecycle-bearing routes
    let reject = controller.swap_pipeline("lifecycle-route", BoxProcessor::new(IdentityProcessor));
    assert!(
        reject.is_err(),
        "swap_pipeline should reject lifecycle route"
    );
    assert!(
        reject
            .unwrap_err()
            .to_string()
            .contains("lifecycle-bearing"),
        "expected lifecycle-bearing rejection"
    );

    // swap_pipeline_raw bypasses the check
    let raw_result = controller.swap_pipeline_raw(
        "lifecycle-route",
        BoxProcessor::new(IdentityProcessor),
        vec![Arc::new(FakeStep) as Arc<dyn StepLifecycle>],
    );
    assert!(
        raw_result.is_ok(),
        "swap_pipeline_raw should accept lifecycle route, got: {:?}",
        raw_result
    );

    // Verify pipeline was actually swapped
    let swapped = controller.get_pipeline("lifecycle-route");
    assert!(swapped.is_some(), "pipeline should exist after raw swap");
}

#[tokio::test]
async fn swap_pipeline_allows_stateless_route() {
    let mut controller = build_controller();

    controller
        .add_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("stateless"))
        .await
        .unwrap();

    let result = controller.swap_pipeline("stateless", BoxProcessor::new(IdentityProcessor));
    assert!(result.is_ok());
    assert!(controller.get_pipeline("stateless").is_some());
}

// ── Hot-reload lifecycle preservation tests (Task 1b-pre) ──

/// Verify that after a raw pipeline swap with lifecycle handles, the new
/// `PipelineAssembly` stores them — so that subsequent route stop drains them.
#[tokio::test]
async fn hot_reload_preserves_lifecycle_handles_in_pipeline_assembly() {
    let mut controller = build_controller_with_components();

    controller
        .add_route(RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("life"))
        .await
        .unwrap();

    let lifecycle: Vec<Arc<dyn StepLifecycle>> = vec![Arc::new(FakeStep)];

    // Simulate Restart path: raw swap with lifecycle.
    controller
        .swap_pipeline_raw(
            "life",
            BoxProcessor::new(IdentityProcessor),
            lifecycle.clone(),
        )
        .expect("swap_pipeline_raw with lifecycle should succeed");

    // Verify new PipelineAssembly carries the lifecycle handles.
    let managed = controller.routes.get("life").expect("route should exist");
    let assembly = managed.pipeline.load();
    assert_eq!(
        assembly.lifecycle.len(),
        1,
        "new pipeline assembly should have 1 lifecycle handle"
    );
    // drop the load guard so the ArcSwap can be updated later
    drop(assembly);
}

/// Verify that hot-reload with empty lifecycle still succeeds
/// (the atomic swap path for stateless routes).
#[tokio::test]
async fn hot_reload_with_empty_lifecycle_still_works() {
    let mut controller = build_controller_with_components();

    controller
        .add_route(
            RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("stateless-hr"),
        )
        .await
        .unwrap();

    // Swap with empty lifecycle (simulates atomic swap for stateless route).
    controller
        .swap_pipeline_raw("stateless-hr", BoxProcessor::new(IdentityProcessor), vec![])
        .expect("swap_pipeline_raw with empty lifecycle should succeed");

    assert!(
        controller.get_pipeline("stateless-hr").is_some(),
        "pipeline should exist after swap with empty lifecycle"
    );

    // Verify assembly has no lifecycle handles.
    let managed = controller
        .routes
        .get("stateless-hr")
        .expect("route should exist");
    let assembly = managed.pipeline.load();
    assert!(
        assembly.lifecycle.is_empty(),
        "pipeline assembly should have empty lifecycle"
    );
}

#[tokio::test]
async fn resequencer_compile_route_returns_ack_and_posts_to_continuation() {
    use camel_api::body::Body;
    use camel_api::exchange::ExchangePattern;
    use camel_processor::LogLevel;
    use camel_processor::resequencer::CAMEL_RESEQUENCER_ACCEPTED;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tower::Service;

    let mut controller = build_controller_with_components();
    register_simple_language(&mut controller);

    // Capture body text from the post-continuation call
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // The recording sender (cloned into every call) so we can capture the
    // exchange body AFTER the resequencer actor + post-driver process it.
    struct RecordingPostProcessor {
        tx: tokio::sync::mpsc::UnboundedSender<String>,
    }
    impl Clone for RecordingPostProcessor {
        fn clone(&self) -> Self {
            Self {
                tx: self.tx.clone(),
            }
        }
    }
    impl Service<Exchange> for RecordingPostProcessor {
        type Response = Exchange;
        type Error = CamelError;
        type Future =
            Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, exchange: Exchange) -> Self::Future {
            let body_text = exchange
                .input
                .body
                .as_text()
                .unwrap_or("<non-text>")
                .to_string();
            // Fire-and-forget: drop send errors on channel close (test teardown)
            let _ = self.tx.send(body_text);
            Box::pin(async move { Ok(exchange) })
        }
    }

    // Route: pre-step (Log) → Resequence → post-step (RecordingPostProcessor)
    let route = RouteDefinition::new(
        "mock:in",
        vec![
            BuilderStep::Log {
                level: LogLevel::Info,
                message: "pre-resequence".into(),
            },
            BuilderStep::Resequence {
                policy_config: camel_api::ResequencePolicyConfig {
                    mode: camel_api::ResequenceMode::Batch {
                        correlation: "${header.id}".into(),
                        sort: "${header.id}".into(),
                        completion: camel_api::BatchCompletion::Size(1),
                    },
                },
            },
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(
                tower::util::BoxCloneService::new(RecordingPostProcessor { tx }),
            ))),
        ],
    )
    .with_route_id("reseq-compile");

    let compiled = controller
        .compile_route_definition_pipeline(route, 1)
        .expect("resequencer route should compile");

    // Build an InOnly exchange with a text body and a header.id for correlation
    let mut input =
        camel_api::Exchange::new(camel_api::Message::new(Body::Text("input-body".into())));
    input.input.set_header("id", "test-1");

    let mut pipeline = compiled.processor.clone();
    pipeline.ready().await.expect("pipeline should be ready");
    let result = pipeline
        .call(input)
        .await
        .expect("pipeline call should succeed");

    // The ack exchange body must be Empty (the resequencer returns an ack, not the input)
    assert!(
        matches!(result.input.body, Body::Empty),
        "ack body should be Empty, got {:?}",
        result.input.body
    );

    // CAMEL_RESEQUENCER_ACCEPTED must be true
    assert_eq!(
        result
            .property(CAMEL_RESEQUENCER_ACCEPTED)
            .and_then(|v| v.as_bool()),
        Some(true),
        "ack should have CAMEL_RESEQUENCER_ACCEPTED=true"
    );

    // The exchange pattern should be InOnly (unchanged by resequencer)
    assert_eq!(
        result.pattern,
        ExchangePattern::InOnly,
        "ack exchange pattern should remain InOnly"
    );

    // ── Verify the post-continuation received the payload ──
    let captured_body = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("post-continuation did not receive exchange within 500ms timeout")
        .expect("capture channel closed without receiving exchange");
    assert_eq!(
        captured_body, "input-body",
        "post-continuation should receive the original input body via PassthroughPolicy"
    );

    // Drain lifecycle to clean up
    for lc in &compiled.lifecycle {
        lc.shutdown(camel_api::StepShutdownReason::RouteStop)
            .await
            .expect("lifecycle shutdown should succeed");
    }
}

#[tokio::test]
async fn resequencer_batch_e2e_sort_and_emit() {
    use camel_api::body::Body;
    use camel_api::exchange::ExchangePattern;
    use camel_processor::resequencer::CAMEL_RESEQUENCER_ACCEPTED;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tower::Service;

    let mut controller = build_controller_with_components();
    register_simple_language(&mut controller);

    // Capture channel for post-continuation exchanges
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Exchange>();

    /// Records received exchanges and their sequence numbers.
    struct RecordingPost {
        tx: tokio::sync::mpsc::UnboundedSender<Exchange>,
    }
    impl Clone for RecordingPost {
        fn clone(&self) -> Self {
            Self {
                tx: self.tx.clone(),
            }
        }
    }
    impl Service<Exchange> for RecordingPost {
        type Response = Exchange;
        type Error = CamelError;
        type Future =
            Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;
        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, exchange: Exchange) -> Self::Future {
            let tx = self.tx.clone();
            Box::pin(async move {
                let _ = tx.send(exchange.clone());
                Ok(exchange)
            })
        }
    }

    // Route: Resequence(batch: Size(3), sort=header.seq) → RecordingPost
    let route = RouteDefinition::new(
        "mock:in",
        vec![
            BuilderStep::Resequence {
                policy_config: camel_api::ResequencePolicyConfig {
                    mode: camel_api::ResequenceMode::Batch {
                        correlation: "${header.id}".into(),
                        sort: "${header.seq}".into(),
                        completion: camel_api::BatchCompletion::Size(3),
                    },
                },
            },
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(
                tower::util::BoxCloneService::new(RecordingPost { tx }),
            ))),
        ],
    )
    .with_route_id("batch-e2e");

    let compiled = controller
        .compile_route_definition_pipeline(route, 1)
        .expect("batch route should compile");

    let mut pipeline = compiled.processor.clone();

    // Send 3 exchanges with out-of-order seq headers: 3, 1, 2
    let seqs = [3, 1, 2];
    for &seq in &seqs {
        let mut ex = Exchange::new(camel_api::Message::new(Body::Text(format!("msg-{seq}"))));
        ex.input.set_header("id", "group-1");
        ex.input.set_header("seq", seq.to_string());
        ex.pattern = ExchangePattern::InOnly;
        pipeline.ready().await.expect("pipeline should be ready");
        let ack = pipeline
            .call(ex)
            .await
            .expect("pipeline call should succeed");
        assert_eq!(
            ack.property(CAMEL_RESEQUENCER_ACCEPTED)
                .and_then(|v| v.as_bool()),
            Some(true),
            "ack should have CAMEL_RESEQUENCER_ACCEPTED=true"
        );
    }

    // Collect post-continuation exchanges (should receive 3 sorted: [msg-1, msg-2, msg-3])
    let mut received = Vec::new();
    for _ in 0..3 {
        let captured = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .expect("post-continuation did not receive exchange within timeout")
            .expect("capture channel closed without receiving exchange");
        received.push(captured);
    }

    assert_eq!(
        received.len(),
        3,
        "should receive exactly 3 exchanges in post-continuation"
    );

    // Verify sorted order by body text
    let bodies: Vec<String> = received
        .iter()
        .map(|ex| ex.input.body.as_text().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        bodies,
        vec!["msg-1", "msg-2", "msg-3"],
        "post-continuation should receive exchanges sorted by seq header"
    );

    // Drain lifecycle
    for lc in &compiled.lifecycle {
        lc.shutdown(camel_api::StepShutdownReason::RouteStop)
            .await
            .expect("lifecycle shutdown should succeed");
    }
}

#[tokio::test]
async fn resequencer_hot_swap_drains_old_service() {
    use camel_api::body::Body;
    use camel_api::exchange::ExchangePattern;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tower::Service;

    let mut controller = build_controller_with_components();
    register_simple_language(&mut controller);

    /// A post-processor that records every exchange body text.
    #[derive(Clone)]
    struct DrainRecorder {
        tx: tokio::sync::mpsc::UnboundedSender<String>,
    }
    impl Service<Exchange> for DrainRecorder {
        type Response = Exchange;
        type Error = CamelError;
        type Future =
            Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;
        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, exchange: Exchange) -> Self::Future {
            let tx = self.tx.clone();
            Box::pin(async move {
                let body = exchange
                    .input
                    .body
                    .as_text()
                    .unwrap_or("<no-body>")
                    .to_string();
                let _ = tx.send(body);
                Ok(exchange)
            })
        }
    }

    // Step 1: Register initial route with resequencer via add_route
    let (old_tx, mut old_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let initial_route = RouteDefinition::new(
        "mock:in",
        vec![
            BuilderStep::Resequence {
                policy_config: camel_api::ResequencePolicyConfig {
                    mode: camel_api::ResequenceMode::Batch {
                        correlation: "${header.id}".into(),
                        sort: "${header.seq}".into(),
                        completion: camel_api::BatchCompletion::Size(1),
                    },
                },
            },
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(
                tower::util::BoxCloneService::new(DrainRecorder { tx: old_tx }),
            ))),
        ],
    )
    .with_route_id("hr-drain");

    controller
        .add_route(initial_route)
        .await
        .expect("add initial route should succeed");

    // Step 2: Compile a NEW pipeline (simulating hot-reload)
    let (new_tx, mut new_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let new_route = RouteDefinition::new(
        "mock:in",
        vec![
            BuilderStep::Resequence {
                policy_config: camel_api::ResequencePolicyConfig {
                    mode: camel_api::ResequenceMode::Batch {
                        correlation: "${header.id}".into(),
                        sort: "${header.seq}".into(),
                        completion: camel_api::BatchCompletion::Size(1),
                    },
                },
            },
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(
                tower::util::BoxCloneService::new(DrainRecorder { tx: new_tx }),
            ))),
        ],
    )
    .with_route_id("hr-drain");

    let new_compiled = controller
        .compile_route_definition_pipeline(new_route, 2)
        .expect("new route should compile");

    // Step 3: Hot-swap — swap_pipeline_raw drains old lifecycles internally
    // (matching ADR-0004 hot-reload Restart path)
    controller
        .swap_pipeline_raw(
            "hr-drain",
            new_compiled.processor.clone(),
            new_compiled.lifecycle.clone(),
        )
        .expect("hot-swap raw with lifecycle should succeed");

    // Verify the old continuation channel is quiet (drain completed, no new messages)
    // The old ResequencerService's drain flushed any buffered exchange through the old
    // post-continuation. Since we sent 0 exchanges, old_rx should be empty.
    match tokio::time::timeout(std::time::Duration::from_millis(100), old_rx.recv()).await {
        Ok(Some(msg)) => {
            // One message may arrive from the old drain (if any buffered)
            // That's fine — the drain DID flush through the OLD continuation.
            // Just verify no SECOND message arrives.
            if let Ok(Some(m2)) =
                tokio::time::timeout(std::time::Duration::from_millis(100), old_rx.recv()).await
            {
                panic!("old continuation received unexpected second exchange after drain: {m2}")
            }
            drop(msg);
        }
        Ok(None) => {} // channel closed — old drain completed and released the sender
        Err(_) => {}   // timeout — expected, no messages on old channel
    }

    // Step 4: Verify the NEW pipeline works after hot-swap
    {
        let swapped = controller
            .get_pipeline("hr-drain")
            .expect("pipeline should exist");
        let mut pipeline = swapped.clone();
        let mut ex = Exchange::new(camel_api::Message::new(Body::Text("new-msg".into())));
        ex.input.set_header("id", "g1");
        ex.input.set_header("seq", "2");
        ex.pattern = ExchangePattern::InOnly;
        pipeline.ready().await.expect("ready");
        pipeline.call(ex).await.expect("new pipeline call");
    }

    let new_body = tokio::time::timeout(std::time::Duration::from_millis(1000), new_rx.recv())
        .await
        .expect("new continuation should receive within timeout")
        .expect("new channel closed");
    assert_eq!(
        new_body, "new-msg",
        "new continuation should receive 'new-msg' after hot-swap"
    );

    // Drain new lifecycles
    for lc in &new_compiled.lifecycle {
        lc.shutdown(camel_api::StepShutdownReason::RouteStop)
            .await
            .expect("new lifecycle shutdown should succeed");
    }
}

#[tokio::test]
async fn insert_prepared_route_failure_drains_staging() {
    // Regression: F2 staging map must be drained when the caller handles
    // insert_prepared_route failure via discard_prepared_staging. Otherwise
    // the ManagedRoute (with CancellationToken + SharedPipeline) leaks as
    // orphan task. This test mirrors the contract reload_actions.rs relies on.
    let mut controller = build_controller_with_components();

    let route_id = "route-leak-test";

    // Prepare a route (stages the ManagedRoute, returns thin token).
    let prepared = controller
        .prepare_route_definition_with_generation(
            RouteDefinition::new("timer:tick?period=100", vec![BuilderStep::Stop])
                .with_route_id(route_id),
            1,
        )
        .expect("prepare must succeed (staging now has 1 entry)");

    // Force insert to fail by pre-inserting the route via the staging-bypass
    // path (add_route_with_generation uses build_managed_route directly
    // without staging — use it to plant a route with the same id).
    controller
        .add_route_with_generation(
            RouteDefinition::new("timer:tick?period=100", vec![BuilderStep::Stop])
                .with_route_id(route_id),
            1,
        )
        .await
        .expect("seed route must install");

    // insert_prepared_route must fail (route exists). On failure, the
    // staging entry is RESTORED (not silently dropped) so the caller can
    // retry or drain explicitly.
    let err = controller
        .insert_prepared_route(prepared)
        .expect_err("insert must fail (duplicate route_id)");
    assert!(err.to_string().contains("already exists"));

    // Pre-drain assertion: staging still holds the entry (caller contract).
    assert!(
        !controller.prepared_staging_is_empty(),
        "staging must be restored on insert failure (caller decides drain)"
    );

    // Caller explicitly drains. Safe because build_managed_route initializes
    // handles to None (no spawned tasks at prepare-time); without this drain,
    // the staged SharedPipeline would accumulate across reload iterations.
    controller.discard_prepared_staging(route_id);

    // Post-drain assertion: staging is empty (no leak).
    assert!(
        controller.prepared_staging_is_empty(),
        "staging must be drained after discard_prepared_staging (F2 regression)"
    );
}

#[tokio::test]
async fn prepare_twice_same_route_id_does_not_overwrite_staging() {
    // Regression: F2 staging must reject double-prepare of the same route_id.
    // Without the guard, the second prepare would overwrite the first staged
    // ManagedRoute, orphaning its CancellationToken + SharedPipeline.
    let mut controller = build_controller_with_components();

    let route_id = "route-double-prepare";

    let prepared1 = controller
        .prepare_route_definition_with_generation(
            RouteDefinition::new("timer:tick?period=200", vec![BuilderStep::Stop])
                .with_route_id(route_id),
            1,
        )
        .expect("first prepare must succeed");

    // Second prepare of the same id must fail with RouteError (staging collision).
    let err = controller
        .prepare_route_definition_with_generation(
            RouteDefinition::new("timer:tick?period=200", vec![BuilderStep::Stop])
                .with_route_id(route_id),
            1,
        )
        .expect_err("second prepare must fail (staging collision)");
    assert!(err.to_string().contains("staging"));

    // Drain via the consumer side (insert succeeds since route not in routes yet).
    controller
        .insert_prepared_route(prepared1)
        .expect("insert of first-prepared must succeed");
    assert!(
        controller.prepared_staging_is_empty(),
        "staging must be drained after successful insert"
    );
}
