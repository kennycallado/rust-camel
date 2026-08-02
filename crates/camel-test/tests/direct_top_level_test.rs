//! Coverage for `to: direct:` as a top-level route step, complementing the
//! regression test in `loop_test.rs`. These tests close the gaps flagged during
//! review of PR #26 (fix for issue #25):
//!   1. sequential multicast wrapping a `to: direct:` branch (inline path),
//!   2. ConsumerRestart re-registering the direct consumer in the registry,
//!   3. error propagation back through a top-level `to: direct:` step (b′).

use std::time::Duration;

use camel_api::{Exchange, Message, RouteStatus, RuntimeCommand};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::LanguageRegistryError;
use camel_language_rhai::RhaiLanguage;
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(camel_component_api::NoOpComponentContext)
}

fn ensure_rhai_registered(ctx: &mut camel_core::CamelContext) {
    match ctx.register_language("rhai", Box::new(RhaiLanguage::new())) {
        Ok(()) | Err(LanguageRegistryError::AlreadyRegistered { .. }) => {}
    }
}

/// Send one exchange via a fresh DirectProducer, swallowing the result. Used
/// while the consumer is stopped, when the producer is expected to error fast.
/// A fresh producer per call avoids poisoning a long-lived producer's
/// poll_ready when the consumer route is stopped mid-test.
async fn send_to_direct_ignoring_error(
    h: &CamelTestContext,
    endpoint_uri: &str,
    exchange: Exchange,
) {
    let producer = {
        let ctx = h.ctx().lock().await;
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered");
        let endpoint = component
            .create_endpoint(endpoint_uri, &*ctx)
            .expect("failed to create direct endpoint");
        endpoint
            .create_producer(test_rt(), &producer_ctx)
            .expect("failed to create direct producer")
    };
    let _ = producer.oneshot(exchange).await;
}

/// Send an exchange, retrying with a fresh producer until the consumer is
/// registered (or `timeout` elapses). Removes startup/restart registration
/// races without relying on fixed sleeps.
async fn send_to_direct_until_delivered(
    h: &CamelTestContext,
    endpoint_uri: &str,
    exchange: Exchange,
    timeout: Duration,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let producer = {
            let ctx = h.ctx().lock().await;
            let producer_ctx = ctx.producer_context();
            let registry = ctx.registry();
            let component = registry
                .get("direct")
                .expect("direct component not registered");
            let endpoint = component
                .create_endpoint(endpoint_uri, &*ctx)
                .expect("failed to create direct endpoint");
            endpoint
                .create_producer(test_rt(), &producer_ctx)
                .expect("failed to create direct producer")
        };
        match producer.oneshot(exchange.clone()).await {
            Ok(_) => return,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("failed to send exchange to {endpoint_uri} within {timeout:?}: {e}"),
        }
    }
}

async fn route_status(h: &CamelTestContext, route_id: &str) -> Option<RouteStatus> {
    let ctx = h.ctx().lock().await;
    let s = ctx
        .runtime_route_status(route_id)
        .await
        .expect("runtime route status query failed")?;
    Some(match s.as_str() {
        "Stopped" | "Registered" => RouteStatus::Stopped,
        "Starting" => RouteStatus::Starting,
        "Started" => RouteStatus::Started,
        "Stopping" => RouteStatus::Stopping,
        "Suspended" => RouteStatus::Suspended,
        "Failed" => RouteStatus::Failed("failed".to_string()),
        other => panic!("unknown route status: {other}"),
    })
}

async fn stop_route(h: &CamelTestContext, route_id: &str) {
    let runtime = {
        let ctx = h.ctx().lock().await;
        ctx.runtime()
    };
    runtime
        .execute(RuntimeCommand::StopRoute {
            route_id: route_id.to_string(),
            command_id: format!("test:stop:{route_id}"),
            causation_id: None,
        })
        .await
        .expect("failed to stop route");
}

async fn start_route(h: &CamelTestContext, route_id: &str) {
    let runtime = {
        let ctx = h.ctx().lock().await;
        ctx.runtime()
    };
    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: route_id.to_string(),
            command_id: format!("test:start:{route_id}"),
            causation_id: None,
        })
        .await
        .expect("failed to start route");
}

// ---------------------------------------------------------------------------
// Test 1: sequential multicast → direct (inline path, same family as top-level)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sequential_multicast_top_level_direct_completes() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let yaml = r#"
routes:
  - id: "seq-multicast-producer"
    from: "timer:seq?period=10&repeatCount=1"
    startup_order: 200
    steps:
      - multicast:
          aggregation: original
          steps:
            - to: "direct:seq-echo"
  - id: "seq-multicast-echo"
    from: "direct:seq-echo"
    startup_order: 50
    steps:
      - to: "mock:seq-multicast-result"
"#;

    for route in camel_dsl::parse_yaml(yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    h.mock()
        .get_endpoint("seq-multicast-result")
        .expect("mock endpoint created during route compilation")
        .await_exchanges(1, Duration::from_secs(2))
        .await;

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Test 2: ConsumerRestart re-registers the direct consumer
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn top_level_direct_consumer_restart_reregisters() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // Consumer-only route; we drive it with a fresh producer per send so that
    // stopping the consumer cannot poison a long-lived producer's poll_ready.
    let route = RouteBuilder::from("direct:restart-echo")
        .route_id("restart-echo")
        .to("mock:restart-result")
        .build()
        .unwrap();
    h.add_route(route).await.unwrap();
    h.start().await;

    let mock = h
        .mock()
        .get_endpoint("restart-result")
        .expect("mock endpoint created during route compilation");

    // 1. First send reaches the registered consumer. Retry covers the startup
    //    registration race without a fixed sleep.
    send_to_direct_until_delivered(
        &h,
        "direct:restart-echo",
        Exchange::new(Message::new("first")),
        Duration::from_secs(2),
    )
    .await;
    mock.await_exchanges(1, Duration::from_secs(2)).await;
    assert_eq!(
        route_status(&h, "restart-echo").await,
        Some(RouteStatus::Started),
        "consumer route should be Started before restart"
    );

    // 2. Stop the consumer; a send now must fail fast (no consumer registered).
    stop_route(&h, "restart-echo").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        route_status(&h, "restart-echo").await,
        Some(RouteStatus::Stopped),
        "consumer route should be Stopped after stop_route"
    );
    send_to_direct_ignoring_error(
        &h,
        "direct:restart-echo",
        Exchange::new(Message::new("while-stopped")),
    )
    .await;
    let count_after_stop = mock.get_received_exchanges().await.len();
    assert_eq!(
        count_after_stop, 1,
        "no exchange may be delivered while the consumer is stopped"
    );

    // 3. Restart the consumer — it must re-register in the DirectRegistry.
    start_route(&h, "restart-echo").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        route_status(&h, "restart-echo").await,
        Some(RouteStatus::Started),
        "consumer route should be Started after start_route"
    );

    // 4. A fresh send reaches the re-registered consumer. Retry covers the
    //    restart registration race.
    send_to_direct_until_delivered(
        &h,
        "direct:restart-echo",
        Exchange::new(Message::new("after-restart")),
        Duration::from_secs(2),
    )
    .await;
    mock.await_exchanges(2, Duration::from_secs(2)).await;

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Test 3: error thrown in the direct consumer propagates back through the
// top-level `to: direct:` step (category b′), so downstream producer steps are
// skipped.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn top_level_direct_propagates_consumer_error() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_direct()
        .with_mock()
        .build()
        .await;
    {
        let mut guard = h.ctx().lock().await;
        ensure_rhai_registered(&mut guard);
    }

    let producer = RouteBuilder::from("timer:err?period=10&repeatCount=1")
        .route_id("err-producer")
        .startup_order(200)
        .to("direct:err-echo")
        .to("mock:producer-after")
        .build()
        .unwrap();

    let consumer = RouteBuilder::from("direct:err-echo")
        .route_id("err-echo")
        .startup_order(50)
        .script("rhai", r#"throw "boom""#)
        .to("mock:err-never")
        .build()
        .unwrap();

    h.add_route(producer).await.unwrap();
    h.add_route(consumer).await.unwrap();
    h.start().await;

    // The timer fires once; the consumer route throws, so neither mock is hit.
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.stop().await;

    let consumer_reached = h
        .mock()
        .get_endpoint("err-never")
        .expect("mock endpoint created during route compilation")
        .get_received_exchanges()
        .await;
    assert_eq!(
        consumer_reached.len(),
        0,
        "consumer mock must not receive exchanges when its script throws"
    );

    let producer_continued = h
        .mock()
        .get_endpoint("producer-after")
        .expect("mock endpoint created during route compilation")
        .get_received_exchanges()
        .await;
    assert_eq!(
        producer_continued.len(),
        0,
        "producer step after the failing `to: direct:` must be skipped (b′ propagation)"
    );
}
