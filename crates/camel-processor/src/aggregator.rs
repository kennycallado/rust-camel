use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tower::Service;

use camel_api::{
    CamelError,
    aggregator::{AggregationStrategy, AggregatorConfig, CompletionCondition},
    body::Body,
    exchange::Exchange,
    message::Message,
};

pub const CAMEL_AGGREGATOR_PENDING: &str = "CamelAggregatorPending";
pub const CAMEL_AGGREGATED_SIZE: &str = "CamelAggregatedSize";
pub const CAMEL_AGGREGATED_KEY: &str = "CamelAggregatedKey";

/// Internal bucket structure with timestamp tracking for TTL eviction.
struct Bucket {
    exchanges: Vec<Exchange>,
    #[allow(dead_code)]
    created_at: Instant,
    last_updated: Instant,
}

impl Bucket {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            exchanges: Vec::new(),
            created_at: now,
            last_updated: now,
        }
    }

    fn push(&mut self, exchange: Exchange) {
        self.exchanges.push(exchange);
        self.last_updated = Instant::now();
    }

    fn len(&self) -> usize {
        self.exchanges.len()
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        Instant::now().duration_since(self.last_updated) >= ttl
    }
}

#[derive(Clone)]
pub struct AggregatorService {
    config: AggregatorConfig,
    buckets: Arc<Mutex<HashMap<String, Bucket>>>,
}

impl AggregatorService {
    pub fn new(config: AggregatorConfig) -> Self {
        Self {
            config,
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Service<Exchange> for AggregatorService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let buckets = Arc::clone(&self.buckets);

        Box::pin(async move {
            // 1. Extract correlation key value from header
            let key_value = exchange
                .input
                .headers
                .get(&config.header_name)
                .cloned()
                .ok_or_else(|| {
                    CamelError::ProcessorError(format!(
                        "Aggregator: missing correlation key header '{}'",
                        config.header_name
                    ))
                })?;

            // Serialize to String for use as HashMap key
            let key_str = serde_json::to_string(&key_value)
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;

            // 2. Insert into bucket and check completion (lock scope)
            let completed_bucket = {
                let mut guard = buckets.lock().unwrap_or_else(|e| e.into_inner());

                // Evict expired buckets if TTL is configured
                if let Some(ttl) = config.bucket_ttl {
                    guard.retain(|_, bucket| !bucket.is_expired(ttl));
                }

                // Enforce max buckets limit - reject new correlation keys if at limit
                if let Some(max) = config.max_buckets
                    && !guard.contains_key(&key_str)
                    && guard.len() >= max
                {
                    tracing::warn!(
                        max_buckets = max,
                        correlation_key = %key_str,
                        "Aggregator reached max buckets limit, rejecting new correlation key"
                    );
                    return Err(CamelError::ProcessorError(format!(
                        "Aggregator reached maximum {} buckets",
                        max
                    )));
                }

                let bucket = guard.entry(key_str.clone()).or_insert_with(Bucket::new);
                bucket.push(exchange);

                let is_complete = match &config.completion {
                    CompletionCondition::Size(n) => bucket.len() >= *n,
                    CompletionCondition::Predicate(pred) => pred(&bucket.exchanges),
                };

                if is_complete {
                    guard.remove(&key_str).map(|b| b.exchanges)
                } else {
                    None
                }
            }; // Mutex released here

            // 3. Emit aggregated exchange or return pending placeholder
            match completed_bucket {
                Some(exchanges) => {
                    let size = exchanges.len();
                    let mut result = aggregate(exchanges, &config.strategy)?;
                    result.set_property(CAMEL_AGGREGATED_SIZE, serde_json::json!(size as u64));
                    result.set_property(CAMEL_AGGREGATED_KEY, key_value);
                    Ok(result)
                }
                None => {
                    let mut pending = Exchange::new(Message {
                        headers: Default::default(),
                        body: Body::Empty,
                    });
                    pending.set_property(CAMEL_AGGREGATOR_PENDING, serde_json::json!(true));
                    Ok(pending)
                }
            }
        })
    }
}

fn aggregate(
    exchanges: Vec<Exchange>,
    strategy: &AggregationStrategy,
) -> Result<Exchange, CamelError> {
    match strategy {
        AggregationStrategy::CollectAll => {
            let bodies: Vec<serde_json::Value> = exchanges
                .into_iter()
                .map(|e| match e.input.body {
                    Body::Json(v) => v,
                    Body::Text(s) => serde_json::Value::String(s),
                    Body::Bytes(b) => {
                        serde_json::Value::String(String::from_utf8_lossy(&b).into_owned())
                    }
                    Body::Empty => serde_json::Value::Null,
                    Body::Stream(s) => serde_json::json!({
                        "_stream": {
                            "origin": s.metadata.origin,
                            "consumed": true,
                            "hint": "Materialize with .into_bytes() before aggregation if content needed"
                        }
                    }),
                })
                .collect();
            Ok(Exchange::new(Message {
                headers: Default::default(),
                body: Body::Json(serde_json::Value::Array(bodies)),
            }))
        }
        AggregationStrategy::Custom(f) => {
            let mut iter = exchanges.into_iter();
            let first = iter.next().ok_or_else(|| {
                CamelError::ProcessorError("Aggregator: empty bucket".to_string())
            })?;
            Ok(iter.fold(first, |acc, next| f(acc, next)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{
        aggregator::{AggregationStrategy, AggregatorConfig},
        body::Body,
        exchange::Exchange,
        message::Message,
    };
    use tower::ServiceExt;

    fn make_exchange(header: &str, value: &str, body: &str) -> Exchange {
        let mut msg = Message {
            headers: Default::default(),
            body: Body::Text(body.to_string()),
        };
        msg.headers
            .insert(header.to_string(), serde_json::json!(value));
        Exchange::new(msg)
    }

    fn config_size(n: usize) -> AggregatorConfig {
        AggregatorConfig::correlate_by("orderId")
            .complete_when_size(n)
            .build()
    }

    #[tokio::test]
    async fn test_pending_exchange_not_yet_complete() {
        let mut svc = AggregatorService::new(config_size(3));
        let ex = make_exchange("orderId", "A", "first");
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert!(matches!(result.input.body, Body::Empty));
        assert_eq!(
            result.property(CAMEL_AGGREGATOR_PENDING),
            Some(&serde_json::json!(true))
        );
    }

    #[tokio::test]
    async fn test_completes_on_size() {
        let mut svc = AggregatorService::new(config_size(3));
        for _ in 0..2 {
            let ex = make_exchange("orderId", "A", "item");
            let r = svc.ready().await.unwrap().call(ex).await.unwrap();
            assert!(matches!(r.input.body, Body::Empty));
        }
        let ex = make_exchange("orderId", "A", "last");
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert!(result.property(CAMEL_AGGREGATOR_PENDING).is_none());
        assert_eq!(
            result.property(CAMEL_AGGREGATED_SIZE),
            Some(&serde_json::json!(3u64))
        );
    }

    #[tokio::test]
    async fn test_collect_all_produces_json_array() {
        let mut svc = AggregatorService::new(config_size(2));
        svc.ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "A", "alpha"))
            .await
            .unwrap();
        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "A", "beta"))
            .await
            .unwrap();
        let Body::Json(v) = &result.input.body else {
            panic!("expected Body::Json")
        };
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0], serde_json::json!("alpha"));
        assert_eq!(arr[1], serde_json::json!("beta"));
    }

    #[tokio::test]
    async fn test_two_keys_independent_buckets() {
        // completionSize=3 so we can test that A and B accumulate independently.
        let mut svc = AggregatorService::new(config_size(3));
        svc.ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "A", "a1"))
            .await
            .unwrap();
        svc.ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "B", "b1"))
            .await
            .unwrap();
        svc.ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "A", "a2"))
            .await
            .unwrap();
        // A has 2 items, B has 1 item — neither complete yet
        let ra = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "A", "a3"))
            .await
            .unwrap();
        // A now has 3 → completes
        assert!(matches!(ra.input.body, Body::Json(_)));
        // B only has 1 → still pending
        let rb = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "B", "b_check"))
            .await
            .unwrap();
        assert!(matches!(rb.input.body, Body::Empty));
    }

    #[tokio::test]
    async fn test_bucket_resets_after_completion() {
        let mut svc = AggregatorService::new(config_size(2));
        svc.ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "A", "x"))
            .await
            .unwrap();
        svc.ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "A", "x"))
            .await
            .unwrap(); // completes
        // New bucket starts
        let r = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "A", "new"))
            .await
            .unwrap();
        assert!(matches!(r.input.body, Body::Empty)); // pending again
    }

    #[tokio::test]
    async fn test_completion_size_1_emits_immediately() {
        let mut svc = AggregatorService::new(config_size(1));
        let ex = make_exchange("orderId", "A", "solo");
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert!(result.property(CAMEL_AGGREGATOR_PENDING).is_none());
    }

    #[tokio::test]
    async fn test_custom_aggregation_strategy() {
        use camel_api::aggregator::AggregationFn;
        use std::sync::Arc;

        let f: AggregationFn = Arc::new(|mut acc: Exchange, next: Exchange| {
            let combined = format!(
                "{}+{}",
                acc.input.body.as_text().unwrap_or(""),
                next.input.body.as_text().unwrap_or("")
            );
            acc.input.body = Body::Text(combined);
            acc
        });
        let config = AggregatorConfig::correlate_by("key")
            .complete_when_size(2)
            .strategy(AggregationStrategy::Custom(f))
            .build();
        let mut svc = AggregatorService::new(config);
        svc.ready()
            .await
            .unwrap()
            .call(make_exchange("key", "X", "hello"))
            .await
            .unwrap();
        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("key", "X", "world"))
            .await
            .unwrap();
        assert_eq!(result.input.body.as_text(), Some("hello+world"));
    }

    #[tokio::test]
    async fn test_completion_predicate() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_when(|bucket| {
                bucket
                    .iter()
                    .any(|e| e.input.body.as_text() == Some("DONE"))
            })
            .build();
        let mut svc = AggregatorService::new(config);
        svc.ready()
            .await
            .unwrap()
            .call(make_exchange("key", "K", "first"))
            .await
            .unwrap();
        svc.ready()
            .await
            .unwrap()
            .call(make_exchange("key", "K", "second"))
            .await
            .unwrap();
        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("key", "K", "DONE"))
            .await
            .unwrap();
        assert!(result.property(CAMEL_AGGREGATOR_PENDING).is_none());
    }

    #[tokio::test]
    async fn test_missing_header_returns_error() {
        let mut svc = AggregatorService::new(config_size(2));
        let msg = Message {
            headers: Default::default(),
            body: Body::Text("no key".into()),
        };
        let ex = Exchange::new(msg);
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            camel_api::CamelError::ProcessorError(_)
        ));
    }

    #[tokio::test]
    async fn test_cloned_service_shares_state() {
        let svc1 = AggregatorService::new(config_size(2));
        let mut svc2 = svc1.clone();
        // send first exchange via svc1
        svc1.clone()
            .ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "A", "from-svc1"))
            .await
            .unwrap();
        // send second exchange via svc2 — should complete because same Arc<Mutex>
        let result = svc2
            .ready()
            .await
            .unwrap()
            .call(make_exchange("orderId", "A", "from-svc2"))
            .await
            .unwrap();
        assert!(result.property(CAMEL_AGGREGATOR_PENDING).is_none());
    }

    #[tokio::test]
    async fn test_camel_aggregated_key_property_set() {
        let mut svc = AggregatorService::new(config_size(1));
        let ex = make_exchange("orderId", "ORDER-42", "body");
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(
            result.property(CAMEL_AGGREGATED_KEY),
            Some(&serde_json::json!("ORDER-42"))
        );
    }

    #[tokio::test]
    async fn test_aggregator_enforces_max_buckets() {
        let config = AggregatorConfig::correlate_by("orderId")
            .complete_when_size(2)
            .max_buckets(3)
            .build();

        let mut svc = AggregatorService::new(config);

        // Create 3 different correlation keys (fills limit)
        for i in 0..3 {
            let ex = make_exchange("orderId", &format!("key-{}", i), "body");
            let _ = svc.ready().await.unwrap().call(ex).await.unwrap();
        }

        // 4th key should be rejected
        let ex = make_exchange("orderId", "key-4", "body");
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_err(), "Should reject when max buckets reached");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("maximum"),
            "Error message should contain 'maximum': {}",
            err
        );
    }

    #[tokio::test]
    async fn test_max_buckets_allows_existing_key() {
        let config = AggregatorConfig::correlate_by("orderId")
            .complete_when_size(5) // Large size so bucket doesn't complete
            .max_buckets(2)
            .build();

        let mut svc = AggregatorService::new(config);

        // Create 2 different correlation keys (fills limit)
        let ex1 = make_exchange("orderId", "key-A", "body1");
        let _ = svc.ready().await.unwrap().call(ex1).await.unwrap();
        let ex2 = make_exchange("orderId", "key-B", "body2");
        let _ = svc.ready().await.unwrap().call(ex2).await.unwrap();

        // Should still allow adding to existing key
        let ex3 = make_exchange("orderId", "key-A", "body3");
        let result = svc.ready().await.unwrap().call(ex3).await;
        assert!(
            result.is_ok(),
            "Should allow adding to existing bucket even at max limit"
        );
    }

    #[tokio::test]
    async fn test_bucket_ttl_eviction() {
        let config = AggregatorConfig::correlate_by("orderId")
            .complete_when_size(10) // Large size so bucket doesn't complete normally
            .bucket_ttl(Duration::from_millis(50))
            .build();

        let mut svc = AggregatorService::new(config);

        // Create a bucket
        let ex1 = make_exchange("orderId", "key-A", "body1");
        let _ = svc.ready().await.unwrap().call(ex1).await.unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create a new bucket - this should trigger eviction of the old one
        let ex2 = make_exchange("orderId", "key-B", "body2");
        let _ = svc.ready().await.unwrap().call(ex2).await.unwrap();

        // The expired bucket should have been evicted, so we should be able to
        // add a new key-A bucket again
        let ex3 = make_exchange("orderId", "key-A", "body3");
        let result = svc.ready().await.unwrap().call(ex3).await;
        assert!(result.is_ok(), "Should be able to recreate evicted bucket");
    }

    #[tokio::test]
    async fn test_aggregate_stream_bodies_creates_valid_json() {
        use camel_api::{Body, StreamBody, StreamMetadata};
        use futures::stream;
        use bytes::Bytes;
        use tokio::sync::Mutex;

        let chunks = vec![Ok(Bytes::from("test"))];
        let stream_body = StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream::iter(chunks))))),
            metadata: StreamMetadata {
                origin: Some("file:///test.txt".to_string()),
                ..Default::default()
            },
        };

        let ex1 = Exchange::new(Message {
            headers: Default::default(),
            body: Body::Stream(stream_body),
        });

        let exchanges = vec![ex1];
        let result = aggregate(exchanges, &AggregationStrategy::CollectAll);
        
        if let Ok(Exchange { input, .. }) = result {
            if let Body::Json(value) = input.body {
                // Should be valid JSON that can be parsed
                let json_str = serde_json::to_string(&value).unwrap();
                let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
                
                // Should contain _stream object
                assert!(parsed.is_array());
                let arr = parsed.as_array().unwrap();
                assert!(arr[0].is_object());
                assert!(arr[0]["_stream"].is_object());
                assert_eq!(arr[0]["_stream"]["origin"], "file:///test.txt");
            } else {
                panic!("Expected Json body");
            }
        } else {
            panic!("Expected Ok result");
        }
    }
}
