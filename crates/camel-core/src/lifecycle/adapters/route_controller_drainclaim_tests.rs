//! Tests for the drainclaim context-global in-flight counter
//! (task 1.3). Sibling file via `#[path]` so the route-controller
//! test module stays scannable; still in-crate for access to the
//! controller internals under test.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tower::{Service, ServiceExt};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::unit_of_work::UnitOfWorkConfig;
use camel_api::{
    AggregatorConfig, BoxProcessor, BoxProcessorExt, CamelError, Exchange, IdentityProcessor,
    Message, OpaqueProcessor, RouteController, Value, ValueSourceDef,
};
use camel_component_api::{
    Component, Consumer, ConsumerContext, Endpoint, ProducerContext, RuntimeObservability,
};

use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinition};
use crate::shared::components::domain::Registry;

use super::DefaultRouteController;

/// ConsumerContext clones captured per consumer boot (same shape as
/// the inline-dispatcher test harness).
type CapturedCtxs = Arc<Mutex<Vec<ConsumerContext>>>;

struct CaptureComponent {
    captured: CapturedCtxs,
}

struct CaptureEndpoint {
    captured: CapturedCtxs,
}

struct CaptureConsumer {
    captured: CapturedCtxs,
}

impl Component for CaptureComponent {
    fn scheme(&self) -> &str {
        "capture"
    }
    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(CaptureEndpoint {
            captured: Arc::clone(&self.captured),
        }))
    }
}

impl Endpoint for CaptureEndpoint {
    fn uri(&self) -> &str {
        "capture"
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(CaptureConsumer {
            captured: Arc::clone(&self.captured),
        }))
    }
    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(IdentityProcessor))
    }
}

#[async_trait::async_trait]
impl Consumer for CaptureConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        self.captured
            .lock()
            .expect("captured lock")
            .push(ctx.clone());
        ctx.cancel_token().cancelled().await;
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

/// Gated probe parts: entry signal + release barrier.
struct ProbeParts {
    entered_rx: mpsc::UnboundedReceiver<()>,
    release_tx: tokio::sync::watch::Sender<u32>,
}

/// Pipeline processor that signals entry, then parks on the release
/// barrier until the test releases its ordinal.
fn gated_processor() -> (BoxProcessor, ProbeParts) {
    let (entered_tx, entered_rx) = mpsc::unbounded_channel::<()>();
    let (release_tx, release_rx) = tokio::sync::watch::channel(0u32);
    let ordinal = Arc::new(AtomicU32::new(0));
    let processor = BoxProcessor::from_fn(move |mut ex: Exchange| {
        let entered_tx = entered_tx.clone();
        let mut release_rx = release_rx.clone();
        let ordinal = Arc::clone(&ordinal);
        async move {
            let _ = entered_tx.send(());
            let mine = ordinal.fetch_add(1, Ordering::SeqCst) + 1;
            release_rx
                .wait_for(|v| *v >= mine)
                .await
                .expect("release channel alive");
            ex.set_property("sink", "gated");
            Ok(ex)
        }
    });
    (
        processor,
        ProbeParts {
            entered_rx,
            release_tx,
        },
    )
}

/// A service whose readiness check always fails — drives the
/// `ready_with_backoff` early-return path in the drain sites.
#[derive(Clone)]
struct NeverReady;

impl Service<Exchange> for NeverReady {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Err(CamelError::ProcessorError(
            "planned readiness failure".into(),
        )))
    }

    fn call(&mut self, ex: Exchange) -> Self::Future {
        Box::pin(async move { Ok(ex) })
    }
}

fn drain_controller(captured: CapturedCtxs) -> DefaultRouteController {
    let registry = Arc::new(Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock")
        .register(Arc::new(CaptureComponent { captured }));
    DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    )
}

async fn await_capture(captured: &CapturedCtxs) -> ConsumerContext {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if let Some(ctx) = captured.lock().expect("captured lock").first().cloned() {
            return ctx;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("consumer context was not captured within 2s");
}

/// Poll until the controller's global in-flight counter reaches
/// `want` (bounded; panics past the deadline).
async fn await_total(counter: &AtomicU64, want: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while counter.load(Ordering::SeqCst) != want {
        assert!(
            tokio::time::Instant::now() < deadline,
            "global in-flight counter did not reach {want} within 2s (now {})",
            counter.load(Ordering::SeqCst)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Poll until the context's global in-flight counter reaches `want`
/// (bounded; panics past the deadline). Same discipline as
/// [`await_total`], for tests driving a full `CamelContext`.
async fn await_ctx_total(ctx: &crate::CamelContext, want: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while ctx.total_in_flight() != want {
        assert!(
            tokio::time::Instant::now() < deadline,
            "total_in_flight did not reach {want} within 2s (now {})",
            ctx.total_in_flight()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn test_exchange(tag: &str) -> Exchange {
    Exchange::new(Message::new(tag))
}

/// Pass-through processor that records each exchange's text body
/// into an unbounded channel, then forwards the exchange unchanged
/// (drainclaim/claimfamily scenario pins).
struct RecordingPost {
    tx: mpsc::UnboundedSender<String>,
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
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let body = exchange
            .input
            .body
            .as_text()
            .unwrap_or_default()
            .to_string();
        let _ = self.tx.send(body);
        Box::pin(async move { Ok(exchange) })
    }
}

#[tokio::test]
async fn pipeline_residency_counted_until_completion() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));
    let (processor, mut parts) = gated_processor();
    let route = RouteDefinition::new(
        "capture:src",
        vec![BuilderStep::Processor(OpaqueProcessor(processor))],
    )
    .with_route_id("rt-drain-residency");
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-drain-residency").await.unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let result = ctx.send_and_wait(test_exchange("residency")).await;
        let _ = done_tx.send(result);
    });

    timeout(Duration::from_secs(2), parts.entered_rx.recv())
        .await
        .expect("pipeline entry within 2s")
        .expect("entry channel alive");
    assert!(
        controller.in_flight_total.load(Ordering::SeqCst) >= 1,
        "exchange parked in the pipeline must be counted"
    );

    parts.release_tx.send(1).expect("release channel alive");
    let result = timeout(Duration::from_secs(2), done_rx)
        .await
        .expect("reply within 2s")
        .expect("reply channel alive");
    assert!(result.is_ok(), "gated pipeline must complete: {result:?}");
    await_total(&controller.in_flight_total, 0).await;

    controller.stop_route("rt-drain-residency").await.unwrap();
}

#[tokio::test]
async fn readiness_failure_releases_claim() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));
    let route = RouteDefinition::new(
        "capture:src",
        vec![BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(
            NeverReady,
        )))],
    )
    .with_route_id("rt-drain-readyfail");
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-drain-readyfail").await.unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;
    let result = timeout(
        Duration::from_secs(2),
        ctx.send_and_wait(test_exchange("ready")),
    )
    .await
    .expect("readiness failure must surface as a reply within 2s");
    assert!(result.is_err(), "readiness failure must fail the exchange");
    // The drain site's early return dropped the claim via scope exit.
    await_total(&controller.in_flight_total, 0).await;

    controller.stop_route("rt-drain-readyfail").await.unwrap();
}

#[tokio::test]
async fn seda_enqueue_through_real_context_counted() {
    let mut ctx = crate::CamelContext::builder().build().await.unwrap();
    ctx.register_component(camel_component_seda::SedaComponent::new());
    let (processor, mut parts) = gated_processor();
    ctx.add_route_definition(
        RouteDefinition::new(
            "seda:a",
            vec![BuilderStep::Processor(OpaqueProcessor(processor))],
        )
        .with_route_id("rt-seda-drain"),
    )
    .await
    .unwrap();
    ctx.start().await.unwrap();

    // Produce through the real registry/compile path.
    let component = ctx.registry().get("seda").unwrap();
    let endpoint = component.create_endpoint("seda:a", &ctx).unwrap();
    let producer = endpoint
        .create_producer(
            Arc::new(camel_component_api::NoOpComponentContext),
            &ctx.producer_context(),
        )
        .unwrap();
    producer
        .clone()
        .oneshot(test_exchange("e2e"))
        .await
        .expect("seda enqueue accepted");

    timeout(Duration::from_secs(2), parts.entered_rx.recv())
        .await
        .expect("pipeline entry within 2s")
        .expect("entry channel alive");
    assert!(
        ctx.total_in_flight() >= 1,
        "exchange parked in the seda-fed pipeline must be counted"
    );

    parts.release_tx.send(1).expect("release channel alive");
    await_ctx_total(&ctx, 0).await;

    ctx.stop().await.unwrap();
}

/// drainclaim claim propagation: an exchange stashed in a pending
/// aggregate bucket keeps its claim (counter >= 1) until the bucket
/// completes, emits, and the post-pipeline continuation finishes.
#[tokio::test]
async fn pending_bucket_keeps_claim() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));

    // Timeout materializes the aggregate SPLIT (size alone compiles
    // an embedded aggregator); 600s keeps the timeout from firing
    // during the test — size 2 completes the bucket on the second
    // exchange.
    let agg_config = AggregatorConfig::correlate_by("key")
        .complete_on_size_or_timeout(2, Duration::from_secs(600))
        .build()
        .unwrap();

    let route = RouteDefinition::new(
        "capture:src",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: ValueSourceDef::Literal(Value::String("k1".into())),
            },
            BuilderStep::Aggregate { config: agg_config },
        ],
    )
    .with_route_id("rt-drain-agg-pending");
    controller.add_route(route).await.unwrap();
    controller
        .start_route("rt-drain-agg-pending")
        .await
        .unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;

    // First exchange: the pending-ack reply proves the stash inside
    // the bucket happened; the claim stays stashed — counter 1.
    let first = ctx
        .send_and_wait(test_exchange("frag-1"))
        .await
        .expect("pending ack reply");
    assert!(
        first.property("CamelAggregatorPending").is_some(),
        "first exchange must be stashed, not completed"
    );
    assert_eq!(
        controller.in_flight_total.load(Ordering::SeqCst),
        1,
        "stashed exchange must stay counted"
    );

    // Second exchange completes the bucket: the aggregated exchange
    // runs the post-pipeline while BOTH claims are held, then they
    // release.
    let second = ctx
        .send_and_wait(test_exchange("frag-2"))
        .await
        .expect("aggregated reply");
    assert!(
        second.property("CamelAggregatorPending").is_none(),
        "second exchange must complete the bucket"
    );
    await_total(&controller.in_flight_total, 0).await;

    controller.stop_route("rt-drain-agg-pending").await.unwrap();
}

/// claimfamily (rc-hllkk): an exchange buffered inside a resequencer
/// policy keeps a claim (counter 1) after its pipeline ack resolves;
/// completing the batch emits through the continuation and the
/// counter returns to 0.
#[tokio::test]
async fn resequencer_buffer_residency_counted_until_emission() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));
    crate::lifecycle::adapters::route_controller::tests::register_simple_language(&mut controller);

    let (emitted_tx, mut emitted_rx) = mpsc::unbounded_channel::<String>();
    let route = RouteDefinition::new(
        "capture:src",
        vec![
            BuilderStep::Resequence {
                policy_config: camel_api::ResequencePolicyConfig {
                    mode: camel_api::ResequenceMode::Batch {
                        correlation: "${header.id}".into(),
                        sort: "${header.id}".into(),
                        completion: camel_api::BatchCompletion::Size(2),
                    },
                },
            },
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(RecordingPost {
                tx: emitted_tx,
            }))),
        ],
    )
    .with_route_id("rt-drain-reseq");
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-drain-reseq").await.unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;

    // First exchange: the pipeline ack resolves (pipeline task done,
    // its envelope claim dropped) but the exchange sits in the batch
    // buffer — the drain-site split claim keeps it counted.
    let mut first = test_exchange("reseq-1");
    first.input.set_header("id", "k");
    let ack = ctx.send_and_wait(first).await.expect("resequencer ack");
    assert_eq!(
        ack.property("CamelResequencerAccepted")
            .and_then(|v| v.as_bool()),
        Some(true),
        "first exchange must be accepted into the buffer"
    );
    await_total(&controller.in_flight_total, 1).await;

    // Second exchange completes the batch (Size 2): both buffered
    // exchanges emit through the post-continuation, then every claim
    // releases.
    let mut second = test_exchange("reseq-2");
    second.input.set_header("id", "k");
    let _ = ctx.send_and_wait(second).await.expect("second ack");
    let mut emitted = Vec::new();
    emitted.push(
        timeout(Duration::from_secs(2), emitted_rx.recv())
            .await
            .expect("first emission within 2s")
            .expect("emission channel alive"),
    );
    emitted.push(
        timeout(Duration::from_secs(2), emitted_rx.recv())
            .await
            .expect("second emission within 2s")
            .expect("emission channel alive"),
    );
    emitted.sort();
    assert_eq!(emitted, vec!["reseq-1", "reseq-2"]);
    await_total(&controller.in_flight_total, 0).await;

    controller.stop_route("rt-drain-reseq").await.unwrap();
}

/// claimfamily (rc-qbigm): a size-only aggregator compiled INSIDE
/// the pipeline (no timeout, no force-completion — the embedded
/// branch of `find_top_level_aggregate_requiring_split`) stashes
/// partial buckets beyond pipeline completion. The drain-site split
/// sibling rides the exchange into the bucket: the counter reads 1
/// after the pending-ack pipeline resolves, and 0 once the
/// completing exchange's aggregated output runs the remaining
/// pipeline.
#[tokio::test]
async fn embedded_aggregator_stash_counted_until_completion() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));

    let (emitted_tx, mut emitted_rx) = mpsc::unbounded_channel::<String>();
    let agg_config = AggregatorConfig::correlate_by("key")
        .complete_when_size(2)
        .build()
        .unwrap();
    let route = RouteDefinition::new(
        "capture:src",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: ValueSourceDef::Literal(Value::String("k1".into())),
            },
            BuilderStep::Aggregate { config: agg_config },
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(RecordingPost {
                tx: emitted_tx,
            }))),
        ],
    )
    .with_route_id("rt-drain-agg-embedded");
    controller.add_route(route).await.unwrap();
    controller
        .start_route("rt-drain-agg-embedded")
        .await
        .unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;

    // First exchange: the pending-ack reply proves the stash inside
    // the embedded bucket; the pipeline task is done, so only the
    // stashed sibling keeps the counter at 1.
    let first = ctx
        .send_and_wait(test_exchange("frag-1"))
        .await
        .expect("pending ack reply");
    assert!(
        first.property("CamelAggregatorPending").is_some(),
        "first exchange must be stashed, not completed"
    );
    await_total(&controller.in_flight_total, 1).await;

    // Second exchange completes the bucket: the aggregated output
    // carries one claim through the remaining pipeline steps (the
    // recording post-step fires), then every claim releases.
    let second = ctx
        .send_and_wait(test_exchange("frag-2"))
        .await
        .expect("aggregated reply");
    assert!(
        second.property("CamelAggregatorPending").is_none(),
        "second exchange must complete the bucket"
    );
    let _ = timeout(Duration::from_secs(2), emitted_rx.recv())
        .await
        .expect("aggregated emission within 2s")
        .expect("emission channel alive");
    await_total(&controller.in_flight_total, 0).await;

    controller
        .stop_route("rt-drain-agg-embedded")
        .await
        .unwrap();
}

/// drainclaim claim propagation: stopping the route with a pending
/// bucket force-completes it; the forced emission's continuation
/// runs and every stashed claim releases — counter 0 after stop.
#[tokio::test]
async fn force_complete_releases_claims() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));

    let agg_config = AggregatorConfig::correlate_by("key")
        .complete_when_size(10)
        .force_completion_on_stop(true)
        .build()
        .unwrap();

    let route = RouteDefinition::new(
        "capture:src",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: ValueSourceDef::Literal(Value::String("k1".into())),
            },
            BuilderStep::Aggregate { config: agg_config },
        ],
    )
    .with_route_id("rt-drain-agg-force");
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-drain-agg-force").await.unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;
    let first = ctx
        .send_and_wait(test_exchange("frag-1"))
        .await
        .expect("pending ack reply");
    assert!(first.property("CamelAggregatorPending").is_some());
    assert_eq!(
        controller.in_flight_total.load(Ordering::SeqCst),
        1,
        "stashed exchange must stay counted"
    );

    // Stop force-completes the bucket: the emission's claims are
    // held across the drain loop's post-pipeline continuation and
    // released afterwards.
    controller.stop_route("rt-drain-agg-force").await.unwrap();
    await_total(&controller.in_flight_total, 0).await;
}

/// claimfamily (rc-e1a4f): a stash site (size-only aggregator)
/// compiled into the POST-pipeline of a route-level aggregate split
/// parks the split sibling with the aggregated output — the counter
/// reads 1 after the completing exchange's reply (the stash,
/// uncounted before this fix, would read 0), and 0 once the
/// embedded bucket completes and emits through the tail.
#[tokio::test]
async fn aggregate_split_post_pipeline_stash_counted_until_completion() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));

    let (emitted_tx, mut emitted_rx) = mpsc::unbounded_channel::<String>();
    // The timeout (600s, never fires) materializes the route-level
    // SPLIT; the second, size-only aggregate is the embedded stash
    // site living in the post-pipeline.
    let route = RouteDefinition::new(
        "capture:src",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: ValueSourceDef::Literal(Value::String("k1".into())),
            },
            BuilderStep::Aggregate {
                config: AggregatorConfig::correlate_by("key")
                    .complete_on_size_or_timeout(2, Duration::from_secs(600))
                    .build()
                    .unwrap(),
            },
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: ValueSourceDef::Literal(Value::String("k1".into())),
            },
            BuilderStep::Aggregate {
                config: AggregatorConfig::correlate_by("key")
                    .complete_when_size(2)
                    .build()
                    .unwrap(),
            },
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(RecordingPost {
                tx: emitted_tx,
            }))),
        ],
    )
    .with_route_id("rt-aggrloop-post-stash");
    controller.add_route(route).await.unwrap();
    controller
        .start_route("rt-aggrloop-post-stash")
        .await
        .unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;

    // First fragment: stashed in the route-level bucket — the
    // envelope's claim parks there, counter 1.
    let reply1 = ctx
        .send_and_wait(test_exchange("frag-1"))
        .await
        .expect("pending ack reply");
    assert!(
        reply1.property("CamelAggregatorPending").is_some(),
        "first exchange must be stashed in the route bucket"
    );
    await_total(&controller.in_flight_total, 1).await;

    // Second fragment completes the route bucket; the aggregated
    // output is stashed in the embedded post-pipeline aggregator.
    // The split sibling parks with it — counter stays 1 (before
    // rc-e1a4f the embedded stash was uncounted and this read 0).
    let reply2 = ctx
        .send_and_wait(test_exchange("frag-2"))
        .await
        .expect("embedded pending marker reply");
    assert!(
        reply2.property("CamelAggregatorPending").is_some(),
        "second exchange must be stashed in the embedded post-pipeline aggregator"
    );
    await_total(&controller.in_flight_total, 1).await;

    // Third fragment opens a fresh route bucket: embedded stash (1)
    // plus the new bucket's claim (1).
    let reply3 = ctx
        .send_and_wait(test_exchange("frag-3"))
        .await
        .expect("pending ack reply");
    assert!(
        reply3.property("CamelAggregatorPending").is_some(),
        "third exchange must be stashed in a new route bucket"
    );
    await_total(&controller.in_flight_total, 2).await;

    // Fourth fragment completes the route bucket again; the second
    // aggregated output completes the embedded bucket, which emits
    // through the RecordingPost tail — no pending marker.
    let reply4 = ctx
        .send_and_wait(test_exchange("frag-4"))
        .await
        .expect("final aggregate reply");
    assert!(
        reply4.property("CamelAggregatorPending").is_none(),
        "fourth exchange must complete the embedded bucket"
    );

    // Recordings happen synchronously before their replies arrive:
    // the embedded pending marker (ex2) and the final aggregate
    // (ex4) both traversed RecordingPost; nothing else did.
    let _ = emitted_rx.try_recv().expect("ex2 pending marker recording");
    let _ = emitted_rx
        .try_recv()
        .expect("ex4 final aggregate recording");
    assert!(
        matches!(emitted_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "no third recording expected"
    );

    await_total(&controller.in_flight_total, 0).await;
    controller
        .stop_route("rt-aggrloop-post-stash")
        .await
        .unwrap();
}

/// claimfamily (rc-e1a4f): a force-completed late emission traverses
/// the post-pipeline with a split sibling; the discarded in-band
/// result releases it — counter 0 after stop, and the emission
/// observably traversed the post-pipeline.
#[tokio::test]
async fn aggregate_split_forced_emission_sibling_released_after_stop() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));

    let (emitted_tx, mut emitted_rx) = mpsc::unbounded_channel::<String>();
    // force_completion_on_stop materializes the SPLIT; size 10 never
    // completes by size, so the only emission is the forced one at
    // stop. The RecordingPost tail is the stash-free post-pipeline.
    let route = RouteDefinition::new(
        "capture:src",
        vec![
            BuilderStep::DeclarativeSetHeader {
                key: "key".into(),
                value: ValueSourceDef::Literal(Value::String("k1".into())),
            },
            BuilderStep::Aggregate {
                config: AggregatorConfig::correlate_by("key")
                    .complete_when_size(10)
                    .force_completion_on_stop(true)
                    .build()
                    .unwrap(),
            },
            BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(RecordingPost {
                tx: emitted_tx,
            }))),
        ],
    )
    .with_route_id("rt-aggrloop-force-sib");
    controller.add_route(route).await.unwrap();
    controller
        .start_route("rt-aggrloop-force-sib")
        .await
        .unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;
    let first = ctx
        .send_and_wait(test_exchange("frag-1"))
        .await
        .expect("pending ack reply");
    assert!(first.property("CamelAggregatorPending").is_some());
    await_total(&controller.in_flight_total, 1).await;

    // Stop force-completes the bucket: the late emission traverses
    // the post-pipeline with a split sibling (SITE D), RecordingPost
    // fires, and the discarded result releases sibling + originals.
    controller
        .stop_route("rt-aggrloop-force-sib")
        .await
        .unwrap();

    let _body = timeout(Duration::from_secs(2), emitted_rx.recv())
        .await
        .expect("forced emission within 2s")
        .expect("emission channel alive");
    await_total(&controller.in_flight_total, 0).await;
}

/// drainclaim release path (aborted pipeline): stopping the route
/// while a pipeline call is parked on a barrier cannot observe
/// cancellation inside the parked processor, so the stop burns its
/// shutdown budget and aborts the pipeline task at the await point.
/// The abort drops the envelope's claim — counter 0 after teardown.
#[tokio::test]
async fn abort_releases_claim() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));
    let (processor, mut parts) = gated_processor();
    let route = RouteDefinition::new(
        "capture:src",
        vec![BuilderStep::Processor(OpaqueProcessor(processor))],
    )
    .with_route_id("rt-drain-abort");
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-drain-abort").await.unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let result = ctx.send_and_wait(test_exchange("abort")).await;
        let _ = done_tx.send(result);
    });

    timeout(Duration::from_secs(2), parts.entered_rx.recv())
        .await
        .expect("pipeline entry within 2s")
        .expect("entry channel alive");
    // claimfamily: the parked exchange holds TWO claims — the
    // task-scoped envelope claim (drainclaim) plus its drain-site
    // split sibling carried on the exchange (stash-site coverage).
    await_total(&controller.in_flight_total, 2).await;

    // Barrier still held: the drain loop is parked inside the
    // pipeline call, past every cancellation select, so the stop
    // path reaches the abort fallback.
    controller.stop_route("rt-drain-abort").await.unwrap();
    await_total(&controller.in_flight_total, 0).await;

    // The parked waiter learns of the teardown through the dropped
    // reply sender (ChannelClosed), not a pipeline result.
    let result = timeout(Duration::from_secs(2), done_rx)
        .await
        .expect("reply resolution within 2s")
        .expect("reply channel alive");
    assert!(
        result.is_err(),
        "aborted pipeline must fail the waiter: {result:?}"
    );
}

/// drainclaim release path (queued-envelope drop): envelopes accepted
/// while the pipeline is parked queue in the dispatch channel, each
/// already carrying its claim (minted at the acceptance boundary).
/// Stopping the route aborts the parked pipeline task, dropping the
/// channel receiver — the queued envelopes and their claims release
/// — counter 0.
#[tokio::test]
async fn queued_envelope_drop_releases_claim() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));
    let (processor, mut parts) = gated_processor();
    let route = RouteDefinition::new(
        "capture:src",
        vec![BuilderStep::Processor(OpaqueProcessor(processor))],
    )
    .with_route_id("rt-drain-queued");
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-drain-queued").await.unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let waiter_ctx = ctx.clone();
    tokio::spawn(async move {
        let result = waiter_ctx.send_and_wait(test_exchange("parked")).await;
        let _ = done_tx.send(result);
    });

    timeout(Duration::from_secs(2), parts.entered_rx.recv())
        .await
        .expect("pipeline entry within 2s")
        .expect("entry channel alive");
    // claimfamily: parked #1 holds two claims (task-scoped envelope
    // claim + drain-site split sibling on the exchange).
    await_total(&controller.in_flight_total, 2).await;

    // The drain loop is parked on envelope #1, so these two
    // fire-and-forget envelopes queue in the channel buffer, each
    // holding a claim minted at enqueue.
    ctx.send(test_exchange("queued-2"))
        .await
        .expect("queued send");
    ctx.send(test_exchange("queued-3"))
        .await
        .expect("queued send");
    await_total(&controller.in_flight_total, 4).await;

    // Teardown aborts the parked pipeline task; the queued envelopes
    // drop with the channel and every claim releases.
    controller.stop_route("rt-drain-queued").await.unwrap();
    await_total(&controller.in_flight_total, 0).await;

    let result = timeout(Duration::from_secs(2), done_rx)
        .await
        .expect("reply resolution within 2s")
        .expect("reply channel alive");
    assert!(
        result.is_err(),
        "aborted pipeline must fail the parked waiter: {result:?}"
    );
}

/// drainclaim release path (panicked pipeline): a panicking processor
/// unwinds the pipeline task — there is no panic-to-error conversion
/// in the drain path — and the claim drops during the unwind. The
/// waiter resolves through the dropped reply sender; counter 0 once
/// the task exits.
#[tokio::test]
async fn panic_releases_claim() {
    let captured: CapturedCtxs = Arc::new(Mutex::new(Vec::new()));
    let mut controller = drain_controller(Arc::clone(&captured));
    let route = RouteDefinition::new(
        "capture:src",
        vec![BuilderStep::Processor(OpaqueProcessor(
            BoxProcessor::from_fn(|_ex: Exchange| async move {
                panic!("planned pipeline panic");
            }),
        ))],
    )
    .with_route_id("rt-drain-panic");
    controller.add_route(route).await.unwrap();
    controller.start_route("rt-drain-panic").await.unwrap();
    controller.activate_cohort();

    let ctx = await_capture(&captured).await;
    // The panic unwinds the pipeline task, not this test task: the
    // waiter only sees the dropped reply sender.
    let result = timeout(
        Duration::from_secs(2),
        ctx.send_and_wait(test_exchange("boom")),
    )
    .await
    .expect("panic must resolve the waiter within 2s");
    assert!(
        result.is_err(),
        "panicking pipeline must fail the exchange: {result:?}"
    );
    await_total(&controller.in_flight_total, 0).await;

    controller.stop_route("rt-drain-panic").await.unwrap();
}

/// Per-endpoint flags recorded by the spy component's
/// `create_producer`: `true` when the runtime handle exposed the
/// context-global counter.
type SpyFlags = Arc<Mutex<HashMap<String, bool>>>;

struct SpyComponent {
    flags: SpyFlags,
}

struct SpyEndpoint {
    flags: SpyFlags,
    base: String,
}

impl Component for SpyComponent {
    fn scheme(&self) -> &str {
        "spy"
    }
    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let base = uri
            .strip_prefix("spy:")
            .unwrap_or(uri)
            .split('?')
            .next()
            .unwrap_or(uri)
            .to_string();
        Ok(Box::new(SpyEndpoint {
            flags: Arc::clone(&self.flags),
            base,
        }))
    }
}

impl Endpoint for SpyEndpoint {
    fn uri(&self) -> &str {
        "spy"
    }
    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        // Spy routes are never started; a consumer is a wiring bug.
        Err(CamelError::ComponentNotFound("spy has no consumer".into()))
    }
    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        self.flags
            .lock()
            .expect("flags lock")
            .insert(self.base.clone(), rt.in_flight_counter().is_some());
        Ok(BoxProcessor::new(IdentityProcessor))
    }
}

/// Deterministic tripwire for the producer-creation wiring: each spy
/// endpoint exercises one `ControllerComponentContext` construction
/// site, and the flag records whether that site's runtime exposed
/// the counter.
#[tokio::test]
async fn producer_path_receives_counter_spy() {
    let flags: SpyFlags = Arc::new(Mutex::new(HashMap::new()));
    let mut ctx = crate::CamelContext::builder().build().await.unwrap();
    ctx.register_component(SpyComponent {
        flags: Arc::clone(&flags),
    });
    ctx.register_component(camel_component_direct::DirectComponent::new());

    // (a) Plain `to:` step — step resolution site
    // (route_compiler_ext.rs `resolve_steps`).
    ctx.add_route_definition(
        RouteDefinition::new("direct:spya", vec![BuilderStep::To("spy:plain".into())])
            .with_route_id("rt-spy-plain"),
    )
    .await
    .unwrap();

    // (b) Error-handler DLC target — `build_eh_config_pipeline` site.
    ctx.add_route_definition(
        RouteDefinition::new("direct:spyb", vec![])
            .with_route_id("rt-spy-dlc")
            .with_error_handler(ErrorHandlerConfig {
                dlc_uri: Some("spy:dlc".into()),
                policies: vec![],
                use_original_message: false,
            }),
    )
    .await
    .unwrap();

    // (d) Managed-route UoW hook — `build_managed_route` site
    // (route_controller.rs).
    ctx.add_route_definition(
        RouteDefinition::new("direct:spyd", vec![])
            .with_route_id("rt-spy-ctl")
            .with_unit_of_work(UnitOfWorkConfig {
                on_complete: Some("spy:ctl".into()),
                on_failure: None,
            }),
    )
    .await
    .unwrap();

    // (c) Route-definition compile path UoW hook —
    // `compile_route_impl` site (route_compiler_ext.rs).
    ctx.runtime_execution_handle()
        .compile_route_definition(
            RouteDefinition::new("direct:spyc", vec![])
                .with_route_id("rt-spy-uow")
                .with_unit_of_work(UnitOfWorkConfig {
                    on_complete: Some("spy:uow".into()),
                    on_failure: None,
                }),
        )
        .await
        .expect("compile spy UoW route");

    let snapshot = flags.lock().expect("flags lock").clone();
    for key in ["plain", "dlc", "ctl", "uow"] {
        assert_eq!(
            snapshot.get(key),
            Some(&true),
            "producer for spy:{key} must see the in-flight counter — a production ControllerComponentContext site was missed"
        );
    }
}
