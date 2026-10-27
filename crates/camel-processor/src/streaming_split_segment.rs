//! ## Stop semantics (ADR-0025)
//!
//! This segment implements `OutcomePipeline` and propagates `PipelineOutcome::Stopped(ex)` with the exchange state intact (including mutations made inside the segment body before Stop fired). See ADR-0025 §3 (stopped-exchange-state-preservation invariant).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::{StreamExt, pin_mut};

use camel_api::{
    AggregationStrategy, Body, CamelError, Exchange, OutcomePipeline, OutcomeSegment,
    PipelineOutcome, StreamingSplitExpression, Value,
};

use crate::split_segment::aggregate_completed;
use crate::splitter::{CAMEL_SPLIT_COMPLETE, CAMEL_SPLIT_INDEX, CAMEL_SPLIT_SIZE};

// ── StreamingSplitSegment (ADR-0025 OutcomePipeline) ─────────────────

/// Outcome-aware StreamingSplit segment. Consumes a stream of fragment
/// Exchanges via `expression(exchange)`, runs `body` on each sequentially.
/// On Stop: return `Stopped(fragment_ex)` immediately; the `stream` local is
/// dropped when this future returns, closing the underlying stream resource.
/// Aggregation is SKIPPED when Stop fires (spec §5.3).
///
/// NOTE: No `CancellationToken` — the stream-drop path (route
/// shutdown) is handled by Drop semantics on the source stream future.
/// Adding an internal cancel token + `tokio::select!` adds dead complexity:
/// nothing external can fire the token (it's private), and the same-future
/// cancel-after-Stop branch can never win (we've already returned).
pub struct StreamingSplitSegment {
    /// Produces a stream of fragment exchanges from the incoming exchange.
    pub expression: StreamingSplitExpression,
    /// The sub-pipeline executed for each fragment.
    pub body: OutcomeSegment,
    /// Strategy for aggregating fragment results.
    pub aggregation: AggregationStrategy,
    /// Whether to stop processing on the first exception.
    ///
    /// When `true`, a `Failed` outcome from any fragment halts processing
    /// immediately (matching legacy behavior). When `false`, the error is
    /// collected and processing continues; the last error is propagated
    /// after the stream is exhausted.
    ///
    /// `Stopped` outcomes always propagate immediately regardless of this
    /// flag (per ADR-0025 §7 — Stop is successful control flow).
    pub stop_on_exception: bool,
}

impl Clone for StreamingSplitSegment {
    fn clone(&self) -> Self {
        Self {
            expression: Arc::clone(&self.expression),
            body: self.body.clone(),
            aggregation: self.aggregation.clone(),
            stop_on_exception: self.stop_on_exception,
        }
    }
}

impl OutcomePipeline for StreamingSplitSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        let expression = Arc::clone(&self.expression);
        let mut body = self.body.clone();
        let aggregation = self.aggregation.clone();
        let stop_on_exception = self.stop_on_exception;

        Box::pin(async move {
            let mut original = exchange.branch_clone();
            if matches!(original.input.body, Body::Stream(_)) {
                original.input.body = Body::Empty;
            }
            let stream = expression(exchange);
            pin_mut!(stream);

            let mut outputs: Vec<Exchange> = Vec::new();
            let mut last_error: Option<CamelError> = None;
            let mut index: u64 = 0;

            // One-entry lookahead: the last fragment is only known once the
            // following stream item arrives, which is what allows
            // CamelSplitComplete and CamelSplitSize to be stamped on it.
            let mut current = stream.next().await;

            while let Some(frag_result) = current.take() {
                let mut frag = match frag_result {
                    Ok(f) => f,
                    // Mid-stream expression errors hard-fail here regardless of
                    // stop_on_exception; covered by the first-item type-error test
                    // (stream_split_non_stream_body_surfaces_expression_error). If
                    // this arm ever becomes index-aware, port the deleted service's
                    // test_streaming_error_in_expression.
                    Err(e) => return PipelineOutcome::Failed(e),
                };

                let next = stream.next().await;
                let is_last = next.is_none();

                frag.set_property(CAMEL_SPLIT_INDEX, Value::from(index));
                frag.set_property(CAMEL_SPLIT_COMPLETE, Value::Bool(is_last));
                // SPLIT_SIZE is unknown until end-of-stream, so it is only
                // present on the last fragment.
                if is_last {
                    frag.set_property(CAMEL_SPLIT_SIZE, Value::from(index + 1));
                }

                match body.run(frag).await {
                    PipelineOutcome::Completed(ex) => {
                        outputs.push(ex);
                        index += 1;
                    }
                    PipelineOutcome::Stopped(stopped_ex) => {
                        // Stop: return immediately. The `stream` local is
                        // dropped when this future returns, closing the
                        // underlying stream resource.
                        return PipelineOutcome::Stopped(stopped_ex);
                    }
                    PipelineOutcome::Failed(err) => {
                        if stop_on_exception {
                            return PipelineOutcome::Failed(err);
                        }
                        // stop_on_exception=false: collect error, continue
                        // processing remaining fragments.
                        last_error = Some(err);
                        index += 1;
                    }
                }

                current = next;
            }

            // Stream exhausted — if any errors were collected, propagate last.
            if let Some(err) = last_error {
                return PipelineOutcome::Failed(err);
            }
            PipelineOutcome::Completed(aggregate_completed(outputs, original, aggregation))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Body, CamelError, Message, Value};
    use std::task::{Context as TaskContext, Poll};
    use tokio::sync::Mutex;

    type BoxedOutcomeFuture = Pin<Box<dyn Future<Output = PipelineOutcome> + Send>>;
    type RecordedMetadata = Arc<Mutex<Vec<(Option<Value>, Option<Value>)>>>;

    /// Closure-backed `OutcomePipeline` used by the ported segment tests.
    #[derive(Clone)]
    struct FnBody {
        f: Arc<dyn Fn(Exchange) -> BoxedOutcomeFuture + Send + Sync>,
    }

    impl OutcomePipeline for FnBody {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(self.clone())
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let f = Arc::clone(&self.f);
            Box::pin(async move { f(exchange).await })
        }
    }

    fn body_from_fn<F>(f: F) -> OutcomeSegment
    where
        F: Fn(Exchange) -> BoxedOutcomeFuture + Send + Sync + 'static,
    {
        OutcomeSegment::new(Box::new(FnBody { f: Arc::new(f) }))
    }

    fn completed_body() -> OutcomeSegment {
        body_from_fn(|ex| Box::pin(async move { PipelineOutcome::Completed(ex) }))
    }

    fn uppercase_body() -> OutcomeSegment {
        body_from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                if let Body::Text(s) = &ex.input.body {
                    ex.input.body = Body::Text(s.to_uppercase());
                }
                PipelineOutcome::Completed(ex)
            })
        })
    }

    fn make_exchange(text: &str) -> Exchange {
        Exchange::new(Message::new(text))
    }

    fn fragments_expression(fragments: Vec<Exchange>) -> StreamingSplitExpression {
        Arc::new(move |_| {
            let frags = fragments.clone();
            Box::pin(futures::stream::iter(frags.into_iter().map(Ok)))
        })
    }

    fn empty_expression() -> StreamingSplitExpression {
        Arc::new(|_| Box::pin(futures::stream::empty()))
    }

    fn stream_body_exchange(chunks: Vec<&str>) -> Exchange {
        let chunks: Vec<Result<bytes::Bytes, CamelError>> = chunks
            .into_iter()
            .map(|chunk| Ok(bytes::Bytes::from(chunk.to_string())))
            .collect();
        let body = camel_api::StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(futures::stream::iter(chunks))))),
            metadata: camel_api::StreamMetadata::default(),
        };
        Exchange::new(Message::new(Body::Stream(body)))
    }

    /// Consumes `Body::Stream` and yields one text fragment per chunk, mirroring
    /// the incremental codec expressions the compiler builds.
    fn chunk_stream_expression() -> StreamingSplitExpression {
        Arc::new(|exchange: Exchange| {
            let (stream_body, parent) = match &exchange.input.body {
                Body::Stream(sb) => (sb.clone(), {
                    let mut p = exchange.branch_clone();
                    p.input.body = Body::Empty;
                    p
                }),
                _ => {
                    return Box::pin(futures::stream::once(async move {
                        Err(CamelError::TypeConversionFailed(
                            "expected Body::Stream".to_string(),
                        ))
                    }));
                }
            };
            let taken = match stream_body.stream.try_lock() {
                Ok(mut guard) => guard.take(),
                Err(_) => None,
            };
            let Some(stream) = taken else {
                return Box::pin(futures::stream::once(async {
                    Err(CamelError::ProcessorError(
                        "stream body already consumed".to_string(),
                    ))
                }));
            };
            Box::pin(async_stream::stream! {
                let mut stream = stream;
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(bytes) => {
                            let mut fragment = parent.branch_clone();
                            fragment.input.body =
                                Body::Text(String::from_utf8_lossy(&bytes).into_owned());
                            yield Ok(fragment);
                        }
                        Err(e) => {
                            yield Err(e);
                            return;
                        }
                    }
                }
            })
        })
    }

    /// Stream wrapper that records when the underlying source is dropped.
    struct DropTrackingStream<S> {
        inner: Pin<Box<S>>,
        dropped: Arc<std::sync::atomic::AtomicBool>,
    }

    impl<S: futures::Stream> futures::Stream for DropTrackingStream<S> {
        type Item = S::Item;

        fn poll_next(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
        ) -> Poll<Option<Self::Item>> {
            self.inner.as_mut().poll_next(cx)
        }
    }

    impl<S> Drop for DropTrackingStream<S> {
        fn drop(&mut self) {
            self.dropped
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Helper: OutcomePipeline body that counts invocations and stops on the
    /// nth invocation (0-indexed).
    #[derive(Clone)]
    struct StopOnNthBody {
        counter: Arc<std::sync::atomic::AtomicUsize>,
        stop_at: usize,
    }
    impl OutcomePipeline for StopOnNthBody {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(self.clone())
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let count = self
                .counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let stop_at = self.stop_at;
            Box::pin(async move {
                if count >= stop_at {
                    PipelineOutcome::Stopped(exchange)
                } else {
                    PipelineOutcome::Completed(exchange)
                }
            })
        }
    }

    /// Helper: OutcomePipeline body that mutates the exchange body then stops.
    #[derive(Clone)]
    struct MutateAndStopBody;
    impl OutcomePipeline for MutateAndStopBody {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(MutateAndStopBody)
        }
        fn run<'a>(
            &'a mut self,
            mut exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            Box::pin(async move {
                exchange.input.body = Body::Text("mutated-by-body".to_string());
                PipelineOutcome::Stopped(exchange)
            })
        }
    }

    // ── Test 1: Stop halts stream consumption ──────────────────────────

    #[tokio::test]
    async fn streaming_split_stop_halts_stream_consumption() {
        // Stream produces 5 fragments. Body stops on fragment 3 (index 2).
        // Remaining 2 fragments are NOT consumed from the stream.
        let fragments: Vec<Exchange> = (0..5)
            .map(|i| Exchange::new(Message::new(format!("frag-{i}"))))
            .collect();
        let stored_frags = fragments.clone();

        let expression: StreamingSplitExpression = Arc::new(move |_| {
            let frags = stored_frags.clone();
            Box::pin(futures::stream::iter(frags.into_iter().map(Ok)))
        });

        let invoke_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let body = StopOnNthBody {
            counter: Arc::clone(&invoke_count),
            stop_at: 2, // stop on 3rd invocation (count=2 before fetch_add → 0,1 pass, 2 stops)
        };

        let mut seg = StreamingSplitSegment {
            expression,
            body: OutcomeSegment::new(Box::new(body)),
            aggregation: AggregationStrategy::LastWins,
            stop_on_exception: true,
        };

        let ex = Exchange::new(Message::new("trigger"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        assert!(
            matches!(result, PipelineOutcome::Stopped(_)),
            "Expected Stopped, got {result:?}"
        );
        // Only 3 fragments processed (indices 0, 1, 2) — fragments 3,4 never touched.
        assert_eq!(invoke_count.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    // ── Test 2: Stop with NO aggregation ───────────────────────────────

    #[tokio::test]
    async fn streaming_split_stop_no_aggregate() {
        // Stop fires on the first fragment. Result MUST be Stopped (NOT Completed).
        // Aggregation is skipped entirely.
        let fragments: Vec<Exchange> = (0..3)
            .map(|i| Exchange::new(Message::new(format!("frag-{i}"))))
            .collect();
        let stored_frags = fragments.clone();

        let expression: StreamingSplitExpression = Arc::new(move |_| {
            let frags = stored_frags.clone();
            Box::pin(futures::stream::iter(frags.into_iter().map(Ok)))
        });

        let mut seg = StreamingSplitSegment {
            expression,
            body: OutcomeSegment::new(Box::new(MutateAndStopBody)),
            aggregation: AggregationStrategy::CollectAll,
            stop_on_exception: true,
        };

        let ex = Exchange::new(Message::new("original"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        match result {
            PipelineOutcome::Stopped(ex) => {
                // Body mutation is preserved
                assert_eq!(
                    ex.input.body.as_text(),
                    Some("mutated-by-body"),
                    "Stopped exchange should carry body mutation"
                );
            }
            other => panic!(
                "Expected Stopped with mutated body, got {other:?} — aggregation should NOT fire"
            ),
        }
    }

    // ── Test 3: Stop preserves exchange mutations ──────────────────────

    #[tokio::test]
    async fn streaming_split_stop_preserves_exchange_mutations() {
        // Verify that when a body segment mutates the exchange AND stops,
        // those mutations are visible in the resulting Stopped exchange.
        let fragments: Vec<Exchange> = (0..5)
            .map(|i| Exchange::new(Message::new(format!("frag-{i}"))))
            .collect();
        let stored_frags = fragments.clone();

        let expression: StreamingSplitExpression = Arc::new(move |_| {
            let frags = stored_frags.clone();
            Box::pin(futures::stream::iter(frags.into_iter().map(Ok)))
        });

        let mut seg = StreamingSplitSegment {
            expression,
            body: OutcomeSegment::new(Box::new(MutateAndStopBody)),
            aggregation: AggregationStrategy::CollectAll,
            stop_on_exception: true,
        };

        let ex = Exchange::new(Message::new("original"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        match result {
            PipelineOutcome::Stopped(ex) => {
                assert_eq!(
                    ex.input.body.as_text(),
                    Some("mutated-by-body"),
                    "Stopped exchange should carry the body mutation from the segment"
                );
            }
            other => panic!("Expected Stopped with mutation, got {other:?}"),
        }
    }

    // ── Test 4: stop_on_exception=true — stream halts on failure ─────

    #[tokio::test]
    async fn streaming_split_stop_on_exception_true() {
        let fragments: Vec<Exchange> = (0..3)
            .map(|i| Exchange::new(Message::new(format!("frag-{i}"))))
            .collect();
        let stored_frags = fragments.clone();

        let expression: StreamingSplitExpression = Arc::new(move |_| {
            let frags = stored_frags.clone();
            Box::pin(futures::stream::iter(frags.into_iter().map(Ok)))
        });

        let invoke_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Body that fails on the 2nd fragment (index 1), passes on others.
        #[derive(Clone)]
        struct FailOnSecondBody {
            counter: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl OutcomePipeline for FailOnSecondBody {
            fn clone_box(&self) -> Box<dyn OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                let count = self
                    .counter
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move {
                    if count == 1 {
                        PipelineOutcome::Failed(CamelError::ProcessorError("fail".into()))
                    } else {
                        PipelineOutcome::Completed(exchange)
                    }
                })
            }
        }

        let body = FailOnSecondBody {
            counter: Arc::clone(&invoke_count),
        };

        let mut seg = StreamingSplitSegment {
            expression,
            body: OutcomeSegment::new(Box::new(body)),
            aggregation: AggregationStrategy::LastWins,
            stop_on_exception: true,
        };

        let ex = Exchange::new(Message::new("trigger"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        assert!(
            matches!(result, PipelineOutcome::Failed(_)),
            "stop_on_exception=true should propagate failure immediately"
        );
        // Only 2 fragments processed (index 0 passed, index 1 failed); fragment 2 never processed.
        assert_eq!(invoke_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    // ── Test 5: stop_on_exception=false — stream continues on failure ─

    #[tokio::test]
    async fn streaming_split_stop_on_exception_false() {
        let fragments: Vec<Exchange> = (0..3)
            .map(|i| Exchange::new(Message::new(format!("frag-{i}"))))
            .collect();
        let stored_frags = fragments.clone();

        let expression: StreamingSplitExpression = Arc::new(move |_| {
            let frags = stored_frags.clone();
            Box::pin(futures::stream::iter(frags.into_iter().map(Ok)))
        });

        let invoke_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Body that fails on the 2nd fragment (index 1), passes on others.
        #[derive(Clone)]
        struct FailOnSecondBody {
            counter: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl OutcomePipeline for FailOnSecondBody {
            fn clone_box(&self) -> Box<dyn OutcomePipeline> {
                Box::new(self.clone())
            }
            fn run<'a>(
                &'a mut self,
                exchange: Exchange,
            ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
                let count = self
                    .counter
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move {
                    if count == 1 {
                        PipelineOutcome::Failed(CamelError::ProcessorError("fail".into()))
                    } else {
                        PipelineOutcome::Completed(exchange)
                    }
                })
            }
        }

        let body = FailOnSecondBody {
            counter: Arc::clone(&invoke_count),
        };

        let mut seg = StreamingSplitSegment {
            expression,
            body: OutcomeSegment::new(Box::new(body)),
            aggregation: AggregationStrategy::LastWins,
            stop_on_exception: false,
        };

        let ex = Exchange::new(Message::new("trigger"));
        let result = OutcomePipeline::run(&mut seg, ex).await;

        // stop_on_exception=false → error propagated at end, but all fragments processed.
        assert!(
            matches!(result, PipelineOutcome::Failed(_)),
            "stop_on_exception=false should still propagate error at end"
        );
        assert_eq!(
            invoke_count.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "all 3 fragments should be processed"
        );
    }

    // ------------------------------------------------------------------
    // Coverage ported from the legacy streaming splitter tests.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn streaming_split_last_wins_aggregation() {
        let mut seg = StreamingSplitSegment {
            expression: fragments_expression(vec![
                make_exchange("a"),
                make_exchange("b"),
                make_exchange("c"),
            ]),
            body: uppercase_body(),
            aggregation: AggregationStrategy::LastWins,
            stop_on_exception: true,
        };

        let result = OutcomePipeline::run(&mut seg, make_exchange("original")).await;
        match result {
            PipelineOutcome::Completed(ex) => {
                assert_eq!(ex.input.body.as_text(), Some("C"));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn streaming_split_original_aggregation() {
        let mut seg = StreamingSplitSegment {
            expression: fragments_expression(vec![make_exchange("a"), make_exchange("b")]),
            body: uppercase_body(),
            aggregation: AggregationStrategy::Original,
            stop_on_exception: true,
        };

        let result = OutcomePipeline::run(&mut seg, make_exchange("original")).await;
        match result {
            PipelineOutcome::Completed(ex) => {
                assert_eq!(ex.input.body.as_text(), Some("original"));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn streaming_split_custom_aggregation() {
        let joiner: Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync> =
            Arc::new(|mut acc: Exchange, next: Exchange| {
                let acc_text = acc.input.body.as_text().unwrap_or("").to_string();
                let next_text = next.input.body.as_text().unwrap_or("").to_string();
                acc.input.body = Body::Text(format!("{acc_text}+{next_text}"));
                acc
            });

        let mut seg = StreamingSplitSegment {
            expression: fragments_expression(vec![
                make_exchange("a"),
                make_exchange("b"),
                make_exchange("c"),
            ]),
            body: uppercase_body(),
            aggregation: AggregationStrategy::Custom(joiner),
            stop_on_exception: true,
        };

        let result = OutcomePipeline::run(&mut seg, make_exchange("original")).await;
        match result {
            PipelineOutcome::Completed(ex) => {
                assert_eq!(ex.input.body.as_text(), Some("A+B+C"));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn streaming_split_empty_stream_returns_original_last_wins() {
        let mut ex = make_exchange("original");
        ex.set_property("marker", Value::Bool(true));

        let mut seg = StreamingSplitSegment {
            expression: empty_expression(),
            body: completed_body(),
            aggregation: AggregationStrategy::LastWins,
            stop_on_exception: true,
        };

        let result = OutcomePipeline::run(&mut seg, ex).await;
        match result {
            PipelineOutcome::Completed(ex) => {
                assert_eq!(ex.input.body.as_text(), Some("original"));
                assert_eq!(ex.property("marker"), Some(&Value::Bool(true)));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn streaming_split_empty_stream_collect_all() {
        let mut ex = make_exchange("original");
        ex.set_property("trace_id", Value::String("empty-test".into()));

        let mut seg = StreamingSplitSegment {
            expression: empty_expression(),
            body: completed_body(),
            aggregation: AggregationStrategy::CollectAll,
            stop_on_exception: true,
        };

        let result = OutcomePipeline::run(&mut seg, ex).await;
        match result {
            PipelineOutcome::Completed(ex) => {
                match &ex.input.body {
                    Body::Json(Value::Array(items)) => {
                        assert!(items.is_empty(), "expected empty array, got {items:?}");
                    }
                    other => panic!("expected Body::Json([]), got {other:?}"),
                }
                assert_eq!(
                    ex.property("trace_id"),
                    Some(&Value::String("empty-test".into()))
                );
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// When the body stops, the segment returns immediately and drops the
    /// source stream future (closing the underlying resource).
    #[tokio::test]
    async fn streaming_split_stop_drops_source_stream() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dropped_flag = Arc::clone(&dropped);
        let expression: StreamingSplitExpression = Arc::new(move |_| {
            let source = DropTrackingStream {
                inner: Box::pin(futures::stream::iter(vec![
                    Ok(make_exchange("a")),
                    Ok(make_exchange("b")),
                ])),
                dropped: Arc::clone(&dropped_flag),
            };
            Box::pin(source)
        });
        let stop_body = body_from_fn(|ex| Box::pin(async move { PipelineOutcome::Stopped(ex) }));

        let mut seg = StreamingSplitSegment {
            expression,
            body: stop_body,
            aggregation: AggregationStrategy::LastWins,
            stop_on_exception: true,
        };

        let result = OutcomePipeline::run(&mut seg, make_exchange("trigger")).await;
        assert!(
            matches!(result, PipelineOutcome::Stopped(_)),
            "got {result:?}"
        );
        assert!(
            dropped.load(std::sync::atomic::Ordering::SeqCst),
            "source stream should be dropped when the segment returns on Stop"
        );
    }

    #[tokio::test]
    async fn streaming_split_metadata_index_and_complete() {
        let records: RecordedMetadata = Arc::new(Mutex::new(Vec::new()));
        let records_clone = Arc::clone(&records);
        let recorder = body_from_fn(move |ex: Exchange| {
            let records = Arc::clone(&records_clone);
            Box::pin(async move {
                let index = ex.property(crate::splitter::CAMEL_SPLIT_INDEX).cloned();
                let complete = ex.property(crate::splitter::CAMEL_SPLIT_COMPLETE).cloned();
                records.lock().await.push((index, complete));
                PipelineOutcome::Completed(ex)
            })
        });

        let mut seg = StreamingSplitSegment {
            expression: fragments_expression(vec![
                make_exchange("x"),
                make_exchange("y"),
                make_exchange("z"),
            ]),
            body: recorder,
            aggregation: AggregationStrategy::LastWins,
            stop_on_exception: true,
        };

        let result = OutcomePipeline::run(&mut seg, make_exchange("original")).await;
        assert!(result.is_success(), "expected Completed, got {result:?}");

        let guard = records.lock().await; // allow-test-wait: pipeline completed before lock; single holder, no contention
        assert_eq!(guard.len(), 3);
        for (i, (index, _complete)) in guard.iter().enumerate() {
            assert_eq!(
                *index,
                Some(Value::from(i as u64)),
                "fragment {i}: CAMEL_SPLIT_INDEX mismatch"
            );
        }
        assert_eq!(guard[0].1, Some(Value::Bool(false)));
        assert_eq!(guard[1].1, Some(Value::Bool(false)));
        assert_eq!(guard[2].1, Some(Value::Bool(true)));
    }

    // Inspiration: Camel SplitterTest C6 — in streaming mode the total count
    // is unknown until end-of-stream; SPLIT_SIZE is therefore absent on
    // intermediate fragments and present (= index+1) on the last one.
    #[tokio::test]
    async fn streaming_split_size_set_only_on_last_fragment() {
        let records: RecordedMetadata = Arc::new(Mutex::new(Vec::new()));
        let records_clone = Arc::clone(&records);
        let recorder = body_from_fn(move |ex: Exchange| {
            let records = Arc::clone(&records_clone);
            Box::pin(async move {
                let index = ex.property(crate::splitter::CAMEL_SPLIT_INDEX).cloned();
                let size = ex.property(crate::splitter::CAMEL_SPLIT_SIZE).cloned();
                records.lock().await.push((index, size));
                PipelineOutcome::Completed(ex)
            })
        });

        let mut seg = StreamingSplitSegment {
            expression: fragments_expression(vec![
                make_exchange("x"),
                make_exchange("y"),
                make_exchange("z"),
            ]),
            body: recorder,
            aggregation: AggregationStrategy::LastWins,
            stop_on_exception: true,
        };

        let result = OutcomePipeline::run(&mut seg, make_exchange("original")).await;
        assert!(result.is_success(), "expected Completed, got {result:?}");

        let guard = records.lock().await; // allow-test-wait: pipeline completed before lock; single holder, no contention
        assert_eq!(guard.len(), 3);
        assert_eq!(guard[0].1, None, "size must be absent on first fragment");
        assert_eq!(guard[1].1, None, "size must be absent on middle fragment");
        assert_eq!(
            guard[2].1,
            Some(Value::from(3u64)),
            "size must equal total on last fragment"
        );
    }

    #[tokio::test]
    async fn streaming_split_original_aggregation_sanitizes_stream_body() {
        let ex = stream_body_exchange(vec!["line1\n"]);
        let mut seg = StreamingSplitSegment {
            expression: chunk_stream_expression(),
            body: completed_body(),
            aggregation: AggregationStrategy::Original,
            stop_on_exception: true,
        };

        let result = OutcomePipeline::run(&mut seg, ex).await;
        match result {
            PipelineOutcome::Completed(ex) => {
                assert!(
                    matches!(ex.input.body, Body::Empty),
                    "original body should be sanitized to Empty, got {:?}",
                    ex.input.body
                );
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }
}
