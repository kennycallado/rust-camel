//! Task 4: SkipTo substitution at the To send point.
//!
//! Task 6: hot-reload rule consistency — a recompiled pipeline keeps
//! applying the frozen rule set.

use std::sync::Arc;
use std::sync::Mutex;

use camel_api::{Exchange, Message, RouteController};
use camel_component_direct::DirectComponent;
use camel_component_mock::MockComponent;
use camel_component_seda::SedaComponent;
use camel_core::intercept::{InterceptAction, InterceptRule, InterceptRules};
use camel_core::route::BuilderStep;
use camel_core::route_controller::DefaultRouteController;
use camel_core::{CamelContext, Registry, RouteDefinition};
use tower::ServiceExt;

use crate::common::{TEST_TIMEOUT, boot_context_with_intercept, send_to_direct, test_rt};

/// Two rules for the same URI: the first declared rule must win.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_uri_match_with_first_match_wins() {
    let rules = InterceptRules::new(vec![
        InterceptRule {
            uri: "seda:out".into(),
            action: InterceptAction::SkipTo {
                uri: "mock:first".into(),
            },
        },
        InterceptRule {
            uri: "seda:out".into(),
            action: InterceptAction::SkipTo {
                uri: "mock:second".into(),
            },
        },
    ])
    .expect("valid mock targets");
    let (mut ctx, mock) = boot_context_with_intercept(Some(rules)).await;
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To("seda:out".into())])
            .with_route_id("first-match-wins"),
    )
    .await
    .expect("route must register");
    ctx.start().await.expect("context start failed");

    send_to_direct(&ctx, "direct:in", Exchange::new(Message::new("hello"))).await;

    let first = mock
        .get_endpoint("first")
        .expect("mock endpoint 'first' must exist");
    first.await_exchanges(1, TEST_TIMEOUT).await;
    first.assert_exchange_count(1).await;
    assert!(
        mock.get_endpoint("second").is_none(),
        "the second rule must never be consulted: mock:second endpoint must not exist"
    );

    ctx.stop().await.expect("context stop failed");
}

/// An empty rule set (installed via the builder) must behave exactly like no
/// interception configuration at all: the send reaches the real destination.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_rule_set_leaves_the_send_untouched() {
    let (mut ctx_empty, mock_empty) =
        boot_context_with_intercept(Some(InterceptRules::default())).await;
    let (mut ctx_plain, mock_plain) = boot_context_with_intercept(None).await;

    for (ctx, mock) in [(&mut ctx_empty, &mock_empty), (&mut ctx_plain, &mock_plain)] {
        ctx.add_route_definition(
            RouteDefinition::new("direct:in", vec![BuilderStep::To("seda:out".into())])
                .with_route_id("send-route"),
        )
        .await
        .expect("send route must register");
        ctx.add_route_definition(
            RouteDefinition::new("seda:out", vec![BuilderStep::To("mock:arrival".into())])
                .with_route_id("consumer-route"),
        )
        .await
        .expect("consumer route must register");
        ctx.start().await.expect("context start failed");

        send_to_direct(ctx, "direct:in", Exchange::new(Message::new("sentinel"))).await;

        let arrival = mock
            .get_endpoint("arrival")
            .expect("mock endpoint 'arrival' must exist");
        arrival.await_exchanges(1, TEST_TIMEOUT).await;
        arrival.assert_exchange_count(1).await;
        let received = arrival.get_received_exchanges().await;
        assert_eq!(
            received[0].input.body.as_text(),
            Some("sentinel"),
            "consumer must record the sentinel body unchanged"
        );

        ctx.stop().await.expect("context stop failed");
    }
}

/// The source URI's component is never resolved: `kafka:` is not registered,
/// yet the route compiles because the send is substituted to `mock:orders`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skipped_uri_with_unregistered_real_component() {
    let rules = InterceptRules::new(vec![InterceptRule {
        uri: "kafka:orders".into(),
        action: InterceptAction::SkipTo {
            uri: "mock:orders".into(),
        },
    }])
    .expect("valid mock targets");
    let (mut ctx, mock) = boot_context_with_intercept(Some(rules)).await;
    // No kafka component is registered — the substitution must make the
    // route compilable anyway.
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To("kafka:orders".into())])
            .with_route_id("unregistered-source"),
    )
    .await
    .expect("route must register despite the unregistered kafka component");
    ctx.start().await.expect("context start failed");

    send_to_direct(&ctx, "direct:in", Exchange::new(Message::new("order"))).await;

    let orders = mock
        .get_endpoint("orders")
        .expect("mock endpoint 'orders' must exist");
    orders.await_exchanges(1, TEST_TIMEOUT).await;
    orders.assert_exchange_count(1).await;

    ctx.stop().await.expect("context stop failed");
}

/// A substituted target that cannot resolve is a compile error, enriched
/// with the intercept target.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skip_target_resolution_failure_is_a_compile_error() {
    let rules = InterceptRules::new(vec![InterceptRule {
        uri: "kafka:x".into(),
        action: InterceptAction::SkipTo {
            uri: "mock:x".into(),
        },
    }])
    .expect("valid mock targets");
    // No mock component registered — only direct.
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("build context");
    ctx.register_component(DirectComponent::new());
    ctx.set_intercept_rules(rules)
        .await
        .expect("rules must install before first use");

    let err = ctx
        .add_route_definition(
            RouteDefinition::new("direct:in", vec![BuilderStep::To("kafka:x".into())])
                .with_route_id("unresolvable-target"),
        )
        .await
        .expect_err("route must fail to compile: mock:x cannot resolve");
    let msg = format!("{err}");
    assert!(
        msg.contains("mock:x"),
        "error must name the intercept target, got: {msg}"
    );
    assert!(
        msg.contains("intercept target:"),
        "error must carry the intercept target enrichment, got: {msg}"
    );

    ctx.stop().await.expect("context stop failed");
}

/// The intercepted send never enqueues into the real destination: a BARRIER
/// enqueued directly afterwards is the only exchange the downstream consumer
/// sees.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skip_replaces_the_enqueue() {
    let rules = InterceptRules::new(vec![InterceptRule {
        uri: "seda:q".into(),
        action: InterceptAction::SkipTo {
            uri: "mock:q".into(),
        },
    }])
    .expect("valid mock targets");
    let (mut ctx, mock) = boot_context_with_intercept(Some(rules)).await;

    // Consumer route: seda:q → mock:sink.
    ctx.add_route_definition(
        RouteDefinition::new("seda:q", vec![BuilderStep::To("mock:sink".into())])
            .with_route_id("sink-route"),
    )
    .await
    .expect("consumer route must register");
    // Intercepted send route: direct:in → seda:q (skipped to mock:q).
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To("seda:q".into())])
            .with_route_id("send-route"),
    )
    .await
    .expect("send route must register");
    ctx.start().await.expect("context start failed");

    // Send the sentinel through the intercepted send.
    send_to_direct(&ctx, "direct:in", Exchange::new(Message::new("sentinel"))).await;

    let q = mock
        .get_endpoint("q")
        .expect("mock endpoint 'q' must exist");
    q.await_exchanges(1, TEST_TIMEOUT).await;
    q.assert_exchange_count(1).await;

    // Enqueue a distinguishable BARRIER directly into seda:q via a separately
    // created seda producer — no interception applies to this direct send.
    let seda = ctx
        .registry()
        .get("seda")
        .expect("seda component not registered");
    let producer_ctx = ctx.producer_context();
    let endpoint = seda
        .create_endpoint("seda:q", &ctx)
        .expect("failed to create seda endpoint");
    let producer = endpoint
        .create_producer(test_rt(), &producer_ctx)
        .expect("failed to create seda producer");
    producer
        .oneshot(Exchange::new(Message::new("BARRIER")))
        .await
        .expect("seda enqueue should succeed");

    // The seda consumer must deliver the BARRIER downstream — proof the
    // intercepted sentinel never entered the queue.
    let sink = mock
        .get_endpoint("sink")
        .expect("mock endpoint 'sink' must exist");
    sink.await_exchanges(1, TEST_TIMEOUT).await;
    sink.assert_exchange_count(1).await;
    let received = sink.get_received_exchanges().await;
    assert_eq!(
        received[0].input.body.as_text(),
        Some("BARRIER"),
        "sink must record the BARRIER, not the intercepted sentinel"
    );

    ctx.stop().await.expect("context stop failed");
}

/// A recompiled pipeline (hot-reload `compile_route_definition` +
/// `swap_pipeline`) must keep applying the frozen intercept rules: the
/// recompiled `To("seda:out")` still skips to `mock:q`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recompiled_pipelines_keep_the_same_rules() {
    let rules = InterceptRules::new(vec![InterceptRule {
        uri: "seda:out".into(),
        action: InterceptAction::SkipTo {
            uri: "mock:q".into(),
        },
    }])
    .expect("valid mock targets");

    let mock = MockComponent::new();
    let registry = Arc::new(Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(mock.clone()));
        guard.register(Arc::new(DirectComponent::new()));
        guard.register(Arc::new(SedaComponent::new()));
    }
    let mut controller = DefaultRouteController::new(
        Arc::clone(&registry),
        Arc::new(camel_api::NoopPlatformService::default()),
    )
    .with_intercept_rules(rules);

    let def = RouteDefinition::new("direct:in", vec![BuilderStep::To("seda:out".into())])
        .with_route_id("recompile-route");
    controller
        .add_route(def)
        .await
        .expect("route must register");
    controller
        .start_route("recompile-route")
        .await
        .expect("route must start");

    // Recompile the same definition and atomically swap the pipeline in.
    let recompiled = controller
        .compile_route_definition(
            RouteDefinition::new("direct:in", vec![BuilderStep::To("seda:out".into())])
                .with_route_id("recompile-route"),
        )
        .expect("recompile must succeed");
    controller
        .swap_pipeline("recompile-route", recompiled)
        .expect("swap must succeed");

    // Drive traffic: the recompiled pipeline must still apply the frozen
    // SkipTo rule — the exchange lands in mock:q, never in seda:out.
    let direct = registry
        .lock()
        .expect("registry lock")
        .get("direct")
        .expect("direct component registered");
    let endpoint = direct
        .create_endpoint("direct:in", &camel_component_api::NoOpComponentContext)
        .expect("failed to create direct endpoint");
    let producer = endpoint
        .create_producer(test_rt(), &camel_component_api::ProducerContext::new())
        .expect("failed to create direct producer");
    producer
        .oneshot(Exchange::new(Message::new("after-recompile")))
        .await
        .expect("direct call should succeed");

    let q = mock
        .get_endpoint("q")
        .expect("mock endpoint 'q' must exist");
    q.await_exchanges(1, TEST_TIMEOUT).await;
    q.assert_exchange_count(1).await;

    controller
        .stop_route("recompile-route")
        .await
        .expect("route stop failed");
}
