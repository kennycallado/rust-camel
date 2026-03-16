use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::future::join_all;
use tokio::sync::Semaphore;
use tower::Service;

use camel_api::{
    AggregationStrategy, Body, BoxProcessor, CamelError, Exchange, SplitterConfig, Value,
};

// ── Metadata property keys ─────────────────────────────────────────────

/// Property key for the zero-based index of a fragment within the split.
pub const CAMEL_SPLIT_INDEX: &str = "CamelSplitIndex";
/// Property key for the total number of fragments produced by the split.
pub const CAMEL_SPLIT_SIZE: &str = "CamelSplitSize";
/// Property key indicating whether this fragment is the last one.
pub const CAMEL_SPLIT_COMPLETE: &str = "CamelSplitComplete";

// ── SplitterService ────────────────────────────────────────────────────

/// Tower Service implementing the Splitter EIP.
///
/// Splits an incoming exchange into fragments via a configurable expression,
/// processes each fragment through a sub-pipeline, and aggregates the results.
///
/// **Note:** In parallel mode, `stop_on_exception` only affects the aggregation
/// phase. All spawned fragments run to completion because `join_all` cannot
/// cancel in-flight futures. Sequential mode stops processing immediately.
#[derive(Clone)]
pub struct SplitterService {
    expression: camel_api::SplitExpression,
    sub_pipeline: BoxProcessor,
    aggregation: AggregationStrategy,
    parallel: bool,
    parallel_limit: Option<usize>,
    stop_on_exception: bool,
}

impl SplitterService {
    /// Create a new `SplitterService` from a [`SplitterConfig`] and a sub-pipeline.
    pub fn new(config: SplitterConfig, sub_pipeline: BoxProcessor) -> Self {
        Self {
            expression: config.expression,
            sub_pipeline,
            aggregation: config.aggregation,
            parallel: config.parallel,
            parallel_limit: config.parallel_limit,
            stop_on_exception: config.stop_on_exception,
        }
    }
}

impl Service<Exchange> for SplitterService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.sub_pipeline.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let original = exchange.clone();
        let expression = self.expression.clone();
        let sub_pipeline = self.sub_pipeline.clone();
        let aggregation = self.aggregation.clone();
        let parallel = self.parallel;
        let parallel_limit = self.parallel_limit;
        let stop_on_exception = self.stop_on_exception;

        Box::pin(async move {
            // Split the exchange into fragments.
            let mut fragments = expression(&exchange);

            // If no fragments were produced, return the original exchange.
            if fragments.is_empty() {
                return Ok(original);
            }

            let total = fragments.len();

            // Set metadata on each fragment.
            for (i, frag) in fragments.iter_mut().enumerate() {
                frag.set_property(CAMEL_SPLIT_INDEX, Value::from(i as u64));
                frag.set_property(CAMEL_SPLIT_SIZE, Value::from(total as u64));
                frag.set_property(CAMEL_SPLIT_COMPLETE, Value::Bool(i == total - 1));
            }

            // Process fragments through the sub-pipeline.
            let results = if parallel {
                process_parallel(fragments, sub_pipeline, parallel_limit, stop_on_exception).await
            } else {
                process_sequential(fragments, sub_pipeline, stop_on_exception).await
            };

            // Aggregate the results.
            aggregate(results, original, aggregation)
        })
    }
}

// ── Sequential processing ──────────────────────────────────────────────

async fn process_sequential(
    fragments: Vec<Exchange>,
    sub_pipeline: BoxProcessor,
    stop_on_exception: bool,
) -> Vec<Result<Exchange, CamelError>> {
    let mut results = Vec::with_capacity(fragments.len());

    for fragment in fragments {
        let mut pipeline = sub_pipeline.clone();
        match tower::ServiceExt::ready(&mut pipeline).await {
            Err(e) => {
                results.push(Err(e));
                if stop_on_exception {
                    break;
                }
            }
            Ok(svc) => {
                let result = svc.call(fragment).await;
                let is_err = result.is_err();
                results.push(result);
                if stop_on_exception && is_err {
                    break;
                }
            }
        }
    }

    results
}

// ── Parallel processing ────────────────────────────────────────────────

async fn process_parallel(
    fragments: Vec<Exchange>,
    sub_pipeline: BoxProcessor,
    parallel_limit: Option<usize>,
    _stop_on_exception: bool,
) -> Vec<Result<Exchange, CamelError>> {
    let semaphore = parallel_limit.map(|limit| std::sync::Arc::new(Semaphore::new(limit)));

    let futures: Vec<_> = fragments
        .into_iter()
        .map(|fragment| {
            let mut pipeline = sub_pipeline.clone();
            let sem = semaphore.clone();
            async move {
                // Acquire semaphore permit if a limit is set.
                let _permit = match &sem {
                    Some(s) => Some(s.acquire().await.map_err(|e| {
                        CamelError::ProcessorError(format!("semaphore error: {e}"))
                    })?),
                    None => None,
                };

                tower::ServiceExt::ready(&mut pipeline).await?;
                pipeline.call(fragment).await
            }
        })
        .collect();

    join_all(futures).await
}

// ── Aggregation ────────────────────────────────────────────────────────

fn aggregate(
    results: Vec<Result<Exchange, CamelError>>,
    original: Exchange,
    strategy: AggregationStrategy,
) -> Result<Exchange, CamelError> {
    match strategy {
        AggregationStrategy::LastWins => {
            // Return the last result (error or success).
            results.into_iter().last().unwrap_or_else(|| Ok(original))
        }
        AggregationStrategy::CollectAll => {
            // Collect all bodies into a JSON array. Errors propagate.
            let mut bodies = Vec::new();
            for result in results {
                let ex = result?;
                let value = match &ex.input.body {
                    Body::Text(s) => Value::String(s.clone()),
                    Body::Json(v) => v.clone(),
                    Body::Xml(s) => Value::String(s.clone()),
                    Body::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
                    Body::Empty => Value::Null,
                    Body::Stream(s) => serde_json::json!({
                        "_stream": {
                            "origin": s.metadata.origin,
                            "placeholder": true,
                            "hint": "Materialize exchange body with .into_bytes() before aggregation if content needed"
                        }
                    }),
                };
                bodies.push(value);
            }
            let mut out = original;
            out.input.body = Body::Json(Value::Array(bodies));
            Ok(out)
        }
        AggregationStrategy::Original => Ok(original),
        AggregationStrategy::Custom(fold_fn) => {
            // Fold using the custom function, starting from the first result.
            let mut iter = results.into_iter();
            let first = iter.next().unwrap_or_else(|| Ok(original.clone()))?;
            iter.try_fold(first, |acc, next_result| {
                let next = next_result?;
                Ok(fold_fn(acc, next))
            })
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessorExt, Message};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    // ── Test helpers ───────────────────────────────────────────────────

    fn passthrough_pipeline() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    fn uppercase_pipeline() -> BoxProcessor {
        BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                if let Body::Text(s) = &ex.input.body {
                    ex.input.body = Body::Text(s.to_uppercase());
                }
                Ok(ex)
            })
        })
    }

    fn failing_pipeline() -> BoxProcessor {
        BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        })
    }

    fn fail_on_nth(n: usize) -> BoxProcessor {
        let count = Arc::new(AtomicUsize::new(0));
        BoxProcessor::from_fn(move |ex: Exchange| {
            let count = Arc::clone(&count);
            Box::pin(async move {
                let c = count.fetch_add(1, Ordering::SeqCst);
                if c == n {
                    Err(CamelError::ProcessorError(format!("fail on {c}")))
                } else {
                    Ok(ex)
                }
            })
        })
    }

    fn make_exchange(text: &str) -> Exchange {
        Exchange::new(Message::new(text))
    }

    // ── 1. Sequential + LastWins ───────────────────────────────────────

    #[tokio::test]
    async fn test_split_sequential_last_wins() {
        let config = SplitterConfig::new(camel_api::split_body_lines())
            .aggregation(AggregationStrategy::LastWins);
        let mut svc = SplitterService::new(config, uppercase_pipeline());

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("a\nb\nc"))
            .await
            .unwrap();
        assert_eq!(result.input.body.as_text(), Some("C"));
    }

    // ── 2. Sequential + CollectAll ─────────────────────────────────────

    #[tokio::test]
    async fn test_split_sequential_collect_all() {
        let config = SplitterConfig::new(camel_api::split_body_lines())
            .aggregation(AggregationStrategy::CollectAll);
        let mut svc = SplitterService::new(config, uppercase_pipeline());

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("a\nb\nc"))
            .await
            .unwrap();
        let expected = serde_json::json!(["A", "B", "C"]);
        match &result.input.body {
            Body::Json(v) => assert_eq!(*v, expected),
            other => panic!("expected JSON body, got {other:?}"),
        }
    }

    // ── 3. Sequential + Original ───────────────────────────────────────

    #[tokio::test]
    async fn test_split_sequential_original() {
        let config = SplitterConfig::new(camel_api::split_body_lines())
            .aggregation(AggregationStrategy::Original);
        let mut svc = SplitterService::new(config, uppercase_pipeline());

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("a\nb\nc"))
            .await
            .unwrap();
        // Original body should be unchanged.
        assert_eq!(result.input.body.as_text(), Some("a\nb\nc"));
    }

    // ── 4. Sequential + Custom aggregation ─────────────────────────────

    #[tokio::test]
    async fn test_split_sequential_custom_aggregation() {
        let joiner: Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync> =
            Arc::new(|mut acc: Exchange, next: Exchange| {
                let acc_text = acc.input.body.as_text().unwrap_or("").to_string();
                let next_text = next.input.body.as_text().unwrap_or("").to_string();
                acc.input.body = Body::Text(format!("{acc_text}+{next_text}"));
                acc
            });

        let config = SplitterConfig::new(camel_api::split_body_lines())
            .aggregation(AggregationStrategy::Custom(joiner));
        let mut svc = SplitterService::new(config, uppercase_pipeline());

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("a\nb\nc"))
            .await
            .unwrap();
        assert_eq!(result.input.body.as_text(), Some("A+B+C"));
    }

    // ── 5. Stop on exception ───────────────────────────────────────────

    #[tokio::test]
    async fn test_split_stop_on_exception() {
        // 5 fragments, fail on the 2nd (index 1), stop=true
        let config = SplitterConfig::new(camel_api::split_body_lines()).stop_on_exception(true);
        let mut svc = SplitterService::new(config, fail_on_nth(1));

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("a\nb\nc\nd\ne"))
            .await;

        // LastWins is default, the last result should be the error from fragment 1.
        assert!(result.is_err(), "expected error due to stop_on_exception");
    }

    // ── 6. Continue on exception ───────────────────────────────────────

    #[tokio::test]
    async fn test_split_continue_on_exception() {
        // 3 fragments, fail on 2nd (index 1), stop=false, LastWins.
        let config = SplitterConfig::new(camel_api::split_body_lines())
            .stop_on_exception(false)
            .aggregation(AggregationStrategy::LastWins);
        let mut svc = SplitterService::new(config, fail_on_nth(1));

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("a\nb\nc"))
            .await;

        // LastWins: last fragment (index 2) succeeded.
        assert!(result.is_ok(), "last fragment should succeed");
    }

    // ── 7. Empty fragments ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_split_empty_fragments() {
        // Body::Empty → no fragments → return original unchanged.
        let config = SplitterConfig::new(camel_api::split_body_lines());
        let mut svc = SplitterService::new(config, passthrough_pipeline());

        let mut ex = Exchange::new(Message::default()); // Body::Empty
        ex.set_property("marker", Value::Bool(true));

        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert!(result.input.body.is_empty());
        assert_eq!(result.property("marker"), Some(&Value::Bool(true)));
    }

    // ── 8. Metadata properties ─────────────────────────────────────────

    #[tokio::test]
    async fn test_split_metadata_properties() {
        // Use passthrough so we can inspect metadata on returned fragments.
        // CollectAll won't preserve metadata, so use a pipeline that records
        // the metadata into the body as JSON.
        let recorder = BoxProcessor::from_fn(|ex: Exchange| {
            Box::pin(async move {
                let idx = ex.property(CAMEL_SPLIT_INDEX).cloned();
                let size = ex.property(CAMEL_SPLIT_SIZE).cloned();
                let complete = ex.property(CAMEL_SPLIT_COMPLETE).cloned();
                let body = serde_json::json!({
                    "index": idx,
                    "size": size,
                    "complete": complete,
                });
                let mut out = ex;
                out.input.body = Body::Json(body);
                Ok(out)
            })
        });

        let config = SplitterConfig::new(camel_api::split_body_lines())
            .aggregation(AggregationStrategy::CollectAll);
        let mut svc = SplitterService::new(config, recorder);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("x\ny\nz"))
            .await
            .unwrap();

        let expected = serde_json::json!([
            {"index": 0, "size": 3, "complete": false},
            {"index": 1, "size": 3, "complete": false},
            {"index": 2, "size": 3, "complete": true},
        ]);
        match &result.input.body {
            Body::Json(v) => assert_eq!(*v, expected),
            other => panic!("expected JSON body, got {other:?}"),
        }
    }

    // ── 9. poll_ready delegates to sub-pipeline ────────────────────────

    #[tokio::test]
    async fn test_poll_ready_delegates_to_sub_pipeline() {
        use std::sync::atomic::AtomicBool;

        // A service that is initially not ready, then becomes ready.
        #[derive(Clone)]
        struct DelayedReady {
            ready: Arc<AtomicBool>,
        }

        impl Service<Exchange> for DelayedReady {
            type Response = Exchange;
            type Error = CamelError;
            type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

            fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                if self.ready.load(Ordering::SeqCst) {
                    Poll::Ready(Ok(()))
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }

            fn call(&mut self, exchange: Exchange) -> Self::Future {
                Box::pin(async move { Ok(exchange) })
            }
        }

        let ready_flag = Arc::new(AtomicBool::new(false));
        let inner = DelayedReady {
            ready: Arc::clone(&ready_flag),
        };
        let boxed: BoxProcessor = BoxProcessor::new(inner);

        let config = SplitterConfig::new(camel_api::split_body_lines());
        let mut svc = SplitterService::new(config, boxed);

        // First poll should be Pending.
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let poll = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(
            poll.is_pending(),
            "expected Pending when sub_pipeline not ready"
        );

        // Mark inner as ready.
        ready_flag.store(true, Ordering::SeqCst);

        let poll = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(
            matches!(poll, Poll::Ready(Ok(()))),
            "expected Ready after sub_pipeline becomes ready"
        );
    }

    // ── 10. Parallel basic ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_split_parallel_basic() {
        let config = SplitterConfig::new(camel_api::split_body_lines())
            .parallel(true)
            .aggregation(AggregationStrategy::CollectAll);
        let mut svc = SplitterService::new(config, uppercase_pipeline());

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("a\nb\nc"))
            .await
            .unwrap();

        let expected = serde_json::json!(["A", "B", "C"]);
        match &result.input.body {
            Body::Json(v) => assert_eq!(*v, expected),
            other => panic!("expected JSON body, got {other:?}"),
        }
    }

    // ── 11. Parallel with limit ────────────────────────────────────────

    #[tokio::test]
    async fn test_split_parallel_with_limit() {
        use std::sync::atomic::AtomicUsize;

        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        let c = Arc::clone(&concurrent);
        let mc = Arc::clone(&max_concurrent);
        let pipeline = BoxProcessor::from_fn(move |ex: Exchange| {
            let c = Arc::clone(&c);
            let mc = Arc::clone(&mc);
            Box::pin(async move {
                let current = c.fetch_add(1, Ordering::SeqCst) + 1;
                // Record the high-water mark.
                mc.fetch_max(current, Ordering::SeqCst);
                // Yield to let other tasks run.
                tokio::task::yield_now().await;
                c.fetch_sub(1, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let config = SplitterConfig::new(camel_api::split_body_lines())
            .parallel(true)
            .parallel_limit(2)
            .aggregation(AggregationStrategy::CollectAll);
        let mut svc = SplitterService::new(config, pipeline);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("a\nb\nc\nd"))
            .await;
        assert!(result.is_ok());

        let observed_max = max_concurrent.load(Ordering::SeqCst);
        assert!(
            observed_max <= 2,
            "max concurrency was {observed_max}, expected <= 2"
        );
    }

    // ── 12. Parallel stop on exception ─────────────────────────────────

    #[tokio::test]
    async fn test_split_parallel_stop_on_exception() {
        let config = SplitterConfig::new(camel_api::split_body_lines())
            .parallel(true)
            .stop_on_exception(true);
        let mut svc = SplitterService::new(config, failing_pipeline());

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("a\nb\nc"))
            .await;

        // All fragments fail; LastWins returns the last error.
        assert!(result.is_err(), "expected error when all fragments fail");
    }

    // ── 13. Stream body aggregation creates valid JSON ───────────────────

    #[tokio::test]
    async fn test_splitter_stream_bodies_creates_valid_json() {
        use bytes::Bytes;
        use camel_api::{StreamBody, StreamMetadata};
        use futures::stream;
        use tokio::sync::Mutex;

        let chunks = vec![Ok(Bytes::from("test"))];
        let stream_body = StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream::iter(chunks))))),
            metadata: StreamMetadata {
                origin: Some("kafka://topic/partition".to_string()),
                ..Default::default()
            },
        };

        let original = Exchange::new(Message {
            headers: Default::default(),
            body: Body::Empty,
        });

        let results = vec![Ok(Exchange::new(Message {
            headers: Default::default(),
            body: Body::Stream(stream_body),
        }))];

        let result = aggregate(results, original, AggregationStrategy::CollectAll);

        let exchange = result.expect("Expected Ok result");
        assert!(
            matches!(exchange.input.body, Body::Json(_)),
            "Expected Json body"
        );

        if let Body::Json(value) = exchange.input.body {
            let json_str = serde_json::to_string(&value).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

            assert!(parsed.is_array());
            let arr = parsed.as_array().unwrap();
            assert!(arr[0].is_object());
            assert!(arr[0]["_stream"].is_object());
            assert_eq!(arr[0]["_stream"]["origin"], "kafka://topic/partition");
            assert_eq!(arr[0]["_stream"]["placeholder"], true);
        }
    }

    #[tokio::test]
    async fn test_splitter_stream_with_none_origin_creates_valid_json() {
        use bytes::Bytes;
        use camel_api::{StreamBody, StreamMetadata};
        use futures::stream;
        use tokio::sync::Mutex;

        let chunks = vec![Ok(Bytes::from("test"))];
        let stream_body = StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream::iter(chunks))))),
            metadata: StreamMetadata {
                origin: None,
                ..Default::default()
            },
        };

        let original = Exchange::new(Message {
            headers: Default::default(),
            body: Body::Empty,
        });

        let results = vec![Ok(Exchange::new(Message {
            headers: Default::default(),
            body: Body::Stream(stream_body),
        }))];

        let result = aggregate(results, original, AggregationStrategy::CollectAll);

        let exchange = result.expect("Expected Ok result");
        assert!(
            matches!(exchange.input.body, Body::Json(_)),
            "Expected Json body"
        );

        if let Body::Json(value) = exchange.input.body {
            let json_str = serde_json::to_string(&value).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

            assert!(parsed.is_array());
            let arr = parsed.as_array().unwrap();
            assert!(arr[0].is_object());
            assert!(arr[0]["_stream"].is_object());
            assert_eq!(arr[0]["_stream"]["origin"], serde_json::Value::Null);
            assert_eq!(arr[0]["_stream"]["placeholder"], true);
        }
    }
}
