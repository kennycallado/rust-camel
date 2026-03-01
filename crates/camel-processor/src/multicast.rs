use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{
    Body, BoxProcessor, CamelError, Exchange, MulticastConfig, MulticastStrategy, Value,
};

// ── Metadata property keys ─────────────────────────────────────────────

/// Property key for the zero-based index of the endpoint being invoked.
pub const CAMEL_MULTICAST_INDEX: &str = "CamelMulticastIndex";
/// Property key indicating whether this is the last endpoint invocation.
pub const CAMEL_MULTICAST_COMPLETE: &str = "CamelMulticastComplete";

// ── MulticastService ───────────────────────────────────────────────────

/// Tower Service implementing the Multicast EIP.
///
/// Sends a message to multiple endpoints, processing each independently,
/// and then aggregating the results.
///
/// Supports both sequential and parallel processing modes, configurable
/// via [`MulticastConfig::parallel`]. When parallel mode is enabled,
/// all endpoints are invoked concurrently with optional concurrency
/// limiting via [`MulticastConfig::parallel_limit`].
#[derive(Clone)]
pub struct MulticastService {
    endpoints: Vec<BoxProcessor>,
    config: MulticastConfig,
}

impl MulticastService {
    /// Create a new `MulticastService` from a list of endpoints and a [`MulticastConfig`].
    pub fn new(endpoints: Vec<BoxProcessor>, config: MulticastConfig) -> Self {
        Self { endpoints, config }
    }
}

impl Service<Exchange> for MulticastService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Check all endpoints for readiness
        for endpoint in &mut self.endpoints {
            match endpoint.poll_ready(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {}
            }
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let original = exchange.clone();
        let endpoints = self.endpoints.clone();
        let config = self.config.clone();

        Box::pin(async move {
            // If no endpoints, return original exchange unchanged
            if endpoints.is_empty() {
                return Ok(original);
            }

            let total = endpoints.len();

            let results = if config.parallel {
                // Process endpoints in parallel
                process_parallel(exchange, endpoints, config.parallel_limit, total).await
            } else {
                // Process each endpoint sequentially
                process_sequential(exchange, endpoints, config.stop_on_exception, total).await
            };

            // Aggregate results per strategy
            aggregate(results, original, config.aggregation)
        })
    }
}

// ── Sequential processing ──────────────────────────────────────────────

async fn process_sequential(
    exchange: Exchange,
    endpoints: Vec<BoxProcessor>,
    stop_on_exception: bool,
    total: usize,
) -> Vec<Result<Exchange, CamelError>> {
    let mut results = Vec::with_capacity(endpoints.len());

    for (i, endpoint) in endpoints.into_iter().enumerate() {
        // Clone the exchange for each endpoint
        let mut cloned_exchange = exchange.clone();

        // Set multicast metadata properties
        cloned_exchange.set_property(CAMEL_MULTICAST_INDEX, Value::from(i as i64));
        cloned_exchange.set_property(CAMEL_MULTICAST_COMPLETE, Value::Bool(i == total - 1));

        let mut endpoint = endpoint;
        match tower::ServiceExt::ready(&mut endpoint).await {
            Err(e) => {
                results.push(Err(e));
                if stop_on_exception {
                    break;
                }
            }
            Ok(svc) => {
                let result = svc.call(cloned_exchange).await;
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
    exchange: Exchange,
    endpoints: Vec<BoxProcessor>,
    parallel_limit: Option<usize>,
    total: usize,
) -> Vec<Result<Exchange, CamelError>> {
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    let semaphore = parallel_limit.map(|limit| Arc::new(Semaphore::new(limit)));

    // Build futures for each endpoint
    let futures: Vec<_> = endpoints
        .into_iter()
        .enumerate()
        .map(|(i, mut endpoint)| {
            let mut ex = exchange.clone();
            ex.set_property(CAMEL_MULTICAST_INDEX, Value::from(i as i64));
            ex.set_property(CAMEL_MULTICAST_COMPLETE, Value::Bool(i == total - 1));
            let sem = semaphore.clone();
            async move {
                // Acquire semaphore permit if limit is set
                let _permit = match &sem {
                    Some(s) => match s.acquire().await {
                        Ok(p) => Some(p),
                        Err(_) => {
                            return Err(CamelError::ProcessorError("semaphore closed".to_string()));
                        }
                    },
                    None => None,
                };

                // Wait for endpoint to be ready, then call it
                tower::ServiceExt::ready(&mut endpoint).await?;
                endpoint.call(ex).await
            }
        })
        .collect();

    // Execute all futures concurrently and collect results
    futures::future::join_all(futures).await
}

// ── Aggregation ────────────────────────────────────────────────────────

fn aggregate(
    results: Vec<Result<Exchange, CamelError>>,
    original: Exchange,
    strategy: MulticastStrategy,
) -> Result<Exchange, CamelError> {
    match strategy {
        MulticastStrategy::LastWins => {
            // Return the last result (error or success).
            // If last result is Err and stop_on_exception=false, return that error.
            results.into_iter().last().unwrap_or_else(|| Ok(original))
        }
        MulticastStrategy::CollectAll => {
            // Collect all bodies into a JSON array. Errors propagate.
            let mut bodies = Vec::new();
            for result in results {
                let ex = result?;
                let value = match &ex.input.body {
                    Body::Text(s) => Value::String(s.clone()),
                    Body::Json(v) => v.clone(),
                    Body::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
                    Body::Empty => Value::Null,
                };
                bodies.push(value);
            }
            let mut out = original;
            out.input.body = Body::Json(Value::Array(bodies));
            Ok(out)
        }
        MulticastStrategy::Original => Ok(original),
        MulticastStrategy::Custom(fold_fn) => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessorExt, Message};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use tower::ServiceExt;

    // ── Test helpers ───────────────────────────────────────────────────

    fn make_exchange(body: &str) -> Exchange {
        Exchange::new(Message::new(body))
    }

    fn uppercase_processor() -> BoxProcessor {
        BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                if let Body::Text(s) = &ex.input.body {
                    ex.input.body = Body::Text(s.to_uppercase());
                }
                Ok(ex)
            })
        })
    }

    fn failing_processor() -> BoxProcessor {
        BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        })
    }

    // ── 1. Sequential + LastWins ───────────────────────────────────────

    #[tokio::test]
    async fn test_multicast_sequential_last_wins() {
        let endpoints = vec![
            uppercase_processor(),
            uppercase_processor(),
            uppercase_processor(),
        ];

        let config = MulticastConfig::new(); // LastWins by default
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("hello"))
            .await
            .unwrap();

        assert_eq!(result.input.body.as_text(), Some("HELLO"));
    }

    // ── 2. Sequential + CollectAll ─────────────────────────────────────

    #[tokio::test]
    async fn test_multicast_sequential_collect_all() {
        let endpoints = vec![
            uppercase_processor(),
            uppercase_processor(),
            uppercase_processor(),
        ];

        let config = MulticastConfig::new().aggregation(MulticastStrategy::CollectAll);
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("hello"))
            .await
            .unwrap();

        let expected = serde_json::json!(["HELLO", "HELLO", "HELLO"]);
        match &result.input.body {
            Body::Json(v) => assert_eq!(*v, expected),
            other => panic!("expected JSON body, got {other:?}"),
        }
    }

    // ── 3. Sequential + Original ───────────────────────────────────────

    #[tokio::test]
    async fn test_multicast_sequential_original() {
        let endpoints = vec![
            uppercase_processor(),
            uppercase_processor(),
            uppercase_processor(),
        ];

        let config = MulticastConfig::new().aggregation(MulticastStrategy::Original);
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("hello"))
            .await
            .unwrap();

        // Original body should be unchanged
        assert_eq!(result.input.body.as_text(), Some("hello"));
    }

    // ── 4. Sequential + Custom aggregation ─────────────────────────────

    #[tokio::test]
    async fn test_multicast_sequential_custom_aggregation() {
        let joiner: Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync> =
            Arc::new(|mut acc: Exchange, next: Exchange| {
                let acc_text = acc.input.body.as_text().unwrap_or("").to_string();
                let next_text = next.input.body.as_text().unwrap_or("").to_string();
                acc.input.body = Body::Text(format!("{acc_text}+{next_text}"));
                acc
            });

        let endpoints = vec![
            uppercase_processor(),
            uppercase_processor(),
            uppercase_processor(),
        ];

        let config = MulticastConfig::new().aggregation(MulticastStrategy::Custom(joiner));
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("a"))
            .await
            .unwrap();

        assert_eq!(result.input.body.as_text(), Some("A+A+A"));
    }

    // ── 5. Stop on exception ───────────────────────────────────────────

    #[tokio::test]
    async fn test_multicast_stop_on_exception() {
        let endpoints = vec![
            uppercase_processor(),
            failing_processor(),
            uppercase_processor(),
        ];

        let config = MulticastConfig::new().stop_on_exception(true);
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("hello"))
            .await;

        assert!(result.is_err(), "expected error due to stop_on_exception");
    }

    // ── 6. Continue on exception ───────────────────────────────────────

    #[tokio::test]
    async fn test_multicast_continue_on_exception() {
        let endpoints = vec![
            uppercase_processor(),
            failing_processor(),
            uppercase_processor(),
        ];

        let config = MulticastConfig::new()
            .stop_on_exception(false)
            .aggregation(MulticastStrategy::LastWins);
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("hello"))
            .await;

        // LastWins: last endpoint succeeded, so result should be OK
        assert!(result.is_ok(), "last endpoint should succeed");
        assert_eq!(result.unwrap().input.body.as_text(), Some("HELLO"));
    }

    // ── 7. Stop on exception with fail_on_nth ─────────────────────────────

    #[tokio::test]
    async fn test_multicast_stop_on_exception_halts_early() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        
        // Track which endpoints actually execute
        let executed = Arc::new(AtomicUsize::new(0));
        
        let exec_clone1 = Arc::clone(&executed);
        let endpoint0 = BoxProcessor::from_fn(move |ex: Exchange| {
            let e = Arc::clone(&exec_clone1);
            Box::pin(async move {
                e.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(ex)
            })
        });
        
        let exec_clone2 = Arc::clone(&executed);
        let endpoint1 = BoxProcessor::from_fn(move |_ex: Exchange| {
            let e = Arc::clone(&exec_clone2);
            Box::pin(async move {
                e.fetch_add(1, AtomicOrdering::SeqCst);
                Err(CamelError::ProcessorError("fail on 1".into()))
            })
        });
        
        let exec_clone3 = Arc::clone(&executed);
        let endpoint2 = BoxProcessor::from_fn(move |ex: Exchange| {
            let e = Arc::clone(&exec_clone3);
            Box::pin(async move {
                e.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(ex)
            })
        });
        
        let endpoints = vec![endpoint0, endpoint1, endpoint2];
        let config = MulticastConfig::new().stop_on_exception(true);
        let mut svc = MulticastService::new(endpoints, config);
        
        let result = svc.ready().await.unwrap().call(make_exchange("x")).await;
        assert!(result.is_err(), "should fail at endpoint 1");
        
        // Only endpoints 0 and 1 should have executed (2 should be skipped)
        let count = executed.load(AtomicOrdering::SeqCst);
        assert_eq!(count, 2, "endpoint 2 should not have executed due to stop_on_exception");
    }

    // ── 8. Continue on exception with fail_on_nth ─────────────────────────

    #[tokio::test]
    async fn test_multicast_continue_on_exception_executes_all() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        
        // Track which endpoints actually execute
        let executed = Arc::new(AtomicUsize::new(0));
        
        let exec_clone1 = Arc::clone(&executed);
        let endpoint0 = BoxProcessor::from_fn(move |ex: Exchange| {
            let e = Arc::clone(&exec_clone1);
            Box::pin(async move {
                e.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(ex)
            })
        });
        
        let exec_clone2 = Arc::clone(&executed);
        let endpoint1 = BoxProcessor::from_fn(move |_ex: Exchange| {
            let e = Arc::clone(&exec_clone2);
            Box::pin(async move {
                e.fetch_add(1, AtomicOrdering::SeqCst);
                Err(CamelError::ProcessorError("fail on 1".into()))
            })
        });
        
        let exec_clone3 = Arc::clone(&executed);
        let endpoint2 = BoxProcessor::from_fn(move |ex: Exchange| {
            let e = Arc::clone(&exec_clone3);
            Box::pin(async move {
                e.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(ex)
            })
        });
        
        let endpoints = vec![endpoint0, endpoint1, endpoint2];
        let config = MulticastConfig::new()
            .stop_on_exception(false)
            .aggregation(MulticastStrategy::LastWins);
        let mut svc = MulticastService::new(endpoints, config);
        
        let result = svc.ready().await.unwrap().call(make_exchange("x")).await;
        assert!(result.is_ok(), "last endpoint should succeed");
        
        // All 3 endpoints should have executed
        let count = executed.load(AtomicOrdering::SeqCst);
        assert_eq!(count, 3, "all endpoints should have executed despite error in endpoint 1");
    }

    // ── 9. Empty endpoints ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_multicast_empty_endpoints() {
        let endpoints: Vec<BoxProcessor> = vec![];

        let config = MulticastConfig::new();
        let mut svc = MulticastService::new(endpoints, config);

        let mut ex = make_exchange("hello");
        ex.set_property("marker", Value::Bool(true));

        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("hello"));
        assert_eq!(result.property("marker"), Some(&Value::Bool(true)));
    }

    // ── 10. Metadata properties ─────────────────────────────────────────

    #[tokio::test]
    async fn test_multicast_metadata_properties() {
        // Use a pipeline that records the metadata into the body as JSON
        let recorder = BoxProcessor::from_fn(|ex: Exchange| {
            Box::pin(async move {
                let idx = ex.property(CAMEL_MULTICAST_INDEX).cloned();
                let complete = ex.property(CAMEL_MULTICAST_COMPLETE).cloned();
                let body = serde_json::json!({
                    "index": idx,
                    "complete": complete,
                });
                let mut out = ex;
                out.input.body = Body::Json(body);
                Ok(out)
            })
        });

        let endpoints = vec![recorder.clone(), recorder.clone(), recorder];

        let config = MulticastConfig::new().aggregation(MulticastStrategy::CollectAll);
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("x"))
            .await
            .unwrap();

        let expected = serde_json::json!([
            {"index": 0, "complete": false},
            {"index": 1, "complete": false},
            {"index": 2, "complete": true},
        ]);
        match &result.input.body {
            Body::Json(v) => assert_eq!(*v, expected),
            other => panic!("expected JSON body, got {other:?}"),
        }
    }

    // ── 11. poll_ready delegates to endpoints ────────────────────────────

    #[tokio::test]
    async fn test_poll_ready_delegates_to_endpoints() {
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

        let config = MulticastConfig::new();
        let mut svc = MulticastService::new(vec![boxed], config);

        // First poll should be Pending.
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let poll = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(
            poll.is_pending(),
            "expected Pending when endpoint not ready"
        );

        // Mark endpoint as ready.
        ready_flag.store(true, Ordering::SeqCst);

        let poll = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(
            matches!(poll, Poll::Ready(Ok(()))),
            "expected Ready after endpoint becomes ready"
        );
    }

    // ── 12. CollectAll with error propagates ────────────────────────────

    #[tokio::test]
    async fn test_multicast_collect_all_error_propagates() {
        let endpoints = vec![
            uppercase_processor(),
            failing_processor(),
            uppercase_processor(),
        ];

        let config = MulticastConfig::new()
            .stop_on_exception(false)
            .aggregation(MulticastStrategy::CollectAll);
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("hello"))
            .await;

        assert!(result.is_err(), "CollectAll should propagate first error");
    }

    // ── 13. LastWins with error last returns error ──────────────────────

    #[tokio::test]
    async fn test_multicast_last_wins_error_last() {
        let endpoints = vec![
            uppercase_processor(),
            uppercase_processor(),
            failing_processor(),
        ];

        let config = MulticastConfig::new()
            .stop_on_exception(false)
            .aggregation(MulticastStrategy::LastWins);
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("hello"))
            .await;

        assert!(result.is_err(), "LastWins should return last error");
    }

    // ── 14. Custom aggregation with error propagates ────────────────────

    #[tokio::test]
    async fn test_multicast_custom_error_propagates() {
        let joiner: Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync> =
            Arc::new(|acc: Exchange, _next: Exchange| acc);

        let endpoints = vec![
            uppercase_processor(),
            failing_processor(),
            uppercase_processor(),
        ];

        let config = MulticastConfig::new()
            .stop_on_exception(false)
            .aggregation(MulticastStrategy::Custom(joiner));
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("hello"))
            .await;

        assert!(
            result.is_err(),
            "Custom aggregation should propagate errors"
        );
    }

    // ── 15. Parallel + CollectAll basic ─────────────────────────────────

    #[tokio::test]
    async fn test_multicast_parallel_basic() {
        let endpoints = vec![uppercase_processor(), uppercase_processor()];

        let config = MulticastConfig::new()
            .parallel(true)
            .aggregation(MulticastStrategy::CollectAll);
        let mut svc = MulticastService::new(endpoints, config);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("test"))
            .await
            .unwrap();

        // Both endpoints uppercase "test" → ["TEST", "TEST"]
        // Note: parallel order is not guaranteed for CollectAll, but with identical processors it doesn't matter
        match &result.input.body {
            Body::Json(v) => {
                let arr = v.as_array().expect("expected array");
                assert_eq!(arr.len(), 2);
                assert!(arr.iter().all(|v| v.as_str() == Some("TEST")));
            }
            other => panic!("expected JSON body, got {:?}", other),
        }
    }

    // ── 16. Parallel with concurrency limit ─────────────────────────────

    #[tokio::test]
    async fn test_multicast_parallel_with_limit() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        let endpoints: Vec<BoxProcessor> = (0..4)
            .map(|_| {
                let c = Arc::clone(&concurrent);
                let mc = Arc::clone(&max_concurrent);
                BoxProcessor::from_fn(move |ex: Exchange| {
                    let c = Arc::clone(&c);
                    let mc = Arc::clone(&mc);
                    Box::pin(async move {
                        let current = c.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                        mc.fetch_max(current, AtomicOrdering::SeqCst);
                        tokio::task::yield_now().await;
                        c.fetch_sub(1, AtomicOrdering::SeqCst);
                        Ok(ex)
                    })
                })
            })
            .collect();

        let config = MulticastConfig::new().parallel(true).parallel_limit(2);
        let mut svc = MulticastService::new(endpoints, config);

        let _ = svc.ready().await.unwrap().call(make_exchange("x")).await;

        let observed_max = max_concurrent.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            observed_max <= 2,
            "max concurrency was {}, expected <= 2",
            observed_max
        );
    }
}
