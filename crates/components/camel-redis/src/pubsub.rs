//! Pub/Sub consumer I/O seam and failover loop.
//!
//! Extracted from `consumer.rs` so the subscription-replay reconnect loop and
//! its test doubles live apart from the lifecycle code. The public surface
//! (`RedisConsumer`, `RedisConsumerMode`) stays in `consumer.rs`.
//!
//! # Delivery semantics
//!
//! Pub/Sub is **best-effort delivery**: messages published while the consumer
//! is disconnected are lost, and a failover reconnect can re-deliver a message
//! that was already handed to the pipeline. Loss and duplicates are possible
//! and expected; the consumer does not attempt exactly-once delivery.

use async_trait::async_trait;
use camel_component_api::{CamelError, NetworkRetryPolicy};
use futures_util::StreamExt;
use redis::Msg;
use std::future::Future;
use std::ops::ControlFlow;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::retry::{retry_budget_exhausted, transient_retry_step};
use crate::topology::{RedisTopology, ServerKind};

/// Injectable I/O seam for the pub/sub consumer's reconnect loop.
///
/// Lets the failover loop be tested without a broker: the real impl talks to
/// Redis, a test double records programmable outcomes.
#[async_trait]
pub(crate) trait PubSubIo: Send {
    /// Establish a dedicated pub/sub connection to `client`.
    async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError>;
    /// Subscribe to a channel (SUBSCRIBE).
    async fn subscribe(&mut self, ch: &str) -> Result<(), CamelError>;
    /// Subscribe to a pattern (PSUBSCRIBE).
    async fn psubscribe(&mut self, pat: &str) -> Result<(), CamelError>;
    /// Poll the next message. `None` means the stream ended (connection closed).
    async fn next_msg(&mut self) -> Option<Msg>;
}

/// Real [`PubSubIo`] backed by a dedicated Redis pub/sub connection.
pub(crate) struct RedisPubSubIo {
    pubsub: Option<redis::aio::PubSub>,
    timeout_secs: u64,
}

impl RedisPubSubIo {
    pub(crate) fn new(timeout_secs: u64) -> Self {
        Self {
            pubsub: None,
            timeout_secs,
        }
    }
}

#[async_trait]
impl PubSubIo for RedisPubSubIo {
    async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError> {
        let pubsub = tokio::time::timeout(
            Duration::from_secs(self.timeout_secs),
            client.get_async_pubsub(),
        )
        .await
        .map_err(|_| {
            CamelError::ProcessorError(format!(
                "PubSub connection timed out after {}s",
                self.timeout_secs
            ))
        })?
        .map_err(|e| {
            CamelError::ProcessorError(format!("Failed to create PubSub connection: {}", e))
        })?;
        self.pubsub = Some(pubsub);
        Ok(())
    }

    async fn subscribe(&mut self, ch: &str) -> Result<(), CamelError> {
        let pubsub = self.pubsub.as_mut().ok_or_else(|| {
            CamelError::ProcessorError("PubSub connection not established".into())
        })?;
        pubsub.subscribe(ch).await.map_err(|e| {
            CamelError::ProcessorError(format!("Failed to subscribe to channel {}: {}", ch, e))
        })
    }

    async fn psubscribe(&mut self, pat: &str) -> Result<(), CamelError> {
        let pubsub = self.pubsub.as_mut().ok_or_else(|| {
            CamelError::ProcessorError("PubSub connection not established".into())
        })?;
        pubsub.psubscribe(pat).await.map_err(|e| {
            CamelError::ProcessorError(format!("Failed to subscribe to pattern {}: {}", pat, e))
        })
    }

    async fn next_msg(&mut self) -> Option<Msg> {
        let pubsub = self.pubsub.as_mut()?;
        pubsub.on_message().next().await
    }
}

/// Replay every channel and pattern subscription on `io`.
///
/// Subscriptions are per-connection state, so this must be re-invoked after
/// every reconnect.
pub(crate) async fn subscribe_all<P: PubSubIo + Send + ?Sized>(
    io: &mut P,
    channels: &[String],
    patterns: &[String],
) -> Result<(), CamelError> {
    for ch in channels {
        io.subscribe(ch).await?;
    }
    for pat in patterns {
        io.psubscribe(pat).await?;
    }
    Ok(())
}

/// Resolve the current master, connect, and replay subscriptions.
///
/// Retries transient resolve/connect/subscribe failures within `policy`'s
/// budget, advancing `attempt` on each failure. Returns `Ok(())` on a live,
/// subscribed connection (or cancellation), or `Err` on budget exhaustion /
/// non-transient error.
async fn connect_and_subscribe(
    topology: &dyn RedisTopology,
    io: &mut dyn PubSubIo,
    channels: &[String],
    patterns: &[String],
    policy: &NetworkRetryPolicy,
    attempt: &mut u32,
    cancel: &CancellationToken,
) -> Result<(), CamelError> {
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        // Resolve the current master (re-resolves on every call for sentinel).
        let client = match topology.resolve(ServerKind::Master).await {
            Ok(client) => client,
            Err(e) => {
                match transient_retry_step(policy, attempt, e, "resolving master for PubSub").await
                {
                    ControlFlow::Continue(()) => continue,
                    ControlFlow::Break(e) => return Err(e),
                }
            }
        };

        // Establish a fresh pub/sub connection to the resolved master.
        if let Err(e) = io.connect(&client).await {
            match transient_retry_step(policy, attempt, e, "connecting for PubSub").await {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(e) => return Err(e),
            }
        }

        // Replay subscriptions — required after every (re)connect.
        if let Err(e) = subscribe_all(io, channels, patterns).await {
            match transient_retry_step(policy, attempt, e, "subscribing for PubSub").await {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(e) => return Err(e),
            }
        }

        return Ok(());
    }
}

/// Run a whole Pub/Sub session on ONE connection.
///
/// After a successful [`connect_and_subscribe`], this loop delivers **every**
/// message from that single connection through `deliver` — it does not
/// reconnect between messages. The connection is re-established (and
/// subscriptions replayed via [`subscribe_all`], because subscriptions are
/// per-connection state) only when:
///
/// - the stream ends (`next_msg` returns `None` — the connection closed), or
/// - a transient error strikes resolve/connect/subscribe during the
///   reconnect itself.
///
/// Returns:
/// - `Ok(())` on cancellation (clean shutdown).
/// - `Err` on budget exhaustion or a non-transient error. The consumer task
///   returns it so Route supervision fires (ADR-0007) — the loop never
///   restarts itself beyond the transport retry budget.
///
/// Each stream-end reconnect cycle consumes one attempt from `policy`'s
/// budget; on exhaustion [`retry_budget_exhausted`] builds the terminal error
/// (classified transient, ADR-0012).
pub(crate) async fn pubsub_session<D, F>(
    topology: &dyn RedisTopology,
    io: &mut dyn PubSubIo,
    channels: &[String],
    patterns: &[String],
    policy: &NetworkRetryPolicy,
    cancel: &CancellationToken,
    mut deliver: D,
) -> Result<(), CamelError>
where
    D: FnMut(Msg) -> F,
    F: Future<Output = ()>,
{
    let mut attempt: u32 = 0;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        connect_and_subscribe(
            topology,
            io,
            channels,
            patterns,
            policy,
            &mut attempt,
            cancel,
        )
        .await?;

        // Deliver every message from this one connection until the stream
        // ends, then fall through to the reconnect above.
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                msg = io.next_msg() => {
                    match msg {
                        Some(m) => deliver(m).await,
                        None => {
                            // Stream ended (connection closed). Reconnect and
                            // replay subscriptions, bounded by the retry budget.
                            attempt += 1;
                            if !policy.should_retry(attempt) {
                                return Err(retry_budget_exhausted(
                                    policy,
                                    "reconnecting after PubSub stream end",
                                    "PubSub stream ended",
                                ));
                            }
                            // log-policy: outside-contract
                            warn!("PubSub stream ended, reconnecting");
                            let delay = policy.delay_for(attempt - 1);
                            tokio::time::sleep(delay).await;
                            break; // → reconnect + replay subscriptions
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RedisEndpointConfig, is_transient_redis_error};
    use crate::topology::{FakeTopology, StandaloneTopology};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Construct a real `redis::Msg` without a broker (redis 1.x parses a
    /// plain `["message", channel, payload]` array).
    fn fake_msg(channel: &str, payload: &str) -> Msg {
        let value = redis::Value::Array(vec![
            redis::Value::BulkString(b"message".to_vec()),
            redis::Value::BulkString(channel.as_bytes().to_vec()),
            redis::Value::BulkString(payload.as_bytes().to_vec()),
        ]);
        Msg::from_value(&value).expect("valid message push value")
    }

    /// What `next_msg` does once the current connection's messages have
    /// drained AND no further batches remain.
    enum TailBehavior {
        /// Return `None` — the stream ends (connection closed).
        End,
        /// Pend forever — the connection stays open but idle.
        Pend,
    }

    /// Test double for [`PubSubIo`]: programmable connect outcomes (reuse last
    /// if exhausted), recording connect/subscribe counts and subscription args.
    ///
    /// `next_msg` yields the current connection's message batch in order
    /// (installed by `connect` from `message_batches`). When the batch drains
    /// it returns `None` (stream end) if another batch remains, else follows
    /// `tail` (`End` by default — preserves the original stream-end tests;
    /// `Pend` keeps the connection open-but-idle so tests can end via
    /// cancellation without triggering a reconnect).
    struct FakePubSubIo {
        connect_outcomes: Vec<Result<(), CamelError>>,
        message_batches: Vec<Vec<Msg>>,
        current: Vec<Msg>,
        tail: TailBehavior,
        connect_count: usize,
        next_msg_count: usize,
        subscribe_call_count: usize,
        subscribed_channels: Vec<String>,
        subscribed_patterns: Vec<String>,
    }

    impl FakePubSubIo {
        fn new(connect_outcomes: Vec<Result<(), CamelError>>) -> Self {
            Self {
                connect_outcomes,
                message_batches: Vec::new(),
                current: Vec::new(),
                tail: TailBehavior::End,
                connect_count: 0,
                next_msg_count: 0,
                subscribe_call_count: 0,
                subscribed_channels: Vec::new(),
                subscribed_patterns: Vec::new(),
            }
        }

        /// One message batch per connection; once the LAST batch drains,
        /// `next_msg` pends forever (open-but-idle connection).
        fn with_messages_per_connect(mut self, batches: Vec<Vec<Msg>>) -> Self {
            self.message_batches = batches;
            self.tail = TailBehavior::Pend;
            self
        }
    }

    #[async_trait]
    impl PubSubIo for FakePubSubIo {
        async fn connect(&mut self, _client: &redis::Client) -> Result<(), CamelError> {
            let idx = self.connect_count;
            self.connect_count += 1;
            let outcome = match self.connect_outcomes.get(idx) {
                Some(o) => o.clone(),
                None => self.connect_outcomes.last().cloned().unwrap_or(Ok(())),
            };
            // Install this connection's message batch, but only when the
            // connect succeeds — a failed connect never reaches next_msg.
            if outcome.is_ok() && !self.message_batches.is_empty() {
                self.current = self.message_batches.remove(0);
            }
            outcome
        }

        async fn subscribe(&mut self, ch: &str) -> Result<(), CamelError> {
            self.subscribe_call_count += 1;
            self.subscribed_channels.push(ch.to_string());
            Ok(())
        }

        async fn psubscribe(&mut self, pat: &str) -> Result<(), CamelError> {
            self.subscribe_call_count += 1;
            self.subscribed_patterns.push(pat.to_string());
            Ok(())
        }

        async fn next_msg(&mut self) -> Option<Msg> {
            self.next_msg_count += 1;
            if !self.current.is_empty() {
                return Some(self.current.remove(0));
            }
            if !self.message_batches.is_empty() {
                // More sessions programmed: end this stream so the session
                // reconnects into the next batch.
                return None;
            }
            match self.tail {
                TailBehavior::End => None, // stream end
                TailBehavior::Pend => std::future::pending().await,
            }
        }
    }

    fn fast_policy(max_attempts: u32) -> NetworkRetryPolicy {
        NetworkRetryPolicy {
            max_attempts,
            initial_delay: Duration::from_millis(0),
            ..NetworkRetryPolicy::default()
        }
    }

    // Deterministic test — no broker. `subscribe_all` replays every channel and
    // pattern in order.
    #[tokio::test]
    async fn subscribe_all_replays_every_channel_and_pattern() {
        let mut io = FakePubSubIo::new(vec![Ok(())]);

        subscribe_all(&mut io, &["a".into(), "b".into()], &["ev*".into()])
            .await
            .unwrap();

        assert_eq!(
            io.subscribed_channels,
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(io.subscribed_patterns, vec!["ev*".to_string()]);
    }

    // Deterministic failover test — no broker. The stream ends on the first
    // connection; the loop re-resolves the master, reconnects, and replays
    // subscriptions before the budget is exhausted.
    #[tokio::test]
    async fn pubsub_resubscribes_after_stream_end() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into(), "redis://b:6379".into()]);
        let mut io = FakePubSubIo::new(vec![Ok(()), Ok(())]);
        let cancel = CancellationToken::new();

        let result = pubsub_session(
            &topology,
            &mut io,
            &["a".into(), "b".into()],
            &["ev*".into()],
            &fast_policy(2),
            &cancel,
            |_| async {},
        )
        .await;

        assert!(
            result.is_err(),
            "budget exhausted after stream-end reconnects"
        );
        assert!(
            topology.resolve_call_count() >= 2,
            "master should be re-resolved after stream end"
        );
        // subscribe_all replayed on both connections: 2 channels + 1 pattern, twice.
        assert_eq!(io.subscribe_call_count, 2 * (2 + 1));
        assert_eq!(io.connect_count, 2);
    }

    // Deterministic failover test — no broker. Connect always fails transiently;
    // the loop exhausts the budget and returns Err (supervision fires).
    #[tokio::test]
    async fn pubsub_returns_err_on_budget_exhaustion() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakePubSubIo::new(vec![Err(CamelError::ProcessorError(
            "connection refused".into(),
        ))]);
        let cancel = CancellationToken::new();

        let result = pubsub_session(
            &topology,
            &mut io,
            &["a".into()],
            &[],
            &fast_policy(3),
            &cancel,
            |_| async {},
        )
        .await;

        assert!(result.is_err());
        assert_eq!(io.connect_count, 3);
    }

    // ADR-0012 observability: a stream-end budget-exhaustion error must
    // classify as transient so the consumer's Err-branch routing fires the
    // transient-budget metric, not `e:redis:message-non-transient`.
    #[tokio::test]
    async fn pubsub_budget_exhaustion_classified_transient() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakePubSubIo::new(vec![Ok(())]);
        let cancel = CancellationToken::new();

        let result = pubsub_session(
            &topology,
            &mut io,
            &["a".into()],
            &[],
            &fast_policy(1),
            &cancel,
            |_| async {},
        )
        .await;

        let err = result.expect_err("stream-end budget exhaustion must return Err");
        assert!(
            is_transient_redis_error(&err),
            "budget-exhaustion error must classify as transient: {}",
            err
        );
    }

    // Task 2.3: the intentional PubSub stream-end behavior change. A real
    // StandaloneTopology feeds the reconnect loop, which re-resolves the same
    // fixed connection and returns Err on budget exhaustion (NOT graceful
    // Ok), so Route supervision fires (ADR-0007). No broker — connect fails
    // via the fake.
    #[tokio::test]
    async fn standalone_pubsub_stream_end_returns_err_on_budget() {
        let cfg = RedisEndpointConfig::from_uri("redis://127.0.0.1:6379?command=SUBSCRIBE")
            .expect("valid uri");
        let topology = StandaloneTopology::new(&cfg);
        let mut io = FakePubSubIo::new(vec![Err(CamelError::ProcessorError(
            "connection refused".into(),
        ))]);
        let cancel = CancellationToken::new();

        let result = pubsub_session(
            &topology,
            &mut io,
            &["a".into()],
            &[],
            &fast_policy(3),
            &cancel,
            |_| async {},
        )
        .await;

        assert!(
            result.is_err(),
            "standalone PubSub must return Err on budget exhaustion, not graceful Ok"
        );
        assert_eq!(io.connect_count, 3);
    }

    // C1 regression: a whole session of N messages must be delivered over ONE
    // connection. The previous one-message-per-connection structure called
    // `connect` (topology resolve + TCP + AUTH + subscribe_all) for EVERY
    // delivered message; this test pins connect_count == 1 while all N
    // messages reach the delivery callback.
    #[tokio::test]
    async fn pubsub_delivers_many_messages_on_one_connection() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakePubSubIo::new(vec![Ok(())]).with_messages_per_connect(vec![vec![
            fake_msg("ch", "m1"),
            fake_msg("ch", "m2"),
            fake_msg("ch", "m3"),
        ]]);
        let cancel = CancellationToken::new();

        let payloads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let delivered = payloads.clone();
        let cancel_in_deliver = cancel.clone();
        let deliver = move |msg: Msg| {
            let delivered = delivered.clone();
            let cancel = cancel_in_deliver.clone();
            async move {
                let payload = msg
                    .get_payload::<String>()
                    .expect("fake payload decodes as String");
                let mut seen = delivered.lock().expect("payloads mutex");
                seen.push(payload);
                // End the session after the last message: the fake's next
                // `next_msg` pends, so cancellation resolves deterministically.
                if seen.len() == 3 {
                    cancel.cancel();
                }
            }
        };

        pubsub_session(
            &topology,
            &mut io,
            &["ch".into()],
            &[],
            &fast_policy(10),
            &cancel,
            deliver,
        )
        .await
        .expect("cancelled session must end Ok");

        assert_eq!(
            *payloads.lock().expect("payloads mutex"),
            vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
            "all messages must be delivered, in order"
        );
        assert_eq!(
            io.connect_count, 1,
            "C1 regression: the whole session must use exactly one connection"
        );
        // 3 message polls, plus (racily) one final poll that pends until the
        // cancel branch of the session's select wins.
        assert!(io.next_msg_count >= 3);
    }

    // C1 companion: after a mid-session stream end, the reconnect delivers
    // messages on the NEW connection without further churn — total connects
    // == sessions, not messages.
    #[tokio::test]
    async fn pubsub_reconnects_once_after_stream_end_not_per_message() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into(), "redis://b:6379".into()]);
        // First connection: one message, then stream end. Second connection:
        // one message, then idle (pend).
        let mut io = FakePubSubIo::new(vec![Ok(()), Ok(())]).with_messages_per_connect(vec![
            vec![fake_msg("ch", "first")],
            vec![fake_msg("ch", "second")],
        ]);
        let cancel = CancellationToken::new();

        let delivered = Arc::new(AtomicUsize::new(0));
        let count = delivered.clone();
        let cancel_in_deliver = cancel.clone();
        let deliver = move |_msg: Msg| {
            let count = count.clone();
            let cancel = cancel_in_deliver.clone();
            async move {
                if count.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    cancel.cancel();
                }
            }
        };

        pubsub_session(
            &topology,
            &mut io,
            &["ch".into()],
            &[],
            &fast_policy(10),
            &cancel,
            deliver,
        )
        .await
        .expect("cancelled session must end Ok");

        assert_eq!(delivered.load(Ordering::SeqCst), 2);
        assert_eq!(
            io.connect_count, 2,
            "one connect per session (stream end), not per message"
        );
        assert_eq!(
            io.subscribe_call_count, 2,
            "subscriptions replayed per session"
        );
    }
}
