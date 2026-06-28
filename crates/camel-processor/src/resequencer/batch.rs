//! Batch resequencing policy — buffer per correlation key, window completion,
//! sort by expression, burst-emit in order.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::exchange::Exchange;
use camel_api::resequencer::BatchCompletion;
use camel_api::value::cmp_values;
use camel_language_api::Expression;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::ResequencePolicy;

/// Per-correlation-key bucket holding pending exchanges.
#[derive(Default)]
struct Bucket {
    exchanges: Vec<Exchange>,
}

/// Batch resequencing policy.
///
/// Buffers exchanges per correlation key. Completion is triggered by
/// window (size and/or timeout). On completion, sorts buffered exchanges
/// by `sort_expr` and returns them as a burst. Timeout tasks hold a
/// `Weak<Self>` reference obtained via `Arc::new_cyclic`.
pub struct BatchPolicy {
    correlation_expr: Arc<dyn Expression>,
    sort_expr: Arc<dyn Expression>,
    completion: BatchCompletion,

    /// Weak self-reference so timeout tasks can upgrade to `Arc<Self>`.
    weak_self: Weak<Self>,

    /// Per-correlation-key buckets (exchanges pending completion).
    buckets: Mutex<HashMap<String, Bucket>>,

    /// Timeout cancellation tokens, keyed by correlation key.
    timeout_tokens: Mutex<HashMap<String, CancellationToken>>,

    /// Timeout task handles, keyed by correlation key.
    timeout_handles: Mutex<HashMap<String, JoinHandle<()>>>,

    /// Channel to the post-driver for timeout-triggered emissions.
    /// Set by `ResequencerService` after channel creation.
    driver_tx: Mutex<Option<mpsc::Sender<Exchange>>>,

    /// Shutdown guard — timeout tasks check this before sending
    /// to avoid racing with post-driver channel close (M7).
    shutdown_started: AtomicBool,
}

impl BatchPolicy {
    /// Create a new `Arc<BatchPolicy>` using `Arc::new_cyclic` so the
    /// policy holds a `Weak<Self>` for timeout task spawning.
    pub fn new_cyclic(
        correlation_expr: Arc<dyn Expression>,
        sort_expr: Arc<dyn Expression>,
        completion: BatchCompletion,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            correlation_expr,
            sort_expr,
            completion,
            weak_self: weak.clone(),
            buckets: Mutex::new(HashMap::new()),
            timeout_tokens: Mutex::new(HashMap::new()),
            timeout_handles: Mutex::new(HashMap::new()),
            driver_tx: Mutex::new(None),
            shutdown_started: AtomicBool::new(false),
        })
    }

    /// Set the driver channel (via `set_timeout_tx` trait method).
    /// Called by `ResequencerService` after channel creation.
    fn set_driver_tx(&self, tx: mpsc::Sender<Exchange>) {
        let mut guard = self.driver_tx.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(tx);
    }

    /// Evaluate the correlation expression against an exchange.
    async fn eval_key(&self, exchange: &Exchange) -> Result<String, String> {
        self.correlation_expr
            .evaluate(exchange)
            .await
            // M4: avoid double-quoting for string values — use as_str() for
            // strings, fall back to to_string() for other types.
            .map(|v| match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            })
            .map_err(|e| format!("correlation expression evaluation failed: {e}"))
    }

    /// Drain a bucket, sort by sort_expr, return sorted Vec.
    async fn drain_and_sort(&self, mut bucket: Bucket) -> Vec<Exchange> {
        let mut indexed: Vec<(serde_json::Value, Exchange)> = Vec::new();
        for ex in bucket.exchanges.drain(..) {
            let val = self
                .sort_expr
                .evaluate(&ex)
                .await
                .unwrap_or(serde_json::Value::Null);
            indexed.push((val, ex));
        }
        indexed.sort_by(|a, b| cmp_values(&a.0, &b.0));
        indexed.into_iter().map(|(_, ex)| ex).collect()
    }

    /// Check if a bucket count satisfies the size-based completion condition.
    fn is_complete_by_size(&self, count: usize) -> bool {
        match self.completion {
            BatchCompletion::Size(s) => count >= s,
            BatchCompletion::Timeout(_) => false,
            BatchCompletion::SizeOrTimeout(s, _) => count >= s,
        }
    }

    /// Whether this completion variant needs timeout tasks spawned.
    fn needs_timeout(&self) -> bool {
        matches!(
            self.completion,
            BatchCompletion::Timeout(_) | BatchCompletion::SizeOrTimeout(..)
        )
    }

    /// Take a bucket by key. Returns `Some(Bucket)` if it existed.
    fn take_bucket(&self, key: &str) -> Option<Bucket> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        buckets.remove(key)
    }

    /// Cancel and remove timeout task for a key.
    fn cancel_timeout(&self, key: &str) {
        {
            let mut tokens = self
                .timeout_tokens
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(token) = tokens.remove(key) {
                token.cancel();
            }
        }
        {
            let mut handles = self
                .timeout_handles
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            handles.remove(key);
        }
    }

    /// Spawn a timeout task for the given key.
    /// Must be called from a method that has access to `&self` (which has the `weak_self`).
    fn spawn_timeout_task(&self, key: String, timeout_ms: u64) {
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        // Store the cancellation token
        {
            let mut tokens = self
                .timeout_tokens
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            tokens.insert(key.clone(), cancel);
        }

        let weak = self.weak_self.clone();
        let key_clone = key.clone();
        let driver_tx_opt = {
            let guard = self.driver_tx.lock().unwrap_or_else(|e| e.into_inner());
            guard.clone()
        };

        let handle = tokio::spawn(async move {
            let timeout = Duration::from_millis(timeout_ms);

            tokio::select! {
                _ = tokio::time::sleep(timeout) => {
                    if cancel_clone.is_cancelled() {
                        return;
                    }
                }
                _ = cancel_clone.cancelled() => {
                    return;
                }
            }

            // Upgrade the weak reference — policy may have been dropped (shutdown)
            let Some(policy) = weak.upgrade() else {
                return;
            };

            // M7: don't send if shutdown has started (driver channel may already be closed)
            if policy.shutdown_started.load(Ordering::SeqCst) {
                return;
            }

            // Drain the bucket
            let bucket = policy.take_bucket(&key_clone);
            let Some(bucket) = bucket else {
                return; // bucket already drained by size-based completion
            };

            let sorted = policy.drain_and_sort(bucket).await;

            // Send via driver channel
            if let Some(tx) = driver_tx_opt {
                for ex in sorted {
                    if tx.send(ex).await.is_err() {
                        tracing::debug!(
                            key = %key_clone,
                            "BatchPolicy timeout: driver channel closed during emission"
                        );
                        break;
                    }
                }
            }

            // Clean up handle entry
            {
                let mut handles = policy
                    .timeout_handles
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                handles.remove(&key_clone);
            }
        });

        {
            let mut handles = self
                .timeout_handles
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            handles.insert(key, handle);
        }
    }
}

#[async_trait]
impl ResequencePolicy for BatchPolicy {
    async fn accept(&self, input: Exchange) -> Vec<Exchange> {
        let correlation_id = input.correlation_id().to_owned();
        let key = match self.eval_key(&input).await {
            Ok(k) => k,
            Err(e) => {
                // log-policy: handler-owned
                tracing::warn!(
                    error = %e,
                    correlation_id = %correlation_id,
                    "BatchPolicy: correlation expression failed, dropping exchange"
                );
                return vec![];
            }
        };

        let bucket_count = {
            let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
            let bucket = buckets.entry(key.clone()).or_default();
            bucket.exchanges.push(input);
            bucket.exchanges.len()
        };

        // Spawn timeout task if needed (first exchange for this key)
        if bucket_count == 1 && self.needs_timeout() {
            let timeout_ms = match self.completion {
                BatchCompletion::Timeout(t) | BatchCompletion::SizeOrTimeout(_, t) => t,
                _ => unreachable!(),
            };
            self.spawn_timeout_task(key.clone(), timeout_ms);
        }

        // Check if the bucket is complete (size-based)
        if self.is_complete_by_size(bucket_count) {
            self.cancel_timeout(&key);
            if let Some(bucket) = self.take_bucket(&key) {
                return self.drain_and_sort(bucket).await;
            }
        }

        vec![]
    }

    async fn flush(&self) -> Vec<Exchange> {
        // M7: signal timeout tasks that shutdown is in progress
        self.shutdown_started.store(true, Ordering::SeqCst);

        let all_keys: Vec<String> = {
            let buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
            buckets.keys().cloned().collect()
        };

        let mut all_sorted = Vec::new();
        for key in &all_keys {
            self.cancel_timeout(key);
            if let Some(bucket) = self.take_bucket(key) {
                let sorted = self.drain_and_sort(bucket).await;
                all_sorted.extend(sorted);
            }
        }

        // Cancel all remaining timeout tasks
        {
            let tokens: HashMap<String, CancellationToken> = {
                let mut guard = self
                    .timeout_tokens
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut *guard)
            };
            for (_, token) in tokens {
                token.cancel();
            }
        }
        // Drop handles — tasks wind down when cancelled
        {
            let _handles = {
                let mut guard = self
                    .timeout_handles
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut *guard)
            };
        }

        all_sorted
    }

    fn name(&self) -> &'static str {
        "batch-resequencer"
    }

    fn set_timeout_tx(&self, tx: tokio::sync::mpsc::Sender<Exchange>) {
        self.set_driver_tx(tx);
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::exchange::ExchangePattern;
    use camel_api::message::Message;

    /// Mock expression that reads a property by name.
    struct PropExpr(String);

    #[async_trait::async_trait]
    impl Expression for PropExpr {
        async fn evaluate(
            &self,
            exchange: &Exchange,
        ) -> Result<serde_json::Value, camel_language_api::LanguageError> {
            Ok(exchange
                .property(&self.0)
                .cloned()
                .unwrap_or(serde_json::Value::Null))
        }
    }

    /// Mock expression that always returns the same string.
    struct ConstExpr(String);

    #[async_trait::async_trait]
    impl Expression for ConstExpr {
        async fn evaluate(
            &self,
            _exchange: &Exchange,
        ) -> Result<serde_json::Value, camel_language_api::LanguageError> {
            Ok(serde_json::Value::String(self.0.clone()))
        }
    }

    /// Mock expression that always fails.
    struct FailingExpr;

    #[async_trait::async_trait]
    impl Expression for FailingExpr {
        async fn evaluate(
            &self,
            _exchange: &Exchange,
        ) -> Result<serde_json::Value, camel_language_api::LanguageError> {
            Err(camel_language_api::LanguageError::EvalError(
                "mock eval failure".into(),
            ))
        }
    }

    fn mk_exchange(seq: i64) -> Exchange {
        let mut ex = Exchange::new(Message::new(camel_api::body::Body::Text(format!(
            "msg-{seq}"
        ))));
        ex.set_property("seq", serde_json::json!(seq));
        ex.pattern = ExchangePattern::InOnly;
        ex
    }

    fn mk_exchange_with_key(seq: i64, key_prop: &str, key_val: &str) -> Exchange {
        let mut ex = Exchange::new(Message::new(camel_api::body::Body::Text(format!(
            "msg-{seq}"
        ))));
        ex.set_property("seq", serde_json::json!(seq));
        ex.set_property(key_prop, serde_json::Value::String(key_val.to_string()));
        ex.pattern = ExchangePattern::InOnly;
        ex
    }

    /// C1.1: 3 exchanges with seq [3,1,2], same correlation key, window size 3 →
    /// on 3rd input accept() returns [1,2,3] sorted by seq.
    #[tokio::test]
    async fn batch_size_completion_emits_sorted_burst() {
        let policy = BatchPolicy::new_cyclic(
            Arc::new(ConstExpr("same".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Size(3),
        );

        assert!(policy.accept(mk_exchange(3)).await.is_empty());
        assert!(policy.accept(mk_exchange(1)).await.is_empty());

        let emitted = policy.accept(mk_exchange(2)).await;
        assert_eq!(emitted.len(), 3, "should emit all 3 on completion");
        let seqs: Vec<i64> = emitted
            .iter()
            .map(|ex| ex.property("seq").and_then(|v| v.as_i64()).unwrap_or(-1))
            .collect();
        assert_eq!(seqs, vec![1, 2, 3], "should be sorted ascending");
    }

    /// C1.2: 2 exchanges, timeout window (no size reached) →
    /// after timeout fires, emit sorted buffered.
    #[tokio::test]
    async fn batch_timeout_completion_emits_after_timeout() {
        let policy = BatchPolicy::new_cyclic(
            Arc::new(ConstExpr("same".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Timeout(50),
        );

        let (tx, mut rx) = mpsc::channel::<Exchange>(16);
        policy.set_driver_tx(tx);

        assert!(policy.accept(mk_exchange(3)).await.is_empty());
        assert!(policy.accept(mk_exchange(1)).await.is_empty());

        let emitted: Vec<Exchange> = tokio::time::timeout(Duration::from_millis(500), async {
            let mut out = Vec::new();
            out.push(rx.recv().await.unwrap());
            out.push(rx.recv().await.unwrap());
            out
        })
        .await
        .expect("timeout should fire within 500ms");

        assert_eq!(emitted.len(), 2);
        let seqs: Vec<i64> = emitted
            .iter()
            .map(|ex| ex.property("seq").and_then(|v| v.as_i64()).unwrap_or(-1))
            .collect();
        assert_eq!(seqs, vec![1, 3], "should be sorted ascending");
    }

    /// C1.3: SizeOrTimeout(3, 5000ms); send 3 → size wins before timeout.
    #[tokio::test]
    async fn batch_size_or_timeout_size_wins() {
        let policy = BatchPolicy::new_cyclic(
            Arc::new(ConstExpr("same".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::SizeOrTimeout(3, 5_000),
        );

        assert!(policy.accept(mk_exchange(2)).await.is_empty());
        assert!(policy.accept(mk_exchange(1)).await.is_empty());

        let emitted = policy.accept(mk_exchange(3)).await;
        assert_eq!(emitted.len(), 3);
        let seqs: Vec<i64> = emitted
            .iter()
            .map(|ex| ex.property("seq").and_then(|v| v.as_i64()).unwrap_or(-1))
            .collect();
        assert_eq!(seqs, vec![1, 2, 3]);
    }

    /// C1.4: Exchanges with different correlation keys buffer independently.
    #[tokio::test]
    async fn batch_multi_key_independence() {
        let policy = BatchPolicy::new_cyclic(
            Arc::new(PropExpr("region".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Size(2),
        );

        let _ = policy
            .accept(mk_exchange_with_key(2, "region", "east"))
            .await;
        let east_emit = policy
            .accept(mk_exchange_with_key(1, "region", "east"))
            .await;
        assert_eq!(east_emit.len(), 2, "east bucket should complete at size 2");

        let west_result = policy
            .accept(mk_exchange_with_key(3, "region", "west"))
            .await;
        assert!(
            west_result.is_empty(),
            "west bucket should NOT complete yet"
        );
    }

    /// C1.5: flush() emits remaining buffered exchanges (within-key sorted).
    /// With a single correlation key, all remain and are sorted together.
    #[tokio::test]
    async fn batch_flush_emits_remaining_sorted() {
        let policy = BatchPolicy::new_cyclic(
            Arc::new(ConstExpr("same".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Size(10),
        );

        assert!(policy.accept(mk_exchange(5)).await.is_empty());
        assert!(policy.accept(mk_exchange(3)).await.is_empty());
        assert!(policy.accept(mk_exchange(1)).await.is_empty());

        let flushed = policy.flush().await;
        assert_eq!(flushed.len(), 3);
        let seqs: Vec<i64> = flushed
            .iter()
            .map(|ex| ex.property("seq").and_then(|v| v.as_i64()).unwrap_or(-1))
            .collect();
        assert_eq!(seqs, vec![1, 3, 5]);
    }

    /// C1.6: Exchange where correlation expression fails → accept()
    /// returns empty vec (no crash).
    #[tokio::test]
    async fn batch_correlation_eval_failure_returns_empty() {
        let policy = BatchPolicy::new_cyclic(
            Arc::new(FailingExpr),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Size(2),
        );

        let result = policy.accept(mk_exchange(1)).await;
        assert!(
            result.is_empty(),
            "failed correlation should return empty vec, not crash"
        );
    }

    /// Verify pure Size completion does not need timeout tasks.
    #[tokio::test]
    async fn batch_pure_size_no_timeout_needed() {
        let policy = BatchPolicy::new_cyclic(
            Arc::new(ConstExpr("same".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Size(2),
        );

        assert!(!policy.needs_timeout());
    }
}
