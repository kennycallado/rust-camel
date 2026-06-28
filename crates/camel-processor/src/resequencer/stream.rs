//! Stream resequencing policy — bounded priority queue keyed by sequence
//! number, gap detection with per-gap timeout, capacity cap, opt-in dedup.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::exchange::Exchange;
use camel_api::resequencer::{CapacityPolicy, GapPolicy};
use camel_language_api::Expression;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::ResequencePolicy;

/// Stream resequencing policy.
///
/// Uses a `BTreeMap<u64, Exchange>` as a priority queue keyed by sequence
/// number. Tracks `next_expected` and emits the contiguous run whenever the
/// expected sequence arrives. Gap timers fire when a missing sequence is not
/// received within `gap_timeout`.
pub struct StreamPolicy {
    sequence_expr: Arc<dyn Expression>,
    capacity: usize,
    gap_timeout: Duration,
    on_gap: GapPolicy,
    on_capacity_exceeded: CapacityPolicy,
    dedup: bool,

    /// Weak self-reference so gap timer tasks can upgrade to `Arc<Self>`.
    weak_self: Weak<Self>,

    /// Priority queue keyed by sequence number (min-first via BTreeMap).
    queue: Mutex<BTreeMap<u64, Exchange>>,

    /// Next expected sequence number.
    next_expected: Mutex<u64>,

    /// Gap cancellation tokens, keyed by the missing sequence number.
    gap_tokens: Mutex<HashMap<u64, CancellationToken>>,

    /// Gap timer task handles, keyed by the missing sequence number.
    gap_handles: Mutex<HashMap<u64, JoinHandle<()>>>,

    /// Channel to the post-driver for gap-timer-triggered emissions.
    driver_tx: Mutex<Option<mpsc::Sender<Exchange>>>,

    /// Shutdown guard — gap tasks check this before sending.
    shutdown_started: AtomicBool,
}

impl StreamPolicy {
    /// Create a new `Arc<StreamPolicy>` using `Arc::new_cyclic`.
    pub fn new_cyclic(
        sequence_expr: Arc<dyn Expression>,
        capacity: usize,
        gap_timeout_ms: u64,
        on_gap: GapPolicy,
        on_capacity_exceeded: CapacityPolicy,
        dedup: bool,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            sequence_expr,
            capacity,
            gap_timeout: Duration::from_millis(gap_timeout_ms),
            on_gap,
            on_capacity_exceeded,
            dedup,
            weak_self: weak.clone(),
            queue: Mutex::new(BTreeMap::new()),
            next_expected: Mutex::new(1),
            gap_tokens: Mutex::new(HashMap::new()),
            gap_handles: Mutex::new(HashMap::new()),
            driver_tx: Mutex::new(None),
            shutdown_started: AtomicBool::new(false),
        })
    }

    /// Set the driver channel (via `set_timeout_tx` trait method).
    fn set_driver_tx(&self, tx: mpsc::Sender<Exchange>) {
        let mut guard = self.driver_tx.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(tx);
    }

    /// Evaluate the sequence expression against an exchange.
    async fn eval_seq(&self, exchange: &Exchange) -> Result<u64, String> {
        let val = self
            .sequence_expr
            .evaluate(exchange)
            .await
            .map_err(|e| format!("sequence expression evaluation failed: {e}"))?;
        match val {
            serde_json::Value::Number(n) => n
                .as_u64()
                .ok_or_else(|| format!("sequence value must be a non-negative integer, got {n}")),
            _ => Err(format!(
                "sequence expression must evaluate to a number, got {}",
                val
            )),
        }
    }

    /// Get the next expected sequence number.
    fn next_expected(&self) -> u64 {
        *self.next_expected.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Set the next expected sequence number.
    fn set_next_expected(&self, v: u64) {
        *self.next_expected.lock().unwrap_or_else(|e| e.into_inner()) = v;
    }

    /// Drain the contiguous tail from the queue, starting at `next_expected`.
    /// Returns all consecutive exchanges in sequence order and advances `next_expected`.
    fn drain_contiguous(&self) -> Vec<Exchange> {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        let mut expected = *self.next_expected.lock().unwrap_or_else(|e| e.into_inner());
        let mut emitted = Vec::new();

        while let Some(ex) = queue.remove(&expected) {
            // Cancel any gap timer for this sequence
            self.cancel_gap_timer(expected);
            emitted.push(ex);
            expected += 1;
        }

        *self.next_expected.lock().unwrap_or_else(|e| e.into_inner()) = expected;
        emitted
    }

    /// Drain ALL held exchanges from the queue in sequence order, returning
    /// `(held_exchanges, max_seq)` where `max_seq` is the largest sequence
    /// number drained (0 if empty).
    fn drain_all_with_max(&self) -> (Vec<Exchange>, u64) {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        let keys: Vec<u64> = queue.keys().copied().collect();
        let max_seq = keys.iter().max().copied().unwrap_or(0);
        let mut held = Vec::new();
        for k in keys {
            if let Some(ex) = queue.remove(&k) {
                self.cancel_gap_timer(k);
                held.push(ex);
            }
        }
        (held, max_seq)
    }

    /// Check if a gap timer exists for a sequence number.
    fn has_gap_timer(&self, seq: u64) -> bool {
        let tokens = self.gap_tokens.lock().unwrap_or_else(|e| e.into_inner());
        tokens.contains_key(&seq)
    }

    /// Cancel and remove a gap timer for a sequence number.
    fn cancel_gap_timer(&self, seq: u64) {
        {
            let mut tokens = self.gap_tokens.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(token) = tokens.remove(&seq) {
                token.cancel();
            }
        }
        {
            let mut handles = self.gap_handles.lock().unwrap_or_else(|e| e.into_inner());
            handles.remove(&seq);
        }
    }

    /// Cancel all gap timers.
    fn cancel_all_gap_timers(&self) {
        let tokens: HashMap<u64, CancellationToken> = {
            let mut guard = self.gap_tokens.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *guard)
        };
        for (_, token) in tokens {
            token.cancel();
        }
        {
            let mut handles = self.gap_handles.lock().unwrap_or_else(|e| e.into_inner());
            handles.clear();
        }
    }

    /// Spawn a gap timer for the given missing sequence.
    fn spawn_gap_timer(&self, missing_seq: u64) {
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        {
            let mut tokens = self.gap_tokens.lock().unwrap_or_else(|e| e.into_inner());
            tokens.insert(missing_seq, cancel);
        }

        let weak = self.weak_self.clone();
        let gap_timeout = self.gap_timeout;
        let on_gap = self.on_gap;
        let driver_tx_opt = {
            let guard = self.driver_tx.lock().unwrap_or_else(|e| e.into_inner());
            guard.clone()
        };

        let handle = tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(gap_timeout) => {
                    if cancel_clone.is_cancelled() {
                        return;
                    }
                }
                _ = cancel_clone.cancelled() => {
                    return;
                }
            }

            let Some(policy) = weak.upgrade() else {
                return;
            };

            if policy.shutdown_started.load(Ordering::SeqCst) {
                return;
            }

            match on_gap {
                GapPolicy::EmitPartial => {
                    // Advance past the missing sequence, then drain only the
                    // contiguous run from the new position. Preserves
                    // resequencing purity: intermediate gaps stay held.
                    policy.set_next_expected(missing_seq + 1);
                    let emitted = policy.drain_contiguous();

                    if emitted.is_empty() {
                        return;
                    }

                    if let Some(tx) = &driver_tx_opt {
                        for ex in emitted {
                            if tx.send(ex).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                GapPolicy::DropAndLog => {
                    let (held, max_seq) = policy.drain_all_with_max();

                    if held.is_empty() {
                        return;
                    }

                    policy.set_next_expected(max_seq + 1);

                    // Drop-and-log: log and drop (best-effort, no dead-letter sink)
                    for ex in &held {
                        tracing::warn!(
                            correlation_id = %ex.correlation_id(),
                            "stream resequencer: gap timeout — dropping held exchange (no dead-letter sink wired)"
                        );
                    }
                    // Held exchanges are dropped
                    let _ = held;
                }
            }

            // Clean up handle and token entries (C2 fix: remove token too)
            {
                let mut handles = policy.gap_handles.lock().unwrap_or_else(|e| e.into_inner());
                handles.remove(&missing_seq);
            }
            {
                let mut tokens = policy.gap_tokens.lock().unwrap_or_else(|e| e.into_inner());
                tokens.remove(&missing_seq);
            }
        });

        {
            let mut handles = self.gap_handles.lock().unwrap_or_else(|e| e.into_inner());
            handles.insert(missing_seq, handle);
        }
    }
}

#[async_trait]
impl ResequencePolicy for StreamPolicy {
    async fn accept(&self, input: Exchange) -> Vec<Exchange> {
        let seq = match self.eval_seq(&input).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    correlation_id = %input.correlation_id(),
                    "StreamPolicy: sequence expression failed, dropping exchange"
                );
                return vec![];
            }
        };

        let expected = self.next_expected();

        if seq == expected {
            // C1: Cancel any gap timer that was armed for this expected sequence
            // (e.g. next_expected=1, seq 2 arrived and armed timer for 1,
            //  then seq 1 arrives — cancel the stale timer before draining).
            self.cancel_gap_timer(expected);
            // Advance next_expected past this sequence BEFORE draining
            self.set_next_expected(seq + 1);
            let mut emitted = vec![input];
            emitted.append(&mut self.drain_contiguous());
            emitted
        } else if seq < expected {
            // Late or duplicate
            if self.dedup {
                // Ignore duplicate
                tracing::debug!(
                    seq = seq,
                    expected = expected,
                    "StreamPolicy: ignoring duplicate/late sequence (dedup enabled)"
                );
                return vec![];
            }
            // dedup=false: insert anyway (Camel 4.x behavior)
            {
                let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                queue.insert(seq, input);
            }
            vec![]
        } else {
            // seq > expected: insert into queue + arm gap timer
            {
                let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                let queue_len = queue.len();

                // I1: dedup check for held future seqs — redelivered held seq
                // should be ignored when dedup is enabled.
                if self.dedup && queue.contains_key(&seq) {
                    tracing::debug!(
                        seq = seq,
                        "StreamPolicy: ignoring redelivered held sequence (dedup enabled)"
                    );
                    return vec![];
                }

                // Capacity overflow check
                if queue_len >= self.capacity {
                    match self.on_capacity_exceeded {
                        CapacityPolicy::LogAndDrop => {
                            tracing::warn!(
                                seq = seq,
                                capacity = self.capacity,
                                "StreamPolicy: capacity exceeded, dropping incoming exchange"
                            );
                            return vec![];
                        }
                        CapacityPolicy::DropOldest => {
                            // Remove the smallest seq in the queue
                            let oldest_key = queue.keys().next().copied();
                            if let Some(oldest) = oldest_key {
                                let dropped = queue.remove(&oldest);
                                self.cancel_gap_timer(oldest);
                                tracing::debug!(
                                    dropped_seq = oldest,
                                    "StreamPolicy: capacity exceeded, dropped oldest exchange"
                                );
                                let _ = dropped;
                            }
                        }
                    }
                }

                queue.insert(seq, input);
            }

            // Arm a gap timer for the expected (missing) sequence if not already armed
            if !self.has_gap_timer(expected) {
                self.spawn_gap_timer(expected);
            }

            vec![]
        }
    }

    async fn flush(&self) -> Vec<Exchange> {
        self.shutdown_started.store(true, Ordering::SeqCst);
        self.cancel_all_gap_timers();

        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        let keys: Vec<u64> = queue.keys().copied().collect();
        let mut held = Vec::new();
        for k in keys {
            if let Some(ex) = queue.remove(&k) {
                held.push(ex);
            }
        }
        held
    }

    fn name(&self) -> &'static str {
        "stream-resequencer"
    }

    fn set_timeout_tx(&self, tx: mpsc::Sender<Exchange>) {
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

    fn mk_exchange(seq: u64) -> Exchange {
        let mut ex = Exchange::new(Message::new(camel_api::body::Body::Text(format!(
            "msg-{seq}"
        ))));
        ex.set_property("seq", serde_json::json!(seq));
        ex.pattern = ExchangePattern::InOnly;
        ex
    }

    fn default_policy() -> Arc<StreamPolicy> {
        StreamPolicy::new_cyclic(
            Arc::new(PropExpr("seq".into())),
            100,
            5000,
            GapPolicy::EmitPartial,
            CapacityPolicy::LogAndDrop,
            false,
        )
    }

    fn seq_of(ex: &Exchange) -> u64 {
        ex.property("seq").and_then(|v| v.as_u64()).unwrap_or(0)
    }

    /// S1: expected=1; receive 2 (held), 1 (emit 1 then drain 2),
    ///     4 (held), 3 (emit 3 then drain 4)
    #[tokio::test]
    async fn stream_emit_contiguous_run() {
        let policy = default_policy();

        // Receive 2 (held: next_expected=1, seq=2 > 1 → held)
        let result2 = policy.accept(mk_exchange(2)).await;
        assert!(
            result2.is_empty(),
            "seq 2 should be held (not yet contiguous)"
        );

        // Receive 1 (expected! → emit 1, then drain 2)
        let result1 = policy.accept(mk_exchange(1)).await;
        assert_eq!(result1.len(), 2, "should emit 1 then drain 2");
        let seqs1: Vec<u64> = result1.iter().map(seq_of).collect();
        assert_eq!(seqs1, vec![1, 2]);

        // Receive 4 (held: next_expected=3, seq=4 > 3 → held)
        let result4 = policy.accept(mk_exchange(4)).await;
        assert!(result4.is_empty(), "seq 4 should be held");

        // Receive 3 (expected! → emit 3, then drain 4)
        let result3 = policy.accept(mk_exchange(3)).await;
        assert_eq!(result3.len(), 2, "should emit 3 then drain 4");
        let seqs3: Vec<u64> = result3.iter().map(seq_of).collect();
        assert_eq!(seqs3, vec![3, 4]);
    }

    /// S2: expected=1; receive 2,3 (held); gap timer fires →
    ///     emit held [2,3], advance expected to 4
    #[tokio::test]
    async fn stream_gap_timeout_emit_partial() {
        let policy = StreamPolicy::new_cyclic(
            Arc::new(PropExpr("seq".into())),
            100,
            50, // 50ms gap timeout
            GapPolicy::EmitPartial,
            CapacityPolicy::LogAndDrop,
            false,
        );

        let (tx, mut rx) = mpsc::channel::<Exchange>(16);
        policy.set_timeout_tx(tx);

        // Receive 2 and 3 (held)
        assert!(policy.accept(mk_exchange(2)).await.is_empty());
        assert!(policy.accept(mk_exchange(3)).await.is_empty());

        // Wait for gap timer to fire (seq 1 never arrives)
        let emitted: Vec<Exchange> = tokio::time::timeout(Duration::from_millis(500), async {
            let mut out = Vec::new();
            out.push(rx.recv().await.unwrap());
            out.push(rx.recv().await.unwrap());
            out
        })
        .await
        .expect("gap timer should fire within 500ms");

        assert_eq!(emitted.len(), 2, "should emit all held exchanges");
        let seqs: Vec<u64> = emitted.iter().map(seq_of).collect();
        assert_eq!(seqs, vec![2, 3], "should emit in sequence order");

        // next_expected should have advanced past the gap to max(2,3) + 1 = 4
        assert_eq!(
            policy.next_expected(),
            4,
            "next_expected should advance past gap to max(drained)+1"
        );
    }

    /// S3: queue fills at capacity; next input triggers LogAndDrop
    #[tokio::test]
    async fn stream_capacity_exceeded_log_and_drop() {
        let policy = StreamPolicy::new_cyclic(
            Arc::new(PropExpr("seq".into())),
            2, // tiny capacity
            5000,
            GapPolicy::EmitPartial,
            CapacityPolicy::LogAndDrop,
            false,
        );

        // Fill queue: seq 3, 4 (next_expected=1)
        assert!(policy.accept(mk_exchange(3)).await.is_empty());
        assert!(policy.accept(mk_exchange(4)).await.is_empty());

        {
            let queue = policy.queue.lock().unwrap();
            assert_eq!(queue.len(), 2, "queue should be full");
        }

        // Next input (seq 5) should trigger LogAndDrop
        let result = policy.accept(mk_exchange(5)).await;
        assert!(
            result.is_empty(),
            "overflow exchange should be dead-lettered (empty result)"
        );

        {
            let queue = policy.queue.lock().unwrap();
            assert_eq!(queue.len(), 2, "queue should stay at capacity");
        }
    }

    /// S4: queue fills; next input drops oldest
    #[tokio::test]
    async fn stream_capacity_exceeded_drop_oldest() {
        let policy = StreamPolicy::new_cyclic(
            Arc::new(PropExpr("seq".into())),
            2,
            5000,
            GapPolicy::EmitPartial,
            CapacityPolicy::DropOldest,
            false,
        );

        // Fill queue: seq 3, 4
        assert!(policy.accept(mk_exchange(3)).await.is_empty());
        assert!(policy.accept(mk_exchange(4)).await.is_empty());

        // Next input (seq 5) should drop oldest (seq 3), insert seq 5
        let result = policy.accept(mk_exchange(5)).await;
        assert!(
            result.is_empty(),
            "overflow with DropOldest should not emit"
        );

        {
            let queue = policy.queue.lock().unwrap();
            assert_eq!(queue.len(), 2, "queue should still be at capacity");
            assert!(!queue.contains_key(&3), "oldest seq 3 should be dropped");
            assert!(queue.contains_key(&4), "seq 4 should remain");
            assert!(queue.contains_key(&5), "seq 5 should be inserted");
        }
    }

    /// S5: with dedup=true, duplicate seq is ignored
    #[tokio::test]
    async fn stream_dedup_on_ignores_duplicate() {
        let policy = StreamPolicy::new_cyclic(
            Arc::new(PropExpr("seq".into())),
            100,
            5000,
            GapPolicy::EmitPartial,
            CapacityPolicy::LogAndDrop,
            true, // dedup on
        );

        // Receive 1 (emit)
        let result1 = policy.accept(mk_exchange(1)).await;
        assert_eq!(result1.len(), 1, "seq 1 should be emitted");

        // Receive 1 again — should be ignored (duplicate with dedup=true)
        let result2 = policy.accept(mk_exchange(1)).await;
        assert!(
            result2.is_empty(),
            "duplicate seq 1 should be ignored with dedup on"
        );
    }

    /// S6: with dedup=false (default), duplicate seq is inserted
    ///     (Camel 4.x behavior)
    #[tokio::test]
    async fn stream_dedup_off_inserts_duplicate() {
        let policy = StreamPolicy::new_cyclic(
            Arc::new(PropExpr("seq".into())),
            100,
            5000,
            GapPolicy::EmitPartial,
            CapacityPolicy::LogAndDrop,
            false, // dedup off (default)
        );

        // Receive 1 (emit)
        let result1 = policy.accept(mk_exchange(1)).await;
        assert_eq!(result1.len(), 1, "seq 1 should be emitted");

        // Receive 1 again — with dedup=false, duplicate is inserted
        let result2 = policy.accept(mk_exchange(1)).await;
        assert!(
            result2.is_empty(),
            "duplicate seq 1 with dedup off should be inserted, not emitted"
        );

        {
            let queue = policy.queue.lock().unwrap();
            assert!(
                queue.contains_key(&1),
                "duplicate seq 1 should be in queue (dedup off)"
            );
        }
    }

    /// S7: flush() emits remaining in seq order
    #[tokio::test]
    async fn stream_flush_emits_remaining_sorted() {
        let policy = default_policy();

        // Emit 1 first
        assert!(!policy.accept(mk_exchange(1)).await.is_empty());

        // Now next_expected=2. Hold 5, 3
        assert!(policy.accept(mk_exchange(5)).await.is_empty());
        assert!(policy.accept(mk_exchange(3)).await.is_empty());

        // Flush
        let flushed = policy.flush().await;
        assert_eq!(flushed.len(), 2, "should emit all remaining held exchanges");
        let seqs: Vec<u64> = flushed.iter().map(seq_of).collect();
        assert_eq!(seqs, vec![3, 5], "should be in sequence order");
    }

    /// S8: seq < next_expected after advance → dedup behavior
    #[tokio::test]
    async fn stream_late_sequence_after_advance() {
        // Policy with dedup=true
        let policy_dedup = StreamPolicy::new_cyclic(
            Arc::new(PropExpr("seq".into())),
            100,
            50,
            GapPolicy::EmitPartial,
            CapacityPolicy::LogAndDrop,
            true,
        );

        // Advance past seq 1,2,3 by setting next_expected to 5
        {
            let mut ne = policy_dedup.next_expected.lock().unwrap();
            *ne = 5;
        }

        // seq 3 < 5, dedup=true → ignored
        let result = policy_dedup.accept(mk_exchange(3)).await;
        assert!(
            result.is_empty(),
            "late seq with dedup=true should be ignored"
        );

        // Policy with dedup=false
        let policy_no_dedup = StreamPolicy::new_cyclic(
            Arc::new(PropExpr("seq".into())),
            100,
            50,
            GapPolicy::EmitPartial,
            CapacityPolicy::LogAndDrop,
            false,
        );

        {
            let mut ne = policy_no_dedup.next_expected.lock().unwrap();
            *ne = 5;
        }

        // seq 3 < 5, dedup=false → inserted into queue
        let result = policy_no_dedup.accept(mk_exchange(3)).await;
        assert!(
            result.is_empty(),
            "late seq with dedup=false should be inserted"
        );

        {
            let queue = policy_no_dedup.queue.lock().unwrap();
            assert!(
                queue.contains_key(&3),
                "late seq should be in queue with dedup off"
            );
        }
    }

    /// M4/C1: next_expected=1 → seq 2 arrives (arms timer for 1) →
    /// seq 1 arrives within timeout → timer 1 must NOT fire → no corruption
    #[tokio::test]
    async fn stream_stale_gap_timer_cancelled_on_normal_arrival() {
        let policy = StreamPolicy::new_cyclic(
            Arc::new(PropExpr("seq".into())),
            100,
            50, // 50ms gap timeout
            GapPolicy::DropAndLog,
            CapacityPolicy::LogAndDrop,
            false,
        );

        let (tx, mut rx) = mpsc::channel::<Exchange>(16);
        policy.set_timeout_tx(tx);

        // next_expected=1, seq 2 arrives → held, gap timer armed for seq 1
        assert!(policy.accept(mk_exchange(2)).await.is_empty());
        assert!(
            policy.has_gap_timer(1),
            "gap timer should be armed for seq 1"
        );

        // seq 1 arrives within timeout → cancels timer, emits 1 and drains 2
        let result = policy.accept(mk_exchange(1)).await;
        assert_eq!(result.len(), 2, "should emit seq 1 and drained seq 2");
        let seqs: Vec<u64> = result.iter().map(seq_of).collect();
        assert_eq!(seqs, vec![1, 2]);

        // Verify gap timer for seq 1 was cancelled (C1 fix)
        assert!(
            !policy.has_gap_timer(1),
            "gap timer for seq 1 should be cancelled after normal arrival"
        );

        // Verify no stale timer fire: the mpsc channel should NOT receive a
        // corrupt re-emission from the old gap timer.
        tokio::time::sleep(Duration::from_millis(200)).await;
        match rx.try_recv() {
            Ok(ex) => {
                panic!(
                    "stale gap timer fired and sent exchange with seq={} — corruption!",
                    seq_of(&ex)
                );
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                // Expected: no stale emission
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {}
        }
    }

    /// M4/I1: dedup=true, seq 5 held, redeliver seq 5 → ignored (original preserved)
    #[tokio::test]
    async fn stream_dedup_held_future_seq() {
        let policy = StreamPolicy::new_cyclic(
            Arc::new(PropExpr("seq".into())),
            100,
            5000,
            GapPolicy::EmitPartial,
            CapacityPolicy::LogAndDrop,
            true, // dedup on
        );

        // next_expected=1, send seq 5 → held
        assert!(policy.accept(mk_exchange(5)).await.is_empty());

        // Redeliver seq 5 → should be ignored with dedup=true
        let result = policy.accept(mk_exchange(5)).await;
        assert!(result.is_empty(), "redelivered held seq should be ignored");

        // Queue should still contain only one seq 5
        {
            let queue = policy.queue.lock().unwrap();
            assert_eq!(queue.len(), 1, "queue should still have exactly one entry");
            assert!(queue.contains_key(&5), "seq 5 should still be in queue");
        }
    }
}
