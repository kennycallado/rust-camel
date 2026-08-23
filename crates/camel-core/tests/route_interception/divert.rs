//! Task 5: DivertCopyTo composition at the To send point.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll};

use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message, Value};
use camel_component_direct::DirectComponent;
use camel_component_seda::SedaComponent;
use camel_core::route::BuilderStep;
use camel_core::{CamelContext, RouteDefinition};
use tokio::sync::{Notify, Semaphore, mpsc};
use tower::Service;

use crate::common::{
    TEST_TIMEOUT, boot_context_with_intercept, send_to_direct, send_to_direct_result,
};
use crate::support::{
    EventRealSvc, OrderLogRealSvc, OrdinalParkSvc, ReadyFailingRealSvc, StubComponent,
    await_signal, boot_stub_context, capture_tracing, divert_rules, real_boom_producer,
};

/// A divert on `to: seda:out` delivers the copy to `mock:tap` AND the real
/// message to the running `from: seda:out` consumer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn divert_delivers_both_copy_and_real_message() {
    let (mut ctx, mock) =
        boot_context_with_intercept(Some(divert_rules("seda:out", "mock:tap"))).await;
    // Consumer route: seda:out → mock:arrival (await-aware arrival via the
    // mock endpoint's notify-aware count primitive).
    ctx.add_route_definition(
        RouteDefinition::new("seda:out", vec![BuilderStep::To("mock:arrival".into())])
            .with_route_id("consumer-route"),
    )
    .await
    .expect("consumer route must register");
    // Intercepted send route: direct:in → seda:out (copied to mock:tap).
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To("seda:out".into())])
            .with_route_id("send-route"),
    )
    .await
    .expect("send route must register");
    ctx.start().await.expect("context start failed");

    send_to_direct(&ctx, "direct:in", Exchange::new(Message::new("hello"))).await;

    // Await the consumer's arrival notification for the real message.
    let arrival = mock
        .get_endpoint("arrival")
        .expect("mock endpoint 'arrival' must exist");
    arrival.await_exchanges(1, TEST_TIMEOUT).await;
    arrival.assert_exchange_count(1).await;
    let received = arrival.get_received_exchanges().await;
    assert_eq!(received[0].input.body.as_text(), Some("hello"));

    // Stop the route: the composed lifecycle drains the in-flight copy
    // before this returns, so mock:tap must have recorded the clone.
    ctx.stop().await.expect("context stop failed");

    let tap = mock
        .get_endpoint("tap")
        .expect("mock endpoint 'tap' must exist");
    tap.await_exchanges(1, TEST_TIMEOUT).await;
    tap.assert_exchange_count(1).await;
    let copies = tap.get_received_exchanges().await;
    assert_eq!(
        copies[0].input.body.as_text(),
        Some("hello"),
        "the copy must carry the same body as the original"
    );
}

/// Under saturation (wiretap bound 20 exhausted by parked copies), the 21st
/// copy runs INLINE (CallerRuns) before the 21st real send, and the 21st
/// outcome is the real producer's sentinel `Ok` verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saturated_divert_runs_the_copy_inline_before_the_real_send() {
    let ordinal = Arc::new(AtomicU32::new(0));
    let real_ordinal = Arc::new(AtomicU32::new(0));
    let order_log: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let park = Arc::new(Semaphore::new(0));
    let (arrivals_tx, mut arrivals_rx) = mpsc::unbounded_channel::<u32>();

    let copy_producer = BoxProcessor::new(OrdinalParkSvc {
        ordinal: Arc::clone(&ordinal),
        arrivals: arrivals_tx,
        order_log: Arc::clone(&order_log),
        park: Arc::clone(&park),
    });
    let real_producer = BoxProcessor::new(OrderLogRealSvc {
        ordinal: Arc::clone(&real_ordinal),
        order_log: Arc::clone(&order_log),
    });

    let mut ctx = boot_stub_context(
        divert_rules("ordered:out", "mock:park"),
        vec![
            StubComponent::new("mock", copy_producer),
            StubComponent::new("ordered", real_producer),
        ],
    )
    .await;
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To("ordered:out".into())])
            .with_route_id("saturated-divert"),
    )
    .await
    .expect("route must register");
    ctx.start().await.expect("context start failed");

    // Send 20 exchanges: each admits a detached copy that parks holding a
    // wiretap permit; each real send completes immediately.
    for i in 0..20 {
        send_to_direct(
            &ctx,
            "direct:in",
            Exchange::new(Message::new(format!("m{i}"))),
        )
        .await;
    }

    // Admission barrier: await all 20 copy arrivals (each receipt is sent
    // after the ordinal increment, so 20 receipts prove 20 parked copies
    // hold every wiretap permit).
    tokio::time::timeout(TEST_TIMEOUT, async {
        for _ in 0..20 {
            arrivals_rx
                .recv()
                .await
                .expect("copy arrival receipt must arrive");
        }
    })
    .await
    .expect("admission barrier must complete within the test budget");
    assert_eq!(ordinal.load(Ordering::SeqCst), 20);

    // 21st send: no wiretap permit is free, so the copy runs inline before
    // the real send.
    let result = send_to_direct_result(
        &ctx,
        "direct:in",
        Exchange::new(Message::new("twenty-first")),
    )
    .await
    .expect("21st send must succeed with the real producer's Ok");
    assert_eq!(
        result.input.headers.get("X-Sentinel"),
        Some(&Value::String("real-ok".to_string())),
        "the 21st outcome must be the real producer's sentinel Ok verbatim"
    );

    let log = order_log.lock().expect("order log mutex").clone();
    let copy_idx = log
        .iter()
        .position(|&e| e == 21)
        .expect("21st copy event must be in the order log");
    let real_idx = log
        .iter()
        .position(|&e| e == 1021)
        .expect("21st real event must be in the order log");
    assert!(
        copy_idx < real_idx,
        "the 21st copy event must precede the 21st real event; log: {log:?}"
    );

    // Cleanup: release the parked permits, then stop/drain the route so
    // in-flight copies finish before the test ends.
    park.add_permits(20);
    ctx.stop().await.expect("context stop failed");
}

/// A failing copy call is suppressed: the real producer's sentinel `Ok` is
/// returned verbatim and a `warn` naming the copy failure is emitted.
/// Current-thread runtime: the warn-emitting tap task is polled on this
/// thread while the capturing subscriber guard is held.
#[tokio::test]
async fn real_ok_outcome_stays_verbatim_when_the_copy_call_fails() {
    let (sink, _guard) = capture_tracing();

    let copy_done = Arc::new(Notify::new());
    let done = Arc::clone(&copy_done);
    let copy_producer = BoxProcessor::from_fn(move |_ex| {
        let done = Arc::clone(&done);
        Box::pin(async move {
            done.notify_one();
            Err(CamelError::ProcessorError("copy-boom".into()))
        })
    });
    let real_producer = BoxProcessor::from_fn(|mut ex| {
        Box::pin(async move {
            ex.input.headers.insert(
                "X-Sentinel".to_string(),
                Value::String("real-ok".to_string()),
            );
            Ok(ex)
        })
    });

    let mut ctx = boot_stub_context(
        divert_rules("stub:real", "mock:boom"),
        vec![
            StubComponent::new("mock", copy_producer),
            StubComponent::new("stub", real_producer),
        ],
    )
    .await;
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To("stub:real".into())])
            .with_route_id("copy-fails"),
    )
    .await
    .expect("route must register");
    ctx.start().await.expect("context start failed");

    let result = send_to_direct(&ctx, "direct:in", Exchange::new(Message::new("main"))).await;
    assert_eq!(
        result.input.headers.get("X-Sentinel"),
        Some(&Value::String("real-ok".to_string())),
        "real producer result must be returned verbatim despite the copy failure"
    );

    // The copy runs detached; await its completion signal before asserting
    // the captured warn (the warn fires when the tap task observes the
    // failure — no await between the signal and the warn).
    await_signal(&copy_done, "copy completion signal").await;
    let captured = String::from_utf8(sink.lock().expect("sink mutex").clone())
        .expect("captured logs must be UTF-8");
    assert!(
        captured.contains("copy-boom"),
        "a warn record mentioning the copy failure should have been emitted; got: {captured}"
    );

    ctx.stop().await.expect("context stop failed");
}

/// A failing real producer is returned verbatim despite a successful copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_err_outcome_stays_verbatim_when_the_copy_succeeds() {
    let copy_done = Arc::new(Notify::new());
    let done = Arc::clone(&copy_done);
    let copy_producer = BoxProcessor::from_fn(move |ex| {
        let done = Arc::clone(&done);
        Box::pin(async move {
            done.notify_one();
            Ok(ex)
        })
    });

    let mut ctx = boot_stub_context(
        divert_rules("stub:real", "mock:copy"),
        vec![
            StubComponent::new("mock", copy_producer),
            StubComponent::new("stub", real_boom_producer()),
        ],
    )
    .await;
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To("stub:real".into())])
            .with_route_id("real-fails"),
    )
    .await
    .expect("route must register");
    ctx.start().await.expect("context start failed");

    let result =
        send_to_direct_result(&ctx, "direct:in", Exchange::new(Message::new("main"))).await;
    // The copy may still be in flight; await its completion before the
    // outcome assertion so the test ends with no dangling copies.
    await_signal(&copy_done, "copy completion signal").await;
    match result {
        Err(CamelError::ProcessorError(msg)) => assert_eq!(
            msg, "real-boom",
            "real producer error must be returned verbatim"
        ),
        other => panic!("expected ProcessorError(\"real-boom\"), got {other:?}"),
    }

    ctx.stop().await.expect("context stop failed");
}

/// A failing copy readiness is swallowed: the real producer's sentinel `Ok`
/// is returned verbatim and a `warn` is captured. Current-thread runtime for
/// deterministic warn capture (see `capture_tracing`).
#[tokio::test]
async fn copy_poll_ready_failure_is_swallowed() {
    let (sink, _guard) = capture_tracing();

    let copy_done = Arc::new(Notify::new());

    // Copy stub whose readiness errs; signals completion as part of the
    // readiness attempt (before the warn it triggers).
    #[derive(Clone)]
    struct ReadyFailingCopySvc {
        done: Arc<Notify>,
    }

    impl Service<Exchange> for ReadyFailingCopySvc {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            // Signal after the readiness attempt has been made: the warn
            // fires later in the same poll batch (no await between).
            self.done.notify_one();
            Poll::Ready(Err(CamelError::ProcessorError("copy-ready-boom".into())))
        }

        fn call(&mut self, ex: Exchange) -> Self::Future {
            Box::pin(async move { Ok(ex) })
        }
    }

    let copy_producer = BoxProcessor::new(ReadyFailingCopySvc {
        done: Arc::clone(&copy_done),
    });
    let real_producer = BoxProcessor::from_fn(|mut ex| {
        Box::pin(async move {
            ex.input.headers.insert(
                "X-Sentinel".to_string(),
                Value::String("real-ok".to_string()),
            );
            Ok(ex)
        })
    });

    let mut ctx = boot_stub_context(
        divert_rules("stub:real", "mock:unready"),
        vec![
            StubComponent::new("mock", copy_producer),
            StubComponent::new("stub", real_producer),
        ],
    )
    .await;
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To("stub:real".into())])
            .with_route_id("copy-unready"),
    )
    .await
    .expect("route must register");
    ctx.start().await.expect("context start failed");

    let result = send_to_direct(&ctx, "direct:in", Exchange::new(Message::new("main"))).await;
    assert_eq!(
        result.input.headers.get("X-Sentinel"),
        Some(&Value::String("real-ok".to_string())),
        "real producer result must be returned verbatim despite the copy readiness failure"
    );

    await_signal(&copy_done, "copy readiness signal").await;
    let captured = String::from_utf8(sink.lock().expect("sink mutex").clone())
        .expect("captured logs must be UTF-8");
    assert!(
        captured.contains("copy-ready-boom"),
        "a warn record mentioning the copy readiness failure should have been emitted; got: {captured}"
    );

    ctx.stop().await.expect("context stop failed");
}

/// A copy target that cannot resolve is a compile error naming the copy
/// target with the intercept enrichment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copy_target_resolution_failure_is_a_compile_error() {
    // No mock component registered — only direct and seda.
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("build context");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(SedaComponent::new());
    ctx.set_intercept_rules(divert_rules("seda:out", "mock:ghost"))
        .await
        .expect("rules must install before first use");

    let err = ctx
        .add_route_definition(
            RouteDefinition::new("direct:in", vec![BuilderStep::To("seda:out".into())])
                .with_route_id("ghost-copy"),
        )
        .await
        .expect_err("route must fail to compile: mock:ghost cannot resolve");
    let msg = format!("{err}");
    assert!(
        msg.contains("mock:ghost"),
        "error must name the copy target, got: {msg}"
    );
    assert!(
        msg.contains("intercept target:"),
        "error must carry the intercept target enrichment, got: {msg}"
    );

    ctx.stop().await.expect("context stop failed");
}

/// The composed divert drives real-producer readiness before call (case 1:
/// success order and sentinel outcome verbatim).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_producer_readiness_is_driven_before_call_success_order() {
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let real_producer = BoxProcessor::new(EventRealSvc {
        events: Arc::clone(&events),
    });
    let copy_producer = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));

    let mut ctx = boot_stub_context(
        divert_rules("stub:real", "mock:copy"),
        vec![
            StubComponent::new("mock", copy_producer),
            StubComponent::new("stub", real_producer),
        ],
    )
    .await;
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To("stub:real".into())])
            .with_route_id("readiness-order"),
    )
    .await
    .expect("route must register");
    ctx.start().await.expect("context start failed");

    let result = send_to_direct(&ctx, "direct:in", Exchange::new(Message::new("main"))).await;

    assert_eq!(
        *events.lock().expect("events mutex"),
        vec!["ready", "call"],
        "real producer readiness must be driven before call"
    );
    assert_eq!(
        result.input.headers.get("X-Sentinel"),
        Some(&Value::String("real-ok".to_string())),
        "returned exchange must be the real producer's sentinel"
    );

    ctx.stop().await.expect("context stop failed");
}

/// The composed divert drives real-producer readiness before call (case 2:
/// readiness failure returns the sentinel error verbatim and skips call).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_producer_readiness_is_driven_before_call_failure_verbatim() {
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let real_producer = BoxProcessor::new(ReadyFailingRealSvc {
        events: Arc::clone(&events),
    });
    let copy_producer = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));

    let mut ctx = boot_stub_context(
        divert_rules("stub:real", "mock:copy"),
        vec![
            StubComponent::new("mock", copy_producer),
            StubComponent::new("stub", real_producer),
        ],
    )
    .await;
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To("stub:real".into())])
            .with_route_id("readiness-failure"),
    )
    .await
    .expect("route must register");
    ctx.start().await.expect("context start failed");

    let result =
        send_to_direct_result(&ctx, "direct:in", Exchange::new(Message::new("main"))).await;
    match result {
        Err(CamelError::ProcessorError(msg)) => assert_eq!(
            msg, "sentinel-ready",
            "real producer readiness error must be returned verbatim"
        ),
        other => panic!("expected ProcessorError(\"sentinel-ready\"), got {other:?}"),
    }
    assert!(
        events.lock().expect("events mutex").is_empty(),
        "real producer call must be skipped on readiness failure"
    );

    ctx.stop().await.expect("context stop failed");
}

/// A divert survives a route stop and restart: after restart the copy still
/// reaches the tap and the real message still reaches the consumer.
///
/// The consumer URI uses `multipleConsumers=true` (fanout mode): a fanout
/// consumer registers a fresh subscriber on every start, so the consumer can
/// re-register after the restart. A single-consumer seda endpoint consumes
/// its one-shot receiver on the first start and cannot re-register — a
/// pre-existing seda limitation independent of interception (bd rc-gwvs).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn divert_survives_route_stop_and_restart() {
    let consumer_uri = "seda:out?multipleConsumers=true";
    let (mut ctx, mock) =
        boot_context_with_intercept(Some(divert_rules(consumer_uri, "mock:tap"))).await;
    ctx.add_route_definition(
        RouteDefinition::new(consumer_uri, vec![BuilderStep::To("mock:arrival".into())])
            .with_route_id("consumer-route"),
    )
    .await
    .expect("consumer route must register");
    ctx.add_route_definition(
        RouteDefinition::new("direct:in", vec![BuilderStep::To(consumer_uri.into())])
            .with_route_id("send-route"),
    )
    .await
    .expect("send route must register");
    ctx.start().await.expect("context start failed");

    // Stop, then restart: the composed lifecycle reopens wiretap admission
    // and the seda consumer is recreated.
    ctx.stop().await.expect("context stop failed");
    ctx.start().await.expect("context restart failed");

    send_to_direct(
        &ctx,
        "direct:in",
        Exchange::new(Message::new("after-restart")),
    )
    .await;

    // Await the consumer's arrival notification for the real message.
    let arrival = mock
        .get_endpoint("arrival")
        .expect("mock endpoint 'arrival' must exist");
    arrival.await_exchanges(1, TEST_TIMEOUT).await;
    arrival.assert_exchange_count(1).await;
    let received = arrival.get_received_exchanges().await;
    assert_eq!(received[0].input.body.as_text(), Some("after-restart"));

    // Stop again (drain) so the copy finishes, then assert both deliveries.
    ctx.stop().await.expect("context stop failed");

    let tap = mock
        .get_endpoint("tap")
        .expect("mock endpoint 'tap' must exist");
    tap.await_exchanges(1, TEST_TIMEOUT).await;
    tap.assert_exchange_count(1).await;
    let copies = tap.get_received_exchanges().await;
    assert_eq!(
        copies[0].input.body.as_text(),
        Some("after-restart"),
        "the copy must carry the same body as the original"
    );
}
