//! Queue-mode consumer I/O seam and failover loop.
//!
//! Extracted from `consumer.rs` so the blocking-pop reconnect loop and its
//! test doubles live apart from the pub/sub and lifecycle code. The public
//! surface (`RedisConsumer`, `RedisConsumerMode`) stays in `consumer.rs`.

use async_trait::async_trait;
use camel_component_api::{CamelError, NetworkRetryPolicy};
use std::future::Future;
use std::ops::ControlFlow;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::retry::transient_retry_step;
use crate::topology::{RedisTopology, ServerKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePopCommand {
    Blpop,
    Brpop,
}

pub(crate) fn queue_command_name(pop_command: QueuePopCommand) -> &'static str {
    match pop_command {
        QueuePopCommand::Blpop => "BLPOP",
        QueuePopCommand::Brpop => "BRPOP",
    }
}

/// Injectable I/O seam for the queue consumer's blocking-pop loop.
///
/// Lets the failover loop be tested without a broker: the real impl talks to
/// Redis, a test double records programmable outcomes.
#[async_trait]
pub(crate) trait QueueIo: Send {
    /// Establish a multiplexed connection to `client`.
    async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError>;
    /// Blocking pop (BLPOP/BRPOP) on `key` with `timeout_secs`.
    async fn blpop(
        &mut self,
        key: &str,
        timeout_secs: u64,
    ) -> Result<Option<(String, String)>, CamelError>;
}

/// Real [`QueueIo`] backed by a multiplexed Redis connection.
pub(crate) struct RedisQueueIo {
    conn: Option<redis::aio::MultiplexedConnection>,
    timeout_secs: u64,
    pop_command: QueuePopCommand,
}

impl RedisQueueIo {
    pub(crate) fn new(timeout_secs: u64, pop_command: QueuePopCommand) -> Self {
        Self {
            conn: None,
            timeout_secs,
            pop_command,
        }
    }
}

#[async_trait]
impl QueueIo for RedisQueueIo {
    async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError> {
        let conn = tokio::time::timeout(
            Duration::from_secs(self.timeout_secs),
            client.get_multiplexed_async_connection(),
        )
        .await
        .map_err(|_| {
            CamelError::ProcessorError(format!(
                "Queue connection timed out after {}s",
                self.timeout_secs
            ))
        })?
        .map_err(|e| CamelError::ProcessorError(format!("Failed to create connection: {}", e)))?;
        self.conn = Some(conn);
        Ok(())
    }

    async fn blpop(
        &mut self,
        key: &str,
        timeout_secs: u64,
    ) -> Result<Option<(String, String)>, CamelError> {
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| CamelError::ProcessorError("Queue connection not established".into()))?;
        let cmd = redis::cmd(queue_command_name(self.pop_command))
            .arg(key)
            .arg(timeout_secs)
            .to_owned();
        cmd.query_async::<Option<(String, String)>>(conn)
            .await
            .map_err(|e| CamelError::ProcessorError(e.to_string()))
    }
}

/// Parameters for the queue consumer loop.
pub(crate) struct QueueConsumerParams {
    pub(crate) key: String,
    pub(crate) timeout: u64,
    pub(crate) pop_command: QueuePopCommand,
}

/// Run a whole queue session on ONE connection.
///
/// Resolves the current master, connects, and performs blocking pops on that
/// single connection, delivering every popped item through `deliver` — it does
/// not reconnect between items. A blocking-pop timeout (`Ok(None)` — no item
/// arrived within `timeout_secs`) is normal and KEEPS the connection.
///
/// On a transient error (connection refused/EOF/timeout/reset/readonly) at any
/// stage the loop re-resolves the master and reconnects, bounded by `policy`;
/// each such failure consumes one attempt. A non-transient error, or budget
/// exhaustion, returns `Err` so Route supervision fires (ADR-0007).
pub(crate) async fn queue_session<D, F>(
    topology: &dyn RedisTopology,
    io: &mut dyn QueueIo,
    params: &QueueConsumerParams,
    policy: &NetworkRetryPolicy,
    cancel: &CancellationToken,
    mut deliver: D,
) -> Result<(), CamelError>
where
    D: FnMut((String, String)) -> F,
    F: Future<Output = ()>,
{
    let key = params.key.as_str();
    let timeout_secs = params.timeout;
    let command_name = queue_command_name(params.pop_command);
    let resolve_stage = format!("resolving master for {command_name}");
    let connect_stage = format!("connecting for {command_name}");
    let pop_stage = format!("popping with {command_name}");
    let mut attempt: u32 = 0;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        // Resolve the current master (re-resolves on every call for sentinel).
        let client = match topology.resolve(ServerKind::Master).await {
            Ok(client) => client,
            Err(e) => match transient_retry_step(policy, &mut attempt, e, &resolve_stage).await {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(e) => return Err(e),
            },
        };

        // Establish a fresh connection to the resolved master.
        if let Err(e) = io.connect(&client).await {
            match transient_retry_step(policy, &mut attempt, e, &connect_stage).await {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(e) => return Err(e),
            }
        }

        // Pop and deliver from this one connection until a transient error
        // forces a reconnect.
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                popped = io.blpop(key, timeout_secs) => {
                    match popped {
                        Ok(Some(item)) => deliver(item).await,
                        Ok(None) => continue, // blocking-pop timeout — stay on this connection
                        Err(e) => {
                            match transient_retry_step(policy, &mut attempt, e, &pop_stage).await {
                                ControlFlow::Continue(()) => break, // → reconnect
                                ControlFlow::Break(e) => return Err(e),
                            }
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
    use crate::config::is_transient_redis_error;
    use crate::topology::FakeTopology;

    /// What `blpop` does once the current connection's outcomes have been
    /// consumed AND no further outcome batches remain.
    enum TailBehavior {
        /// Reuse the last outcome (original default).
        ReuseLast,
        /// Pend forever — the connection stays open, blocking indefinitely.
        Pend,
    }

    type PopOutcome = Result<Option<(String, String)>, CamelError>;

    /// Test double for [`QueueIo`]: programmable connect/blpop outcomes,
    /// recording `connect_count` and `blpop_count`.
    ///
    /// Two programming modes:
    /// - **Legacy** (`new(connect_outcomes, blpop_outcomes)`): one shared
    ///   blpop outcome queue; when exhausted, the last outcome repeats.
    /// - **Per-connection** (`with_blpop_per_connect(batches)`): `connect`
    ///   installs the next outcome batch; when the LAST batch drains, `blpop`
    ///   pends forever (open-but-idle connection), so session tests can end
    ///   via cancellation without inventing extra items.
    struct FakeQueueIo {
        connect_outcomes: Vec<Result<(), CamelError>>,
        blpop_queue: Vec<PopOutcome>,
        blpop_batches: Vec<Vec<PopOutcome>>,
        last_outcome: Option<PopOutcome>,
        tail: TailBehavior,
        connect_count: usize,
        blpop_count: usize,
    }

    impl FakeQueueIo {
        fn new(
            connect_outcomes: Vec<Result<(), CamelError>>,
            blpop_outcomes: Vec<PopOutcome>,
        ) -> Self {
            Self {
                connect_outcomes,
                last_outcome: blpop_outcomes.last().cloned(),
                blpop_queue: blpop_outcomes,
                blpop_batches: Vec::new(),
                tail: TailBehavior::ReuseLast,
                connect_count: 0,
                blpop_count: 0,
            }
        }

        /// One blpop-outcome batch per connection; once the LAST batch drains,
        /// `blpop` pends forever (open-but-idle connection).
        fn with_blpop_per_connect(mut self, batches: Vec<Vec<PopOutcome>>) -> Self {
            self.blpop_batches = batches;
            self.tail = TailBehavior::Pend;
            self
        }

        fn connect_count(&self) -> usize {
            self.connect_count
        }
    }

    #[async_trait]
    impl QueueIo for FakeQueueIo {
        async fn connect(&mut self, _client: &redis::Client) -> Result<(), CamelError> {
            let idx = self.connect_count;
            self.connect_count += 1;
            let outcome = match self.connect_outcomes.get(idx) {
                Some(o) => o.clone(),
                None => self.connect_outcomes.last().cloned().unwrap_or(Ok(())),
            };
            // Per-connection mode: install this connection's outcome batch,
            // but only when the connect succeeds — a failed connect never
            // reaches blpop, so its batch must stay queued for the next one.
            if outcome.is_ok() && !self.blpop_batches.is_empty() {
                self.blpop_queue = self.blpop_batches.remove(0);
                self.last_outcome = self.blpop_queue.last().cloned();
            }
            outcome
        }

        async fn blpop(&mut self, _key: &str, _timeout_secs: u64) -> PopOutcome {
            self.blpop_count += 1;
            if !self.blpop_queue.is_empty() {
                return self.blpop_queue.remove(0);
            }
            match self.tail {
                TailBehavior::ReuseLast => self.last_outcome.clone().unwrap_or(Ok(None)),
                TailBehavior::Pend => std::future::pending().await,
            }
        }
    }

    fn item(v: &str) -> Result<Option<(String, String)>, CamelError> {
        Ok(Some(("k".to_string(), v.to_string())))
    }

    fn fast_policy(max_attempts: u32) -> NetworkRetryPolicy {
        NetworkRetryPolicy {
            max_attempts,
            initial_delay: Duration::from_millis(0),
            ..NetworkRetryPolicy::default()
        }
    }

    fn queue_params() -> QueueConsumerParams {
        QueueConsumerParams {
            key: "queue".to_string(),
            timeout: 1,
            pop_command: QueuePopCommand::Blpop,
        }
    }

    // Deterministic failover test — no broker. First connect fails transiently,
    // second succeeds; the loop re-resolves the master and delivers the item.
    #[tokio::test]
    async fn queue_recovers_after_connection_loss() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into(), "redis://b:6379".into()]);
        let mut io = FakeQueueIo::new(
            vec![
                Err(CamelError::ProcessorError("connection reset".into())),
                Ok(()),
            ],
            vec![],
        )
        .with_blpop_per_connect(vec![vec![item("v")]]);
        let cancel = CancellationToken::new();

        let delivered = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = delivered.clone();
        let cancel_in_deliver = cancel.clone();
        let deliver = move |(_k, v): (String, String)| {
            let count = count.clone();
            let cancel = cancel_in_deliver.clone();
            async move {
                assert_eq!(v, "v");
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                cancel.cancel();
            }
        };

        queue_session(
            &topology,
            &mut io,
            &queue_params(),
            &fast_policy(2),
            &cancel,
            deliver,
        )
        .await
        .expect("session must end Ok after cancellation");

        assert_eq!(delivered.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            topology.resolve_call_count() >= 2,
            "master should be re-resolved after connection loss"
        );
        assert_eq!(io.connect_count(), 2);
    }

    // Deterministic failover test — no broker. Connect always fails transiently;
    // the loop exhausts the budget and returns Err (supervision fires).
    #[tokio::test]
    async fn queue_returns_err_when_failover_budget_exhausted() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeQueueIo::new(
            vec![Err(CamelError::ProcessorError("connection refused".into()))],
            vec![],
        );
        let cancel = CancellationToken::new();

        let result = queue_session(
            &topology,
            &mut io,
            &queue_params(),
            &fast_policy(3),
            &cancel,
            |_| async {},
        )
        .await;

        assert!(result.is_err());
        assert_eq!(io.connect_count(), 3);
    }

    // ADR-0012 observability: a budget-exhaustion error must classify as
    // transient so the consumer's Err-branch routing fires the transient-budget
    // metric, not `e:redis:message-non-transient`.
    #[tokio::test]
    async fn queue_budget_exhaustion_classified_transient() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeQueueIo::new(
            vec![Ok(())],
            vec![Err(CamelError::ProcessorError("connection reset".into()))],
        );
        let cancel = CancellationToken::new();

        let result = queue_session(
            &topology,
            &mut io,
            &queue_params(),
            &fast_policy(1),
            &cancel,
            |_| async {},
        )
        .await;

        let err = result.expect_err("blpop budget exhaustion must return Err");
        assert!(
            is_transient_redis_error(&err),
            "budget-exhaustion error must classify as transient: {}",
            err
        );
    }

    // Deterministic failover test — no broker. The master drops mid-BLPOP
    // (transient error); the loop re-resolves the master, reconnects, and
    // delivers the item from the new connection. This is the primary runtime
    // scenario for sentinel failover.
    #[tokio::test]
    async fn queue_recovers_after_blpop_transient_error() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into(), "redis://b:6379".into()]);
        let mut io = FakeQueueIo::new(vec![Ok(()), Ok(())], vec![]).with_blpop_per_connect(vec![
            vec![Err(CamelError::ProcessorError("connection reset".into()))],
            vec![item("v")],
        ]);
        let cancel = CancellationToken::new();

        let delivered = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = delivered.clone();
        let cancel_in_deliver = cancel.clone();
        let deliver = move |(_k, v): (String, String)| {
            let count = count.clone();
            let cancel = cancel_in_deliver.clone();
            async move {
                assert_eq!(v, "v");
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                cancel.cancel();
            }
        };

        queue_session(
            &topology,
            &mut io,
            &queue_params(),
            &fast_policy(2),
            &cancel,
            deliver,
        )
        .await
        .expect("session must end Ok after cancellation");

        assert_eq!(delivered.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            topology.resolve_call_count() >= 2,
            "master should be re-resolved after blpop transient error"
        );
        assert_eq!(io.connect_count(), 2);
    }

    // C1 regression: a whole session of N items must be delivered over ONE
    // connection. The previous one-item-per-connection structure called
    // `connect` (topology resolve + TCP + AUTH) for EVERY popped item; this
    // test pins connect_count == 1 while all N items reach the delivery
    // callback.
    #[tokio::test]
    async fn queue_delivers_many_items_on_one_connection() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeQueueIo::new(vec![Ok(())], vec![]).with_blpop_per_connect(vec![vec![
            item("v1"),
            item("v2"),
            item("v3"),
        ]]);
        let cancel = CancellationToken::new();

        let values = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = values.clone();
        let cancel_in_deliver = cancel.clone();
        let deliver = move |(_k, v): (String, String)| {
            let seen = seen.clone();
            let cancel = cancel_in_deliver.clone();
            async move {
                let mut values = seen.lock().expect("values mutex");
                values.push(v);
                // End the session after the last item: the fake's next `blpop`
                // pends, so cancellation resolves deterministically.
                if values.len() == 3 {
                    cancel.cancel();
                }
            }
        };

        queue_session(
            &topology,
            &mut io,
            &queue_params(),
            &fast_policy(10),
            &cancel,
            deliver,
        )
        .await
        .expect("cancelled session must end Ok");

        assert_eq!(
            *values.lock().expect("values mutex"),
            vec!["v1".to_string(), "v2".to_string(), "v3".to_string()],
            "all items must be delivered, in order"
        );
        assert_eq!(
            io.connect_count(),
            1,
            "C1 regression: the whole session must use exactly one connection"
        );
        // 3 item pops, plus (racily) one final poll that pends until the
        // cancel branch of the session's select wins.
        assert!(io.blpop_count >= 3);
    }

    // C1 companion: blocking-pop timeouts must NOT reconnect. A timeout means
    // no item arrived — the connection is healthy. The previous structure
    // returned to the outer loop per timeout and reconnected each time; this
    // test pins connect_count == 1 across timeouts and a later item.
    #[tokio::test]
    async fn queue_blocking_pop_timeout_keeps_connection() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeQueueIo::new(vec![Ok(())], vec![]).with_blpop_per_connect(vec![vec![
            Ok(None), // timeout
            Ok(None), // timeout
            Ok(None), // timeout
            item("v1"),
        ]]);
        let cancel = CancellationToken::new();

        let delivered = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = delivered.clone();
        let cancel_in_deliver = cancel.clone();
        let deliver = move |(_k, _v): (String, String)| {
            let count = count.clone();
            let cancel = cancel_in_deliver.clone();
            async move {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                cancel.cancel();
            }
        };

        queue_session(
            &topology,
            &mut io,
            &queue_params(),
            &fast_policy(10),
            &cancel,
            deliver,
        )
        .await
        .expect("cancelled session must end Ok");

        assert_eq!(delivered.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            io.connect_count(),
            1,
            "blocking-pop timeouts must keep the same connection"
        );
        // 3 timeouts + 1 item, plus (racily) one final poll that pends until
        // the cancel branch of the session's select wins.
        assert!(io.blpop_count >= 4);
    }
}
