//! ## Stop semantics (ADR-0025)
//!
//! This segment implements `OutcomePipeline` and propagates `PipelineOutcome::Stopped(ex)` with the exchange state intact (including mutations made inside the segment body before Stop fired). See ADR-0025 §3 (stopped-exchange-state-preservation invariant).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::{StreamExt, pin_mut};

use camel_api::{
    AggregationStrategy, CamelError, Exchange, OutcomePipeline, OutcomeSegment, PipelineOutcome,
    StreamingSplitExpression,
};

use crate::split_segment::aggregate_completed;

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
            let original = exchange.clone();
            let stream = expression(exchange);
            pin_mut!(stream);

            let mut outputs: Vec<Exchange> = Vec::new();
            let mut last_error: Option<CamelError> = None;

            while let Some(frag_result) = stream.next().await {
                let frag = match frag_result {
                    Ok(f) => f,
                    Err(e) => return PipelineOutcome::Failed(e),
                };

                match body.run(frag).await {
                    PipelineOutcome::Completed(ex) => outputs.push(ex),
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
                    }
                }
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
    use camel_api::{Body, CamelError, Message};

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
}
