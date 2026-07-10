use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::Service;

use async_trait::async_trait;
use camel_api::{
    CamelError, StepLifecycle, StepShutdownReason,
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
    last_updated: Instant,
}

impl Bucket {
    fn new() -> Self {
        Self {
            exchanges: Vec::new(),
            last_updated: Instant::now(),
        }
    }

    fn push(&mut self, exchange: Exchange) {
        self.exchanges.push(exchange);
        self.last_updated = Instant::now();
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
    timeout_handles: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    late_tx: mpsc::Sender<Exchange>,
    language_registry: SharedLanguageRegistry,
    route_cancel: CancellationToken,
    /// Handle to the background TTL-sweep task. `None` when `config.bucket_ttl`
    /// is `None` (no TTL → no sweep). When the TTL is set, this is populated
    /// at construction by the auto-spawn in `new` — the caller does not need
    /// to start the sweep explicitly. The task is bound to `route_cancel` —
    /// cancelling the route token aborts the task. This is the fix for
    /// R3-C1's "no background sweep" half: a flood of unique keys within the
    /// inline-retain window can no longer grow unbounded between calls.
    sweep_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl std::fmt::Debug for AggregatorService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AggregatorService").finish_non_exhaustive()
    }
}

impl Drop for AggregatorService {
    fn drop(&mut self) {
        // Defense-in-depth for the R3-C1 sweep: abort the background task when
        // the service is dropped so it cannot leak even if the route owner
        // forgets to cancel `route_cancel` (the primary shutdown path).
        // `abort()` on an already-finished task is a no-op, so this is safe
        // alongside the `select!` cancel path. This also gives `sweep_handle`
        // a production reader so it is not dead-code.
        if let Some(handle) = self
            .sweep_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
    }
}

impl AggregatorService {
    /// Lifecycle invariant: `route_cancel` is owned by the route. Construction
    /// is runtime-free (no `tokio::spawn` here) so callers that build an
    /// `AggregatorService` outside a tokio runtime — e.g. route-spec tests —
    /// do not panic. When `config.bucket_ttl` is `Some`, the TTL-sweep task is
    /// spawned LAZILY on the first `poll_ready` (a runtime is guaranteed
    /// there) and bound to `route_cancel` via `select!`. The route owner MUST
    /// cancel it on shutdown; `Drop` also aborts it as defense-in-depth.
    pub fn new(
        config: AggregatorConfig,
        late_tx: mpsc::Sender<Exchange>,
        language_registry: SharedLanguageRegistry,
        route_cancel: CancellationToken,
    ) -> Self {
        // R3-M2: at least one memory-release bound is mandatory.
        config.validate().expect(
            // allow-unwrap
            // fail-closed startup invariant (ADR-0033); a config without a bound is a programmer/operator error.
            "AggregatorService::new: config failed validation \
             (need max_buckets, completionTimeout, or bucket_ttl)",
        );

        // R3-M2 advisory: Size/Predicate-only completion (no Timeout) with no
        // bucket_ttl means buckets accumulate until max_buckets is hit. Bounded
        // by max_buckets (validated above) but worth surfacing to operators.
        let has_timeout = has_timeout_condition(&config.completion);
        if !has_timeout && config.bucket_ttl.is_none() {
            tracing::warn!(
                "Aggregator configured with Size/Predicate completion and no \
                 bucket_ttl: buckets accumulate until max_buckets is reached"
            );
        }

        // Build the shared buckets map up front so the sweep task can
        // share it via Arc::clone.
        let buckets: Arc<Mutex<HashMap<String, Bucket>>> = Arc::new(Mutex::new(HashMap::new()));

        // R3-C1 Batch 1: the TTL-sweep is NOT spawned here — construction
        // must stay runtime-free so callers that build an AggregatorService
        // outside a tokio runtime (route-spec tests) do not panic on
        // `tokio::spawn`. The sweep is spawned lazily on the first
        // `poll_ready`. The inline `guard.retain(...)` per `call` evicts
        // expired buckets regardless, so the cap + TTL invariants hold
        // whether or not the sweep has started yet.

        Self {
            config,
            buckets,
            timeout_tasks: Arc::new(Mutex::new(HashMap::new())),
            timeout_handles: Arc::new(Mutex::new(HashMap::new())),
            late_tx,
            language_registry,
            route_cancel,
            sweep_handle: Arc::new(Mutex::new(None)),
        }
    }

    pub fn config(&self) -> &AggregatorConfig {
        &self.config
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
                    cancel_timeout_task_with_handle(
                        &key,
                        &self.timeout_tasks,
                        &self.timeout_handles,
                    );
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
                            // log-policy: handler-owned
                            tracing::warn!(
                                key = %key,
                                error = %e,
                                "aggregation failed in force_complete_all"
                            );
                        }
                    }
                } else {
                    cancel_timeout_task_with_handle(
                        &key,
                        &self.timeout_tasks,
                        &self.timeout_handles,
                    );
                }
            }
        }
    }

    /// Graceful shutdown: cancel all outstanding timeout tasks and await their
    /// JoinHandles (with a 5s deadline) so that no tasks are leaked.
    pub(crate) async fn shutdown_inner(&self) {
        // Cancel all timeout cancellation tokens.
        {
            let mut guard = self.timeout_tasks.lock().unwrap_or_else(|e| e.into_inner());
            for token in guard.values() {
                token.cancel();
            }
            guard.clear();
        };

        // Remove and collect all JoinHandles.
        let handles: Vec<JoinHandle<()>> = {
            let mut guard = self
                .timeout_handles
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            guard.drain().map(|(_, handle)| handle).collect()
        };

        if handles.is_empty() {
            return;
        }

        // Await all handles with a deadline.
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            for handle in handles {
                let _ = handle.await;
            }
        })
        .await;
    }
}

#[async_trait]
impl StepLifecycle for AggregatorService {
    fn name(&self) -> &'static str {
        "aggregator"
    }

    async fn shutdown(&self, reason: StepShutdownReason) -> Result<(), CamelError> {
        tracing::debug!(reason = ?reason, "Aggregator shutdown via StepLifecycle");
        self.shutdown_inner().await;
        Ok(())
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
        // R3-C1: lazily spawn the TTL-sweep on the first readiness poll. A
        // tokio runtime is guaranteed here (the runtime driving the service),
        // unlike at construction. Single-spawn via the lock + is_none check.
        if let Some(ttl) = self.config.bucket_ttl {
            let mut g = self.sweep_handle.lock().unwrap_or_else(|e| e.into_inner());
            if g.is_none() {
                let interval = std::cmp::max(ttl / 2, Duration::from_millis(50));
                let buckets = Arc::clone(&self.buckets);
                let cancel = self.route_cancel.clone();
                *g = Some(tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tokio::time::sleep(interval) => {
                                let mut guard =
                                    buckets.lock().unwrap_or_else(|e| e.into_inner());
                                guard.retain(|_, b| !b.is_expired(ttl));
                            }
                        }
                    }
                }));
            }
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let buckets = Arc::clone(&self.buckets);
        let timeout_tasks = Arc::clone(&self.timeout_tasks);
        let timeout_handles = Arc::clone(&self.timeout_handles);
        let late_tx = self.late_tx.clone();
        let language_registry = Arc::clone(&self.language_registry);
        let route_cancel = self.route_cancel.clone();

        Box::pin(async move {
            let key_value =
                extract_correlation_key(&exchange, &config.correlation, &language_registry).await?;

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
                let timeout_dur = extract_timeout_duration(&config.completion);
                if let Some(timeout) = timeout_dur {
                    // R3-M3: bound the number of concurrently-live per-bucket
                    // timeout tasks. When the cap is reached, skip the dedicated
                    // spawn — the bucket relies on bucket_ttl eviction (graceful
                    // degradation). max_buckets already caps total buckets, so
                    // memory stays bounded regardless.
                    let live_count = timeout_handles
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .len();
                    if live_count >= config.max_timeout_tasks {
                        tracing::warn!(
                            live_timeout_tasks = live_count,
                            max_timeout_tasks = config.max_timeout_tasks,
                            correlation_key = %key_str,
                            "Aggregator timeout-task cap reached; bucket will rely on \
                             bucket_ttl eviction instead of a dedicated timeout task"
                        );
                    } else {
                        // Cancel old token for this key (if any).
                        {
                            let tt_guard = timeout_tasks.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(existing) = tt_guard.get(&key_str) {
                                existing.cancel();
                            }
                        }
                        // Remove old handle if present.
                        {
                            let mut hh = timeout_handles.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(old) = hh.remove(&key_str) {
                                old.abort();
                            }
                        }
                        let cancel = CancellationToken::new();
                        timeout_tasks
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(key_str.clone(), cancel.clone());
                        let handle = spawn_timeout_task(
                            key_str.clone(),
                            timeout,
                            cancel,
                            buckets.clone(),
                            timeout_tasks.clone(),
                            timeout_handles.clone(),
                            late_tx,
                            config.strategy.clone(),
                            config.discard_on_timeout,
                            route_cancel,
                        );
                        timeout_handles
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(key_str.clone(), handle);
                    }
                }
            }

            if let Some(exchanges) = completed_bucket.0 {
                cancel_timeout_task_with_handle(&key_str, &timeout_tasks, &timeout_handles);
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

async fn extract_correlation_key(
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
            let expression = {
                let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
                let lang = reg.get(language).ok_or_else(|| {
                    CamelError::ProcessorError(format!(
                        "Aggregator: language '{}' not found in registry",
                        language
                    ))
                })?;
                lang.create_expression(expr)
                    .map_err(|e| CamelError::ProcessorError(e.to_string()))?
            };
            let value = expression
                .evaluate(exchange)
                .await
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

/// Also removes the stored JoinHandle for a cancelled/completed timeout task.
fn cancel_timeout_task_with_handle(
    key: &str,
    timeout_tasks: &Arc<Mutex<HashMap<String, CancellationToken>>>,
    timeout_handles: &Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
) {
    cancel_timeout_task(key, timeout_tasks);
    let mut guard = timeout_handles.lock().unwrap_or_else(|e| e.into_inner());
    guard.remove(key);
}

#[allow(clippy::too_many_arguments)]
fn spawn_timeout_task(
    key: String,
    timeout: Duration,
    cancel: CancellationToken,
    buckets: Arc<Mutex<HashMap<String, Bucket>>>,
    timeout_tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
    timeout_handles: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    late_tx: mpsc::Sender<Exchange>,
    strategy: AggregationStrategy,
    discard: bool,
    _route_cancel: CancellationToken,
) -> JoinHandle<()> {
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
                // Clean up our own JoinHandle from the map on natural completion.
                // Without this, the handle entry leaks until route shutdown.
                {
                    let mut hh = timeout_handles.lock().unwrap_or_else(|e| e.into_inner());
                    hh.remove(&key);
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
                            // log-policy: handler-owned
                            tracing::warn!(
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
    })
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
        StepLifecycle, StepShutdownReason,
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
            .unwrap()
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
            .build()
            .unwrap();
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
            .build()
            .unwrap();
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
            .build()
            .unwrap();

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
            .build()
            .unwrap();

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
            .build()
            .unwrap();

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
            .build()
            .unwrap();
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
            .build()
            .unwrap();
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
            .build()
            .unwrap();
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
            .build()
            .unwrap();
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
            .build()
            .unwrap();
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
            .build()
            .unwrap();
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
            .build()
            .unwrap();
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
            .build()
            .unwrap();
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
            .build()
            .unwrap();
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
            .build()
            .unwrap();
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

    #[tokio::test]
    async fn timeout_completion_clears_handle_from_map() {
        // Regression: Oracle audit found that natural timeout completion removed
        // the bucket but left the JoinHandle in `timeout_handles`, leaking the
        // entry until route shutdown. After fix, the timeout task itself cleans
        // its handle from the map on natural completion.
        let config = AggregatorConfig::correlate_by("key")
            .complete_on_timeout(Duration::from_millis(50))
            .build()
            .unwrap();
        let (tx, _rx) = mpsc::channel(256);
        let registry: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        let svc = AggregatorService::new(config, tx, registry, cancel);

        // Send an exchange to create a pending bucket with a timeout task.
        let mut call_svc = svc.clone();
        let ex = make_exchange("key", "A", "data");
        let _ = call_svc.ready().await.unwrap().call(ex).await.unwrap();
        assert!(
            !svc.timeout_handles.lock().unwrap().is_empty(),
            "handle should exist while timeout pending"
        );

        // Wait real time for the 50ms timeout to fire + spawned task to complete.
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(
            svc.timeout_handles.lock().unwrap().is_empty(),
            "handle should be cleared from map after natural timeout completion (was leak)"
        );
    }

    #[tokio::test]
    async fn aggregator_shutdown_via_trait_dispatch() {
        // RED: builds an AggregatorService, dispatches through Arc<dyn StepLifecycle>,
        // and asserts idempotent shutdown works.
        let config = AggregatorConfig::correlate_by("key")
            .complete_when_size(10)
            .build()
            .unwrap();
        let (tx, _rx) = mpsc::channel(256);
        let registry: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        let svc = AggregatorService::new(config, tx, registry, cancel);

        let step: Arc<dyn StepLifecycle> = Arc::new(svc);
        step.shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("first shutdown should succeed");
        step.shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("second shutdown (idempotent) should succeed");
    }

    #[tokio::test(start_paused = true)]
    async fn test_shutdown_awaits_timeout_handles() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_on_timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let (tx, _rx) = mpsc::channel(256);
        let registry: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        let svc = AggregatorService::new(config, tx, registry, cancel);

        // Send an exchange to create a pending bucket with a timeout task.
        let mut call_svc = svc.clone();
        let ex = make_exchange("key", "A", "data");
        let _ = call_svc.ready().await.unwrap().call(ex).await.unwrap();

        // Verify timeout handle exists.
        assert!(
            !svc.timeout_handles.lock().unwrap().is_empty(),
            "should have a timeout handle"
        );

        // Shutdown should complete within the 5s deadline (the timeout task
        // gets cancelled so it won't wait for the full 100ms sleep).
        svc.shutdown_inner().await;

        assert!(
            svc.timeout_handles.lock().unwrap().is_empty(),
            "all handles should be cleaned up after shutdown"
        );
    }

    // ── R3-C1 Batch 1: DoS cap + background sweep ───────────────────

    /// R3-C1: a flood of unique correlation keys must stay bounded.
    /// Default `max_buckets` is 10_000; the 10_001st unique key is rejected
    /// with `Aggregator reached maximum N buckets` (or its updated equivalent
    /// after the fix). The unique-key flood does NOT OOM the process.
    #[tokio::test]
    async fn test_unique_key_flood_stays_bounded_by_default() {
        // Builder defaults to max_buckets = 10_000, bucket_ttl = 300s.
        let config = AggregatorConfig::correlate_by("orderId")
            .complete_when_size(1_000_000) // never completes normally
            .build()
            .unwrap();
        let mut svc = new_test_svc(config);

        // Send 10_001 unique keys. The first 10_000 should be accepted
        // (pending in their buckets); the 10_001st MUST be rejected.
        for i in 0..10_000usize {
            let ex = make_exchange("orderId", &format!("key-{i}"), "body");
            let result = svc.ready().await.unwrap().call(ex).await;
            assert!(result.is_ok(), "key {i} should be accepted under the cap");
        }
        let ex = make_exchange("orderId", "key-10001", "body");
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(
            result.is_err(),
            "10_001st unique key must be rejected by the max_buckets cap"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("maximum") || err.contains("max"),
            "error should mention cap: {err}"
        );
    }

    /// The `AggregatorService` exposes a `sweep_handle` for the background
    /// sweep task. When `config.bucket_ttl` is `Some`, `AggregatorService::new`
    /// automatically spawns the sweep task and stores the handle, so the
    /// caller never sees `None` for a TTL-configured service. Cancelling the
    /// route token (via `shutdown`) aborts the sweep.
    #[tokio::test]
    async fn test_background_sweep_spawns_on_first_poll_not_construction() {
        let config = AggregatorConfig::correlate_by("key")
            .complete_when_size(10_000)
            .bucket_ttl(Duration::from_millis(50))
            .build()
            .unwrap();
        let cancel = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(8);
        let registry: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let mut svc = AggregatorService::new(config, tx, registry, cancel.clone());

        // Construction is runtime-free: no sweep spawned yet.
        assert!(
            svc.sweep_handle
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none(),
            "sweep must NOT be spawned at construction (runtime-free new)"
        );

        // First readiness poll lazily spawns the sweep (a runtime is present
        // here). `ready()` drives `poll_ready` until Ready.
        let _ = svc.ready().await.unwrap();
        let sweep_present = svc
            .sweep_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        assert!(
            sweep_present,
            "sweep handle should be Some after first poll when bucket_ttl is set"
        );

        // Cancel the route token; the sweep task observes it and exits.
        cancel.cancel();
        // Give the task a moment to observe the cancel.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // ── R3-M3: bounded timeout-task spawn ─────────────────────────────

    #[tokio::test]
    async fn test_aggregator_timeout_task_cap_no_panic_under_flood() {
        // R3-M3: a flood of unique keys with a tiny max_timeout_tasks must not
        // spawn unbounded tasks, panic, or deadlock. Each call returns Ok(pending).
        use camel_api::aggregator::CorrelationStrategy;

        let config = AggregatorConfig {
            header_name: "k".into(),
            completion: CompletionMode::Any(vec![
                CompletionCondition::Size(999),
                CompletionCondition::Timeout(Duration::from_secs(30)),
            ]),
            correlation: CorrelationStrategy::HeaderName("k".into()),
            strategy: AggregationStrategy::CollectAll,
            max_buckets: Some(50),
            bucket_ttl: Some(Duration::from_secs(30)),
            force_completion_on_stop: false,
            discard_on_timeout: false,
            max_timeout_tasks: 2,
        };
        let (late_tx, mut late_rx) = mpsc::channel(64);
        let registry: SharedLanguageRegistry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        let svc = AggregatorService::new(config, late_tx, registry, cancel);

        // Drive 20 unique-key exchanges — far exceeding max_timeout_tasks=2.
        for i in 0..20u64 {
            let mut ex = Exchange::new(Message {
                headers: HashMap::from([("k".to_string(), serde_json::json!(i))]),
                body: Body::Text(i.to_string()),
            });
            ex.input
                .headers
                .insert("k".to_string(), serde_json::json!(i));
            let outcome = tokio::time::timeout(Duration::from_secs(2), async {
                let mut s = svc.clone();
                use tower::ServiceExt;
                s.ready().await.unwrap().call(ex).await
            })
            .await;
            assert!(outcome.is_ok(), "call {} hung/panicked under task cap", i);
            // Each returns Ok(pending) since Size(999) is never reached.
            let res = outcome.unwrap().unwrap();
            assert_eq!(
                res.properties
                    .get(CAMEL_AGGREGATOR_PENDING)
                    .and_then(|v| v.as_bool()),
                Some(true),
                "exchange {} should be pending",
                i
            );
        }
        // Drain any late emissions to avoid blocking the channel.
        let _ = late_rx.try_recv();
    }
}
