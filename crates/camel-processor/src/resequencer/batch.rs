//! Batch resequencing policy — buffer per correlation key, window completion,
//! sort by expression, burst-emit in order.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::exchange::Exchange;
use camel_api::resequencer::BatchCompletion;
use camel_api::value::cmp_values;
use camel_language_api::Expression;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::ResequencePolicy;

/// Default upper bound on simultaneously open correlation buckets
/// (audit 2026-08-31, F6-1). Mirrors the aggregator's `max_buckets` default
/// (`camel-api` aggregator.rs:209). New keys beyond the cap are dropped.
pub const DEFAULT_MAX_BUCKETS: usize = 10_000;

/// Default upper bound on buffered exchanges inside ONE bucket (F6-1). A hot
/// correlation key can otherwise buffer unboundedly until completion fires.
/// Exchanges beyond the cap are dropped (fail-visible: warn + ack drop, same
/// class as the resequencer's post-ack drop semantics in ADR-0029).
pub const DEFAULT_MAX_BUCKET_SIZE: usize = 1_000;

/// Default upper bound on live timeout tasks (F6-1). Mirrors the aggregator's
/// `max_timeout_tasks` default (`camel-api` aggregator.rs:214). When the cap is
/// reached, new buckets still buffer but get no per-key timer — their
/// completion then relies on size or shutdown flush.
pub const DEFAULT_MAX_TIMEOUT_TASKS: usize = 1024;

/// Per-correlation-key bucket holding pending exchanges.
#[derive(Default)]
struct Bucket {
    exchanges: Vec<Exchange>,
}

/// One live timeout task: the cancellation token plus the spawn generation
/// that owns it. The generation makes cleanup compare-and-remove safe under
/// key reuse (a stale task's removal is a no-op once superseded).
struct TimeoutEntry {
    generation: u64,
    cancel: CancellationToken,
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

    /// Live timeout tasks, keyed by correlation key. Token and handle share
    /// one entry so they share one lifecycle: removing the entry retires
    /// both (re-review of F6-1: the original two-map layout leaked the
    /// token entry on natural timeout completion).
    timeout_tasks: Mutex<HashMap<String, TimeoutEntry>>,

    /// Monotonic spawn generation. Guards timeout ownership: a stale task
    /// (superseded by key reuse after size-based completion) can neither
    /// drain the newer bucket nor remove the newer task's entry.
    timeout_generation: AtomicU64,

    /// Test-only synchronization point INSIDE
    /// `take_bucket_if_current_timeout_task`, between the generation check
    /// and the bucket take — i.e. while the `timeout_tasks` lock is still
    /// held. Lets the regression test force the interleaving that a
    /// separate-check-then-take implementation would permit (re-review 2
    /// of F6-1: proving the test bites).
    #[cfg(test)]
    interleave_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,

    /// Channel to the post-driver for timeout-triggered emissions.
    /// Set by `ResequencerService` after channel creation.
    driver_tx: Mutex<Option<mpsc::Sender<Exchange>>>,

    /// Shutdown guard — timeout tasks check this before sending
    /// to avoid racing with post-driver channel close (M7).
    shutdown_started: AtomicBool,

    /// Upper bound on simultaneously open buckets (F6-1).
    max_buckets: usize,

    /// Upper bound on buffered exchanges inside one bucket (F6-1).
    max_bucket_size: usize,

    /// Upper bound on live timeout tasks (F6-1).
    max_timeout_tasks: usize,
}

impl BatchPolicy {
    /// Create a new `Arc<BatchPolicy>` using `Arc::new_cyclic` so the
    /// policy holds a `Weak<Self>` for timeout task spawning.
    pub fn new_cyclic(
        correlation_expr: Arc<dyn Expression>,
        sort_expr: Arc<dyn Expression>,
        completion: BatchCompletion,
    ) -> Arc<Self> {
        Self::with_limits(
            correlation_expr,
            sort_expr,
            completion,
            DEFAULT_MAX_BUCKETS,
            DEFAULT_MAX_BUCKET_SIZE,
            DEFAULT_MAX_TIMEOUT_TASKS,
        )
    }

    /// Create a `BatchPolicy` with explicit resource bounds (F6-1).
    /// `new_cyclic` delegates here with the `DEFAULT_*` constants.
    pub fn with_limits(
        correlation_expr: Arc<dyn Expression>,
        sort_expr: Arc<dyn Expression>,
        completion: BatchCompletion,
        max_buckets: usize,
        max_bucket_size: usize,
        max_timeout_tasks: usize,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            correlation_expr,
            sort_expr,
            completion,
            weak_self: weak.clone(),
            buckets: Mutex::new(HashMap::new()),
            timeout_tasks: Mutex::new(HashMap::new()),
            timeout_generation: AtomicU64::new(0),
            #[cfg(test)]
            interleave_hook: Mutex::new(None),
            driver_tx: Mutex::new(None),
            shutdown_started: AtomicBool::new(false),
            max_buckets,
            max_bucket_size,
            max_timeout_tasks,
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
            BatchCompletion::SizeOrTimeout(s, _) => count >= s,
            // Timeout and any future variant are not size-complete.
            _ => false,
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

    /// Cancel and remove the timeout task for `key` (size-based completion,
    /// flush). Removing the whole entry first makes a concurrently waking
    /// stale task a no-op via the generation guard.
    fn cancel_timeout(&self, key: &str) {
        if let Some(entry) = self
            .timeout_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key)
        {
            entry.cancel.cancel();
        }
    }

    /// Atomically verify this task still owns the timeout entry for `key`
    /// AND take the bucket — ONE critical section holding `timeout_tasks`
    /// across the bucket removal (re-review 2 of F6-1). Closing the gap
    /// between a separate generation check and a separate `take_bucket`
    /// prevents the reuse race where a stale task passes the guard, gets
    /// interleaved by size-completion + key reuse, and then drains the NEW
    /// generation's bucket. Lock order: `timeout_tasks` → `buckets`.
    fn take_bucket_if_current_timeout_task(&self, key: &str, generation: u64) -> Option<Bucket> {
        let tasks = self.timeout_tasks.lock().unwrap_or_else(|e| e.into_inner());
        if !tasks
            .get(key)
            .is_some_and(|entry| entry.generation == generation)
        {
            return None;
        }
        // Test-only interleaving point: runs while `timeout_tasks` is held.
        // A correct (combined) implementation serializes any concurrent
        // supersede behind this lock (the hook's bounded wait elapses); a
        // separate check-then-take implementation lets the supersede
        // complete inside the gap (the hook's wait succeeds).
        #[cfg(test)]
        if let Some(hook) = self
            .interleave_hook
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            hook();
        }
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        buckets.remove(key)
    }

    /// Remove the timeout entry for `key` iff it still belongs to `generation`.
    fn remove_timeout_task_if_current(&self, key: &str, generation: u64) {
        let mut tasks = self.timeout_tasks.lock().unwrap_or_else(|e| e.into_inner());
        if tasks
            .get(key)
            .is_some_and(|entry| entry.generation == generation)
        {
            tasks.remove(key);
        }
    }

    /// Spawn a timeout task for the given key.
    /// Must be called from a method that has access to `&self` (which has the `weak_self`).
    ///
    /// Generation-safety (re-review of F6-1): each spawn takes a fresh
    /// monotonic generation. The task drains and cleans up only through
    /// generation-checked combined operations, so a stale task (superseded
    /// by key reuse after size-based completion) can neither steal the
    /// newer bucket nor delete the newer task's entry. The entry (token +
    /// generation) is inserted BEFORE spawning so the map never holds a
    /// half-observed task; the task itself is detached (cancellation winds
    /// it down, flush drops all entries).
    fn spawn_timeout_task(&self, key: String, timeout_ms: u64) {
        let generation = self.timeout_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        // Store the entry before the task exists.
        {
            let mut tasks = self.timeout_tasks.lock().unwrap_or_else(|e| e.into_inner());
            tasks.insert(key.clone(), TimeoutEntry { generation, cancel });
        }

        let weak = self.weak_self.clone();
        let key_clone = key.clone();
        let driver_tx_opt = {
            let guard = self.driver_tx.lock().unwrap_or_else(|e| e.into_inner());
            guard.clone()
        };

        tokio::spawn(async move {
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

            // Atomically verify ownership AND take the bucket in one
            // critical section. A stale task (superseded by key reuse after
            // size-based completion) gets None here and can neither drain
            // the newer bucket nor remove the newer entry.
            let bucket = policy.take_bucket_if_current_timeout_task(&key_clone, generation);
            let Some(bucket) = bucket else {
                // Bucket already drained by size-based completion (or a
                // newer task owns the key) — clean up our own entry only if
                // it is still ours (compare-and-remove; no-op when
                // superseded). The original two-map layout leaked the token
                // here: re-review of F6-1.
                policy.remove_timeout_task_if_current(&key_clone, generation);
                return;
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

            // Clean up our entry — only if a newer task has not superseded us.
            policy.remove_timeout_task_if_current(&key_clone, generation);
        });
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
            // F6-1 bucket-count cap: refuse to open a NEW bucket past the cap.
            // Existing buckets keep accepting (bounded per-bucket below).
            if !buckets.contains_key(&key) && buckets.len() >= self.max_buckets {
                // log-policy: handler-owned
                tracing::warn!(
                    correlation_id = %correlation_id,
                    max_buckets = self.max_buckets,
                    "BatchPolicy: bucket cap reached, dropping exchange"
                );
                return vec![];
            }
            let bucket = buckets.entry(key.clone()).or_default();
            // F6-1 per-bucket cap: a hot key cannot buffer unboundedly.
            if bucket.exchanges.len() >= self.max_bucket_size {
                // log-policy: handler-owned
                tracing::warn!(
                    correlation_id = %correlation_id,
                    max_bucket_size = self.max_bucket_size,
                    "BatchPolicy: per-bucket cap reached, dropping exchange"
                );
                return vec![];
            }
            bucket.exchanges.push(input);
            bucket.exchanges.len()
        };

        // Spawn timeout task if needed (first exchange for this key), subject
        // to the F6-1 timeout-task cap: past the cap the bucket still buffers
        // and completes on size or shutdown flush (mirrors the aggregator's
        // graceful degradation to TTL-only eviction).
        if bucket_count == 1 && self.needs_timeout() {
            let live_tasks = {
                let tasks = self.timeout_tasks.lock().unwrap_or_else(|e| e.into_inner());
                tasks.len()
            };
            if live_tasks < self.max_timeout_tasks {
                let timeout_ms = match self.completion {
                    BatchCompletion::Timeout(t) | BatchCompletion::SizeOrTimeout(_, t) => t,
                    _ => unreachable!(),
                };
                self.spawn_timeout_task(key.clone(), timeout_ms);
            } else {
                // log-policy: handler-owned
                tracing::warn!(
                    correlation_id = %correlation_id,
                    max_timeout_tasks = self.max_timeout_tasks,
                    "BatchPolicy: timeout-task cap reached; bucket relies on size/flush completion"
                );
            }
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

        // Cancel all remaining timeout tasks (dropping the entries detaches
        // the tasks; they wind down once cancelled)
        {
            let tasks: HashMap<String, TimeoutEntry> = {
                let mut guard = self.timeout_tasks.lock().unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut *guard)
            };
            for (_, entry) in tasks {
                entry.cancel.cancel();
            }
        }

        all_sorted
    }

    fn name(&self) -> &'static str {
        "batch-resequencer"
    }

    fn buffered(&self) -> usize {
        let buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        buckets.values().map(|b| b.exchanges.len()).sum()
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

    // -----------------------------------------------------------------------
    // F6-1 resource-bound tests (audit 2026-08-31)
    // -----------------------------------------------------------------------

    /// Bucket-count cap: N unique keys fill the map; key N+1 is dropped.
    #[tokio::test]
    async fn batch_bucket_count_cap_drops_new_keys() {
        let policy = BatchPolicy::with_limits(
            Arc::new(PropExpr("key".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Size(100), // never completes by size in this test
            4,                          // max_buckets
            1000,                       // max_bucket_size
            16,                         // max_timeout_tasks
        );

        for i in 0..4 {
            let mut ex = mk_exchange(i);
            ex.set_property("key", serde_json::json!(format!("k{i}")));
            assert!(policy.accept(ex).await.is_empty(), "buffered, not emitted");
        }
        assert_eq!(policy.buckets.lock().unwrap().len(), 4);

        // Fifth unique key must be dropped (cap reached).
        let mut ex = mk_exchange(99);
        ex.set_property("key", serde_json::json!("k-overflow"));
        assert!(policy.accept(ex).await.is_empty());
        assert_eq!(
            policy.buckets.lock().unwrap().len(),
            4,
            "no new bucket past the cap"
        );

        // Existing keys still accept (bounded per-bucket).
        let mut ex = mk_exchange(100);
        ex.set_property("key", serde_json::json!("k0"));
        assert!(policy.accept(ex).await.is_empty());
        assert_eq!(
            policy.buckets.lock().unwrap()["k0"].exchanges.len(),
            2,
            "existing bucket keeps accepting"
        );
    }

    /// Per-bucket cap: one hot key cannot buffer more than max_bucket_size.
    #[tokio::test]
    async fn batch_per_bucket_cap_drops_overflow() {
        let policy = BatchPolicy::with_limits(
            Arc::new(ConstExpr("hot".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Size(1_000_000), // effectively never
            10,                               // max_buckets
            3,                                // max_bucket_size
            16,                               // max_timeout_tasks
        );

        for i in 0..3 {
            assert!(policy.accept(mk_exchange(i)).await.is_empty());
        }
        assert_eq!(policy.buffered(), 3);

        // 4th and 5th on the same key are dropped.
        assert!(policy.accept(mk_exchange(3)).await.is_empty());
        assert!(policy.accept(mk_exchange(4)).await.is_empty());
        assert_eq!(
            policy.buffered(),
            3,
            "bucket must not grow past max_bucket_size"
        );
    }

    /// Timeout-task cap: past the cap, no new task is spawned (bucket relies on
    /// size/flush). Uses Timeout completion so every new key wants a task.
    #[tokio::test]
    async fn batch_timeout_task_cap_stops_spawning() {
        let policy = BatchPolicy::with_limits(
            Arc::new(PropExpr("key".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Timeout(60_000), // long; we flush before it fires
            16,                               // max_buckets
            100,                              // max_bucket_size
            2,                                // max_timeout_tasks
        );

        for i in 0..4 {
            let mut ex = mk_exchange(i);
            ex.set_property("key", serde_json::json!(format!("k{i}")));
            assert!(policy.accept(ex).await.is_empty());
        }

        assert_eq!(
            policy.timeout_tasks.lock().unwrap().len(),
            2,
            "no more than max_timeout_tasks tasks spawned"
        );
        assert_eq!(
            policy.buckets.lock().unwrap().len(),
            4,
            "buckets still buffered even without their own timer"
        );

        // Flush completes everything (no hang, no loss of buffered exchanges).
        let flushed = policy.flush().await;
        assert_eq!(flushed.len(), 4);
    }

    /// Re-review of F6-1 (token-entry leak): sequential unique keys that all
    /// complete by natural timeout must not leave entries behind — the
    /// timeout map returns to empty after each task fires.
    #[tokio::test]
    async fn batch_sequential_unique_keys_do_not_leak_timeout_entries() {
        let policy = BatchPolicy::with_limits(
            Arc::new(PropExpr("key".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Timeout(30),
            100, // max_buckets
            100, // max_bucket_size
            16,  // max_timeout_tasks
        );

        for i in 0..8 {
            let mut ex = mk_exchange(i);
            ex.set_property("key", serde_json::json!(format!("k{i}")));
            assert!(policy.accept(ex).await.is_empty());
            // Let the timeout fire and the task finish its cleanup.
            tokio::time::sleep(Duration::from_millis(80)).await;
            assert_eq!(
                policy.timeout_tasks.lock().unwrap().len(),
                0,
                "timeout entry must be removed on natural completion (k{i})"
            );
        }
        assert_eq!(policy.buckets.lock().unwrap().len(), 0);
    }

    /// Re-review 2 of F6-1 (guard/take TOCTOU under key reuse): the
    /// generation check and the bucket take must be ONE critical section.
    /// Drives the supersede cycle, then asserts that a stale generation's
    /// COMBINED take returns None AND leaves the newer bucket intact, while
    /// the current generation's take drains it.
    #[tokio::test]
    async fn batch_timeout_key_reuse_stale_take_leaves_newer_bucket() {
        let policy = BatchPolicy::with_limits(
            Arc::new(ConstExpr("hot".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Timeout(60_000), // long — nothing fires naturally here
            16,
            100,
            16,
        );

        // Bucket + timeout task for key "hot" (generation 1).
        assert!(policy.accept(mk_exchange(1)).await.is_empty());
        let first_gen = {
            let tasks = policy.timeout_tasks.lock().unwrap();
            tasks.get("hot").map(|e| e.generation).unwrap()
        };

        // Supersede: size-based completion path cancels + removes generation
        // 1 AND drains the bucket, then a new exchange respawns generation
        // 2 with a fresh bucket for the same key.
        policy.cancel_timeout("hot");
        assert!(policy.take_bucket("hot").is_some());
        assert!(policy.timeout_tasks.lock().unwrap().is_empty());
        assert!(policy.accept(mk_exchange(2)).await.is_empty());
        let second_gen = {
            let tasks = policy.timeout_tasks.lock().unwrap();
            tasks.get("hot").map(|e| e.generation).unwrap()
        };
        assert_ne!(first_gen, second_gen);

        // A stale cleanup (as if the generation-1 task woke up late) must
        // not remove the generation-2 entry.
        policy.remove_timeout_task_if_current("hot", first_gen);
        assert_eq!(
            policy.timeout_tasks.lock().unwrap().len(),
            1,
            "stale generation cleanup must not remove the newer entry"
        );

        // The stale task's combined guard+take must return None AND leave
        // the newer generation's bucket untouched.
        let stolen = policy.take_bucket_if_current_timeout_task("hot", first_gen);
        assert!(
            stolen.is_none(),
            "stale generation must not take the bucket"
        );
        assert_eq!(
            policy.buffered(),
            1,
            "newer bucket must remain after the stale combined take"
        );

        // The current generation's combined take succeeds and drains.
        let bucket = policy.take_bucket_if_current_timeout_task("hot", second_gen);
        assert_eq!(
            bucket
                .expect("current generation drains its bucket")
                .exchanges
                .len(),
            1
        );
        policy.remove_timeout_task_if_current("hot", second_gen);
        assert!(policy.timeout_tasks.lock().unwrap().is_empty());

        // Flush has nothing left — exactly-once semantics preserved.
        let flushed = policy.flush().await;
        assert!(flushed.is_empty());
    }

    /// Re-review 2 of F6-1 (regression bite): proves the combined
    /// guard+take is atomic by forcing the interleaving DETERMINISTICALLY.
    /// A background thread performs the full supersede (cancel + newer
    /// bucket + newer generation entry) exactly between the generation
    /// check and the bucket take, via the test-only hook inside the
    /// critical section. Under the combined implementation the supersede
    /// blocks on the held `timeout_tasks` lock and the take retrieves the
    /// ORIGINAL bucket; a separate check-then-take implementation would
    /// let the supersede complete in the gap and the take would STEAL the
    /// newer generation's bucket — failing the assertions below.
    #[tokio::test]
    async fn batch_timeout_combined_take_atomic_under_interleaved_supersede() {
        let policy = BatchPolicy::with_limits(
            Arc::new(ConstExpr("hot".into())),
            Arc::new(PropExpr("seq".into())),
            BatchCompletion::Timeout(60_000), // long — nothing fires naturally here
            16,
            100,
            16,
        );

        // Bucket + timeout task for key "hot" (generation 1, seq 1).
        assert!(policy.accept(mk_exchange(1)).await.is_empty());
        let first_gen = {
            let tasks = policy.timeout_tasks.lock().unwrap();
            tasks.get("hot").map(|e| e.generation).unwrap()
        };

        let (start_tx, start_rx) = std::sync::mpsc::channel::<()>();
        let (supersede_done_tx, supersede_done_rx) = std::sync::mpsc::channel::<()>();
        let supersede_done_rx = std::sync::Arc::new(std::sync::Mutex::new(supersede_done_rx));
        *policy.interleave_hook.lock().unwrap() = Some(Arc::new(move || {
            // Signal the background supersede, then wait (bounded) for it to
            // complete. Under the CORRECT combined implementation the
            // supersede blocks on the held `timeout_tasks` lock, this wait
            // times out, and the take proceeds with the original bucket.
            // Under a separate check-then-take implementation the supersede
            // completes in the unlocked gap, this wait succeeds, and the
            // subsequent take steals the newer bucket — failing the test.
            start_tx.send(()).expect("hook start signal");
            let _ = supersede_done_rx
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(2));
        }));

        let bg_policy = Arc::clone(&policy);
        let bg = std::thread::spawn(move || {
            start_rx.recv().expect("bg start");
            // Full supersede, as a size-completion + key reuse would do.
            bg_policy.cancel_timeout("hot");
            {
                let mut buckets = bg_policy.buckets.lock().unwrap();
                buckets.insert(
                    "hot".to_string(),
                    Bucket {
                        exchanges: vec![mk_exchange(2)],
                    },
                );
            }
            let newer_gen = bg_policy.timeout_generation.fetch_add(1, Ordering::SeqCst) + 1;
            bg_policy.timeout_tasks.lock().unwrap().insert(
                "hot".to_string(),
                TimeoutEntry {
                    generation: newer_gen,
                    cancel: CancellationToken::new(),
                },
            );
            supersede_done_tx.send(()).expect("supersede done signal");
        });

        // The operation under test: guard + take in one critical section.
        // join() blocks until the background supersede finishes (it is
        // serialized behind the lock the take holds).
        let taken = policy.take_bucket_if_current_timeout_task("hot", first_gen);
        bg.join().expect("background thread clean");

        let taken = taken.expect("generation 1 still owned the take at check time");
        // The taken bucket is the ORIGINAL (seq 1) — never the newer one.
        let seq_of = |ex: &Exchange| ex.property("seq").cloned().unwrap_or_default();
        assert_eq!(
            seq_of(&taken.exchanges[0]),
            serde_json::json!(1),
            "combined take must retrieve the original bucket, not steal the newer one"
        );
        // The newer generation's bucket survived the interleaving.
        assert_eq!(
            policy.buffered(),
            1,
            "newer bucket must remain after the interleaved supersede"
        );
        let newer_bucket = policy.take_bucket("hot").expect("newer bucket present");
        assert_eq!(seq_of(&newer_bucket.exchanges[0]), serde_json::json!(2));

        policy.flush().await;
    }
}
