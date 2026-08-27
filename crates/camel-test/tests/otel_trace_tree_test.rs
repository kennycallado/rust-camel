//! End-to-end trace-tree shape tests (trace-model-tree T1.6).
//!
//! Asserts the landed span model through a real `CamelTestContext` wiring
//! (direct consumers, direct-producer drive, tracing enabled) against the
//! SDK in-memory exporter:
//!
//! Tree 1 (`direct_hop_nests_subroute_root_under_caller_step`): one
//! exchange through `tree-main` (process, `to: direct:tree-sub`, process)
//! produces a `tree-main` route root span; the closure steps
//! `tree-main:step-0/2` and the labeled dispatch step
//! `tree-main:to:direct` are siblings under the root; the `tree-sub` route
//! root nests under the dispatching step `tree-main:to:direct`;
//! `tree-sub:step-0/1` nest under `tree-sub`; every child's
//! `[start, end]` is contained in its parent's; the sequential steps are
//! time-ordered; the whole run is one trace.
//!
//! Tree 2 (`split_fragments_nest_under_segment_span_one_trace`): one
//! exchange through `tree-split` (one split segment over a two-line body,
//! each fragment dispatched to `direct:tree-sub`) produces a `tree-split`
//! root; the split segment span `tree-split:split` is a child of the
//! root; each fragment's `tree-sub` route root is a child of that segment
//! span; the whole run is one trace.
//!
//! The span harness replicates camel-core's `span_test_util` contract
//! locally (it is `#[cfg(test)]`-private and not importable): one global
//! `SdkTracerProvider` per process, exporter reset per test, and an async
//! mutex guard that serializes test bodies so spans cannot leak between
//! tests.

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_lines};
use camel_api::{CamelError, Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_test::CamelTestContext;
use opentelemetry::global;
use opentelemetry::trace::{SpanId, SpanKind};
use opentelemetry_sdk::trace::{
    InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor, SpanData,
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Span harness (local replica of camel-core's span_test_util contract)
// ---------------------------------------------------------------------------

/// Handle returned by [`test_spans`].
///
/// Holding `TestSpans` keeps the serialization guard alive; pass it to
/// [`finish`] to flush and collect the spans exported during the test.
struct TestSpans {
    provider: SdkTracerProvider,
    exporter: Arc<InMemorySpanExporter>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

/// Install (once per process) the global in-memory tracer provider, reset
/// the exporter, and acquire the lock that serializes test bodies.
async fn test_spans() -> TestSpans {
    static HARNESS: OnceLock<(SdkTracerProvider, Arc<InMemorySpanExporter>)> = OnceLock::new();
    static LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();

    let (provider, exporter) = HARNESS.get_or_init(|| {
        let exporter = Arc::new(InMemorySpanExporter::default());
        let provider = SdkTracerProvider::builder()
            .with_span_processor(SimpleSpanProcessor::new(exporter.as_ref().clone()))
            .build();
        global::set_tracer_provider(provider.clone());
        (provider, exporter)
    });

    let guard = LOCK
        .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
        .lock_owned()
        .await;

    exporter.reset();

    TestSpans {
        provider: provider.clone(),
        exporter: Arc::clone(exporter),
        _guard: guard,
    }
}

/// Flush the provider and collect the spans exported while the guard was
/// held.
fn finish(spans: TestSpans) -> Vec<SpanData> {
    spans.provider.force_flush().expect("flush exported spans");
    spans
        .exporter
        .get_finished_spans()
        .expect("read exported spans")
}

// ---------------------------------------------------------------------------
// Span lookup and shape helpers
// ---------------------------------------------------------------------------

/// The single span named `name`; fails if the name matches zero or several
/// spans, so duplicate roots or steps cannot pass silently.
fn span<'a>(all: &'a [SpanData], name: &str) -> &'a SpanData {
    let found: Vec<&SpanData> = all.iter().filter(|s| s.name == name).collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one span named {name}, found {}",
        found.len()
    );
    found[0]
}

/// All spans named `name`.
fn spans_named<'a>(all: &'a [SpanData], name: &str) -> Vec<&'a SpanData> {
    all.iter().filter(|s| s.name == name).collect()
}

/// Assert the child's `[start, end]` is contained in the parent's.
fn assert_contained(child: &SpanData, parent: &SpanData, what: &str) {
    assert!(
        child.start_time >= parent.start_time && child.end_time <= parent.end_time,
        "{what}: child [{:?}..{:?}] must be contained in parent [{:?}..{:?}]",
        child.start_time,
        child.end_time,
        parent.start_time,
        parent.end_time
    );
}

/// Assert every exported span of the run shares one trace id.
fn assert_single_trace(all: &[SpanData]) {
    let trace_ids: HashSet<_> = all.iter().map(|s| s.span_context.trace_id()).collect();
    assert_eq!(
        trace_ids.len(),
        1,
        "the whole run must be a single trace, found {} trace ids",
        trace_ids.len()
    );
}

// ---------------------------------------------------------------------------
// Route wiring (mirrors otel_direct_hop_regression.rs)
// ---------------------------------------------------------------------------

fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

/// True once `route_id` reports the `Started` status.
async fn route_started(h: &CamelTestContext, route_id: &str) -> bool {
    let ctx = h.ctx().lock().await;
    matches!(
        ctx.runtime_route_status(route_id).await,
        Ok(Some(status)) if status == "Started"
    )
}

/// Poll until every route reports `Started`.
async fn wait_for_started(h: &CamelTestContext, route_ids: &[&str]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut all_started = true;
        for id in route_ids {
            if !route_started(h, id).await {
                all_started = false;
                break;
            }
        }
        if all_started {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "routes {route_ids:?} did not reach Started within 5s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Drive one InOut exchange through the `endpoint` direct pipeline (fresh
/// producer per attempt, retried on fast errors until `retry_window`
/// elapses, so startup registration races cannot masquerade as failures).
/// Returns the reply exchange on success.
async fn drive_direct_in_out(
    h: &CamelTestContext,
    endpoint: &str,
    body: &str,
    retry_window: Duration,
) -> Result<Exchange, CamelError> {
    let deadline = tokio::time::Instant::now() + retry_window;
    loop {
        let producer = {
            let ctx = h.ctx().lock().await;
            let producer_ctx = ctx.producer_context();
            let registry = ctx.registry();
            let component = registry
                .get("direct")
                .expect("direct component not registered");
            let endpoint = component
                .create_endpoint(endpoint, &*ctx)
                .expect("failed to create direct endpoint");
            endpoint
                .create_producer(test_rt(), &producer_ctx)
                .expect("failed to create direct producer")
        };
        match producer
            .oneshot(Exchange::new_in_out(Message::new(body)))
            .await
        {
            Ok(reply) => return Ok(reply),
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Route B: consumes `direct:tree-sub` with two no-op process steps, so a
/// visit produces exactly `tree-sub` + `tree-sub:step-0/1`. Lower
/// startup_order starts this consumer before the routes that call it.
fn tree_sub_route() -> camel_core::route::RouteDefinition {
    RouteBuilder::from("direct:tree-sub")
        .route_id("tree-sub")
        .startup_order(50)
        .process(|ex: Exchange| async move { Ok(ex) })
        .process(|ex: Exchange| async move { Ok(ex) })
        .build()
        .expect("tree-sub route builds")
}

/// Route A: three steps — no-op process (step-0), direct dispatch to
/// `tree-sub` (`to:direct` span, index 1), no-op process (step-2).
fn tree_main_route() -> camel_core::route::RouteDefinition {
    RouteBuilder::from("direct:tree-main")
        .route_id("tree-main")
        .startup_order(200)
        .process(|ex: Exchange| async move { Ok(ex) })
        .to("direct:tree-sub")
        .process(|ex: Exchange| async move { Ok(ex) })
        .build()
        .expect("tree-main route builds")
}

/// Route C: one split segment over a two-line body; each fragment
/// is dispatched to `direct:tree-sub` inside the segment.
fn tree_split_route() -> camel_core::route::RouteDefinition {
    RouteBuilder::from("direct:tree-split")
        .route_id("tree-split")
        .startup_order(200)
        .split(SplitterConfig::new(split_body_lines()).aggregation(AggregationStrategy::CollectAll))
        .to("direct:tree-sub")
        .end_split()
        .build()
        .expect("tree-split route builds")
}

// ---------------------------------------------------------------------------
// Tree 1: direct hop nests the sub-route root under the caller step
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_hop_nests_subroute_root_under_caller_step() {
    let spans = test_spans().await;
    let h = CamelTestContext::builder().with_direct().build().await;
    h.ctx().lock().await.set_tracing(true).await;

    h.add_route(tree_main_route()).await.expect("add tree-main");
    h.add_route(tree_sub_route()).await.expect("add tree-sub");
    h.start().await;
    wait_for_started(&h, &["tree-main", "tree-sub"]).await;

    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct_in_out(&h, "direct:tree-main", "hello", Duration::from_secs(4)),
    )
    .await
    .expect("exchange through tree-main timed out")
    .expect("exchange through tree-main failed");
    assert_eq!(reply.input.body.as_text(), Some("hello"));

    h.stop().await;
    let all = finish(spans);

    assert_single_trace(&all);

    // Route root span: one, named after the route, parentless.
    let root = span(&all, "tree-main");
    assert_eq!(
        root.parent_span_id,
        SpanId::INVALID,
        "tree-main root span must have no parent"
    );

    // Steps 0/1/2 are siblings under the root, each within its bounds. The
    // dispatch step carries its DSL label (`to:direct`); the closures keep
    // the positional fallback names.
    let step0 = span(&all, "tree-main:step-0");
    let step1 = span(&all, "tree-main:to:direct");
    let step2 = span(&all, "tree-main:step-2");
    let root_span_id = root.span_context.span_id();
    for (step, name) in [(step0, "step-0"), (step1, "to:direct"), (step2, "step-2")] {
        assert_eq!(
            step.parent_span_id, root_span_id,
            "tree-main:{name} must be parented by the tree-main root"
        );
        assert_contained(
            step,
            root,
            &format!("tree-main:{name} under tree-main root"),
        );
    }

    // The sub-route root nests under the dispatching step, not the root.
    let sub = span(&all, "tree-sub");
    assert_eq!(
        sub.parent_span_id,
        step1.span_context.span_id(),
        "tree-sub root must nest under tree-main:to:direct (the dispatching step)"
    );
    assert_contained(sub, step1, "tree-sub under tree-main:to:direct");

    // The sub-route's own steps nest under its root.
    let sub_step0 = span(&all, "tree-sub:step-0");
    let sub_step1 = span(&all, "tree-sub:step-1");
    let sub_span_id = sub.span_context.span_id();
    for (step, name) in [(sub_step0, "step-0"), (sub_step1, "step-1")] {
        assert_eq!(
            step.parent_span_id, sub_span_id,
            "tree-sub:{name} must be parented by the tree-sub root"
        );
        assert_contained(step, sub, &format!("tree-sub:{name} under tree-sub root"));
    }

    // Sequential sibling ordering: the dispatch step starts only after the
    // previous step has ended.
    assert!(
        step1.start_time >= step0.end_time,
        "tree-main:to:direct must start after tree-main:step-0 ends"
    );
}

// ---------------------------------------------------------------------------
// Tree 2: split fragments nest under the segment span, one trace
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_fragments_nest_under_segment_span_one_trace() {
    let spans = test_spans().await;
    let h = CamelTestContext::builder().with_direct().build().await;
    h.ctx().lock().await.set_tracing(true).await;

    h.add_route(tree_split_route())
        .await
        .expect("add tree-split");
    h.add_route(tree_sub_route()).await.expect("add tree-sub");
    h.start().await;
    wait_for_started(&h, &["tree-split", "tree-sub"]).await;

    // Two lines -> two fragments, each dispatched to direct:tree-sub.
    let _reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct_in_out(
            &h,
            "direct:tree-split",
            "alpha\nbeta",
            Duration::from_secs(4),
        ),
    )
    .await
    .expect("exchange through tree-split timed out")
    .expect("exchange through tree-split failed");

    h.stop().await;
    let all = finish(spans);

    assert_single_trace(&all);

    // Route root span: one, named after the route, parentless.
    let root = span(&all, "tree-split");
    assert_eq!(
        root.parent_span_id,
        SpanId::INVALID,
        "tree-split root span must have no parent"
    );

    // The split step compiles to a segment: its labeled attempt span
    // (`split`) is a direct child of the route root.
    let segment = span(&all, "tree-split:split");
    assert_eq!(
        segment.parent_span_id,
        root.span_context.span_id(),
        "tree-split:split (segment span) must be parented by the tree-split root"
    );

    // One tree-sub root per fragment, each nesting under the segment span
    // (not the route root, not each other).
    let subs = spans_named(&all, "tree-sub");
    assert_eq!(subs.len(), 2, "one tree-sub root per split fragment");
    let segment_span_id = segment.span_context.span_id();
    for sub in &subs {
        assert_eq!(
            sub.parent_span_id, segment_span_id,
            "fragment tree-sub root must nest under tree-split:split"
        );
    }
}

// ---------------------------------------------------------------------------
// Regression guard: direct-only routes keep every span Internal
// ---------------------------------------------------------------------------

/// Task 1.3 (span-kind-hint): `direct:` steps (and every other non-hinted
/// step) map `Internal`; kind threading must never leak a hinted kind into
/// route root spans or the split segment span. Runs BOTH tree scenarios in
/// one trace-free sweep and asserts the kinds: every route root, every step
/// span, and the split segment span report `SpanKind::Internal`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_and_segment_stay_internal() {
    let spans = test_spans().await;
    let h = CamelTestContext::builder().with_direct().build().await;
    h.ctx().lock().await.set_tracing(true).await;

    h.add_route(tree_main_route()).await.expect("add tree-main");
    h.add_route(tree_sub_route()).await.expect("add tree-sub");
    h.add_route(tree_split_route())
        .await
        .expect("add tree-split");
    h.start().await;
    wait_for_started(&h, &["tree-main", "tree-sub", "tree-split"]).await;

    // Scenario 1 (tree 1): direct hop tree-main -> tree-sub.
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct_in_out(&h, "direct:tree-main", "hello", Duration::from_secs(4)),
    )
    .await
    .expect("exchange through tree-main timed out")
    .expect("exchange through tree-main failed");
    assert_eq!(reply.input.body.as_text(), Some("hello"));

    // Scenario 2 (tree 2): split tree-split -> one tree-sub per fragment.
    let _reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct_in_out(
            &h,
            "direct:tree-split",
            "alpha\nbeta",
            Duration::from_secs(4),
        ),
    )
    .await
    .expect("exchange through tree-split timed out")
    .expect("exchange through tree-split failed");

    h.stop().await;
    let all = finish(spans);

    // Named anchors from both scenarios: route roots, closure step spans,
    // the labeled dispatch step, and the split segment span.
    for name in [
        "tree-main",
        "tree-main:step-0",
        "tree-main:to:direct",
        "tree-main:step-2",
        "tree-split",
        "tree-split:split",
    ] {
        assert_eq!(
            span(&all, name).span_kind,
            SpanKind::Internal,
            "{name} must stay Internal"
        );
    }

    // tree-sub roots: one from the tree-1 hop plus one per split fragment.
    let subs = spans_named(&all, "tree-sub");
    assert_eq!(subs.len(), 3, "one tree-sub root per hop/fragment");
    for sub in &subs {
        assert_eq!(
            sub.span_kind,
            SpanKind::Internal,
            "tree-sub route root must stay Internal"
        );
    }

    // Blanket: direct-only routes never hint a kind, so every exported
    // span of both scenarios — roots, steps, segment span alike — stays
    // Internal.
    for s in &all {
        assert_eq!(
            s.span_kind,
            SpanKind::Internal,
            "span {} must stay Internal for direct-only routes",
            s.name
        );
    }
}
