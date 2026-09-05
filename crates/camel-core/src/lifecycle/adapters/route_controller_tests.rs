use super::*;
use crate::lifecycle::adapters::pipeline_runtime::PipelineAssembly;
use crate::lifecycle::adapters::route_helpers::runtime_failure_command;
use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinition};
use crate::shared::components::domain::Registry;
use arc_swap::ArcSwap;
use camel_api::SpanKindHint;
use camel_api::function::PrepareToken;
use camel_api::security_policy::{
    AuthContext, AuthorizationDecision, CredentialSource, Principal, SecurityPolicy,
    SecurityPolicyConfig,
};
use camel_api::{
    BoxProcessor, BoxProcessorExt, ExchangePatch, FunctionDefinition, FunctionDiff, FunctionId,
    FunctionInvocationError, FunctionInvoker, FunctionInvokerSync, IdentityProcessor, Message,
    OpaqueProcessor, RuntimeCommand, StepLifecycle, StepShutdownReason, SyncBoxProcessor, Value,
    ValueSourceDef,
};
use camel_auth::TokenAuthenticator;
use camel_component_api::{
    Component, ComponentContext, ConcurrencyModel, ConsumerStartupMode, Endpoint, ProducerContext,
    RuntimeObservability, SecurityContext,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Serializes tests that touch the global `START_ROUTE_EVENT_HOOK`. The hook
/// is a single process-wide slot, so concurrent hook-using tests would
/// clobber each other's recorders; taking this guard makes them run one at a
/// time while leaving the rest of the suite parallel.
static START_ROUTE_HOOK_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
            provider_registry: None,
            security_plan: None,
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
    assert!(dup_err.to_string().contains("duplicate"));
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

/// rc-082j regression: the global start-route event hook is route-scoped —
/// a parallel test starting its own routes must not land events in another
/// test's recorder (the hook is a single process-wide slot).
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn start_route_hook_ignores_foreign_routes() {
    let _hook_guard = START_ROUTE_HOOK_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let events: Arc<std::sync::Mutex<Vec<&'static str>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    set_start_route_event_hook(Some({
        let events = Arc::clone(&events);
        Arc::new(move |event, route_id| {
            if route_id == "hook-owner" {
                events.lock().expect("events").push(event);
            }
        })
    }));

    // Foreign route start — simulates ANY parallel test starting a route
    // while the hook is installed. None of it may reach the recorder.
    let mut controller = build_controller_with_components();
    let route = RouteDefinition::new(
        "timer:tick?period=10&repeatCount=1",
        vec![BuilderStep::To("mock:out".into())],
    )
    .with_route_id("foreign-rt");
    controller.add_route(route).await.unwrap();
    controller.start_route("foreign-rt").await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    controller.stop_route("foreign-rt").await.unwrap();

    set_start_route_event_hook(None);
    let events = events.lock().expect("events").clone();
    assert!(
        events.is_empty(),
        "foreign route events leaked into hook-owner recorder: {events:?}"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn start_route_spawns_pipeline_before_consumer_for_eager_consumers() {
    let _hook_guard = START_ROUTE_HOOK_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    set_start_route_event_hook(Some({
        let events = Arc::clone(&events);
        Arc::new(move |event, route_id| {
            if route_id == "startup-order" {
                events.lock().expect("events lock").push(event);
            }
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
    // rc-jxkj: fresh controller → cohort gate closed; this test exercises
    // backpressure, not the barrier, so open the gate for dispatch.
    controller.cohort.open();

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
    assert!(dup.to_string().contains("duplicate"));

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
    assert!(err.to_string().contains("duplicate"));
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
    // rc-jxkj: fresh controller → cohort gate closed; open for dispatch.
    controller.cohort.open();

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
async fn direct_entry_aggregate_delivers_single_aggregated_reply() {
    let mock = Arc::new(camel_component_mock::MockComponent::new());
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(camel_component_timer::TimerComponent::new()));
        guard.register(Arc::new(camel_component_direct::DirectComponent::new()));
        guard.register(Arc::clone(&mock) as Arc<dyn camel_component_api::Component>);
    }
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    // Timeout in the completion policy materializes the pre/agg/post split;
    // natural completion at size 5 delivers without waiting the 2s ceiling.
    let agg_config = camel_api::AggregatorConfig::correlate_by("key")
        .complete_on_size_or_timeout(5, Duration::from_secs(2))
        .build()
        .unwrap();

    let agg_route = RouteDefinition::new(
        "direct:agg-in",
        vec![
            BuilderStep::Aggregate { config: agg_config },
            BuilderStep::To("mock:sink".into()),
        ],
    )
    .with_route_id("agg-split");
    controller.add_route(agg_route).await.unwrap();
    controller.start_route("agg-split").await.unwrap();

    // Producing route: one timer tick, split into 5 distinct fragments
    // sharing the correlation header, each dispatched via direct:agg-in.
    let fragment_bodies: Vec<String> = (0..5).map(|i| format!("frag-{i}")).collect();
    let split_config =
        camel_api::splitter::SplitterConfig::new(Arc::new(move |exchange: &Exchange| {
            Ok(fragment_bodies
                .iter()
                .map(|body| {
                    camel_api::splitter::fragment_exchange(
                        exchange,
                        camel_api::Body::Text(body.clone()),
                    )
                })
                .collect())
        }));
    let producer_route = RouteDefinition::new(
        "timer:drive?period=10&repeatCount=1",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: camel_api::ValueSourceDef::Literal(camel_api::Value::String(
                    "order-1".into(),
                )),
            },
            BuilderStep::Split {
                config: split_config,
                steps: vec![BuilderStep::To("direct:agg-in".into())],
            },
        ],
    )
    .with_route_id("agg-producer");
    controller.add_route(producer_route).await.unwrap();
    controller.start_route("agg-producer").await.unwrap();
    // rc-jxkj: fresh controller → cohort gate closed; open for dispatch.
    controller.cohort.open();

    let sink = mock.get_endpoint("sink").expect("mock sink endpoint");
    sink.await_exchanges(1, Duration::from_millis(500)).await;
    let received = sink.get_received_exchanges().await;
    assert_eq!(
        received.len(),
        1,
        "expected exactly 1 aggregated reply, got {}",
        received.len()
    );
    let aggregated = match &received[0].input.body {
        camel_api::Body::Json(serde_json::Value::Array(items)) => items.clone(),
        other => panic!("expected aggregated JSON array body, got {other:?}"),
    };
    assert_eq!(
        aggregated.len(),
        5,
        "aggregated reply should carry all 5 fragments: {aggregated:?}"
    );
    for i in 0..5 {
        let expected = serde_json::json!(format!("frag-{i}"));
        assert!(
            aggregated.contains(&expected),
            "aggregated reply missing body of frag-{i}: {aggregated:?}"
        );
    }
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

/// Build a 4-step pass-through pipeline the way production routes are
/// composed (`compose_pipeline` over identity `Process` steps).
///
/// Note: the change plan suggested composing `IdentityProcessor` layers via
/// `tower::ServiceBuilder`, but tower 0.5's `ServiceBuilder::service` is a
/// terminal call — chaining `.service(a).service(b)` does not compile — so
/// the production composition path is used instead.
fn four_layer_identity_pipeline() -> camel_api::BoxProcessor {
    use crate::lifecycle::adapters::route_compiler::{PipelineRuntimeCtx, compose_pipeline};
    use crate::lifecycle::adapters::step_compilers::CompiledStep;

    let identity_step = || CompiledStep::Process {
        kind_hint: SpanKindHint::Internal,
        processor: camel_api::BoxProcessor::new(camel_api::IdentityProcessor),
        body_contract: None,
        lifecycle: None,
        label: None,
    };
    compose_pipeline(
        vec![
            identity_step(),
            identity_step(),
            identity_step(),
            identity_step(),
        ],
        PipelineRuntimeCtx::compile_time(),
    )
}

/// Per-clone acquisition latency under 64-way contention must stay far
/// below the ceiling a serializing mutex would inflate it to.
///
/// The retired `Mutex<BoxProcessor>` wrapper held the lock only for the
/// µs-scale clone, so the discriminating signal is per-clone latency (a
/// 64-way futex convoy inflates exactly that), not hold-under-lock sleep
/// work — hence the tight loop with no sleeps.
///
/// Multi-thread runtime required: the clone loop has no `.await`, so a
/// current-thread runtime would run each task to completion before polling
/// the next — zero contention, and the convoy would go undetected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lockfree_pipeline_acquisition_per_clone_latency_ceiling() {
    use crate::lifecycle::adapters::pipeline_runtime::PipelineAssembly;
    use arc_swap::ArcSwap;
    use camel_api::SyncBoxProcessor;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let assembly = PipelineAssembly::new(
        SyncBoxProcessor::new(four_layer_identity_pipeline()),
        vec![],
    );
    let shared: Arc<ArcSwap<PipelineAssembly>> = Arc::new(ArcSwap::from_pointee(assembly));

    const TASKS: usize = 64;
    const ITERS: usize = 200;

    let mut handles = Vec::with_capacity(TASKS);
    for _ in 0..TASKS {
        let shared = shared.clone();
        handles.push(tokio::spawn(async move {
            let mut durations = Vec::with_capacity(ITERS);
            for _ in 0..ITERS {
                let start = Instant::now();
                let _cloned = shared.load().processor.clone_inner();
                durations.push(start.elapsed());
            }
            durations
        }));
    }

    let mut all = Vec::with_capacity(TASKS * ITERS);
    for h in handles {
        all.extend(h.await.expect("clone task must not panic"));
    }
    assert_eq!(all.len(), TASKS * ITERS);
    all.sort_unstable();

    // Lock-free clone_box stays in the µs range (1000x margin); the retired
    // mutex convoy blew per-clone latency well past this ceiling.
    let p99 = all[(all.len() * 99) / 100];
    assert!(
        p99 < Duration::from_millis(10),
        "p99 clone_inner latency {p99:?} >= 10 ms — mutex convoy suspected"
    );
}

/// Hot-reload coherence (ADR-0004): concurrent acquisition while a writer
/// stores new snapshots must yield only coherent pipelines that process
/// exchanges with their bodies intact.
///
/// Multi-thread runtime required so acquisitions genuinely interleave with
/// the writer's swaps instead of all completing before the first store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_swap_during_concurrent_acquisition_is_coherent() {
    use crate::lifecycle::adapters::pipeline_runtime::PipelineAssembly;
    use crate::lifecycle::adapters::route_compiler::{PipelineRuntimeCtx, compose_pipeline};
    use crate::lifecycle::adapters::step_compilers::CompiledStep;
    use arc_swap::ArcSwap;
    use camel_api::{
        Body, BoxProcessor, BoxProcessorExt, Exchange, Message, SyncBoxProcessor, Value,
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    let assembly = PipelineAssembly::new(
        SyncBoxProcessor::new(four_layer_identity_pipeline()),
        vec![],
    );
    let shared: Arc<ArcSwap<PipelineAssembly>> = Arc::new(ArcSwap::from_pointee(assembly));

    const ACQUIRERS: usize = 16;
    const PER_ACQUIRER: usize = 20;
    const SWAPS: u32 = 20;

    let mut acquirers = Vec::with_capacity(ACQUIRERS);
    for t in 0..ACQUIRERS {
        let shared = shared.clone();
        acquirers.push(tokio::spawn(async move {
            for i in 0..PER_ACQUIRER {
                let payload = format!("body-{t}-{i}");
                let cloned = shared.load().processor.clone_inner();
                let out = cloned
                    .oneshot(Exchange::new(Message::new(Body::Text(payload.clone()))))
                    .await
                    .expect("cloned pipeline must process the exchange");
                assert!(
                    matches!(&out.input.body, Body::Text(s) if *s == payload),
                    "exchange body must pass through the swapped pipeline intact"
                );
                // Coherence: acquisitions that landed on a swapped snapshot
                // must see one of the writer's generation markers. The
                // initial identity stack carries no marker, so absence is
                // valid; a torn snapshot would surface as a mangled or
                // out-of-range value.
                if let Some(Value::String(g)) = out.input.header("gen") {
                    let generation: u32 =
                        g.parse().expect("gen marker must be a numeric generation");
                    assert!(
                        generation < SWAPS,
                        "gen marker {generation} out of range — torn snapshot"
                    );
                }
            }
        }));
    }

    // Writer: store SWAPS new snapshots, each a distinct composed stack —
    // a marker step tagging its generation plus identity steps.
    let writer = {
        let shared = shared.clone();
        tokio::spawn(async move {
            for generation in 0..SWAPS {
                let marker = BoxProcessor::from_fn(move |mut ex: Exchange| {
                    ex.input
                        .set_header("gen", Value::String(generation.to_string()));
                    async move { Ok(ex) }
                });
                let identity_step = || CompiledStep::Process {
                    kind_hint: SpanKindHint::Internal,
                    processor: camel_api::BoxProcessor::new(camel_api::IdentityProcessor),
                    body_contract: None,
                    lifecycle: None,
                    label: None,
                };
                let stack = compose_pipeline(
                    vec![
                        CompiledStep::Process {
                            kind_hint: SpanKindHint::Internal,
                            processor: marker,
                            body_contract: None,
                            lifecycle: None,
                            label: None,
                        },
                        identity_step(),
                        identity_step(),
                        identity_step(),
                    ],
                    PipelineRuntimeCtx::compile_time(),
                );
                shared.store(Arc::new(PipelineAssembly::new(
                    SyncBoxProcessor::new(stack),
                    vec![],
                )));
            }
        })
    };

    writer.await.expect("writer task must not panic");
    for h in acquirers {
        h.await.expect("acquirer task must not panic");
    }
}

/// Clone-cost tripwire: mean clone_inner cost on a representative 4-step
/// stack must stay well under the ceiling, catching accidental deep-copy
/// reintroduction.
#[tokio::test]
async fn multi_step_pipeline_clone_cost_tripwire() {
    use camel_api::SyncBoxProcessor;
    use std::time::{Duration, Instant};

    let sync = SyncBoxProcessor::new(four_layer_identity_pipeline());

    // 3 runs; take the MINIMUM run mean so a single OS stall (preemption
    // during one run) cannot flake the tripwire.
    const RUNS: usize = 3;
    const CLONES: usize = 1000;
    let mut means = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let start = Instant::now();
        for _ in 0..CLONES {
            let _cloned = sync.clone_inner();
        }
        means.push(start.elapsed() / CLONES as u32);
    }
    let min_mean = means.into_iter().min().expect("at least one run");
    assert!(
        min_mean < Duration::from_micros(50),
        "mean clone_inner cost {min_mean:?} >= 50 µs — deep-copy suspected"
    );
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
    // rc-jxkj: fresh controller → cohort gate closed; open for dispatch.
    controller.cohort.open();

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

/// rc-z5qz regression: an OPEN cohort gate must win over a concurrently
/// cancelled pipeline token in the forward loop's inner gate select.
/// Unbiased, tokio picks randomly among ready branches and the cancel arm
/// drops a deliverable envelope; force_complete_all then finds no buckets
/// and the pending exchange is lost.
///
/// Determinism: the timer (`delay=0`) fires tick #1 immediately, then parks
/// on the 60 s tick toward repeatCount=2 — the consumer-exit monitor never
/// fires, so the only cancel source is this test. After tick #1 the forward
/// loop parks on the CLOSED gate inside the inner select. The
/// current-thread test runtime runs no other task between the two sync
/// toggles below, so the parked loop resumes with BOTH branches ready —
/// exactly the both-ready interleaving that used to coin-flip. An unbiased
/// select survives one iteration with p=0.5, so 24 fresh route/controller
/// iterations all pass only with p=0.5^24 (≈6e-8): an unbiased select
/// cannot pass this test.
#[tokio::test]
async fn aggregate_open_gate_beats_concurrent_cancel_delivers_pending_bucket() {
    const ITERATIONS: usize = 24;
    for i in 0..ITERATIONS {
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

        let key_val = format!("order-{i}");
        let route = RouteDefinition::new(
            "timer:gate-race?period=60000&delay=0&repeatCount=2",
            vec![
                BuilderStep::DeclarativeSetHeader {
                    key: "key".into(),
                    value: camel_api::ValueSourceDef::Literal(camel_api::Value::String(
                        key_val.clone(),
                    )),
                },
                BuilderStep::Aggregate { config: agg_config },
                BuilderStep::To("mock:gate-race-sink".into()),
            ],
        )
        .with_route_id("gate-race-agg");
        controller.add_route(route).await.unwrap();
        controller.start_route("gate-race-agg").await.unwrap();

        // Let tick #1 land: the forward loop dequeues the envelope and
        // parks on the closed cohort gate inside the inner select.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Both-ready interleaving, back-to-back with no await between:
        // gate branch ready, cancel branch ready.
        controller.cohort.open();
        controller
            .routes
            .get("gate-race-agg")
            .expect("route must exist after start")
            .pipeline_cancel_token
            .cancel();

        let sink = mock
            .get_endpoint("gate-race-sink")
            .expect("mock sink endpoint");
        sink.await_exchanges(1, Duration::from_secs(2)).await;
        let received = sink.get_received_exchanges().await;
        assert_eq!(
            received.len(),
            1,
            "iteration {i}: open gate must deliver the pending bucket, got {}",
            received.len()
        );
        assert_eq!(
            received[0].property("CamelAggregatedCompletionReason"),
            Some(&serde_json::json!("stop"))
        );
    }
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
            provider_registry: None,
            security_plan: None,
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
            provider_registry: None,
            security_plan: None,
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
            provider_registry: None,
            security_plan: None,
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
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(RecordingPostProcessor {
                tx,
            }))),
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
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(RecordingPost { tx }))),
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
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(DrainRecorder {
                tx: old_tx,
            }))),
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
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(DrainRecorder {
                tx: new_tx,
            }))),
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
    assert!(err.to_string().contains("duplicate"));

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

// ── StepLifecycle start()/rollback tests (Task 3.3) ──

/// Records every `start()` by pushing its label into a shared ordered log.
/// Used to assert in-order start before the pipeline task spawns.
#[derive(Debug)]
struct RecordingStartLifecycle {
    label: &'static str,
    log: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

#[async_trait::async_trait]
impl StepLifecycle for RecordingStartLifecycle {
    fn name(&self) -> &'static str {
        self.label
    }
    async fn start(&self) -> Result<(), CamelError> {
        self.log.lock().expect("start log").push(self.label);
        Ok(())
    }
    async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
        Ok(())
    }
}

/// Records `shutdown()` reasons. `start()` uses the default Ok path.
#[derive(Debug)]
struct ShutdownSpy {
    shutdowns: Arc<std::sync::Mutex<Vec<StepShutdownReason>>>,
}

#[async_trait::async_trait]
impl StepLifecycle for ShutdownSpy {
    fn name(&self) -> &'static str {
        "shutdown-spy"
    }
    async fn shutdown(&self, reason: StepShutdownReason) -> Result<(), CamelError> {
        self.shutdowns.lock().expect("shutdowns").push(reason);
        Ok(())
    }
}

/// A step whose `start()` always returns `Err`.
#[derive(Debug)]
struct FailingStartStep;

#[async_trait::async_trait]
impl StepLifecycle for FailingStartStep {
    fn name(&self) -> &'static str {
        "failing-start"
    }
    async fn start(&self) -> Result<(), CamelError> {
        Err(CamelError::ProcessorError("start failed".into()))
    }
    async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
        Ok(())
    }
}

/// `start_route` awaits each step `start()` in route order, completing all of
/// them before the pipeline task is spawned.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn start_route_awaits_start_in_order() {
    let _hook_guard = START_ROUTE_HOOK_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let log: Arc<std::sync::Mutex<Vec<&'static str>>> = Arc::new(std::sync::Mutex::new(Vec::new()));

    // The event hook shares the same ordered log, so start() labels and spawn
    // events land on one timeline.
    set_start_route_event_hook(Some({
        let log = Arc::clone(&log);
        Arc::new(move |event, route_id| {
            if route_id == "start-order" {
                log.lock().expect("log").push(event);
            }
        })
    }));

    let mut controller = build_controller_with_components();
    controller
        .add_route(
            RouteDefinition::new("timer:tick?period=60000", vec![]).with_route_id("start-order"),
        )
        .await
        .unwrap();

    // Inject two lifecycle handles in route order via raw swap.
    let handles: Vec<Arc<dyn StepLifecycle>> = vec![
        Arc::new(RecordingStartLifecycle {
            label: "start-1",
            log: Arc::clone(&log),
        }),
        Arc::new(RecordingStartLifecycle {
            label: "start-2",
            log: Arc::clone(&log),
        }),
    ];
    controller
        .swap_pipeline_raw("start-order", BoxProcessor::new(IdentityProcessor), handles)
        .expect("swap_pipeline_raw");

    // Discard setup events (add_route auto-spawn) so only start_route's own
    // events are asserted below.
    log.lock().expect("log").clear();

    controller.start_route("start-order").await.unwrap();
    set_start_route_event_hook(None);
    controller.stop_route("start-order").await.unwrap();

    let log = log.lock().expect("log").clone();
    let i1 = log
        .iter()
        .position(|e| *e == "start-1")
        .expect("start-1 recorded");
    let i2 = log
        .iter()
        .position(|e| *e == "start-2")
        .expect("start-2 recorded");
    let ip = log
        .iter()
        .position(|e| *e == "pipeline_spawned")
        .expect("pipeline_spawned");

    assert!(i1 < i2, "start() must run in route order: {log:?}");
    assert!(
        i2 < ip,
        "start() must complete before pipeline spawn: {log:?}"
    );
}

/// On the Nth `start()` failure, already-started handles are shut down
/// (`RouteStop`, reverse order, best-effort); the pipeline is NOT spawned and
/// `start_route` returns the original start `Err`.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn start_route_rolls_back_on_failure() {
    let _hook_guard = START_ROUTE_HOOK_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let events: Arc<std::sync::Mutex<Vec<&'static str>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    set_start_route_event_hook(Some({
        let events = Arc::clone(&events);
        Arc::new(move |event, route_id| {
            if route_id == "rollback" {
                events.lock().expect("events").push(event);
            }
        })
    }));

    let shutdowns: Arc<std::sync::Mutex<Vec<StepShutdownReason>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    let mut controller = build_controller_with_components();
    controller
        .add_route(
            RouteDefinition::new("timer:tick?period=60000", vec![]).with_route_id("rollback"),
        )
        .await
        .unwrap();

    // handle 1 starts OK and records shutdown reasons; handle 2 fails to start.
    let handles: Vec<Arc<dyn StepLifecycle>> = vec![
        Arc::new(ShutdownSpy {
            shutdowns: Arc::clone(&shutdowns),
        }),
        Arc::new(FailingStartStep),
    ];
    controller
        .swap_pipeline_raw("rollback", BoxProcessor::new(IdentityProcessor), handles)
        .expect("swap_pipeline_raw");

    let result = controller.start_route("rollback").await;
    set_start_route_event_hook(None);

    assert!(result.is_err(), "expected start_route to fail");

    let shutdowns = shutdowns.lock().expect("shutdowns").clone();
    assert_eq!(
        shutdowns,
        vec![StepShutdownReason::RouteStop],
        "handle 1 must be rolled back with RouteStop: {shutdowns:?}"
    );

    let events = events.lock().expect("events").clone();
    assert!(
        !events.contains(&"pipeline_spawned"),
        "pipeline must not spawn on start failure: {events:?}"
    );

    let managed = controller.routes.get("rollback").expect("route exists");
    assert!(
        managed.pipeline_handle.is_none(),
        "no pipeline handle stored on failure"
    );
    assert!(
        managed.consumer_handle.is_none(),
        "no consumer handle stored on failure"
    );
}

/// A route whose handles use the default no-op `start` behaves identically to
/// before the change: `start_route` succeeds and spawns both tasks.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn start_route_default_start_noop_unaffected() {
    let _hook_guard = START_ROUTE_HOOK_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let events: Arc<std::sync::Mutex<Vec<&'static str>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    set_start_route_event_hook(Some({
        let events = Arc::clone(&events);
        Arc::new(move |event, route_id| {
            if route_id == "default-start" {
                events.lock().expect("events").push(event);
            }
        })
    }));

    let mut controller = build_controller_with_components();
    controller
        .add_route(
            RouteDefinition::new("timer:tick?period=60000", vec![]).with_route_id("default-start"),
        )
        .await
        .unwrap();

    // FakeStep does NOT override start() — it uses the trait default (Ok).
    let handles: Vec<Arc<dyn StepLifecycle>> = vec![Arc::new(FakeStep)];
    controller
        .swap_pipeline_raw(
            "default-start",
            BoxProcessor::new(IdentityProcessor),
            handles,
        )
        .expect("swap_pipeline_raw");

    controller.start_route("default-start").await.unwrap();
    set_start_route_event_hook(None);
    controller.stop_route("default-start").await.unwrap();

    let events = events.lock().expect("events").clone();
    assert!(
        events.contains(&"pipeline_spawned"),
        "pipeline must spawn for default-noop start: {events:?}"
    );
    assert!(
        events.contains(&"consumer_spawned"),
        "consumer must spawn for default-noop start: {events:?}"
    );
}

/// A post-start failure (here: `create_route_consumer` failing on an unknown
/// consumer scheme) must roll back the already-started handles in reverse
/// order (`RouteStop`, best-effort), must NOT spawn the pipeline, and
/// `start_route` must return `Err`. This locks the ADR-0022 SPI ("if
/// start_route returns Err, no started handle remains running") for the
/// post-start failure path — distinct from the mid-loop start-failure test
/// above (`start_route_rolls_back_on_failure`).
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn start_route_rolls_back_on_consumer_creation_failure() {
    let _hook_guard = START_ROUTE_HOOK_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let events: Arc<std::sync::Mutex<Vec<&'static str>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    set_start_route_event_hook(Some({
        let events = Arc::clone(&events);
        Arc::new(move |event, route_id| {
            if route_id == "post-start-rollback" {
                events.lock().expect("events").push(event);
            }
        })
    }));

    // start() record: proves the start loop completed (we are on the
    // POST-start failure path, not the mid-loop path).
    let start_log: Arc<std::sync::Mutex<Vec<&'static str>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    // shutdown() record: proves the rollback fired.
    let shutdowns: Arc<std::sync::Mutex<Vec<StepShutdownReason>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    let mut controller = build_controller_with_components();
    // Unknown consumer scheme: `add_route` succeeds (consumer is created
    // lazily in `start_route`), but `create_route_consumer` fails there.
    controller
        .add_route(
            RouteDefinition::new("missing:source", vec![]).with_route_id("post-start-rollback"),
        )
        .await
        .unwrap();

    // Two handles whose start() succeeds (default Ok for ShutdownSpy). The
    // RecordingStartLifecycle confirms start() ran; the ShutdownSpy captures
    // the rollback shutdown.
    let handles: Vec<Arc<dyn StepLifecycle>> = vec![
        Arc::new(RecordingStartLifecycle {
            label: "started-1",
            log: Arc::clone(&start_log),
        }),
        Arc::new(ShutdownSpy {
            shutdowns: Arc::clone(&shutdowns),
        }),
    ];
    controller
        .swap_pipeline_raw(
            "post-start-rollback",
            BoxProcessor::new(IdentityProcessor),
            handles,
        )
        .expect("swap_pipeline_raw");

    let result = controller.start_route("post-start-rollback").await;
    set_start_route_event_hook(None);

    assert!(
        result.is_err(),
        "start_route must fail when consumer creation fails"
    );

    // The start loop ran (post-start path), so start() was awaited.
    let start_log = start_log.lock().expect("start log").clone();
    assert!(
        start_log.contains(&"started-1"),
        "handle start() must have run (post-start path): {start_log:?}"
    );

    // Rollback fired on the started handles.
    let shutdowns = shutdowns.lock().expect("shutdowns").clone();
    assert_eq!(
        shutdowns,
        vec![StepShutdownReason::RouteStop],
        "started handle must be rolled back with RouteStop: {shutdowns:?}"
    );

    // Pipeline never spawned.
    let events = events.lock().expect("events").clone();
    assert!(
        !events.contains(&"pipeline_spawned"),
        "pipeline must not spawn on consumer creation failure: {events:?}"
    );

    // No execution handles stored.
    let managed = controller
        .routes
        .get("post-start-rollback")
        .expect("route exists");
    assert!(
        managed.pipeline_handle.is_none(),
        "no pipeline handle stored on failure"
    );
    assert!(
        managed.consumer_handle.is_none(),
        "no consumer handle stored on failure"
    );
}

// rc-kh7c: Verify the consumer task is properly cleaned up when startup fails.
// When the startup handshake returns Err — `await_consumer_startup` for
// Explicit bind failures; Immediate fast start() errors never reach a
// controller await (pre-resolved receiver — the detached watcher owns
// cleanup) — the spawned consumer task and its child tasks MUST be
// stopped — not left detached. Dropping a JoinHandle detaches the task
// (Tokio contract); it keeps running in the background.
//
// The fix aborts the consumer JoinHandle AND cancels the consumer's
// CancellationToken so child tasks that observe ctx.cancelled() also stop.
//
// Test design: register a mock component whose consumer's `start()` spawns
// a background counter task that respects ctx.cancelled() (as a well-behaved
// consumer would), then returns Err. If the fix works, the cancel token is
// cancelled and the counter stops. If the fix regresses, the counter keeps
// incrementing (orphan task leak).

/// Mock consumer whose `start()` spawns a background counter task that
/// respects `ctx.cancel_token()`, then returns Err (simulated bind failure).
/// If the fix cancels the token, the counter stops; otherwise it leaks.
struct RcKh7cFailBindConsumer {
    counter: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl Consumer for RcKh7cFailBindConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let counter = Arc::clone(&self.counter);
        let cancel = ctx.cancel_token();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        });
        Err(CamelError::RouteError("simulated bind failure".to_string()))
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }
}

/// Mock endpoint that vends `RcKh7cFailBindConsumer`.
struct RcKh7cFailBindEndpoint {
    uri: String,
    counter: Arc<AtomicU64>,
}

impl Endpoint for RcKh7cFailBindEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(RcKh7cFailBindConsumer {
            counter: Arc::clone(&self.counter),
        }))
    }
    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::ProcessorError(
            "failbind does not support producers".into(),
        ))
    }
}

/// Mock component that vends `RcKh7cFailBindEndpoint` under the
/// `"failbind"` scheme.
struct RcKh7cFailBindComponent {
    counter: Arc<AtomicU64>,
}

impl Component for RcKh7cFailBindComponent {
    fn scheme(&self) -> &str {
        "failbind"
    }
    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(RcKh7cFailBindEndpoint {
            uri: uri.to_string(),
            counter: Arc::clone(&self.counter),
        }))
    }
}

/// Mock consumer whose `start()` returns Err promptly (simulated immediate
/// failure). `startup_mode()` is NOT overridden — Immediate is the trait
/// default — so the rc-slvd three-latch handshake observes the fast error
/// and fails route start/resume loudly instead of fire-and-forgetting it.
struct ImmediateFailConsumer;

#[async_trait::async_trait]
impl Consumer for ImmediateFailConsumer {
    async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
        Err(CamelError::RouteError(
            "simulated immediate failure".to_string(),
        ))
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

/// Mock endpoint that vends `ImmediateFailConsumer`.
struct ImmediateFailEndpoint {
    uri: String,
}

impl Endpoint for ImmediateFailEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(ImmediateFailConsumer))
    }
    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::ProcessorError(
            "immediatefail does not support producers".into(),
        ))
    }
}

/// Mock component that vends `ImmediateFailEndpoint` under the
/// `"immediatefail"` scheme.
struct ImmediateFailComponent;

impl Component for ImmediateFailComponent {
    fn scheme(&self) -> &str {
        "immediatefail"
    }
    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(ImmediateFailEndpoint {
            uri: uri.to_string(),
        }))
    }
}

/// rc-a7rh: mock Explicit consumer whose `start()` latches readiness via
/// `ctx.mark_ready()` and then panics — the production defect class where
/// the bind succeeded but the consumer task died right after. The detached
/// outer-task watcher must observe the dead handle and transition the Route
/// to Failed; without it the Route stays Started with a dead handle.
struct PanickingAfterReadyConsumer;

#[async_trait::async_trait]
impl Consumer for PanickingAfterReadyConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        ctx.mark_ready();
        panic!("consumer exploded after readiness");
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }
}

/// Mock endpoint that vends `PanickingAfterReadyConsumer`.
struct PanickingAfterReadyEndpoint {
    uri: String,
}

impl Endpoint for PanickingAfterReadyEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(PanickingAfterReadyConsumer))
    }
    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::ProcessorError(
            "panicready does not support producers".into(),
        ))
    }
}

/// Mock component that vends `PanickingAfterReadyEndpoint` under the
/// `"panicready"` scheme.
struct PanickingAfterReadyComponent;

impl Component for PanickingAfterReadyComponent {
    fn scheme(&self) -> &str {
        "panicready"
    }
    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(PanickingAfterReadyEndpoint {
            uri: uri.to_string(),
        }))
    }
}

/// Mock consumer whose `start()` fails on demand: when `fail_next` is set it
/// spawns an rc-kh7c-style counter child task (respects `ctx.cancelled()`,
/// increments every 10ms) and returns Err (simulated resume failure);
/// otherwise it parks loop-style on `ctx.cancelled()` (Ok path). Pins abort
/// parity on a failed resume: the cancel token must be cancelled so the
/// counter stops — no detached task may keep incrementing.
struct FlakyResumeConsumer {
    counter: Arc<AtomicU64>,
    fail_next: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Consumer for FlakyResumeConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            let counter = Arc::clone(&self.counter);
            let cancel = ctx.cancel_token();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_millis(10)) => {
                            counter.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            });
            Err(CamelError::RouteError(
                "simulated resume failure".to_string(),
            ))
        } else {
            // Loop-style Ok path: park until cancellation.
            ctx.cancelled().await;
            Ok(())
        }
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

/// Mock endpoint that vends `FlakyResumeConsumer`, threading both Arcs
/// through the endpoint chain (mirrors `RcKh7cFailBindEndpoint`).
struct FlakyResumeEndpoint {
    uri: String,
    counter: Arc<AtomicU64>,
    fail_next: Arc<AtomicBool>,
}

impl Endpoint for FlakyResumeEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(FlakyResumeConsumer {
            counter: Arc::clone(&self.counter),
            fail_next: Arc::clone(&self.fail_next),
        }))
    }
    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::ProcessorError(
            "flakyresume does not support producers".into(),
        ))
    }
}

/// Mock component that vends `FlakyResumeEndpoint` under the
/// `"flakyresume"` scheme.
struct FlakyResumeComponent {
    counter: Arc<AtomicU64>,
    fail_next: Arc<AtomicBool>,
}

impl Component for FlakyResumeComponent {
    fn scheme(&self) -> &str {
        "flakyresume"
    }
    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(FlakyResumeEndpoint {
            uri: uri.to_string(),
            counter: Arc::clone(&self.counter),
            fail_next: Arc::clone(&self.fail_next),
        }))
    }
}

#[tokio::test]
async fn start_route_aborts_consumer_task_on_startup_failure() {
    let counter = Arc::new(AtomicU64::new(0));

    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(RcKh7cFailBindComponent {
            counter: Arc::clone(&counter),
        }));
    }
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    controller
        .add_route(RouteDefinition::new("failbind:test", vec![]).with_route_id("rc-kh7c-abort"))
        .await
        .expect("add_route");

    let result = controller.start_route("rc-kh7c-abort").await;
    assert!(
        result.is_err(),
        "start_route must fail when consumer's start() returns Err"
    );

    // Verify the consumer task was aborted. If the task (or any task
    // spawned by it) is still running, the counter will keep incrementing
    // during the sleep. With the fix, the consumer task's JoinHandle is
    // `.abort()`ed, so the counter stops.
    let v1 = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let v2 = counter.load(Ordering::SeqCst);
    assert_eq!(
        v1, v2,
        "consumer task was not aborted: counter advanced from {v1} to {v2} \
         after start_route returned Err (orphan task leak)"
    );
}

// rc-slvd: an Immediate consumer whose `start()` returns Err promptly is
// handled by the DETACHED failure watcher — every lifecycle operation
// returns Ok and the route reaches Failed asynchronously. The E2E tests
// below drive a REAL RuntimeBus wired to a DefaultRouteController through
// the controller actor and observe the outcome ONLY via
// `RuntimeQuery::GetRouteStatus` polling (the handle-liveness
// `inferred_lifecycle_label` idiom cannot emit Failed, route_helpers.rs).

use camel_api::{RuntimeCommandBus as _, RuntimeQueryBus as _};

/// Wire a real RuntimeBus to a DefaultRouteController (via the controller
/// actor) sharing one InMemoryRuntimeStore — production topology, minus
/// the context plumbing. The controller actor learns the bus handle so the
/// failure watcher's FailRoute reaches this bus.
#[allow(clippy::type_complexity)]
async fn wired_bus_and_controller(
    registry: Arc<std::sync::Mutex<Registry>>,
) -> (
    Arc<crate::lifecycle::application::runtime_bus::RuntimeBus>,
    crate::lifecycle::adapters::controller_actor::RouteControllerHandle,
) {
    use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
    use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
    use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
    use crate::lifecycle::application::runtime_bus::RuntimeBus;

    let controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );
    let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);
    let store = InMemoryRuntimeStore::default();
    let execution: std::sync::Arc<dyn crate::lifecycle::application::ports::RuntimeExecutionPort> =
        Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
    let bus = Arc::new(
        RuntimeBus::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
        )
        .with_uow(Arc::new(store.clone()))
        .with_execution(execution),
    );
    ctrl_handle
        .set_runtime_handle(bus.clone())
        .await
        .expect("wire bus handle into controller actor");
    (bus, ctrl_handle)
}

/// Poll `GetRouteStatus` until `want` (10ms interval, 2s bound) — the
/// normative observation idiom for the async watcher transition.
async fn poll_route_status(
    bus: &crate::lifecycle::application::runtime_bus::RuntimeBus,
    route_id: &str,
    want: &str,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match bus
            .ask(camel_api::RuntimeQuery::GetRouteStatus {
                route_id: route_id.to_string(),
            })
            .await
            .expect("status query")
        {
            camel_api::RuntimeQueryResult::RouteStatus { status, .. } if status == want => {
                return;
            }
            camel_api::RuntimeQueryResult::RouteStatus { status, .. } => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "route {route_id} never reached {want} within 2s (last: {status})"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            other => panic!("unexpected query result for {route_id}: {other:?}"),
        }
    }
}

async fn register_route_via_bus(
    bus: &crate::lifecycle::application::runtime_bus::RuntimeBus,
    def: RouteDefinition,
) {
    use crate::lifecycle::application::ports::RouteRegistrationPort;
    RouteRegistrationPort::register_route(bus, def)
        .await
        .expect("register route through the bus");
}

#[tokio::test]
async fn immediate_consumer_error_transitions_route_to_failed() {
    // rc-slvd: start_route returns Ok (pre-resolved receiver); the detached
    // failure watcher transitions the route to Failed asynchronously —
    // observed through the REAL bus, never via handle liveness.
    // FlakyResumeComponent with fail_next pre-armed = an Immediate consumer
    // whose first start() spawns a counter child then fails — the same
    // abort-parity leg the resume E2E exercises.
    let counter = Arc::new(AtomicU64::new(0));
    let fail_next = Arc::new(AtomicBool::new(true));

    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(FlakyResumeComponent {
            counter: Arc::clone(&counter),
            fail_next: Arc::clone(&fail_next),
        }));
    }
    let (bus, _ctrl) = wired_bus_and_controller(registry).await;

    register_route_via_bus(
        &bus,
        RouteDefinition::new("flakyresume:x", vec![]).with_route_id("immediate-fail-e2e"),
    )
    .await;

    // start_route returns Ok — the receiver is pre-resolved; the watcher
    // owns the failure surface (either Started-then-Failed or the Phase 2a
    // supersede — both Ok).
    let result = bus
        .execute(RuntimeCommand::StartRoute {
            route_id: "immediate-fail-e2e".to_string(),
            command_id: "e2e-start".to_string(),
            causation_id: None,
        })
        .await;
    assert!(
        result.is_ok(),
        "StartRoute must return Ok for Immediate consumers (watcher handles failure): {result:?}"
    );

    poll_route_status(&bus, "immediate-fail-e2e", "Failed").await;

    // Abort parity: the failed start leaves no detached child tasks —
    // the counter child spawned by the failing start() must have stopped
    // (double-snapshot equal).
    tokio::time::sleep(Duration::from_millis(50)).await;
    let v1 = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let v2 = counter.load(Ordering::SeqCst);
    assert_eq!(
        v1, v2,
        "counter child task kept running after the failed start (detached task leak): {v1} -> {v2}"
    );
}

// rc-a7rh: E2E for the production defect this change fixes — an Explicit
// consumer that latches readiness then dies (panic). The detached outer-task
// watcher must observe the dead handle and transition the Route to Failed.
// Before the wiring, all three production call sites dropped the outer-watcher
// inputs unwatched and the Route stayed Started with a dead handle.

#[tokio::test]
async fn outer_task_watcher_panic_after_ready_transitions_route_to_failed() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(PanickingAfterReadyComponent));
    }
    let (bus, _ctrl) = wired_bus_and_controller(registry).await;

    register_route_via_bus(
        &bus,
        RouteDefinition::new("panicready:x", vec![]).with_route_id("panic-after-ready-e2e"),
    )
    .await;

    // mark_ready latches BEFORE the panic — first-signal-wins resolves the
    // handshake Ok, so StartRoute must return Ok and leave the failure
    // surface to the outer-task watcher.
    let result = bus
        .execute(RuntimeCommand::StartRoute {
            route_id: "panic-after-ready-e2e".to_string(),
            command_id: "e2e-start".to_string(),
            causation_id: None,
        })
        .await;
    assert!(
        result.is_ok(),
        "StartRoute must return Ok (mark_ready latched before the panic): {result:?}"
    );

    poll_route_status(&bus, "panic-after-ready-e2e", "Failed").await;
}

// rc-slvd: a failed resume of an Immediate consumer returns Ok and the
// detached watcher transitions the route to Failed — the watcher's abort +
// cancel leave no detached child tasks (counter abort parity).

#[tokio::test]
async fn immediate_consumer_error_on_resume_transitions_to_failed() {
    let counter = Arc::new(AtomicU64::new(0));
    let fail_next = Arc::new(AtomicBool::new(false));

    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(FlakyResumeComponent {
            counter: Arc::clone(&counter),
            fail_next: Arc::clone(&fail_next),
        }));
    }
    let (bus, _ctrl) = wired_bus_and_controller(registry).await;

    register_route_via_bus(
        &bus,
        RouteDefinition::new("flakyresume:x", vec![]).with_route_id("flaky-resume-e2e"),
    )
    .await;

    // Initial start: loop-style Ok path.
    bus.execute(RuntimeCommand::StartRoute {
        route_id: "flaky-resume-e2e".to_string(),
        command_id: "e2e-resume-start".to_string(),
        causation_id: None,
    })
    .await
    .expect("initial start");
    poll_route_status(&bus, "flaky-resume-e2e", "Started").await;

    // Suspend: cancels the consumer token, keeps the pipeline.
    bus.execute(RuntimeCommand::SuspendRoute {
        route_id: "flaky-resume-e2e".to_string(),
        command_id: "e2e-resume-suspend".to_string(),
        causation_id: None,
    })
    .await
    .expect("suspend");
    poll_route_status(&bus, "flaky-resume-e2e", "Suspended").await;

    // Arm the failure for the next start() (the resume).
    fail_next.store(true, Ordering::SeqCst);

    // resume_route returns Ok — the receiver is pre-resolved; the watcher
    // owns the failure surface.
    let result = bus
        .execute(RuntimeCommand::ResumeRoute {
            route_id: "flaky-resume-e2e".to_string(),
            command_id: "e2e-resume-resume".to_string(),
            causation_id: None,
        })
        .await;
    assert!(
        result.is_ok(),
        "ResumeRoute must return Ok for Immediate consumers (watcher handles failure): {result:?}"
    );

    poll_route_status(&bus, "flaky-resume-e2e", "Failed").await;

    // Abort parity: the failed resume leaves no detached child tasks —
    // the counter child spawned by the failing start() must have stopped
    // (double-snapshot equal).
    tokio::time::sleep(Duration::from_millis(50)).await;
    let v1 = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let v2 = counter.load(Ordering::SeqCst);
    assert_eq!(
        v1, v2,
        "counter child task kept running after the failed resume (detached task leak): {v1} -> {v2}"
    );
}

// rc-slvd: the aggregate start path (route_controller.rs) behaves the same
// — Ok for Immediate consumers, the watcher owns the failure surface, and
// the route reaches Failed through the real bus.

#[tokio::test]
async fn aggregate_immediate_error_transitions_to_failed() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(ImmediateFailComponent));
    }
    let (bus, _ctrl) = wired_bus_and_controller(registry).await;

    // force_completion_on_stop triggers the aggregate split, routing this
    // through start_aggregate_route.
    let agg_config = camel_api::AggregatorConfig::correlate_by("key")
        .complete_when_size(10)
        .force_completion_on_stop(true)
        .build()
        .unwrap();

    register_route_via_bus(
        &bus,
        RouteDefinition::new(
            "immediatefail:x",
            vec![BuilderStep::Aggregate { config: agg_config }],
        )
        .with_route_id("agg-immediate-e2e"),
    )
    .await;

    // Aggregate start_route returns Ok — the receiver is pre-resolved.
    let result = bus
        .execute(RuntimeCommand::StartRoute {
            route_id: "agg-immediate-e2e".to_string(),
            command_id: "e2e-agg-start".to_string(),
            causation_id: None,
        })
        .await;
    assert!(
        result.is_ok(),
        "aggregate StartRoute must return Ok for Immediate consumers (watcher handles failure): {result:?}"
    );

    poll_route_status(&bus, "agg-immediate-e2e", "Failed").await;
}

// rc-slvd: CamelContext::start no longer fails fast on Immediate consumer
// errors — the failing route transitions to Failed asynchronously while
// healthy siblings still reach Started (spec: ctx no-fail-fast).

#[tokio::test]
async fn context_start_does_not_fail_fast_on_immediate_error() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(ImmediateFailComponent));
        guard.register(Arc::new(camel_component_timer::TimerComponent::new()));
    }
    let mut ctx = crate::context::CamelContext::builder()
        .registry(registry)
        .build()
        .await
        .expect("build context");

    ctx.add_route_definition(
        RouteDefinition::new("immediatefail:x", vec![]).with_route_id("ctx-immediate-fail"),
    )
    .await
    .expect("add failing route");
    ctx.add_route_definition(
        RouteDefinition::new("timer:tick?period=60000&repeatCount=1", vec![])
            .with_route_id("ctx-healthy-sibling"),
    )
    .await
    .expect("add sibling route");

    // Context start must NOT fail fast on the Immediate consumer's prompt
    // error — the watcher owns the failure surface.
    ctx.start()
        .await
        .expect("CamelContext::start must return Ok despite the Immediate error");

    // Observe outcomes through the context's real runtime handle (the
    // context-internal RuntimeBus), never via handle liveness.
    let runtime = ctx.runtime();
    let mut sibling_started = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let failing_failed = matches!(
            runtime
                .ask(camel_api::RuntimeQuery::GetRouteStatus {
                    route_id: "ctx-immediate-fail".to_string(),
                })
                .await,
            Ok(camel_api::RuntimeQueryResult::RouteStatus { ref status, .. }) if status == "Failed"
        );
        if !sibling_started {
            sibling_started = matches!(
                runtime
                    .ask(camel_api::RuntimeQuery::GetRouteStatus {
                        route_id: "ctx-healthy-sibling".to_string(),
                    })
                    .await,
                Ok(camel_api::RuntimeQueryResult::RouteStatus { ref status, .. })
                    if status == "Started"
            );
        }
        if failing_failed && sibling_started {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "failing route Failed and sibling Started not both reached within 2s \
             (failed route done: {failing_failed}, sibling started: {sibling_started})"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let _ = ctx.stop().await;
}

// rc-slvd task 2.2: deterministic startup-reentrancy regression. An
// Immediate loop-style consumer (timer analogue) emits a ControlBus-style
// StopRoute against a sibling whose start() is still parked mid-start —
// the controller actor must never be delayed by the Immediate handshake
// (discriminating sub-grace timeout below), and the sibling's lifecycle
// must commit and honor the stop cleanly (no invalid-transition error, no
// lost StopRoute, final Stopped).
//
// DEVIATION from the task text (reported): the task resolves sibling B as
// a held EXPLICIT consumer. Empirically (probe on this exact tree), a stop
// dispatched while a held Explicit sibling parks the actor in the
// handshake is DETERMINISTICALLY rejected by the bus pre-validation — the
// aggregate sits at `Starting` and Stop-from-Starting returns
// `invalid transition: Starting -> Stopped` before the command ever
// reaches the controller actor — so the task's assertion set (StopRoute
// Ok, "stop intent honored after commit", no invalid-transition error in
// the proxy log) cannot pass in that construction. The deterministic
// green construction keeps the hold in B's CONSUMER task (off-actor, per
// the deadlock rule) but declares B loop-style Immediate: B's StartRoute
// commits Started at once while B's start() stays parked on `hold` — the
// emission provably lands inside B's *start()* window (start_entered
// fired; hold still held when the dispatch is observed) and the stop is
// honored after the commit, which is exactly what the assertions pin.

/// One recorded proxy operation — command label, target route, and the
/// forwarded execution outcome.
#[derive(Debug, Clone)]
struct ProxyRecord {
    label: &'static str,
    route_id: String,
    ok: bool,
    detail: String,
}

/// Recording proxy `RuntimeHandle`: records every command/query AND
/// forwards to the real RuntimeBus, so the reentrant StopRoute genuinely
/// reaches the controller actor through the bus (a pure recorder that
/// swallows the command would make the test vacuously green).
struct RecordingProxyRuntime {
    bus: Arc<crate::lifecycle::application::runtime_bus::RuntimeBus>,
    dispatched_tx: tokio::sync::mpsc::UnboundedSender<String>,
    log: std::sync::Mutex<Vec<ProxyRecord>>,
}

impl RecordingProxyRuntime {
    fn command_label(cmd: &RuntimeCommand) -> (&'static str, String) {
        match cmd {
            RuntimeCommand::StartRoute { route_id, .. } => ("StartRoute", route_id.clone()),
            RuntimeCommand::StopRoute { route_id, .. } => ("StopRoute", route_id.clone()),
            RuntimeCommand::SuspendRoute { route_id, .. } => ("SuspendRoute", route_id.clone()),
            RuntimeCommand::ResumeRoute { route_id, .. } => ("ResumeRoute", route_id.clone()),
            RuntimeCommand::FailRoute { route_id, .. } => ("FailRoute", route_id.clone()),
            RuntimeCommand::RemoveRoute { route_id, .. } => ("RemoveRoute", route_id.clone()),
            _ => ("Other", String::new()),
        }
    }
}

#[async_trait::async_trait]
impl camel_api::RuntimeCommandBus for RecordingProxyRuntime {
    async fn execute(
        &self,
        cmd: RuntimeCommand,
    ) -> Result<camel_api::RuntimeCommandResult, CamelError> {
        let (label, route_id) = Self::command_label(&cmd);
        // Record the DISPATCH at entry — the test observes this while the
        // sibling's start() is still parked on its hold.
        let _ = self.dispatched_tx.send(format!("{label}:{route_id}"));
        let result = self.bus.execute(cmd).await;
        let (ok, detail) = match &result {
            Ok(res) => (true, format!("{res:?}")),
            Err(e) => (false, format!("{e:?}")),
        };
        self.log.lock().expect("proxy log lock").push(ProxyRecord {
            label,
            route_id,
            ok,
            detail,
        });
        result
    }
}

#[async_trait::async_trait]
impl camel_api::RuntimeQueryBus for RecordingProxyRuntime {
    async fn ask(
        &self,
        query: camel_api::RuntimeQuery,
    ) -> Result<camel_api::RuntimeQueryResult, CamelError> {
        let result = self.bus.ask(query).await;
        let (ok, detail) = match &result {
            Ok(res) => (true, format!("{res:?}")),
            Err(e) => (false, format!("{e:?}")),
        };
        self.log.lock().expect("proxy log lock").push(ProxyRecord {
            label: "Query",
            route_id: String::new(),
            ok,
            detail,
        });
        result
    }
}

/// Sibling B: loop-style Immediate consumer whose start() fires
/// `start_entered` as its FIRST action, then parks on the `hold` gate
/// (off-actor — the consumer task spawned by spawn_consumer_task), then
/// serves its loop-style lifetime on ctx.cancelled(). The hold IS the
/// "sibling mid-start" barrier.
struct HeldStartConsumer {
    entered_tx: tokio::sync::watch::Sender<bool>,
    hold_rx: tokio::sync::watch::Receiver<bool>,
}

#[async_trait::async_trait]
impl Consumer for HeldStartConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // FIRST action inside start(): prove B is mid-start (inside the
        // start window) rather than inferring from the dispatch.
        let _ = self.entered_tx.send(true);
        while !*self.hold_rx.borrow_and_update() {
            if self.hold_rx.changed().await.is_err() {
                break;
            }
        }
        // Keep the task's mark_ready shape from the task text (a no-op on
        // the pre-resolved Immediate signal) and park loop-style.
        ctx.mark_ready();
        ctx.cancelled().await;
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

struct HeldStartEndpoint {
    uri: String,
    entered_tx: tokio::sync::watch::Sender<bool>,
    hold_rx: tokio::sync::watch::Receiver<bool>,
}

impl Endpoint for HeldStartEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(HeldStartConsumer {
            entered_tx: self.entered_tx.clone(),
            hold_rx: self.hold_rx.clone(),
        }))
    }
    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::ProcessorError(
            "heldstart does not support producers".into(),
        ))
    }
}

struct HeldStartComponent {
    entered_tx: tokio::sync::watch::Sender<bool>,
    hold_rx: tokio::sync::watch::Receiver<bool>,
}

impl Component for HeldStartComponent {
    fn scheme(&self) -> &str {
        "heldstart"
    }
    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(HeldStartEndpoint {
            uri: uri.to_string(),
            entered_tx: self.entered_tx.clone(),
            hold_rx: self.hold_rx.clone(),
        }))
    }
}

/// Emitter A: Immediate loop-style consumer (timer analogue) whose FIRST
/// EMISSION is blocked on the `emit_gate` oneshot-analogue. When released,
/// the emission drives `StopRoute{target: B}` through the RECORDING PROXY
/// runtime handle (record + forward to the real bus), then serves its
/// loop-style lifetime on ctx.cancelled().
struct GatedEmitConsumer {
    runtime: Arc<dyn camel_api::RuntimeHandle>,
    target_route_id: String,
    emit_gate_rx: tokio::sync::watch::Receiver<bool>,
    emission_done_tx: tokio::sync::watch::Sender<bool>,
}

#[async_trait::async_trait]
impl Consumer for GatedEmitConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // Park the FIRST EMISSION on the gate — the two-way barrier's
        // emitter side.
        while !*self.emit_gate_rx.borrow_and_update() {
            if self.emit_gate_rx.changed().await.is_err() {
                break;
            }
        }
        // The emission (ControlBus-style stop of the sibling) through the
        // recording proxy — records the dispatch AND forwards, so the
        // reentrant StopRoute genuinely reaches the controller actor.
        let _ = self
            .runtime
            .execute(RuntimeCommand::StopRoute {
                route_id: self.target_route_id.clone(),
                command_id: "reentrancy-emission-stop".to_string(),
                causation_id: None,
            })
            .await;
        let _ = self.emission_done_tx.send(true);
        ctx.cancelled().await;
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

struct GatedEmitEndpoint {
    uri: String,
    runtime: Arc<dyn camel_api::RuntimeHandle>,
    target_route_id: String,
    emit_gate_rx: tokio::sync::watch::Receiver<bool>,
    emission_done_tx: tokio::sync::watch::Sender<bool>,
}

impl Endpoint for GatedEmitEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(GatedEmitConsumer {
            runtime: Arc::clone(&self.runtime),
            target_route_id: self.target_route_id.clone(),
            emit_gate_rx: self.emit_gate_rx.clone(),
            emission_done_tx: self.emission_done_tx.clone(),
        }))
    }
    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::ProcessorError(
            "gatedemit does not support producers".into(),
        ))
    }
}

struct GatedEmitComponent {
    runtime: Arc<dyn camel_api::RuntimeHandle>,
    target_route_id: String,
    emit_gate_rx: tokio::sync::watch::Receiver<bool>,
    emission_done_tx: tokio::sync::watch::Sender<bool>,
}

impl Component for GatedEmitComponent {
    fn scheme(&self) -> &str {
        "gatedemit"
    }
    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(GatedEmitEndpoint {
            uri: uri.to_string(),
            runtime: Arc::clone(&self.runtime),
            target_route_id: self.target_route_id.clone(),
            emit_gate_rx: self.emit_gate_rx.clone(),
            emission_done_tx: self.emission_done_tx.clone(),
        }))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn timer_emission_during_start_does_not_corrupt_sibling() {
    use crate::lifecycle::adapters::consumer_management::CONSUMER_IMMEDIATE_GRACE;
    let half_grace = CONSUMER_IMMEDIATE_GRACE / 2; // 25ms sub-grace bound

    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let (bus, _ctrl) = wired_bus_and_controller(Arc::clone(&registry)).await;

    // The proxy forwards to the real bus; A's consumer emits through it.
    let (dispatched_tx, mut dispatched_rx) = tokio::sync::mpsc::unbounded_channel();
    let proxy = Arc::new(RecordingProxyRuntime {
        bus: Arc::clone(&bus),
        dispatched_tx,
        log: std::sync::Mutex::new(Vec::new()),
    });

    // B's two-way barrier channels: start_entered (fired FIRST inside
    // start()) and hold (parks B's start until the test releases it).
    let (entered_tx, mut entered_rx) = tokio::sync::watch::channel(false);
    let (hold_tx, hold_rx) = tokio::sync::watch::channel(false);
    // A's barrier channels: emit_gate (parks A's first emission) and
    // emission_done (A's emission — the forwarded StopRoute — returned).
    let (emit_gate_tx, emit_gate_rx) = tokio::sync::watch::channel(false);
    let (emission_done_tx, mut emission_done_rx) = tokio::sync::watch::channel(false);

    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(HeldStartComponent {
            entered_tx,
            hold_rx,
        }));
        guard.register(Arc::new(GatedEmitComponent {
            runtime: proxy.clone() as Arc<dyn camel_api::RuntimeHandle>,
            target_route_id: "reentrancy-sibling-b".to_string(),
            emit_gate_rx,
            emission_done_tx,
        }));
    }

    register_route_via_bus(
        &bus,
        RouteDefinition::new("heldstart:x", vec![]).with_route_id("reentrancy-sibling-b"),
    )
    .await;
    register_route_via_bus(
        &bus,
        RouteDefinition::new("gatedemit:x", vec![]).with_route_id("reentrancy-emitter-a"),
    )
    .await;

    // --- Point 3 (design-discriminating): A's StartRoute resolves within
    // half the grace FIRST — under the synchronous design the actor parks
    // in A's grace select and this timeout fails (RED); under the async
    // watcher design the Immediate handshake never delays the actor.
    let test_start = tokio::time::Instant::now();
    let a_resolved = tokio::time::timeout(
        half_grace,
        bus.execute(RuntimeCommand::StartRoute {
            route_id: "reentrancy-emitter-a".to_string(),
            command_id: "reentrancy-start-a".to_string(),
            causation_id: None,
        }),
    )
    .await
    .expect("A's StartRoute must resolve within half the immediate grace (25ms) — the controller actor must never park in the Immediate handshake");
    let a_resolved_at = tokio::time::Instant::now();
    assert!(
        a_resolved.is_ok(),
        "A's StartRoute must succeed: {a_resolved:?}"
    );

    // THEN B: dispatch its StartRoute (Immediate handshake — the command
    // commits Started at once while B's start() stays parked on the hold)
    // and await the start_entered barrier.
    let bus_for_b = Arc::clone(&bus);
    let b_start_handle = tokio::spawn(async move {
        bus_for_b
            .execute(RuntimeCommand::StartRoute {
                route_id: "reentrancy-sibling-b".to_string(),
                command_id: "reentrancy-start-b".to_string(),
                causation_id: None,
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !*entered_rx.borrow_and_update() {
            entered_rx
                .changed()
                .await
                .expect("B must enter start() (start_entered barrier)");
        }
    })
    .await
    .expect("start_entered barrier must fire within 2s — B never entered start()");
    let b_entered_at = tokio::time::Instant::now();
    assert!(
        a_resolved_at < b_entered_at,
        "A's StartRoute resolution must precede B's start entry (actor not delayed): \
         a_resolved_at={a_resolved_at:?}, b_entered_at={b_entered_at:?}"
    );
    assert!(
        a_resolved_at.duration_since(test_start) <= half_grace,
        "A's StartRoute resolved {half_grace:?} after the test start — the Immediate \
         handshake delayed the controller actor"
    );

    // B's StartRoute commits Started while B's start() stays parked on the
    // hold (see the DEVIATION note above: awaiting the commit makes the
    // emission's landing point deterministic — the stop is then honored
    // after the commit, exactly as the assertions require).
    let b_result = tokio::time::timeout(Duration::from_secs(2), b_start_handle)
        .await
        .expect("B's StartRoute-commit barrier must resolve within 2s — the Immediate handshake never parks the actor")
        .expect("join B StartRoute task")
        .expect("B's StartRoute must succeed and commit");
    assert!(
        matches!(
            &b_result,
            camel_api::RuntimeCommandResult::RouteStateChanged { status, .. } if status == "Started"
        ),
        "B's StartRoute must commit Started: {b_result:?}"
    );

    // --- Points 4-5: release the emission gate; A's first emission drives
    // StopRoute{B} through the recording proxy. Observe the DISPATCH in
    // the proxy's recording while B's start() is still parked on the hold
    // — the emission provably lands inside B's uncommitted start window.
    emit_gate_tx.send_replace(true);
    let dispatched = dispatched_rx
        .recv()
        .await
        .expect("the emission must dispatch StopRoute through the proxy");
    assert_eq!(
        dispatched, "StopRoute:reentrancy-sibling-b",
        "unexpected proxied dispatch: {dispatched}"
    );
    // Only NOW release B's hold — B cannot have left start() before the
    // StopRoute dispatch was observed (deterministic barrier, no timing
    // luck). B's consumer proceeds to ctx.cancelled(), already cancelled
    // by the in-flight stop, so the controller's stop join completes.
    hold_tx.send_replace(true);

    // --- Point 6: assertions.
    while !*emission_done_rx.borrow_and_update() {
        emission_done_rx
            .changed()
            .await
            .expect("A's emission must complete (forwarded StopRoute returned)");
    }
    poll_route_status(&bus, "reentrancy-sibling-b", "Stopped").await;

    let log = proxy.log.lock().expect("proxy log lock").clone();
    assert_eq!(
        log.len(),
        1,
        "the proxy must record exactly one command (the emission's StopRoute): {log:?}"
    );
    assert_eq!(log[0].label, "StopRoute");
    assert_eq!(log[0].route_id, "reentrancy-sibling-b");
    assert!(
        log[0].ok,
        "the reentrant StopRoute must succeed — no lost command, no invalid-transition \
         error (Registered -> Stopped / Starting -> Stopped): {}",
        log[0].detail
    );
}

// === Task 4: EndpointIndex integration tests ===

fn make_test_controller() -> DefaultRouteController {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    )
}

#[tokio::test]
async fn controller_add_route_indexes_endpoint() {
    let mut c = make_test_controller();
    let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("r1");
    c.add_route(def).await.unwrap();
    assert_eq!(c.routes_for_endpoint("timer:tick"), vec!["r1"]);
}

#[tokio::test]
async fn controller_add_route_with_generation_indexes_endpoint() {
    let mut c = make_test_controller();
    let def = RouteDefinition::new("direct:gen", vec![]).with_route_id("r2");
    c.add_route_with_generation(def, 1).await.unwrap();
    assert_eq!(c.routes_for_endpoint("direct:gen"), vec!["r2"]);
}

#[tokio::test]
async fn controller_remove_route_clears_endpoint() {
    let mut c = make_test_controller();
    let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("r1");
    c.add_route(def).await.unwrap();
    c.remove_route("r1").await.unwrap();
    assert!(c.routes_for_endpoint("timer:tick").is_empty());
}

#[tokio::test]
async fn controller_remove_preserving_clears_endpoint() {
    let mut c = make_test_controller();
    let def = RouteDefinition::new("direct:cleanup", vec![]).with_route_id("r3");
    c.add_route(def).await.unwrap();
    c.remove_route_preserving_functions("r3").await.unwrap();
    assert!(c.routes_for_endpoint("direct:cleanup").is_empty());
}

#[tokio::test]
async fn controller_multiple_routes_same_uri() {
    let mut c = make_test_controller();
    c.add_route(RouteDefinition::new("direct:shared", vec![]).with_route_id("a"))
        .await
        .unwrap();
    c.add_route(RouteDefinition::new("direct:shared", vec![]).with_route_id("b"))
        .await
        .unwrap();
    let routes = c.routes_for_endpoint("direct:shared");
    assert!(routes.contains(&"a".to_string()));
    assert!(routes.contains(&"b".to_string()));
}

#[tokio::test]
async fn controller_list_endpoint_uris() {
    let mut c = make_test_controller();
    c.add_route(RouteDefinition::new("timer:a", vec![]).with_route_id("r1"))
        .await
        .unwrap();
    c.add_route(RouteDefinition::new("direct:b", vec![]).with_route_id("r2"))
        .await
        .unwrap();
    c.add_route(RouteDefinition::new("seda:c", vec![]).with_route_id("r3"))
        .await
        .unwrap();
    let uris = c.list_endpoint_uris();
    assert!(uris.contains(&"timer:a".to_string()));
    assert!(uris.contains(&"direct:b".to_string()));
    assert!(uris.contains(&"seda:c".to_string()));
}

#[tokio::test]
async fn controller_list_endpoint_uris_empty() {
    let c = make_test_controller();
    assert!(c.list_endpoint_uris().is_empty());
}

#[tokio::test]
async fn controller_insert_prepared_route_indexes_endpoint() {
    let mut c = make_test_controller();
    let def = RouteDefinition::new("seda:staged", vec![]).with_route_id("r4");
    let prepared = c.prepare_route_definition_with_generation(def, 1).unwrap();
    c.insert_prepared_route(prepared).unwrap();
    assert_eq!(c.routes_for_endpoint("seda:staged"), vec!["r4"]);
}

// === Task 5: Adapter integration tests ===

use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
use crate::lifecycle::application::ports::RuntimeExecutionPort;

fn build_adapter() -> (RuntimeExecutionAdapter, tokio::task::JoinHandle<()>) {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("lock");
        guard.register(Arc::new(camel_component_timer::TimerComponent::new()));
        guard.register(Arc::new(camel_component_mock::MockComponent::new()));
    }
    let controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );
    let (handle, join) = spawn_controller_actor(controller);
    (RuntimeExecutionAdapter::new(handle), join)
}

#[tokio::test]
async fn adapter_list_endpoints_returns_registered_uris() {
    let (adapter, _join) = build_adapter();
    adapter
        .register_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("r1"))
        .await
        .unwrap();
    let endpoints = adapter.list_endpoints().await.unwrap();
    assert!(endpoints.contains(&"timer:tick".to_string()));
}

#[tokio::test]
async fn adapter_routes_for_endpoint_returns_route_id() {
    let (adapter, _join) = build_adapter();
    adapter
        .register_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("r1"))
        .await
        .unwrap();
    let routes = adapter.routes_for_endpoint("timer:tick").await.unwrap();
    assert!(routes.contains(&"r1".to_string()));
}

#[tokio::test]
async fn adapter_routes_for_endpoint_unknown_returns_empty() {
    let (adapter, _join) = build_adapter();
    let routes = adapter.routes_for_endpoint("direct:unknown").await.unwrap();
    assert!(routes.is_empty());
}

#[tokio::test]
async fn adapter_health_check_endpoint_returns_healthy() {
    let (adapter, _join) = build_adapter();
    adapter
        .register_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("r1"))
        .await
        .unwrap();
    let status = adapter.health_check_endpoint("timer:tick").await.unwrap();
    assert_eq!(status, camel_api::HealthStatus::Healthy);
}

#[tokio::test]
async fn adapter_health_check_endpoint_unknown_returns_error() {
    let (adapter, _join) = build_adapter();
    assert!(
        adapter
            .health_check_endpoint("direct:unknown")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn adapter_remove_route_clears_endpoint() {
    let (adapter, _join) = build_adapter();
    adapter
        .register_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("r1"))
        .await
        .unwrap();
    adapter.remove_route("r1").await.unwrap();
    assert!(
        adapter
            .routes_for_endpoint("timer:tick")
            .await
            .unwrap()
            .is_empty()
    );
}

/// Regression guard for the credential-sources activation (Task 3.1): the
/// route controller must pass the route-declared `credential_sources` into the
/// consumer `SecurityContext`, not the hardcoded header-only default.
///
/// A capture component records the `SecurityContext` it receives in
/// `set_security_context`, so the test asserts the wiring end-to-end without
/// reaching into consumer internals.
struct CaptureSecCtxPolicy;

#[async_trait::async_trait]
impl SecurityPolicy for CaptureSecCtxPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut camel_api::Exchange,
        _auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        Ok(AuthorizationDecision::Granted {
            principal: Principal {
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

struct CaptureSecCtxAuthenticator;

#[async_trait::async_trait]
impl TokenAuthenticator for CaptureSecCtxAuthenticator {
    async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
        Ok(Principal {
            subject: "test".into(),
            issuer: "test".into(),
            audience: vec![],
            scopes: vec![],
            roles: vec![],
            claims: serde_json::Value::Null,
        })
    }
}

struct CaptureSecCtxConsumer {
    captured: Arc<std::sync::Mutex<Option<SecurityContext>>>,
}

#[async_trait::async_trait]
impl Consumer for CaptureSecCtxConsumer {
    async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
    fn set_security_context(&mut self, ctx: SecurityContext) {
        *self.captured.lock().expect("capture lock") = Some(ctx);
    }
}

struct CaptureSecCtxEndpoint {
    uri: String,
    captured: Arc<std::sync::Mutex<Option<SecurityContext>>>,
}

impl Endpoint for CaptureSecCtxEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(CaptureSecCtxConsumer {
            captured: Arc::clone(&self.captured),
        }))
    }
    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::ProcessorError(
            "capturesec does not support producers".into(),
        ))
    }
}

struct CaptureSecCtxComponent {
    captured: Arc<std::sync::Mutex<Option<SecurityContext>>>,
}

impl Component for CaptureSecCtxComponent {
    fn scheme(&self) -> &str {
        "capturesec"
    }
    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(CaptureSecCtxEndpoint {
            uri: uri.to_string(),
            captured: Arc::clone(&self.captured),
        }))
    }
}

#[tokio::test]
async fn start_route_wires_declared_credential_sources() {
    let captured: Arc<std::sync::Mutex<Option<SecurityContext>>> =
        Arc::new(std::sync::Mutex::new(None));

    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(CaptureSecCtxComponent {
            captured: Arc::clone(&captured),
        }));
    }
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    let sources = vec![CredentialSource::Cookie {
        name: "session".into(),
    }];
    controller
        .add_route(
            RouteDefinition::new("capturesec:test", vec![])
                .with_route_id("cred-sources-activation")
                .with_security_policy(
                    SecurityPolicyConfig::new(CaptureSecCtxPolicy)
                        .with_credential_sources(sources.clone()),
                )
                .with_security_authenticator(Arc::new(CaptureSecCtxAuthenticator)),
        )
        .await
        .expect("add_route");

    controller
        .start_route("cred-sources-activation")
        .await
        .expect("start_route");

    let guard = captured.lock().expect("capture lock");
    let sec_ctx = guard
        .as_ref()
        .expect("consumer received a security context");
    assert_eq!(
        sec_ctx.credential_sources, sources,
        "route-declared credential_sources must reach the consumer SecurityContext"
    );
}

#[cfg(test)]
mod late_registration_gate {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use camel_api::security_policy::{
        AuthContext, AuthorizationDecision, Principal, SecurityPolicy, SecurityPolicyConfig,
    };
    use camel_api::{BoxProcessor, Exchange, Message, OpaqueProcessor};
    use camel_auth::{ProviderEntry, ProviderRegistry, TokenAuthenticator};
    use tower::ServiceExt;

    struct AllowPolicy;

    #[async_trait::async_trait]
    impl SecurityPolicy for AllowPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
            _auth: &AuthContext<'_>,
        ) -> Result<AuthorizationDecision, CamelError> {
            Ok(AuthorizationDecision::Granted {
                principal: Principal {
                    subject: "tester".into(),
                    issuer: "test".into(),
                    audience: vec![],
                    scopes: vec![],
                    roles: vec![],
                    claims: serde_json::Value::Null,
                },
            })
        }
    }

    struct StubAuth;

    #[async_trait::async_trait]
    impl TokenAuthenticator for StubAuth {
        async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
            Ok(Principal {
                subject: "tester".into(),
                issuer: "test".into(),
                audience: vec![],
                scopes: vec![],
                roles: vec![],
                claims: serde_json::Value::Null,
            })
        }
    }

    fn sole_provider() -> Arc<ProviderRegistry> {
        let registry = ProviderRegistry::new();
        registry.register(
            "idp-a",
            ProviderEntry {
                authenticator: Arc::new(StubAuth),
                audience_binding: None,
            },
        );
        Arc::new(registry)
    }

    async fn running_listener(bind: &str) -> DefaultRouteController {
        let mut controller = build_controller_with_components();
        controller
            .add_route(
                RouteDefinition::new("timer:late-registration-listener?period=60000", vec![])
                    .with_route_id("listener"),
            )
            .await
            .expect("listener route should register");
        controller
            .start_route("listener")
            .await
            .expect("listener route should start");

        // The timer fixture supplies the existing running consumer task. The
        // controller's gate only needs the listener URI and running handle;
        // replacing the URI avoids binding a real socket in this unit suite.
        controller
            .routes
            .get_mut("listener")
            .expect("listener route exists")
            .from_uri = bind.to_string();
        controller
    }

    #[tokio::test]
    async fn late_registration_gate_includes_preregistered_sibling_plans() {
        let mut controller = build_controller_with_components();
        // Pre-registered Public sibling on the target bind, added while the
        // bind is not running (the start path owns that window).
        controller
            .add_route(
                RouteDefinition::new("http://0.0.0.0:8080/sibling", vec![])
                    .with_route_id("sibling-public"),
            )
            .await
            .expect("sibling registers before bind runs");
        controller
            .add_route(
                RouteDefinition::new("timer:late-registration-listener?period=60000", vec![])
                    .with_route_id("listener"),
            )
            .await
            .expect("listener route should register");
        controller
            .start_route("listener")
            .await
            .expect("listener route should start");
        controller
            .routes
            .get_mut("listener")
            .expect("listener route exists")
            .from_uri = "http://0.0.0.0:8080".to_string();

        // Authenticated candidate: its own plan alone would pass the gate,
        // so the rejection can only come from the pre-registered sibling's
        // Public plan — proving the late gate aggregates sibling plans.
        let err = controller
            .add_route(
                RouteDefinition::new("http://0.0.0.0:8080/late-auth", vec![])
                    .with_route_id("late-auth")
                    .with_security_policy(SecurityPolicyConfig::new(AllowPolicy))
                    .with_security_authenticator(Arc::new(StubAuth))
                    .with_provider_registry(sole_provider()),
            )
            .await
            .expect_err("sibling Public plan must trip the late gate");

        let message = err.to_string();
        assert!(
            message.contains("0.0.0.0:8080"),
            "error must name bind: {message}"
        );
        assert!(
            message.contains("late-auth"),
            "error must name route: {message}"
        );
        assert!(!controller.routes.contains_key("late-auth"));

        controller
            .stop_route("listener")
            .await
            .expect("listener route should stop");
    }

    #[tokio::test]
    async fn late_registration_with_generation_also_gates() {
        let mut controller = running_listener("http://0.0.0.0:8090").await;

        let err = controller
            .add_route_with_generation(
                RouteDefinition::new("http://0.0.0.0:8090/hot", vec![]).with_route_id("hot-public"),
                1,
            )
            .await
            .expect_err("hot-reload insertion must hit the same gate");

        let message = err.to_string();
        assert!(
            message.contains("0.0.0.0:8090"),
            "error must name bind: {message}"
        );
        assert!(!controller.routes.contains_key("hot-public"));

        controller
            .stop_route("listener")
            .await
            .expect("listener route should stop");
    }

    #[tokio::test]
    async fn late_public_route_nonloopback_rejected() {
        let mut controller = running_listener("http://0.0.0.0:8080").await;

        let err = controller
            .add_route(
                RouteDefinition::new("http://0.0.0.0:8080/late", vec![])
                    .with_route_id("late-public"),
            )
            .await
            .expect_err("unacknowledged non-loopback public route must be rejected");

        let message = err.to_string();
        assert!(
            message.contains("0.0.0.0:8080"),
            "error must name bind: {message}"
        );
        assert!(
            message.contains("late-public"),
            "error must name route: {message}"
        );
        assert!(!controller.routes.contains_key("late-public"));
        assert!(
            controller
                .routes_for_endpoint("http://0.0.0.0:8080/late")
                .is_empty()
        );

        controller
            .stop_route("listener")
            .await
            .expect("listener route should stop");
    }

    #[tokio::test]
    async fn late_route_loopback_public_accepted() {
        let mut controller = running_listener("http://127.0.0.1:0").await;
        let dispatched = Arc::new(AtomicUsize::new(0));
        let dispatched_by_route = Arc::clone(&dispatched);
        let processor = BoxProcessor::from_fn(move |exchange: Exchange| {
            dispatched_by_route.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(exchange) })
        });

        controller
            .add_route(
                RouteDefinition::new(
                    "http://127.0.0.1:0/late",
                    vec![BuilderStep::Processor(OpaqueProcessor(processor))],
                )
                .with_route_id("late-public"),
            )
            .await
            .expect("loopback public route should register");

        assert!(controller.routes.contains_key("late-public"));
        assert_eq!(
            controller.routes_for_endpoint("http://127.0.0.1:0/late"),
            vec!["late-public".to_string()]
        );

        let pipeline = {
            let managed = controller
                .routes
                .get("late-public")
                .expect("late route exists");
            crate::lifecycle::adapters::pipeline_runtime::get_pipeline(&managed.pipeline)
        };
        pipeline
            .oneshot(Exchange::new(Message::new("late")))
            .await
            .expect("late route dispatch should succeed");
        assert_eq!(dispatched.load(Ordering::SeqCst), 1);

        controller
            .stop_route("listener")
            .await
            .expect("listener route should stop");
    }
}

#[cfg(test)]
mod dispatch_enforcement {
    //! ADR-0061 Task 2.9 strict-mode dispatch enforcement: a non-Public plan
    //! requires the kernel-minted typed carrier on the Exchange. The
    //! pre-pipeline check (route_controller_trait) denies a carrier-less
    //! Exchange BEFORE the pipeline runs; the transport renders the denial
    //! in its own idiom via reply_tx.

    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    use camel_api::security_policy::{
        AccessMode, AuthContext, AuthorizationDecision, CredentialSource, Principal,
        RouteSecurityPlan, SecurityPolicy, SecurityPolicyConfig, TransportId,
    };
    use camel_api::{Body, Exchange, Message, OpaqueProcessor};
    use camel_auth::credential_source::ExtractedToken;
    use camel_auth::{ProviderEntry, ProviderRegistry, TokenAuthenticator};

    struct AllowPolicy;

    #[async_trait::async_trait]
    impl SecurityPolicy for AllowPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
            _auth: &AuthContext<'_>,
        ) -> Result<AuthorizationDecision, CamelError> {
            Ok(AuthorizationDecision::Granted {
                principal: Principal {
                    subject: "tester".into(),
                    issuer: "test".into(),
                    audience: vec![],
                    scopes: vec![],
                    roles: vec![],
                    claims: serde_json::Value::Null,
                },
            })
        }
    }

    struct StubAuth;

    #[async_trait::async_trait]
    impl TokenAuthenticator for StubAuth {
        async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
            Ok(Principal {
                subject: "tester".into(),
                issuer: "test".into(),
                audience: vec![],
                scopes: vec![],
                roles: vec![],
                claims: serde_json::Value::Null,
            })
        }
    }

    fn sole_provider() -> Arc<ProviderRegistry> {
        let registry = ProviderRegistry::new();
        registry.register(
            "idp-a",
            ProviderEntry {
                authenticator: Arc::new(StubAuth),
                audience_binding: None,
            },
        );
        Arc::new(registry)
    }

    /// Probe consumer: sends ONE Exchange (optionally carrying a pre-minted
    /// carrier) through the route channel and records the pipeline's reply.
    struct DispatchProbeConsumer {
        outcome: Arc<std::sync::Mutex<Option<Result<Exchange, CamelError>>>>,
        carrier: Option<camel_auth::AuthenticatedPrincipal>,
    }

    #[async_trait::async_trait]
    impl Consumer for DispatchProbeConsumer {
        async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
            let mut exchange = Exchange::new(Message::new("probe"));
            if let Some(principal) = &self.carrier {
                camel_auth::install_carrier(&mut exchange, principal);
            }
            let outcome = ctx.send_and_wait(exchange).await;
            *self.outcome.lock().expect("probe lock") = Some(outcome);
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    struct DispatchProbeEndpoint {
        uri: String,
        outcome: Arc<std::sync::Mutex<Option<Result<Exchange, CamelError>>>>,
        carrier: Option<camel_auth::AuthenticatedPrincipal>,
    }

    impl Endpoint for DispatchProbeEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }
        fn create_consumer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
        ) -> Result<Box<dyn Consumer>, CamelError> {
            Ok(Box::new(DispatchProbeConsumer {
                outcome: Arc::clone(&self.outcome),
                carrier: self.carrier.clone(),
            }))
        }
        fn create_producer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Err(CamelError::ProcessorError(
                "dispatch probe does not support producers".into(),
            ))
        }
    }

    /// Poll the probe outcome until the pipeline replies (or panic).
    async fn probe_outcome(
        outcome: &Arc<std::sync::Mutex<Option<Result<Exchange, CamelError>>>>,
    ) -> Result<Exchange, CamelError> {
        for _ in 0..400 {
            let found = {
                let mut guard = outcome.lock().expect("probe lock");
                guard.take()
            };
            if let Some(result) = found {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("probe outcome never arrived: pipeline did not reply within 10s");
    }

    #[tokio::test]
    async fn dispatch_enforcement_denies_nonpublic_without_carrier() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let outcome: Arc<std::sync::Mutex<Option<Result<Exchange, CamelError>>>> =
            Arc::new(std::sync::Mutex::new(None));
        {
            let mut guard = registry.lock().expect("registry lock");
            guard.register(Arc::new(DispatchProbeComponent {
                outcome: Arc::clone(&outcome),
                carrier: None,
            }));
        }
        let mut controller = DefaultRouteController::new(
            registry,
            Arc::new(camel_api::NoopPlatformService::default()),
        );

        // Counting pipeline step: a granted dispatch must run exactly once;
        // the denied dispatch must never reach it.
        let dispatched = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&dispatched);
        let processor = BoxProcessor::from_fn(move |mut exchange: Exchange| {
            // Fn closure: clone the Arc for the owned future on each call.
            let counter = Arc::clone(&counter);
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                exchange.input.body = Body::Text("ran".into());
                Ok(exchange)
            })
        });

        // Loopback http bind: Public-silently-allowed; the Authorized plan
        // (policy + provider) makes the route non-Public.
        controller
            .add_route(
                RouteDefinition::new(
                    "http://127.0.0.1:1/secured",
                    vec![BuilderStep::Processor(OpaqueProcessor(processor))],
                )
                .with_route_id("dispatch-deny-probe")
                .with_security_policy(SecurityPolicyConfig::new(AllowPolicy))
                .with_security_authenticator(Arc::new(StubAuth))
                .with_provider_registry(sole_provider()),
            )
            .await
            .expect("add_route");
        controller
            .start_route("dispatch-deny-probe")
            .await
            .expect("start_route");
        // rc-jxkj: fresh controller → cohort gate closed; the denial this
        // test asserts happens after the gate, so open it.
        controller.cohort.open();

        // The probe's carrier-less Exchange is denied BEFORE the pipeline.
        let result = probe_outcome(&outcome).await;
        match result {
            Err(CamelError::Unauthenticated(message)) => {
                assert!(
                    message.contains("no authenticated principal"),
                    "denial must name the missing carrier: {message}"
                );
            }
            other => panic!("expected Unauthenticated denial, got: {other:?}"),
        }
        assert_eq!(
            dispatched.load(Ordering::SeqCst),
            0,
            "the pipeline step must never run for a carrier-less Exchange"
        );

        controller
            .stop_route("dispatch-deny-probe")
            .await
            .expect("stop_route");
    }

    #[tokio::test]
    async fn dispatch_enforcement_grants_nonpublic_with_carrier() {
        // Control for the denial test: the same non-Public route with a
        // kernel-minted carrier dispatches into the pipeline — the strict
        // check gates, it does not block granted flows.
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let outcome: Arc<std::sync::Mutex<Option<Result<Exchange, CamelError>>>> =
            Arc::new(std::sync::Mutex::new(None));
        // Mint a real carrier through the kernel for this route's provider.
        let providers = sole_provider();
        let plan = RouteSecurityPlan {
            access_mode: AccessMode::Authenticated,
            provider_ref: Some("idp-a".to_string()),
            transport: TransportId::Http,
            credential_sources: vec![CredentialSource::AuthorizationHeader],
            audience_binding: None,
        };
        let credentials = ExtractedToken {
            token: "t-a".to_string(),
            source: CredentialSource::AuthorizationHeader,
        };
        let carrier = camel_auth::kernel_authenticate(&plan, &providers, &credentials)
            .await
            .expect("kernel mints the carrier");
        {
            let mut guard = registry.lock().expect("registry lock");
            guard.register(Arc::new(DispatchProbeComponent {
                outcome: Arc::clone(&outcome),
                carrier: Some(carrier),
            }));
        }
        let mut controller = DefaultRouteController::new(
            registry,
            Arc::new(camel_api::NoopPlatformService::default()),
        );

        let dispatched = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&dispatched);
        let processor = BoxProcessor::from_fn(move |mut exchange: Exchange| {
            // Fn closure: clone the Arc for the owned future on each call.
            let counter = Arc::clone(&counter);
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                exchange.input.body = Body::Text("ran".into());
                Ok(exchange)
            })
        });

        controller
            .add_route(
                RouteDefinition::new(
                    "http://127.0.0.1:1/secured",
                    vec![BuilderStep::Processor(OpaqueProcessor(processor))],
                )
                .with_route_id("dispatch-grant-probe")
                .with_security_policy(SecurityPolicyConfig::new(AllowPolicy))
                .with_security_authenticator(Arc::new(StubAuth))
                .with_provider_registry(sole_provider()),
            )
            .await
            .expect("add_route");
        controller
            .start_route("dispatch-grant-probe")
            .await
            .expect("start_route");
        // rc-jxkj: fresh controller → cohort gate closed; open for dispatch.
        controller.cohort.open();

        let result = probe_outcome(&outcome)
            .await
            .expect("a carrier-carrying Exchange must dispatch into the pipeline");
        assert_eq!(
            dispatched.load(Ordering::SeqCst),
            1,
            "the pipeline step must run exactly once for a granted dispatch"
        );
        assert!(
            matches!(result.input.body, Body::Text(ref s) if s == "ran"),
            "the step's mutation must be visible on the reply"
        );

        controller
            .stop_route("dispatch-grant-probe")
            .await
            .expect("stop_route");
    }

    /// Component vending [`DispatchProbeEndpoint`] under the `http` scheme
    /// (scheme choice matters: plan compilation only classifies listener
    /// schemes, so the route actually gets a compiled non-Public plan).
    struct DispatchProbeComponent {
        outcome: Arc<std::sync::Mutex<Option<Result<Exchange, CamelError>>>>,
        carrier: Option<camel_auth::AuthenticatedPrincipal>,
    }

    impl Component for DispatchProbeComponent {
        fn scheme(&self) -> &str {
            "http"
        }
        fn create_endpoint(
            &self,
            uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(DispatchProbeEndpoint {
                uri: uri.to_string(),
                outcome: Arc::clone(&self.outcome),
                carrier: self.carrier.clone(),
            }))
        }
    }
}

/// rc-jxkj cohort gate — drain-loop parking tests. A fresh controller's
/// gate is closed, so a started route parks envelope dispatch until
/// `controller.cohort.open()` (the test stand-in for the startup cohort's
/// activate step wired in Task 1.4).
mod drain_gate {
    use super::*;
    use camel_api::CamelError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Dispatch observer: counts pipeline invocations.
    fn counting_processor(count: Arc<AtomicUsize>) -> BoxProcessor {
        BoxProcessor::from_fn(move |exchange: Exchange| {
            let count = count.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(exchange)
            })
        })
    }

    /// Timer route whose consumer never fires inside the test window
    /// (backpressure-test idiom): only manually injected envelopes reach
    /// the drain loop.
    fn parked_route(route_id: &str, count: &Arc<AtomicUsize>) -> RouteDefinition {
        RouteDefinition::new(
            "timer:tick?period=10000&delay=10000",
            vec![BuilderStep::Processor(OpaqueProcessor(counting_processor(
                Arc::clone(count),
            )))],
        )
        .with_route_id(route_id)
    }

    /// Aggregate variant of `parked_route` (force-agg idiom): the
    /// force-completion config makes the route aggregate-split, so
    /// start/restart dispatch through `start_aggregate_route` and
    /// injected envelopes park at the aggregate drain gate (the
    /// `envelope_opt` arm). The counter sits before the Aggregate step
    /// because a parked exchange (complete_when_size unreachable) never
    /// reaches steps after the aggregator.
    fn aggregate_parked_route(route_id: &str, count: &Arc<AtomicUsize>) -> RouteDefinition {
        let agg_config = camel_api::AggregatorConfig::correlate_by("key")
            .complete_when_size(10)
            .force_completion_on_stop(true)
            .build()
            .unwrap();
        RouteDefinition::new(
            "timer:tick?period=10000&delay=10000",
            vec![
                BuilderStep::DeclarativeSetHeader {
                    key: "key".into(),
                    value: camel_api::ValueSourceDef::Literal(camel_api::Value::String(
                        "order-1".into(),
                    )),
                },
                BuilderStep::Processor(OpaqueProcessor(counting_processor(Arc::clone(count)))),
                BuilderStep::Aggregate { config: agg_config },
            ],
        )
        .with_route_id(route_id)
    }

    fn sender_for(
        controller: &DefaultRouteController,
        route_id: &str,
    ) -> tokio::sync::mpsc::Sender<ExchangeEnvelope> {
        controller
            .routes
            .get(route_id)
            .and_then(|r| r.channel_sender.clone())
            .expect("channel sender should exist after start")
    }

    async fn yield_pump() {
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    /// Polls until the counting processor observes a dispatch, failing
    /// past the deadline (the gate is already open when this is called).
    async fn await_dispatch(count: &Arc<AtomicUsize>, topology: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while count.load(Ordering::SeqCst) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "dispatch did not occur after gate open ({topology})"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn drain_gate_concurrent_parks_until_activation() {
        let mut controller = build_controller_with_components();
        let count = Arc::new(AtomicUsize::new(0));

        let route = parked_route("rt-gate-conc", &count)
            .with_concurrency(ConcurrencyModel::Concurrent { max: Some(2) });
        controller.add_route(route).await.unwrap();
        controller.start_route("rt-gate-conc").await.unwrap();
        yield_pump().await;

        sender_for(&controller, "rt-gate-conc")
            .send(ExchangeEnvelope {
                exchange: Exchange::new(Message::new("A")),
                reply_tx: None,
            })
            .await
            .unwrap();

        yield_pump().await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "concurrent drain must park the envelope while the gate is closed"
        );

        controller.cohort.open();
        await_dispatch(&count, "concurrent").await;

        controller.stop_route("rt-gate-conc").await.unwrap();
    }

    #[tokio::test]
    async fn drain_gate_sequential_parks_until_activation() {
        let mut controller = build_controller_with_components();
        let count = Arc::new(AtomicUsize::new(0));

        let route = parked_route("rt-gate-seq", &count);
        controller.add_route(route).await.unwrap();
        controller.start_route("rt-gate-seq").await.unwrap();
        yield_pump().await;

        sender_for(&controller, "rt-gate-seq")
            .send(ExchangeEnvelope {
                exchange: Exchange::new(Message::new("A")),
                reply_tx: None,
            })
            .await
            .unwrap();

        yield_pump().await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "sequential drain must park the envelope while the gate is closed"
        );

        controller.cohort.open();
        await_dispatch(&count, "sequential").await;

        controller.stop_route("rt-gate-seq").await.unwrap();
    }

    #[tokio::test]
    async fn drain_gate_restart_parks_until_activation() {
        let mut controller = build_controller_with_components();
        let count = Arc::new(AtomicUsize::new(0));

        // Aggregate topology: start/restart dispatch through
        // start_aggregate_route, so the restart leg exercises the
        // aggregate drain gate rather than a second pass over the
        // Sequential site.
        let route = aggregate_parked_route("rt-gate-restart", &count);
        controller.add_route(route).await.unwrap();
        controller.start_route("rt-gate-restart").await.unwrap();
        yield_pump().await;

        // First start parks the envelope at the aggregate gate; stop
        // cancels the pipeline token, which drops it (reply_tx is None —
        // nothing to resolve) and runs the cancel-arm cleanup
        // (force_complete_all + late drain).
        sender_for(&controller, "rt-gate-restart")
            .send(ExchangeEnvelope {
                exchange: Exchange::new(Message::new("A")),
                reply_tx: None,
            })
            .await
            .unwrap();
        yield_pump().await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "first start must park the envelope while the gate is closed"
        );

        controller.stop_route("rt-gate-restart").await.unwrap();

        // Restart with the gate still held closed: the fresh aggregate
        // drain loop must park again.
        controller.start_route("rt-gate-restart").await.unwrap();
        yield_pump().await;
        sender_for(&controller, "rt-gate-restart")
            .send(ExchangeEnvelope {
                exchange: Exchange::new(Message::new("B")),
                reply_tx: None,
            })
            .await
            .unwrap();
        yield_pump().await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "restarted aggregate drain must park the envelope while the gate is held closed"
        );

        controller.cohort.open();
        await_dispatch(&count, "restart").await;

        controller.stop_route("rt-gate-restart").await.unwrap();
    }

    #[tokio::test]
    async fn drain_gate_parked_exits_on_cancel() {
        let mut controller = build_controller_with_components();
        let count = Arc::new(AtomicUsize::new(0));

        let route = parked_route("rt-gate-cancel", &count);
        controller.add_route(route).await.unwrap();
        controller.start_route("rt-gate-cancel").await.unwrap();
        yield_pump().await;

        // InOut-style envelope: the waiter keeps the reply receiver.
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        sender_for(&controller, "rt-gate-cancel")
            .send(ExchangeEnvelope {
                exchange: Exchange::new(Message::new("A")),
                reply_tx: Some(reply_tx),
            })
            .await
            .unwrap();
        yield_pump().await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "envelope must be parked (no dispatch) before cancel"
        );

        // Cancel the managed pipeline token (parent of the drain loop's
        // child token) and take the drain task's JoinHandle (stop_route
        // tolerates the missing handle — handle_is_running falls back to
        // the still-running consumer task).
        let (pipeline_cancel, pipeline_handle) = {
            let r = controller
                .routes
                .get_mut("rt-gate-cancel")
                .expect("route exists while started");
            (
                r.pipeline_cancel_token.clone(),
                r.pipeline_handle
                    .take()
                    .expect("pipeline handle while started"),
            )
        };
        pipeline_cancel.cancel();

        tokio::time::timeout(Duration::from_secs(2), pipeline_handle)
            .await
            .expect("parked drain task must exit after pipeline cancel")
            .expect("drain task must not panic");

        // send_and_wait semantics: dropping the parked envelope's reply_tx
        // resolves the waiter with ChannelClosed — existing mapping, no new
        // error variant.
        let reply = tokio::time::timeout(Duration::from_secs(2), reply_rx)
            .await
            .expect("reply must resolve after the drain task drops the envelope")
            .map_err(|_| CamelError::ChannelClosed)
            .unwrap_err();
        assert!(matches!(reply, CamelError::ChannelClosed));

        controller.stop_route("rt-gate-cancel").await.unwrap();
    }
}

// ============================================================================
// rc-hrm1.3: controller-path metrics wiring regression tests
//
// The route controller hands components their collector through
// `ControllerComponentContext` (see `resolve_steps`/`build_managed_route` in
// route_controller.rs), seeded from `tracer_metrics`. These tests pin that
// the controller path resolves the REGISTERED collector — the shared
// late-bound `MetricsHandle` built by `CamelContextBuilder::build()` —
// instead of the `NoOpMetrics` fallback a pre-wiring build leaves behind.
// ============================================================================

/// Records every `MetricsCollector` call as a `"method:primary-arg"` string.
struct RecordingCollector {
    calls: Arc<std::sync::Mutex<Vec<String>>>,
}

impl MetricsCollector for RecordingCollector {
    fn record_exchange_duration(&self, route_id: &str, _duration: Duration) {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("record_exchange_duration:{route_id}"));
    }
    fn increment_errors(&self, route_id: &str, _error_type: &str) {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("increment_errors:{route_id}"));
    }
    fn increment_exchanges(&self, route_id: &str) {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("increment_exchanges:{route_id}"));
    }
    fn set_queue_depth(&self, route_id: &str, _depth: usize) {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("set_queue_depth:{route_id}"));
    }
    fn record_circuit_breaker_change(&self, route_id: &str, _from: &str, _to: &str) {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("record_circuit_breaker_change:{route_id}"));
    }
    fn record_histogram(&self, name: &str, _value: f64, _labels: &[(&str, &str)]) {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("record_histogram:{name}"));
    }
    fn record_counter(&self, name: &str, _value: f64, _labels: &[(&str, &str)]) {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("record_counter:{name}"));
    }
}

/// Lifecycle service exposing the [`RecordingCollector`] through
/// `as_metrics_collector`, mirroring how otel/prometheus services register.
struct RecordingLifecycle {
    calls: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl camel_api::Lifecycle for RecordingLifecycle {
    fn name(&self) -> &str {
        "recording-lifecycle"
    }
    async fn start(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
    fn as_metrics_collector(&self) -> Option<Arc<dyn MetricsCollector>> {
        Some(Arc::new(RecordingCollector {
            calls: Arc::clone(&self.calls),
        }))
    }
}

/// Shared slot where the probe component stores the collector it resolved at
/// step-resolution time through its `RuntimeObservability`.
type CapturedCollector = Arc<std::sync::Mutex<Option<Arc<dyn MetricsCollector>>>>;

fn recording_has(calls: &Arc<std::sync::Mutex<Vec<String>>>, entry: &str) -> bool {
    calls.lock().expect("calls lock").iter().any(|e| e == entry)
}

/// Pass-through producer that counts the exchanges it processes.
#[derive(Clone)]
struct CountingPassthrough {
    seen: Arc<AtomicU64>,
}

impl tower::Service<Exchange> for CountingPassthrough {
    type Response = Exchange;
    type Error = CamelError;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), CamelError>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        self.seen.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok(exchange) })
    }
}

/// Endpoint whose producer activation captures the collector supplied via
/// `RuntimeObservability` — the exact path a real component uses.
struct ProbeEndpoint {
    slot: CapturedCollector,
    seen: Arc<AtomicU64>,
}

impl Endpoint for ProbeEndpoint {
    fn uri(&self) -> &str {
        "probe:sink"
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::ProcessorError(
            "probe does not support consumers".into(),
        ))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        *self.slot.lock().expect("probe slot lock") = Some(rt.metrics());
        Ok(BoxProcessor::new(CountingPassthrough {
            seen: Arc::clone(&self.seen),
        }))
    }
}

/// Minimal test component vending [`ProbeEndpoint`] under the `probe` scheme.
struct ProbeComponent {
    slot: CapturedCollector,
    seen: Arc<AtomicU64>,
}

impl Component for ProbeComponent {
    fn scheme(&self) -> &str {
        "probe"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(ProbeEndpoint {
            slot: Arc::clone(&self.slot),
            seen: Arc::clone(&self.seen),
        }))
    }
}

/// Enable the tracer pipeline at the CONTROLLER level (actor handle).
///
/// `CamelContext::set_tracer_config` is deliberately avoided: pre-change it
/// reverse-injects the context's own collector snapshot into the config,
/// which would hand the controller a live collector even without the shared
/// handle wiring these regressions pin — invalidating the red state.
async fn enable_tracer_pipeline_at_controller(ctx: &crate::context::CamelContext) {
    ctx.runtime_execution_handle()
        .controller
        .set_tracer_config(TracerConfig {
            enabled: true,
            ..Default::default()
        })
        .await
        .expect("enable tracer pipeline");
}

/// rc-hrm1.3 (compile-level): `TracerConfig` constructs with no collector
/// reference, and its plain derived Debug needs no redaction. Lives here,
/// not in config.rs, so the deleted field's identifier has zero textual
/// presence in the config source.
#[test]
fn tracer_config_carries_no_collector_field() {
    let tracer_config = TracerConfig {
        enabled: true,
        tracing_enabled_explicit: false,
        pipeline_enabled: true,
        detail_level: DetailLevel::default(),
        outputs: crate::shared::observability::domain::TracerOutputs::default(),
        metrics_levers: crate::shared::observability::domain::MetricsLeversConfig::default(),
    };
    let dbg = format!("{tracer_config:?}");
    assert!(dbg.contains("TracerConfig"));
    assert!(!dbg.contains("metrics_collector"));
}

/// Drive a timer→probe route through a built context until the probe producer
/// has processed its single exchange.
async fn run_probe_route_once(_ctx: &crate::context::CamelContext, seen: &Arc<AtomicU64>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while seen.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "probe producer never received an exchange"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn controller_path_receives_registered_collector() {
    let calls: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let slot: CapturedCollector = Arc::default();
    let seen = Arc::new(AtomicU64::new(0));

    let mut ctx = crate::context::CamelContext::builder()
        .with_lifecycle(RecordingLifecycle {
            calls: Arc::clone(&calls),
        })
        .build()
        .await
        .expect("build context");
    ctx.register_component(ProbeComponent {
        slot: Arc::clone(&slot),
        seen: Arc::clone(&seen),
    });
    ctx.register_component(camel_component_timer::TimerComponent::new());

    enable_tracer_pipeline_at_controller(&ctx).await;

    ctx.add_route_definition(
        RouteDefinition::new(
            "timer:tick?period=10&repeatCount=1",
            vec![BuilderStep::To("probe:sink".into())],
        )
        .with_route_id("probe-route"),
    )
    .await
    .expect("add route");

    ctx.start().await.expect("start context");
    run_probe_route_once(&ctx, &seen).await;

    // THE rc-hrm1.3 gate: the collector captured through the controller path
    // must reach the REGISTERED RecordingCollector. Pre-wiring the captured
    // collector is the NoOp fallback — the write goes into the void and the
    // recording list does not grow.
    let captured = slot
        .lock()
        .expect("probe slot lock")
        .clone()
        .expect("probe producer must capture a collector at resolution");
    let before = calls.lock().expect("calls lock").len();
    captured.increment_exchanges("probe");
    {
        let recorded = calls.lock().expect("calls lock");
        assert!(
            recorded.len() > before,
            "captured collector wrote into the void; recorded: {recorded:?}"
        );
    }

    // The same captured path must also carry the real per-exchange emissions
    // of the enabled tracer pipeline (the tracer records after the wrapped
    // producer future resolves, so poll briefly).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !recording_has(&calls, "increment_exchanges:probe-route")
        || !recording_has(&calls, "record_exchange_duration:probe-route")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "tracer exchange emissions never reached the recording collector; \
             recorded: {:?}",
            calls.lock().expect("calls lock")
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let _ = ctx.stop().await;
}

#[tokio::test]
async fn late_registration_after_build_observed() {
    let calls: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let seen = Arc::new(AtomicU64::new(0));

    // Built with NO metrics service anywhere near the builder — the
    // collector registers on the BUILT context, after construction.
    let mut ctx = crate::context::CamelContext::builder()
        .build()
        .await
        .expect("build context");
    ctx.register_component(ProbeComponent {
        slot: Arc::default(),
        seen: Arc::clone(&seen),
    });
    ctx.register_component(camel_component_timer::TimerComponent::new());

    // Late registration on the built context.
    ctx = ctx.with_lifecycle(RecordingLifecycle {
        calls: Arc::clone(&calls),
    });

    enable_tracer_pipeline_at_controller(&ctx).await;

    ctx.add_route_definition(
        RouteDefinition::new(
            "timer:tick?period=10&repeatCount=1",
            vec![BuilderStep::To("probe:sink".into())],
        )
        .with_route_id("late-route"),
    )
    .await
    .expect("add route");

    ctx.start().await.expect("start context");
    run_probe_route_once(&ctx, &seen).await;

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !recording_has(&calls, "increment_exchanges:late-route")
        || !recording_has(&calls, "record_exchange_duration:late-route")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "late-registered collector never observed the exchange; \
             recorded: {:?}",
            calls.lock().expect("calls lock")
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let _ = ctx.stop().await;
}

/// Task 2.1 follow-up (inter-phase review): pin the
/// `set_tracer_config` → `tracer_gating` derivation — the sole bridge
/// from the config truth table to runtime pipeline gating.
#[test]
fn set_tracer_config_derives_gating() {
    let mut controller = build_controller();
    assert!(!controller.tracer_gating.pipeline_enabled);
    controller.set_tracer_config(&TracerConfig {
        enabled: false,
        pipeline_enabled: true, // exporter raised the pipeline (otel/prom on)
        tracing_enabled_explicit: true,
        detail_level: DetailLevel::default(),
        outputs: crate::shared::observability::domain::TracerOutputs::default(),
        metrics_levers: crate::shared::observability::domain::MetricsLeversConfig {
            enabled: true,
            exchange: false,
            duration: false,
            components: true,
        },
    });
    assert!(
        controller.tracer_gating.pipeline_enabled,
        "pipeline follows enabled OR raised pipeline_enabled"
    );
    assert!(
        !controller.tracer_gating.spans_enabled,
        "spans follow enabled alone"
    );
    assert!(
        !controller.tracer_gating.levers.exchange
            && !controller.tracer_gating.levers.duration
            && controller.tracer_gating.levers.components,
        "levers snapshot verbatim"
    );
}
