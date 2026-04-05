use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower::Service;

use camel_api::{
    CamelError,
    aggregator::{
        AggregationStrategy, AggregatorConfig, CompletionCondition, CompletionMode,
        CompletionReason, CorrelationStrategy,
    },
    body::Body,
    exchange::Exchange,
    message::Message,
};
use camel_language_api::Language;

pub type SharedLanguageRegistry = Arc<std::sync::Mutex<HashMap<String, Arc<dyn Language>>>>;

pub const CAMEL_AGGREGATOR_PENDING: &str = "CamelAggregatorPending";
pub const CAMEL_AGGREGATED_SIZE: &str = "CamelAggregatedSize";
pub const CAMEL_AGGREGATED_KEY: &str = "CamelAggregatedKey";
pub const CAMEL_AGGREGATED_COMPLETION_REASON: &str = "CamelAggregatedCompletionReason";

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

    #[allow(dead_code)]
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
    timeout_tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
    late_tx: mpsc::Sender<Exchange>,
    language_registry: SharedLanguageRegistry,
    route_cancel: CancellationToken,
}

impl AggregatorService {
    pub fn new(
        config: AggregatorConfig,
        late_tx: mpsc::Sender<Exchange>,
        language_registry: SharedLanguageRegistry,
        route_cancel: CancellationToken,
    ) -> Self {
        Self {
            config,
            buckets: Arc::new(Mutex::new(HashMap::new())),
            timeout_tasks: Arc::new(Mutex::new(HashMap::new())),
            late_tx,
            language_registry,
            route_cancel,
        }
    }

    pub fn has_timeout(&self) -> bool {
        has_timeout_condition(&self.config.completion)
    }

    pub fn force_complete_all(&self) {
        let mut buckets_guard = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let keys: Vec<String> = buckets_guard.keys().cloned().collect();

        for key in keys {
            if let Some(bucket) = buckets_guard.remove(&key) {
                if self.config.force_completion_on_stop {
                    cancel_timeout_task(&key, &self.timeout_tasks);
                    match aggregate(bucket.exchanges, &self.config.strategy) {
                        Ok(mut result) => {
                            result.set_property(
                                CAMEL_AGGREGATED_COMPLETION_REASON,
                                serde_json::json!(CompletionReason::Stop.as_str()),
                            );
                            if self.late_tx.try_send(result).is_err() {
                                tracing::warn!(
                                    key = %key,
                                    "aggregator force-complete emit dropped: late channel full"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                key = %key,
                                error = %e,
                                "aggregation failed in force_complete_all"
                            );
                        }
                    }
                } else {
                    cancel_timeout_task(&key, &self.timeout_tasks);
                }
            }
        }
    }
}

pub fn has_timeout_condition(mode: &CompletionMode) -> bool {
    match mode {
        CompletionMode::Single(CompletionCondition::Timeout(_)) => true,
        CompletionMode::Any(conditions) => conditions
            .iter()
            .any(|c| matches!(c, CompletionCondition::Timeout(_))),
        _ => false,
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
        let timeout_tasks = Arc::clone(&self.timeout_tasks);
        let late_tx = self.late_tx.clone();
        let language_registry = Arc::clone(&self.language_registry);
        let route_cancel = self.route_cancel.clone();

        Box::pin(async move {
            let key_value =
                extract_correlation_key(&exchange, &config.correlation, &language_registry)?;

            let key_str = serde_json::to_string(&key_value)
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;

            let completed_bucket = {
                let mut guard = buckets.lock().unwrap_or_else(|e| e.into_inner());

                if let Some(ttl) = config.bucket_ttl {
                    guard.retain(|_, bucket| !bucket.is_expired(ttl));
                }

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

                let (is_complete, reason) =
                    check_sync_completion(&config.completion, &bucket.exchanges);

                if is_complete {
                    let exchanges = guard.remove(&key_str).map(|b| b.exchanges);
                    (exchanges, reason)
                } else {
                    (None, CompletionReason::Size) // placeholder; reason unused when None
                }
            };

            if completed_bucket.0.is_none() && has_timeout_condition(&config.completion) {
                let cancel = {
                    let mut tt_guard = timeout_tasks.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(existing) = tt_guard.get(&key_str) {
                        existing.cancel();
                    }
                    let token = CancellationToken::new();
                    tt_guard.insert(key_str.clone(), token.clone());
                    token
                };

                let timeout_dur = extract_timeout_duration(&config.completion);
                if let Some(timeout) = timeout_dur {
                    spawn_timeout_task(
                        key_str.clone(),
                        timeout,
                        cancel,
                        buckets.clone(),
                        timeout_tasks.clone(),
                        late_tx,
                        config.strategy.clone(),
                        config.discard_on_timeout,
                        route_cancel,
                    );
                }
            }

            if let Some(exchanges) = completed_bucket.0 {
                cancel_timeout_task(&key_str, &timeout_tasks);
                let reason = completed_bucket.1;
                let size = exchanges.len();
                let mut result = aggregate(exchanges, &config.strategy)?;
                result.set_property(CAMEL_AGGREGATED_SIZE, serde_json::json!(size as u64));
                result.set_property(CAMEL_AGGREGATED_KEY, key_value);
                result.set_property(
                    CAMEL_AGGREGATED_COMPLETION_REASON,
                    serde_json::json!(reason.as_str()),
                );
                Ok(result)
            } else {
                let mut pending = Exchange::new(Message {
                    headers: Default::default(),
                    body: Body::Empty,
                });
                pending.set_property(CAMEL_AGGREGATOR_PENDING, serde_json::json!(true));
                Ok(pending)
            }
        })
    }
}

fn extract_correlation_key(
    exchange: &Exchange,
    strategy: &CorrelationStrategy,
    registry: &SharedLanguageRegistry,
) -> Result<serde_json::Value, CamelError> {
    match strategy {
        CorrelationStrategy::HeaderName(h) => {
            exchange.input.headers.get(h).cloned().ok_or_else(|| {
                CamelError::ProcessorError(format!(
                    "Aggregator: missing correlation key header '{}'",
                    h
                ))
            })
        }
        CorrelationStrategy::Expression { expr, language } => {
            let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
            let lang = reg.get(language).ok_or_else(|| {
                CamelError::ProcessorError(format!(
                    "Aggregator: language '{}' not found in registry",
                    language
                ))
            })?;
            let expression = lang
                .create_expression(expr)
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;
            let value = expression
                .evaluate(exchange)
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;
            if value.is_null() {
                return Err(CamelError::ProcessorError(format!(
                    "Aggregator: correlation expression '{}' evaluated to null",
                    expr
                )));
            }
            Ok(value)
        }
        CorrelationStrategy::Fn(f) => f(exchange).map(serde_json::Value::String).ok_or_else(|| {
            CamelError::ProcessorError("Aggregator: correlation function returned None".to_string())
        }),
    }
}

fn check_sync_completion(
    mode: &CompletionMode,
    exchanges: &[Exchange],
) -> (bool, CompletionReason) {
    match mode {
        CompletionMode::Single(cond) => check_single(cond, exchanges),
        CompletionMode::Any(conditions) => {
            for cond in conditions {
                if let CompletionCondition::Timeout(_) = cond {
                    continue;
                }
                let (done, reason) = check_single(cond, exchanges);
                if done {
                    return (true, reason);
                }
            }
            (false, CompletionReason::Size)
        }
    }
}

fn check_single(cond: &CompletionCondition, exchanges: &[Exchange]) -> (bool, CompletionReason) {
    match cond {
        CompletionCondition::Size(n) => (exchanges.len() >= *n, CompletionReason::Size),
        CompletionCondition::Predicate(pred) => (pred(exchanges), CompletionReason::Predicate),
        CompletionCondition::Timeout(_) => (false, CompletionReason::Timeout),
    }
}

fn extract_timeout_duration(mode: &CompletionMode) -> Option<Duration> {
    match mode {
        CompletionMode::Single(CompletionCondition::Timeout(d)) => Some(*d),
        CompletionMode::Any(conditions) => conditions.iter().find_map(|c| {
            if let CompletionCondition::Timeout(d) = c {
                Some(*d)
            } else {
                None
            }
        }),
        _ => None,
    }
}

fn cancel_timeout_task(key: &str, timeout_tasks: &Arc<Mutex<HashMap<String, CancellationToken>>>) {
    let mut guard = timeout_tasks.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(token) = guard.remove(key) {
        token.cancel();
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_timeout_task(
    key: String,
    timeout: Duration,
    cancel: CancellationToken,
    buckets: Arc<Mutex<HashMap<String, Bucket>>>,
    timeout_tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
    late_tx: mpsc::Sender<Exchange>,
    strategy: AggregationStrategy,
    discard: bool,
    _route_cancel: CancellationToken,
) {
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(timeout) => {
                let should_proceed = {
                    let mut tt_guard = timeout_tasks.lock().unwrap_or_else(|e| e.into_inner());
                    if cancel_clone.is_cancelled() {
                        false
                    } else {
                        tt_guard.remove(&key);
                        true
                    }
                };
                if !should_proceed {
                    return;
                }
                let bucket_exchanges = {
                    let mut guard = buckets.lock().unwrap_or_else(|e| e.into_inner());
                    guard.remove(&key).map(|b| b.exchanges)
                };
                if let Some(exchanges) = bucket_exchanges
                    && !discard
                {
                    match aggregate(exchanges, &strategy) {
                        Ok(mut result) => {
                            result.set_property(
                                CAMEL_AGGREGATED_COMPLETION_REASON,
                                serde_json::json!(CompletionReason::Timeout.as_str()),
                            );
                            if late_tx.try_send(result).is_err() {
                                tracing::warn!(
                                    key = %key,
                                    "aggregator timeout emit dropped: late channel full"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                key = %key,
                                error = %e,
                                "aggregation failed in timeout task"
                            );
                        }
                    }
                }
            }
            _ = cancel_clone.cancelled() => {}
        }
    });
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
                    Body::Xml(s) => serde_json::Value::String(s),
                    Body::Bytes(b) => {
                        serde_json::Value::String(String::from_utf8_lossy(&b).into_owned())
                    }
                    Body::Empty => serde_json::Value::Null,
                    Body::Stream(s) => serde_json::json!({
                        "_stream": {
                            "origin": s.metadata.origin,
                            "placeholder": true,
                            "hint": "Materialize exchange body with .into_bytes() before aggregation if content needed"
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
    use std::collections::HashMap;

    use camel_api::{
        aggregator::{AggregationStrategy, AggregatorConfig},
        body::Body,
        exchange::Exchange,
        message::Message,
    };
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;
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

    fn new_test_svc(config: AggregatorConfig) -> AggregatorService {
        let (tx, _rx) = mpsc::channel(256);
        let registry: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        AggregatorService::new(config, tx, registry, cancel)
    }

    #[tokio::test]
    async fn test_pending_exchange_not_yet_complete() {
        let mut svc = new_test_svc(config_size(3));
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
        let mut svc = new_test_svc(config_size(3));
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
        let mut svc = new_test_svc(config_size(2));
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
        let mut svc = new_test_svc(config_size(3));
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
        let mut svc = new_test_svc(config_size(2));
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
        let mut svc = new_test_svc(config_size(1));
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
        let mut svc = new_test_svc(config);
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
        let mut svc = new_test_svc(config);
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
        let mut svc = new_test_svc(config_size(2));
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
        let svc1 = new_test_svc(config_size(2));
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
        let mut svc = new_test_svc(config_size(1));
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

        let mut svc = new_test_svc(config);

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

        let mut svc = new_test_svc(config);

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

        let mut svc = new_test_svc(config);

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

    #[tokio::test(start_paused = true)]
    async fn test_timeout_completes_bucket() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_on_timeout(Duration::from_millis(100))
            .build();
        let mut svc = new_test_svc(config);
        let ex = make_exchange("key", "A", "data");
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert!(result.property(CAMEL_AGGREGATOR_PENDING).is_some());

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(
            svc.buckets.lock().unwrap().len(),
            0,
            "bucket should be removed after timeout"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_timeout_resets_on_new_exchange() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_on_timeout(Duration::from_millis(150))
            .build();
        let mut svc = new_test_svc(config);

        let ex1 = make_exchange("key", "A", "first");
        let _ = svc.ready().await.unwrap().call(ex1).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        let ex2 = make_exchange("key", "A", "second");
        let _ = svc.ready().await.unwrap().call(ex2).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(
            svc.buckets.lock().unwrap().len(),
            1,
            "bucket should still exist — timeout was reset"
        );

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(
            svc.buckets.lock().unwrap().len(),
            0,
            "bucket should be gone after timeout fires"
        );
    }

    #[tokio::test]
    async fn test_composable_size_and_timeout() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_on_size_or_timeout(2, Duration::from_millis(200))
            .build();
        let mut svc = new_test_svc(config);

        let ex1 = make_exchange("key", "A", "first");
        let _ = svc.ready().await.unwrap().call(ex1).await.unwrap();
        assert!(svc.buckets.lock().unwrap().contains_key("\"A\""));

        let ex2 = make_exchange("key", "A", "second");
        let result = svc.ready().await.unwrap().call(ex2).await.unwrap();
        assert!(result.property(CAMEL_AGGREGATOR_PENDING).is_none());
        assert_eq!(
            result.property(CAMEL_AGGREGATED_COMPLETION_REASON),
            Some(&serde_json::json!("size"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_discard_on_timeout() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_on_timeout(Duration::from_millis(50))
            .discard_on_timeout(true)
            .build();
        let (tx, mut rx) = mpsc::channel(256);
        let registry: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        let mut svc = AggregatorService::new(config, tx, registry, cancel);

        let ex = make_exchange("key", "A", "data");
        let _ = svc.ready().await.unwrap().call(ex).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(
            rx.try_recv().is_err(),
            "no emit expected with discard_on_timeout"
        );
        assert_eq!(svc.buckets.lock().unwrap().len(), 0);
        assert!(
            svc.timeout_tasks.lock().unwrap().is_empty(),
            "timeout task should be cleaned up"
        );
    }

    #[tokio::test]
    async fn test_force_completion_on_stop() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_when_size(10)
            .force_completion_on_stop(true)
            .build();
        let (tx, mut rx) = mpsc::channel(256);
        let registry: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        let svc = AggregatorService::new(config, tx, registry, cancel);

        let mut call_svc = svc.clone();
        let ex = make_exchange("key", "A", "data");
        let _ = call_svc.ready().await.unwrap().call(ex).await.unwrap();

        svc.force_complete_all();

        let result = rx.try_recv().expect("should emit on force-complete");
        assert!(
            result.input.body.as_text().is_some() || matches!(result.input.body, Body::Json(_))
        );
        assert_eq!(
            result.property(CAMEL_AGGREGATED_COMPLETION_REASON),
            Some(&serde_json::json!("stop"))
        );
    }

    #[tokio::test]
    async fn test_completion_reason_property_size() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_when_size(1)
            .build();
        let mut svc = new_test_svc(config);
        let ex = make_exchange("key", "X", "body");
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(
            result.property(CAMEL_AGGREGATED_COMPLETION_REASON),
            Some(&serde_json::json!("size"))
        );
    }

    #[tokio::test]
    async fn test_completion_reason_property_predicate() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_when(|_| true)
            .build();
        let mut svc = new_test_svc(config);
        let ex = make_exchange("key", "X", "body");
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(
            result.property(CAMEL_AGGREGATED_COMPLETION_REASON),
            Some(&serde_json::json!("predicate"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_size_completes_before_timeout() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_on_size_or_timeout(2, Duration::from_millis(200))
            .build();
        let mut svc = new_test_svc(config);

        let ex1 = make_exchange("key", "A", "first");
        let _ = svc.ready().await.unwrap().call(ex1).await.unwrap();

        let ex2 = make_exchange("key", "A", "second");
        let result = svc.ready().await.unwrap().call(ex2).await.unwrap();

        assert!(result.property(CAMEL_AGGREGATOR_PENDING).is_none());
        assert_eq!(
            result.property(CAMEL_AGGREGATED_COMPLETION_REASON),
            Some(&serde_json::json!("size"))
        );
        assert_eq!(svc.buckets.lock().unwrap().len(), 0);

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            svc.buckets.lock().unwrap().len(),
            0,
            "no re-fire after timeout"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_concurrent_timeout_fire_and_new_exchange() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_on_size_or_timeout(2, Duration::from_millis(100))
            .build();
        let (tx, mut rx) = mpsc::channel(256);
        let registry: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        let mut svc = AggregatorService::new(config, tx, registry, cancel);

        let ex = make_exchange("key", "A", "data");
        let _ = svc.ready().await.unwrap().call(ex).await.unwrap();

        // Advance time past timeout — timeout task fires and removes bucket
        tokio::time::sleep(Duration::from_millis(150)).await;

        // New exchange arrives after timeout — starts a fresh bucket
        let ex2 = make_exchange("key", "A", "data2");
        let result = svc.ready().await.unwrap().call(ex2).await.unwrap();
        assert!(
            result.property(CAMEL_AGGREGATOR_PENDING).is_some(),
            "should be pending in new bucket"
        );

        // Drain late emits from timeout
        let mut late_count = 0;
        while rx.try_recv().is_ok() {
            late_count += 1;
        }
        assert_eq!(
            late_count, 1,
            "exactly 1 late emit from the timed-out bucket"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_late_channel_full_drops_with_warning() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_on_timeout(Duration::from_millis(50))
            .build();
        let (tx, mut rx) = mpsc::channel(1);
        rx.close();
        let registry: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        let mut svc = AggregatorService::new(config, tx, registry, cancel);

        let ex = make_exchange("key", "A", "data");
        let _ = svc.ready().await.unwrap().call(ex).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            svc.buckets.lock().unwrap().len(),
            0,
            "bucket removed despite channel closed"
        );
    }

    #[tokio::test]
    async fn test_aggregate_stream_bodies_creates_valid_json() {
        use bytes::Bytes;
        use camel_api::{Body, StreamBody, StreamMetadata};
        use futures::stream;
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

        let exchange = result.expect("Expected Ok result");
        assert!(
            matches!(exchange.input.body, Body::Json(_)),
            "Expected Json body"
        );

        if let Body::Json(value) = exchange.input.body {
            let json_str = serde_json::to_string(&value).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

            assert!(parsed.is_array(), "Result should be an array");
            let arr = parsed.as_array().unwrap();
            assert!(arr[0].is_object(), "First element should be an object");
            assert!(
                arr[0]["_stream"].is_object(),
                "Should contain _stream object"
            );
            assert_eq!(arr[0]["_stream"]["origin"], "file:///test.txt");
            assert_eq!(
                arr[0]["_stream"]["placeholder"], true,
                "placeholder flag should be true"
            );
        }
    }

    #[tokio::test]
    async fn test_aggregate_stream_bodies_with_none_origin() {
        use bytes::Bytes;
        use camel_api::{Body, StreamBody, StreamMetadata};
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

        let ex1 = Exchange::new(Message {
            headers: Default::default(),
            body: Body::Stream(stream_body),
        });

        let exchanges = vec![ex1];
        let result = aggregate(exchanges, &AggregationStrategy::CollectAll);

        let exchange = result.expect("Expected Ok result");
        assert!(
            matches!(exchange.input.body, Body::Json(_)),
            "Expected Json body"
        );

        if let Body::Json(value) = exchange.input.body {
            let json_str = serde_json::to_string(&value).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

            assert!(parsed.is_array(), "Result should be an array");
            let arr = parsed.as_array().unwrap();
            assert!(arr[0].is_object(), "First element should be an object");
            assert!(
                arr[0]["_stream"].is_object(),
                "Should contain _stream object"
            );
            assert_eq!(
                arr[0]["_stream"]["origin"],
                serde_json::Value::Null,
                "origin should be null when None"
            );
            assert_eq!(
                arr[0]["_stream"]["placeholder"], true,
                "placeholder flag should be true"
            );
        }
    }
}
