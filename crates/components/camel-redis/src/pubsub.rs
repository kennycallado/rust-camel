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
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::retry::{retry_budget_exhausted, transient_retry_step};
use crate::topology::{RedisTopology, ServerKind};
use crate::transport_error::{TransientByProse, TransportTimeout, marker_camel};

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
            marker_camel(
                format!("PubSub connection timed out after {}s", self.timeout_secs),
                TransportTimeout {
                    stage: "pubsub connect",
                },
            )
        })?
        .map_err(|e| {
            marker_camel(
                format!("Failed to create PubSub connection: {}", e),
                TransientByProse {
                    site: "pubsub connect",
                },
            )
        })?;
        self.pubsub = Some(pubsub);
        Ok(())
    }

    async fn subscribe(&mut self, ch: &str) -> Result<(), CamelError> {
        let pubsub = self.pubsub.as_mut().ok_or_else(|| {
            marker_camel(
                "PubSub connection not established".into(),
                TransientByProse {
                    site: "pubsub guard",
                },
            )
        })?;
        pubsub.subscribe(ch).await.map_err(|e| {
            CamelError::ProcessorErrorWithSource(
                format!("Failed to subscribe to channel {}: {}", ch, e),
                Arc::new(e),
            )
        })
    }

    async fn psubscribe(&mut self, pat: &str) -> Result<(), CamelError> {
        let pubsub = self.pubsub.as_mut().ok_or_else(|| {
            marker_camel(
                "PubSub connection not established".into(),
                TransientByProse {
                    site: "pubsub guard",
                },
            )
        })?;
        pubsub.psubscribe(pat).await.map_err(|e| {
            CamelError::ProcessorErrorWithSource(
                format!("Failed to subscribe to pattern {}: {}", pat, e),
                Arc::new(e),
            )
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
///
/// `on_ready` fires once, immediately after the FIRST successful
/// `connect_and_subscribe` (every channel and pattern ack received), before
/// any message delivery. It does not re-fire on reconnect re-subscriptions,
/// and it is skipped when the session was cancelled mid-connect without ever
/// subscribing.
#[allow(clippy::too_many_arguments)] // session-driver parameter list prescribed by the task spec
pub(crate) async fn pubsub_session<D, F>(
    topology: &dyn RedisTopology,
    io: &mut dyn PubSubIo,
    channels: &[String],
    patterns: &[String],
    policy: &NetworkRetryPolicy,
    cancel: &CancellationToken,
    mut on_ready: Option<Box<dyn FnOnce() + Send>>,
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

        // First live, fully-subscribed connection: readiness fires only
        // now — signalling earlier opened a window in which start()
        // returned before the server had registered the SUBSCRIBE, so a
        // publish landing there was lost forever (rc-3ckqr). The cancel
        // guard skips a session that returned Ok early because it was
        // cancelled mid-connect without ever subscribing;
        // `Option::take` makes it fire-once across reconnect
        // re-subscriptions.
        if !cancel.is_cancelled()
            && let Some(f) = on_ready.take()
        {
            f();
        }

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
                                // audit (task 2.3): the only error construction
                                // inside the session loops; already structural —
                                // retry.rs attaches the
                                // TransientRetryBudgetExhausted marker.
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
#[path = "pubsub_tests.rs"]
mod tests;
