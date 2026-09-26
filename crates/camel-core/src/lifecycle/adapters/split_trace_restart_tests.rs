//! Split trace-restart span tests (splittrace 2.1).
//!
//! Span-exporting tests use `span_test_util`: ONE global in-memory provider
//! per test binary, test bodies serialized by a process-wide mutex, spans
//! filtered by the trace id each test set up itself. The pipelines are built
//! by hand — a `camel_processor::SplitSegment` whose body is
//! `TraceRestartBody::wrap(step_segment, "r", threshold)` — driven through
//! `compose_traced_pipeline` so the `r:split` segment span exists as the
//! link target.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use camel_api::{
    AggregationStrategy, Body, CamelError, Exchange, Message, OutcomePipeline, OutcomeSegment,
    PipelineOutcome, SplitExpression,
};
use camel_processor::SplitSegment;
use opentelemetry::global;
use opentelemetry::trace::{Span, SpanId, Status, Tracer};
use opentelemetry_sdk::trace::SpanData;
use tower::{Service, ServiceExt};

use crate::lifecycle::adapters::route_compiler::{PipelineRuntimeCtx, compose_traced_pipeline};
use crate::lifecycle::adapters::step_compilers::CompiledStep;
use crate::lifecycle::adapters::trace_restart::TraceRestartBody;
use crate::shared::observability::adapters::span_test_util::{finish, test_spans};
use crate::shared::observability::domain::DetailLevel;

/// Per-fragment body: opens one `item-step` child span on the incoming
/// context (standing in for the compiled step spans), then completes.
#[derive(Clone)]
struct SpanningSegment;

impl OutcomePipeline for SpanningSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            let tracer = global::tracer("camel-core-test");
            let entry = exchange.otel_context.clone();
            let mut span = tracer
                .span_builder("item-step")
                .start_with_context(&tracer, &entry);
            span.end();
            PipelineOutcome::Completed(exchange)
        })
    }
}

/// Per-fragment body that fails on one 0-indexed fragment, completes on the
/// rest.
#[derive(Clone)]
struct FailOnNthSegment {
    counter: Arc<AtomicUsize>,
    fail_at: usize,
}

impl OutcomePipeline for FailOnNthSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        let count = self.counter.fetch_add(1, Ordering::SeqCst);
        let fail_at = self.fail_at;
        Box::pin(async move {
            if count == fail_at {
                PipelineOutcome::Failed(CamelError::ProcessorError("item boom".into()))
            } else {
                PipelineOutcome::Completed(exchange)
            }
        })
    }
}

/// Splitter yielding one text fragment per input line (the custom-splitter
/// pattern from `split_segment.rs` tests).
fn line_splitter() -> SplitExpression {
    Arc::new(|ex: &Exchange| {
        Ok(ex
            .input
            .body
            .as_text()
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let mut frag = ex.clone();
                frag.input.body = Body::Text(line.to_string());
                frag
            })
            .collect())
    })
}

/// Drive a hand-built split segment (body = `body`, one fragment per line of
/// `input`) through a one-step traced pipeline rooted at `r`, and return the
/// pipeline result. The caller owns the span harness guard.
async fn drive_split(
    body: OutcomeSegment,
    input: &str,
    parallel: bool,
) -> Result<Exchange, CamelError> {
    let split = SplitSegment {
        splitter: line_splitter(),
        body,
        parallel,
        parallel_limit: None,
        stop_on_exception: true,
        aggregation: AggregationStrategy::LastWins,
    };
    let mut pipeline = compose_traced_pipeline(
        vec![CompiledStep::Segment {
            segment: OutcomeSegment::new(Box::new(split)),
            body_contract: None,
            lifecycle: None,
            label: Some("split".into()),
        }],
        "r",
        true,
        DetailLevel::Minimal,
        None,
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    pipeline
        .ready()
        .await
        .expect("pipeline ready")
        .call(Exchange::new(Message::new(input)))
        .await
}

/// The single exported span named `name`, or panic with the actual names.
fn find_one<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    let matches: Vec<&SpanData> = spans.iter().filter(|s| s.name == name).collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one {name} span, got {}",
        spans
            .iter()
            .map(|s| s.name.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    matches[0]
}

/// The `i64` attribute value under `key`, if present.
fn attr_i64(span: &SpanData, key: &str) -> Option<i64> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .and_then(|kv| match kv.value {
            opentelemetry::Value::I64(v) => Some(v),
            _ => None,
        })
}

/// Every exported span id for spans named `name`.
fn span_ids(spans: &[SpanData], name: &str) -> Vec<SpanId> {
    spans
        .iter()
        .filter(|s| s.name == name)
        .map(|s| s.span_context.span_id())
        .collect()
}

#[tokio::test]
async fn at_threshold_stays_nested_single_trace() {
    let spans = test_spans().await;
    let body = TraceRestartBody::wrap(
        OutcomeSegment::new(Box::new(SpanningSegment)),
        "r".into(),
        2,
    );
    let result = drive_split(body, "a\nb", false).await;
    assert!(result.is_ok(), "split at threshold must complete");

    let all = finish(spans);
    assert!(!all.is_empty(), "spans exported");
    let trace_ids: HashSet<_> = all.iter().map(|s| s.span_context.trace_id()).collect();
    assert_eq!(
        trace_ids.len(),
        1,
        "all spans share one trace id, got {trace_ids:?}"
    );
    assert!(
        all.iter().all(|s| s.links.is_empty()),
        "no exported span carries links"
    );
    assert_eq!(
        all.iter().filter(|s| s.name == "item-step").count(),
        2,
        "one step span per fragment"
    );
    let split = find_one(&all, "r:split");
    for step in all.iter().filter(|s| s.name == "item-step") {
        assert_eq!(
            step.parent_span_id,
            split.span_context.span_id(),
            "step span nests under the split segment span"
        );
    }
}

#[tokio::test]
async fn above_threshold_one_trace_per_item_with_link() {
    let spans = test_spans().await;
    let body = TraceRestartBody::wrap(
        OutcomeSegment::new(Box::new(SpanningSegment)),
        "r".into(),
        2,
    );
    let result = drive_split(body, "a\nb\nc", false).await;
    assert!(result.is_ok(), "split above threshold must complete");

    let all = finish(spans);
    let split = find_one(&all, "r:split");
    let root = find_one(&all, "r");
    let root_trace = root.span_context.trace_id();

    let items: Vec<&SpanData> = all.iter().filter(|s| s.name == "r:split-item").collect();
    assert_eq!(items.len(), 3, "one item root per fragment");

    let mut seen_traces = HashSet::new();
    for item in &items {
        assert_ne!(
            item.span_context.trace_id(),
            root_trace,
            "item trace differs from the route root trace"
        );
        assert!(
            seen_traces.insert(item.span_context.trace_id()),
            "item root traces are pairwise distinct"
        );
        assert_eq!(
            item.parent_span_id,
            SpanId::INVALID,
            "item root has no parent span id"
        );
        assert_eq!(item.links.len(), 1, "exactly one link per item root");
        assert_eq!(
            &item.links[0].span_context, &split.span_context,
            "link targets the split segment span context"
        );
    }
    let mut indexes: Vec<i64> = items
        .iter()
        .filter_map(|s| attr_i64(s, "split.item.index"))
        .collect();
    indexes.sort_unstable();
    assert_eq!(
        indexes,
        vec![0, 1, 2],
        "index attributes cover all fragments"
    );
    assert!(
        items
            .iter()
            .all(|s| attr_i64(s, "split.item.total") == Some(3)),
        "total attribute is the fragment count"
    );
    for item in &items {
        let steps: Vec<&SpanData> = all
            .iter()
            .filter(|s| {
                s.name == "item-step" && s.span_context.trace_id() == item.span_context.trace_id()
            })
            .collect();
        assert_eq!(steps.len(), 1, "one step span inside the item trace");
        assert_eq!(
            steps[0].parent_span_id,
            item.span_context.span_id(),
            "step span nests under its item root"
        );
    }
}

#[tokio::test]
async fn zero_threshold_disables_restart() {
    let spans = test_spans().await;
    // Mirrors the compiler gate: threshold 0 wires the body WITHOUT the
    // wrapper, so the run keeps the legacy nested shape.
    let body = OutcomeSegment::new(Box::new(SpanningSegment));
    let result = drive_split(body, "a\nb\nc\nd", false).await;
    assert!(result.is_ok(), "unwrapped split must complete");

    let all = finish(spans);
    let trace_ids: HashSet<_> = all.iter().map(|s| s.span_context.trace_id()).collect();
    assert_eq!(trace_ids.len(), 1, "all spans share one trace id");
    assert!(
        all.iter().all(|s| s.links.is_empty()),
        "no exported span carries links"
    );
    assert!(
        all.iter().all(|s| s.name != "r:split-item"),
        "no item root spans without the wrapper"
    );
    assert_eq!(
        all.iter().filter(|s| s.name == "item-step").count(),
        4,
        "one step span per fragment"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn parallel_above_threshold_links_survive() {
    // Lock the harness FIRST: every span creation in this test must happen
    // under the process-wide guard so concurrent tests cannot see (nor be
    // seen by) these spans. The sequential reference only contributes its
    // aggregated exchange body — its incidental item-step spans carry names
    // and trace ids no assertion below filters on.
    let spans = test_spans().await;
    let mut seq = SplitSegment {
        splitter: line_splitter(),
        body: OutcomeSegment::new(Box::new(SpanningSegment)),
        parallel: false,
        parallel_limit: None,
        stop_on_exception: true,
        aggregation: AggregationStrategy::LastWins,
    };
    let seq_body =
        match camel_api::OutcomePipeline::run(&mut seq, Exchange::new(Message::new("a\nb\nc")))
            .await
        {
            PipelineOutcome::Completed(ex) => ex.input.body.as_text().map(str::to_owned),
            other => panic!("sequential reference completes, got {other:?}"),
        };

    let body = TraceRestartBody::wrap(
        OutcomeSegment::new(Box::new(SpanningSegment)),
        "r".into(),
        1,
    );
    let result = drive_split(body, "a\nb\nc", true).await;
    let par_body = result.expect("parallel split completes");
    assert_eq!(
        par_body.input.body.as_text().map(str::to_owned),
        seq_body,
        "parallel aggregation matches the sequential result"
    );

    let all = finish(spans);
    let split = find_one(&all, "r:split");
    let root = find_one(&all, "r");
    let items: Vec<&SpanData> = all.iter().filter(|s| s.name == "r:split-item").collect();
    assert_eq!(items.len(), 3, "one item root per fragment");
    let item_ids: HashSet<_> = span_ids(&all, "r:split-item").into_iter().collect();
    assert_eq!(item_ids.len(), 3, "item roots are distinct spans");
    for item in &items {
        assert_ne!(
            item.span_context.trace_id(),
            root.span_context.trace_id(),
            "item trace differs from the route root trace"
        );
        assert_eq!(
            item.parent_span_id,
            SpanId::INVALID,
            "item root has no parent span id"
        );
        assert_eq!(item.links.len(), 1, "exactly one link per item root");
        assert_eq!(
            &item.links[0].span_context, &split.span_context,
            "link targets the split segment span context"
        );
    }
}

#[tokio::test]
async fn failed_fragment_marks_item_span_error() {
    let spans = test_spans().await;
    let body = TraceRestartBody::wrap(
        OutcomeSegment::new(Box::new(FailOnNthSegment {
            counter: Arc::new(AtomicUsize::new(0)),
            fail_at: 1,
        })),
        "r".into(),
        1,
    );
    let result = drive_split(body, "a\nb", false).await;
    assert!(result.is_err(), "failed fragment must propagate");

    let all = finish(spans);
    let items: Vec<&SpanData> = all.iter().filter(|s| s.name == "r:split-item").collect();
    assert_eq!(items.len(), 2, "both item roots ended and exported");
    let failed: Vec<&&SpanData> = items
        .iter()
        .filter(|s| matches!(s.status, Status::Error { .. }))
        .collect();
    assert_eq!(failed.len(), 1, "exactly the failing item root is in error");
    assert_eq!(
        attr_i64(failed[0], "split.item.index"),
        Some(1),
        "the failing item root is the second fragment"
    );
    assert_eq!(
        failed[0].events.len(),
        1,
        "one exception event on the failed item root"
    );
    assert_eq!(failed[0].events[0].name, "exception");
    let ok = items
        .iter()
        .find(|s| matches!(s.status, Status::Ok))
        .expect("the healthy item root is Ok");
    assert_eq!(
        attr_i64(ok, "split.item.index"),
        Some(0),
        "the healthy item root is the first fragment"
    );
}
