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
//! Tree 3 (split forest tests): when the fragment count exceeds the
//! splitter's `trace_item_threshold`, every fragment instead gets its own
//! `{route_id}:split-item` root span — no parent span id, a fresh trace id,
//! exactly one link to the `{route_id}:split` segment span's context, and
//! `split.item.index`/`split.item.total` attributes — and the fragment
//! sub-route roots nest under their item root. At or below the threshold
//! (explicit, zero, or the default 100) the run stays the Tree 2 nested
//! shape with no links. One variant drives the same forest shape through a
//! hand-built `BuilderStep::DeclarativeSplit` route (simple `${body}`
//! expression) to cover the declarative compiler arm.
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
use camel_api::{CamelError, Exchange, LanguageExpressionDef, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::test_support::acquire_deadline;
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

/// The `i64` value of a span attribute, if present and numeric.
fn attr_i64(s: &SpanData, key: &str) -> Option<i64> {
    s.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .and_then(|kv| match kv.value {
            opentelemetry::Value::I64(n) => Some(n),
            _ => None,
        })
}

/// Assert the split-forest shape for `route_id` over a split whose fragment
/// count exceeded the trace-item threshold:
///
/// - the route root and the `{route_id}:split` segment span share the
///   route trace;
/// - exactly `fragment_count` `{route_id}:split-item` roots exist, each
///   parentless, on its own fresh trace (pairwise distinct, none equal to
///   the route trace), carrying exactly one link to the segment span's
///   context plus `split.item.index`/`split.item.total` attributes;
/// - each fragment's `tree-sub` sub-route root nests under its item root,
///   exactly one sub-route per item root;
/// - no span other than the item roots carries links.
fn assert_split_forest_shape(all: &[SpanData], route_id: &str, fragment_count: usize) {
    let root = span(all, route_id);
    let segment = span(all, &format!("{route_id}:split"));
    let item_name = format!("{route_id}:split-item");
    let items = spans_named(all, &item_name);
    assert_eq!(
        items.len(),
        fragment_count,
        "exactly one {item_name} root per fragment"
    );

    let route_trace = root.span_context.trace_id();
    assert_eq!(
        segment.span_context.trace_id(),
        route_trace,
        "{route_id} root and split segment must share the route trace"
    );

    let mut item_traces = HashSet::new();
    let mut item_indices = HashSet::new();
    for item in &items {
        assert_eq!(
            item.parent_span_id,
            SpanId::INVALID,
            "{item_name} root must have no parent span"
        );
        assert_ne!(
            item.span_context.trace_id(),
            route_trace,
            "{item_name} root must start a fresh trace"
        );
        item_traces.insert(item.span_context.trace_id());
        item_indices.insert(
            attr_i64(item, "split.item.index")
                .unwrap_or_else(|| panic!("{item_name} root missing split.item.index")),
        );
        assert_eq!(
            attr_i64(item, "split.item.total"),
            Some(fragment_count as i64),
            "{item_name} root must stamp split.item.total"
        );

        assert_eq!(
            item.links.len(),
            1,
            "{item_name} root must carry exactly one link"
        );
        let linked = &item.links.links[0].span_context;
        assert_eq!(
            linked.trace_id(),
            segment.span_context.trace_id(),
            "item root link must target the split segment's trace"
        );
        assert_eq!(
            linked.span_id(),
            segment.span_context.span_id(),
            "item root link must target the split segment's span"
        );
    }
    assert_eq!(
        item_traces.len(),
        fragment_count,
        "item roots must not share traces with each other"
    );
    let expected_indices: HashSet<_> = (0..fragment_count as i64).collect();
    assert_eq!(
        item_indices, expected_indices,
        "item indices must cover 0..{fragment_count}"
    );

    // Only the item roots carry links.
    for s in all {
        if s.name.as_ref() == item_name {
            continue;
        }
        assert!(
            s.links.links.is_empty(),
            "span {} must not carry links; only {item_name} roots do",
            s.name
        );
    }

    // Each fragment's sub-route root nests under its item root (the active
    // span inside the fragment body), exactly one sub-route per item.
    let item_ids: HashSet<_> = items.iter().map(|i| i.span_context.span_id()).collect();
    let nested: Vec<&SpanData> = spans_named(all, "tree-sub")
        .into_iter()
        .filter(|sub| item_ids.contains(&sub.parent_span_id))
        .collect();
    assert_eq!(
        nested.len(),
        fragment_count,
        "one tree-sub root per fragment must nest under its {item_name} root"
    );
    for sub in &nested {
        let parent = items
            .iter()
            .find(|item| item.span_context.span_id() == sub.parent_span_id)
            .expect("parent item root looked up from its span id");
        assert_contained(sub, parent, "fragment sub-route under its item root");
    }
    for item in &items {
        let children = nested
            .iter()
            .filter(|sub| sub.parent_span_id == item.span_context.span_id())
            .count();
        assert_eq!(
            children, 1,
            "each {item_name} root must host exactly one fragment sub-route"
        );
    }
}

// ---------------------------------------------------------------------------
// Route wiring (mirrors otel_direct_hop_regression.rs)
// ---------------------------------------------------------------------------

fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

/// True once `route_id` reports the `Started` status.
async fn route_started(h: &CamelTestContext, route_id: &str) -> bool {
    let ctx = acquire_deadline(
        h.ctx(),
        "camel context (route_started)",
        Duration::from_secs(10),
    )
    .await;
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
            if !tokio::time::timeout(Duration::from_secs(5), route_started(h, id))
                .await
                .expect("route status poll stalled in wait_for_started")
            {
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
    // Anti-wedge backstop (lintwiden D4.1S): the retry loop's own deadline
    // governs startup-race exhaustion; this outer bound only turns a stalled
    // ctx-lock or producer await into a loud failure instead of a wedge.
    tokio::time::timeout(retry_window + Duration::from_secs(5), async {
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
    })
    .await
    .expect("drive_direct_in_out stalled beyond its retry deadline")
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

/// An `n`-line body (`line0\nline1\n...`), one line per split fragment.
fn line_body(fragments: usize) -> String {
    (0..fragments)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A programmatic split route like [`tree_split_route`] under an explicit
/// id and splitter config, so tests can tune the fragment count (at drive
/// time, via the body) and the trace knob (`trace_item_threshold`) without
/// touching the shared routes. Each fragment dispatches to
/// `direct:tree-sub` inside the segment.
fn tree_split_route_with(
    route_id: &str,
    config: SplitterConfig,
) -> camel_core::route::RouteDefinition {
    RouteBuilder::from(&format!("direct:{route_id}"))
        .route_id(route_id)
        .startup_order(200)
        .split(config)
        .to("direct:tree-sub")
        .end_split()
        .build()
        .expect("split route builds")
}

/// A declarative-language split route: the split step is a
/// `BuilderStep::DeclarativeSplit` over a simple `${body}` expression (the
/// expression result is split by lines), each fragment dispatched to
/// `direct:tree-sub`. Mirrors how canonical/YAML routes express
/// language-driven splits, with the trace knob riding on the step itself.
fn tree_declarative_split_route() -> camel_core::route::RouteDefinition {
    camel_core::route::RouteDefinition::new(
        "direct:tree-dsplit",
        vec![camel_core::route::BuilderStep::DeclarativeSplit {
            expression: LanguageExpressionDef {
                language: "simple".to_string(),
                source: "${body}".to_string(),
            },
            aggregation: AggregationStrategy::LastWins,
            parallel: false,
            parallel_limit: None,
            trace_item_threshold: Some(2),
            stop_on_exception: true,
            steps: vec![camel_core::route::BuilderStep::To(
                "direct:tree-sub".to_string(),
            )],
        }],
    )
    .with_route_id("tree-dsplit")
    .with_startup_order(200)
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
// Tree 3: split fragments above the threshold restart traces (forest)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_above_threshold_starts_item_traces_with_links() {
    let spans = test_spans().await;
    let h = CamelTestContext::builder().with_direct().build().await;
    h.ctx().lock().await.set_tracing(true).await; // allow-test-wait: harness ctx lock for tracing toggle — momentary, uncontended in-test (ADR-0069 §13.2 R1)

    let config = SplitterConfig::new(split_body_lines())
        .aggregation(AggregationStrategy::CollectAll)
        .trace_item_threshold(2);
    h.add_route(tree_split_route_with("tree-split-forest", config))
        .await
        .expect("add tree-split-forest");
    h.add_route(tree_sub_route()).await.expect("add tree-sub");
    h.start().await;
    wait_for_started(&h, &["tree-split-forest", "tree-sub"]).await;

    // Three lines -> three fragments, above the threshold of 2.
    let _reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct_in_out(
            &h,
            "direct:tree-split-forest",
            &line_body(3),
            Duration::from_secs(4),
        ),
    )
    .await
    .expect("exchange through tree-split-forest timed out")
    .expect("exchange through tree-split-forest failed");

    h.stop().await;
    let all = finish(spans);

    assert_split_forest_shape(&all, "tree-split-forest", 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_below_threshold_stays_single_trace() {
    let spans = test_spans().await;
    let h = CamelTestContext::builder().with_direct().build().await;
    h.ctx().lock().await.set_tracing(true).await; // allow-test-wait: harness ctx lock for tracing toggle — momentary, uncontended in-test (ADR-0069 §13.2 R1)

    let config = SplitterConfig::new(split_body_lines())
        .aggregation(AggregationStrategy::CollectAll)
        .trace_item_threshold(2);
    h.add_route(tree_split_route_with("tree-split-nested", config))
        .await
        .expect("add tree-split-nested");
    h.add_route(tree_sub_route()).await.expect("add tree-sub");
    h.start().await;
    wait_for_started(&h, &["tree-split-nested", "tree-sub"]).await;

    // Two lines -> two fragments, at (not above) the threshold of 2.
    let _reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct_in_out(
            &h,
            "direct:tree-split-nested",
            &line_body(2),
            Duration::from_secs(4),
        ),
    )
    .await
    .expect("exchange through tree-split-nested timed out")
    .expect("exchange through tree-split-nested failed");

    h.stop().await;
    let all = finish(spans);

    // Legacy shape: one trace, no links, no split-item roots.
    assert_single_trace(&all);
    assert!(
        all.iter().all(|s| s.links.links.is_empty()),
        "no span may carry links at the threshold"
    );
    assert!(
        all.iter().all(|s| !s.name.ends_with(":split-item")),
        "no split-item roots may exist at the threshold"
    );

    // Same nesting as split_fragments_nest_under_segment_span_one_trace:
    // segment under the route root, fragment sub-routes under the segment.
    let root = span(&all, "tree-split-nested");
    assert_eq!(
        root.parent_span_id,
        SpanId::INVALID,
        "tree-split-nested root span must have no parent"
    );
    let segment = span(&all, "tree-split-nested:split");
    assert_eq!(
        segment.parent_span_id,
        root.span_context.span_id(),
        "tree-split-nested:split must be parented by the route root"
    );
    let subs = spans_named(&all, "tree-sub");
    assert_eq!(subs.len(), 2, "one tree-sub root per split fragment");
    for sub in &subs {
        assert_eq!(
            sub.parent_span_id,
            segment.span_context.span_id(),
            "fragment tree-sub root must nest under tree-split-nested:split"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_trace_item_threshold_zero_keeps_nested() {
    let spans = test_spans().await;
    let h = CamelTestContext::builder().with_direct().build().await;
    h.ctx().lock().await.set_tracing(true).await; // allow-test-wait: harness ctx lock for tracing toggle — momentary, uncontended in-test (ADR-0069 §13.2 R1)

    // Threshold 0 disables trace restart entirely.
    let config = SplitterConfig::new(split_body_lines())
        .aggregation(AggregationStrategy::CollectAll)
        .trace_item_threshold(0);
    h.add_route(tree_split_route_with("tree-split-zero", config))
        .await
        .expect("add tree-split-zero");
    h.add_route(tree_sub_route()).await.expect("add tree-sub");
    h.start().await;
    wait_for_started(&h, &["tree-split-zero", "tree-sub"]).await;

    // 500 lines -> 500 fragments, but the knob is off.
    let _reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct_in_out(
            &h,
            "direct:tree-split-zero",
            &line_body(500),
            Duration::from_secs(4),
        ),
    )
    .await
    .expect("exchange through tree-split-zero timed out")
    .expect("exchange through tree-split-zero failed");

    h.stop().await;
    let all = finish(spans);

    assert_single_trace(&all);
    assert!(
        all.iter().all(|s| s.links.links.is_empty()),
        "no span may carry links when the knob is off"
    );
    assert!(
        all.iter().all(|s| !s.name.ends_with(":split-item")),
        "no split-item roots may exist when the knob is off"
    );
    let segment = span(&all, "tree-split-zero:split");
    let subs = spans_named(&all, "tree-sub");
    assert_eq!(subs.len(), 500, "one tree-sub root per split fragment");
    for sub in &subs {
        assert_eq!(
            sub.parent_span_id,
            segment.span_context.span_id(),
            "fragment tree-sub root must nest under tree-split-zero:split"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_default_threshold_hundred_applies() {
    let spans = test_spans().await;
    let h = CamelTestContext::builder().with_direct().build().await;
    h.ctx().lock().await.set_tracing(true).await; // allow-test-wait: harness ctx lock for tracing toggle — momentary, uncontended in-test (ADR-0069 §13.2 R1)

    // Both routes use SplitterConfig::new's default (threshold 100) — the
    // knob is never set explicitly.
    h.add_route(tree_split_route_with(
        "tree-split-dflt",
        SplitterConfig::new(split_body_lines()).aggregation(AggregationStrategy::CollectAll),
    ))
    .await
    .expect("add tree-split-dflt");
    h.add_route(tree_split_route_with(
        "tree-split-dflt-many",
        SplitterConfig::new(split_body_lines()).aggregation(AggregationStrategy::CollectAll),
    ))
    .await
    .expect("add tree-split-dflt-many");
    h.add_route(tree_sub_route()).await.expect("add tree-sub");
    h.start().await;
    wait_for_started(&h, &["tree-split-dflt", "tree-split-dflt-many", "tree-sub"]).await;

    // Route A: 100 fragments, at the default threshold -> nested.
    let _reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct_in_out(
            &h,
            "direct:tree-split-dflt",
            &line_body(100),
            Duration::from_secs(4),
        ),
    )
    .await
    .expect("exchange through tree-split-dflt timed out")
    .expect("exchange through tree-split-dflt failed");

    // Route B: 101 fragments, above the default threshold -> forest.
    let _reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct_in_out(
            &h,
            "direct:tree-split-dflt-many",
            &line_body(101),
            Duration::from_secs(4),
        ),
    )
    .await
    .expect("exchange through tree-split-dflt-many timed out")
    .expect("exchange through tree-split-dflt-many failed");

    h.stop().await;
    let all = finish(spans);

    // A-side: default threshold keeps an at-threshold split nested and
    // link-free (restart only kicks in above 100).
    let root_a = span(&all, "tree-split-dflt");
    let segment_a = span(&all, "tree-split-dflt:split");
    let trace_a = root_a.span_context.trace_id();
    assert_eq!(
        segment_a.span_context.trace_id(),
        trace_a,
        "tree-split-dflt root and segment must share one trace"
    );
    assert!(
        spans_named(&all, "tree-split-dflt:split-item").is_empty(),
        "100 fragments must not exceed the default threshold of 100"
    );
    let subs_a: Vec<_> = spans_named(&all, "tree-sub")
        .into_iter()
        .filter(|s| s.span_context.trace_id() == trace_a)
        .collect();
    assert_eq!(subs_a.len(), 100, "one tree-sub root per route-A fragment");
    for sub in &subs_a {
        assert_eq!(
            sub.parent_span_id,
            segment_a.span_context.span_id(),
            "route-A fragment tree-sub root must nest under the segment span"
        );
    }
    for s in all.iter().filter(|s| s.span_context.trace_id() == trace_a) {
        assert!(
            s.links.links.is_empty(),
            "route-A span {} must stay link-free below the default threshold",
            s.name
        );
    }

    // B-side: 101 fragments restart traces under the default threshold.
    assert_split_forest_shape(&all, "tree-split-dflt-many", 101);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declarative_split_forest_above_threshold() {
    let spans = test_spans().await;
    let h = CamelTestContext::builder().with_direct().build().await;
    h.ctx().lock().await.set_tracing(true).await; // allow-test-wait: harness ctx lock for tracing toggle — momentary, uncontended in-test (ADR-0069 §13.2 R1)

    h.add_route(tree_declarative_split_route())
        .await
        .expect("add tree-dsplit");
    h.add_route(tree_sub_route()).await.expect("add tree-sub");
    h.start().await;
    wait_for_started(&h, &["tree-dsplit", "tree-sub"]).await;

    // Three lines -> three fragments, above the threshold of 2; the
    // simple ${body} expression hands the splitter the body text, which
    // the declarative arm splits by lines.
    let _reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct_in_out(
            &h,
            "direct:tree-dsplit",
            &line_body(3),
            Duration::from_secs(4),
        ),
    )
    .await
    .expect("exchange through tree-dsplit timed out")
    .expect("exchange through tree-dsplit failed");

    h.stop().await;
    let all = finish(spans);

    assert_split_forest_shape(&all, "tree-dsplit", 3);
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
