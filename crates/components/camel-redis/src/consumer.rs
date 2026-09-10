use async_trait::async_trait;
use camel_component_api::{Body, CamelError, Exchange, Message};
use camel_component_api::{
    ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode, RuntimeObservability,
};
use redis::Msg;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::{RedisCommand, RedisEndpointConfig, is_transient_redis_error};
use crate::pubsub::{PubSubIo, RedisPubSubIo, pubsub_session};
use crate::queue::{
    QueueConsumerParams, QueueIo, QueuePopCommand, RedisQueueIo, queue_command_name, queue_session,
};
use crate::topology::RedisTopology;

/// Mode of operation for the Redis consumer.
#[derive(Debug, Clone)]
pub enum RedisConsumerMode {
    /// Pub/Sub mode for real-time message streams.
    PubSub {
        /// Channels to subscribe to (SUBSCRIBE)
        channels: Vec<String>,
        /// Patterns to subscribe to (PSUBSCRIBE)
        patterns: Vec<String>,
    },
    /// Queue mode for blocking list operations.
    Queue {
        /// Key to watch for items
        key: String,
        /// Timeout in seconds for blocking pop
        timeout: u64,
        /// Blocking pop command to use (left or right)
        pop_command: QueuePopCommand,
    },
}

/// Redis consumer implementation supporting both Pub/Sub and Queue modes.
pub struct RedisConsumer {
    config: RedisEndpointConfig,
    mode: RedisConsumerMode,
    /// Topology used to resolve the current master (enables sentinel failover).
    topology: Arc<dyn RedisTopology>,
    /// Runtime observability for ADR-0012 metrics calls
    runtime: Arc<dyn RuntimeObservability>,
    /// Cancellation token for graceful shutdown
    cancel_token: Option<CancellationToken>,
    /// Handle to the spawned consumer task
    task_handle: Option<JoinHandle<Result<(), CamelError>>>,
}

impl RedisConsumer {
    /// Creates a new RedisConsumer with the given configuration.
    ///
    /// The mode is automatically determined from the command type in the config:
    /// - SUBSCRIBE → PubSub with channels
    /// - PSUBSCRIBE → PubSub with patterns
    /// - BLPOP/BRPOP → Queue mode
    ///
    /// Returns an error for commands that are not valid consumer commands
    /// (REDIS-003: no silent fallback to BLPOP).
    pub fn new(
        config: RedisEndpointConfig,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> Result<Self, CamelError> {
        let mode = match &config.command {
            RedisCommand::Subscribe => RedisConsumerMode::PubSub {
                channels: config.channels.clone(),
                patterns: vec![],
            },
            RedisCommand::Psubscribe => RedisConsumerMode::PubSub {
                channels: vec![],
                patterns: config.channels.clone(),
            },
            RedisCommand::Blpop | RedisCommand::Brpop => {
                let key = config.key.clone().unwrap_or_else(|| "queue".to_string());
                let pop_command = if config.command == RedisCommand::Brpop {
                    QueuePopCommand::Brpop
                } else {
                    QueuePopCommand::Blpop
                };
                RedisConsumerMode::Queue {
                    key,
                    timeout: config.timeout,
                    pop_command,
                }
            }
            other => {
                // REDIS-003: Return error instead of silent fallback
                return Err(CamelError::InvalidUri(format!(
                    "Invalid consumer command: {:?}. Only SUBSCRIBE, PSUBSCRIBE, BLPOP, and BRPOP are valid consumer commands.",
                    other
                )));
            }
        };

        // Build the topology from the endpoint config (same factory as the
        // producer) so the consumer re-resolves the master on failover.
        let topology = crate::topology::topology_from_config(&config)?;

        Ok(Self {
            config,
            mode,
            topology,
            runtime,
            cancel_token: None,
            task_handle: None,
        })
    }

    /// Test-only accessor exposing the resolved topology so tests can assert
    /// the standalone vs sentinel kind without a broker.
    #[cfg(test)]
    pub(crate) fn topology(&self) -> &Arc<dyn RedisTopology> {
        &self.topology
    }
}

#[async_trait]
impl Consumer for RedisConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // REDIS-016: Clean up any stale state from a previous stop before restarting
        if self.task_handle.is_some() {
            warn!("Consumer start called while task handle is still present; cleaning up first");
            // Cancel any existing token
            if let Some(token) = &self.cancel_token {
                token.cancel();
            }
            if let Some(handle) = self.task_handle.take() {
                match handle.await {
                    Ok(result) => {
                        if let Err(e) = result {
                            warn!(
                                "Previous consumer task exited with error during cleanup: {}",
                                e
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to join previous consumer task during cleanup: {}",
                            e
                        );
                    }
                }
            }
            self.cancel_token = None;
        }

        // Create cancellation token for this consumer: a child of the context
        // token, so a local stop() cancels only this consumer while route
        // stop still cascades through the parent link.
        let cancel_token = ctx.cancel_token().child_token();
        self.cancel_token = Some(cancel_token.clone());

        // Clone config and mode for the spawned task
        let config = self.config.clone();
        let mode = self.mode.clone();

        info!(
            endpoint = %config.safe_endpoint(),
            mode = ?mode,
            "Starting Redis consumer"
        );

        // Capture runtime for ADR-0012 metrics in spawned tasks
        let runtime = self.runtime.clone();
        let topology = self.topology.clone();
        let handle = match mode {
            RedisConsumerMode::PubSub { channels, patterns } => tokio::spawn(run_pubsub_consumer(
                config,
                channels,
                patterns,
                ctx,
                cancel_token,
                runtime,
                topology,
            )),
            RedisConsumerMode::Queue {
                key,
                timeout,
                pop_command,
            } => tokio::spawn(run_queue_consumer(
                config,
                QueueConsumerParams {
                    key,
                    timeout,
                    pop_command,
                },
                ctx,
                cancel_token,
                runtime,
                topology,
            )),
        };

        self.task_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        info!("Stopping Redis consumer");

        // Cancel the token to signal shutdown
        if let Some(token) = &self.cancel_token {
            token.cancel();
        }

        // Wait for the task to complete (safe to call multiple times — take() returns None on second call)
        if let Some(handle) = self.task_handle.take() {
            match handle.await {
                Ok(result) => {
                    if let Err(e) = result {
                        // log-policy: system-broken
                        error!("Consumer task exited with error: {}", e);
                    }
                }
                Err(e) => {
                    // log-policy: system-broken
                    error!("Failed to join consumer task: {}", e);
                }
            }
        }

        self.cancel_token = None;
        info!("Redis consumer stopped");
        Ok(())
    }

    /// Redis consumers are sequential by default to maintain message order.
    ///
    /// This default is chosen for the following reasons:
    /// - **Pub/Sub**: Messages often need ordering (e.g., event streams, notifications)
    /// - **Queue (BLPOP)**: Queue items should be processed in order
    /// - **Backpressure**: Sequential processing naturally applies backpressure
    ///   when the consumer is slower than the producer
    ///
    /// Users can override this with `.concurrent(n)` in the route DSL if they
    /// want parallel processing and ordering is not a concern.
    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    fn background_task_handle(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
        self.task_handle.take()
    }
}

/// Runs a Pub/Sub consumer loop.
///
/// Drives the [`pubsub_session`] failover core: the session holds ONE
/// connection and delivers every message on it, reconnecting (with
/// subscription replay) only on stream end or transient error. For each
/// delivered message this wrapper builds an Exchange and sends it through the
/// consumer context. On budget exhaustion the session returns `Err` so Route
/// supervision fires (ADR-0007).
///
/// Delivery is **best-effort**: messages published while disconnected are
/// lost, and a failover reconnect can re-deliver a message already handed to
/// the pipeline. Loss and duplicates are possible and expected.
async fn run_pubsub_consumer(
    config: RedisEndpointConfig,
    channels: Vec<String>,
    patterns: Vec<String>,
    ctx: ConsumerContext,
    cancel_token: CancellationToken,
    runtime: Arc<dyn RuntimeObservability>,
    topology: Arc<dyn RedisTopology>,
) -> Result<(), CamelError> {
    info!(endpoint = %config.safe_endpoint(), "PubSub consumer connecting");

    let mut io: Box<dyn PubSubIo> = Box::new(RedisPubSubIo::new(config.connection_timeout_secs));

    info!("PubSub consumer started, waiting for messages");

    let route_id = ctx.route_id().to_string();
    let route_id_err = route_id.clone();
    let runtime_err = Arc::clone(&runtime);
    // Readiness fires only after the first SUBSCRIBE/PSUBSCRIBE ack comes
    // back (pubsub_session's `on_ready`): signalling it eagerly let start()
    // return before the server registered the subscription, and a publish
    // landing in that window was lost — pubsub has no replay (rc-3ckqr).
    // Same clone-per-closure pattern as `deliver` below.
    let ready_ctx = ctx.clone();
    let on_ready = Some(Box::new(move || ready_ctx.mark_ready()) as Box<dyn FnOnce() + Send>);
    // Per-message delivery: build the Exchange and hand it to the pipeline.
    // A clone of `ctx` per message keeps the closure `Fn` (the session may
    // call it any number of times on one connection).
    let deliver = move |msg: Msg| {
        let ctx = ctx.clone();
        let runtime = Arc::clone(&runtime);
        let route_id = route_id.clone();
        let exchange = build_exchange_from_pubsub(msg);
        async move {
            if let Err(e) = ctx.send(exchange).await {
                runtime
                    .metrics()
                    .increment_errors(&route_id, "b-prime:redis:pubsub-channel-closed");
                // log-policy: outside-contract
                error!("Failed to send exchange to pipeline: {}", e);
                // Don't break - continue processing messages
            }
        }
    };

    tokio::select! {
        _ = cancel_token.cancelled() => {
            info!("PubSub consumer received shutdown signal");
        }
        result = pubsub_session(
            &*topology,
            &mut *io,
            &channels,
            &patterns,
            &config.reconnect,
            &cancel_token,
            on_ready,
            deliver,
        ) => {
            match result {
                Ok(()) => {
                    // Cancellation observed inside the session.
                    info!("PubSub consumer received shutdown signal");
                }
                Err(e) => {
                    if is_transient_redis_error(&e) {
                        // Budget exhaustion — the task ends, supervision fires (ADR-0007).
                        runtime_err
                            .metrics()
                            .increment_errors(&route_id_err, "e:redis:message-transient-budget");
                        // log-policy: system-broken
                        error!(
                            error = %e,
                            "PubSub consumer terminated after transient-error budget exhausted"
                        );
                    } else {
                        // Non-transient error — the route terminates and
                        // supervision restarts it (ADR-0007).
                        runtime_err
                            .metrics()
                            .increment_errors(&route_id_err, "e:redis:message-non-transient");
                        // log-policy: system-broken
                        error!(error = %e, "Non-transient error");
                    }
                    return Err(e);
                }
            }
        }
    }

    Ok(())
}

/// Runs a Queue consumer loop using BLPOP or BRPOP.
///
/// Drives the [`queue_session`] failover core: the session resolves the
/// master, connects once, and performs blocking pops on that single
/// connection, delivering every item through the wrapper. On a transient
/// error the session re-resolves the master and reconnects (enabling sentinel
/// failover); on budget exhaustion it returns `Err` so Route supervision
/// fires (ADR-0007).
async fn run_queue_consumer(
    config: RedisEndpointConfig,
    params: QueueConsumerParams,
    ctx: ConsumerContext,
    cancel_token: CancellationToken,
    runtime: Arc<dyn RuntimeObservability>,
    topology: Arc<dyn RedisTopology>,
) -> Result<(), CamelError> {
    let key = &params.key;
    let timeout = params.timeout;
    let pop_command = params.pop_command;
    let queue_cmd = queue_command_name(pop_command);
    info!(
        endpoint = %config.safe_endpoint(),
        key = %key,
        command = %queue_cmd,
        timeout_s = timeout,
        "Queue consumer connecting"
    );

    let mut io: Box<dyn QueueIo> = Box::new(RedisQueueIo::new(
        config.connection_timeout_secs,
        pop_command,
    ));

    info!("Queue consumer started, waiting for items");
    ctx.mark_ready();

    let route_id = ctx.route_id().to_string();
    let route_id_err = route_id.clone();
    let runtime_err = Arc::clone(&runtime);
    // Per-item delivery: build the Exchange and hand it to the pipeline.
    let deliver = move |(item_key, value): (String, String)| {
        let ctx = ctx.clone();
        let runtime = Arc::clone(&runtime);
        let route_id = route_id.clone();
        let exchange = build_exchange_from_blpop(item_key, value);
        async move {
            if let Err(e) = ctx.send(exchange).await {
                runtime
                    .metrics()
                    .increment_errors(&route_id, "b-prime:redis:blpop-channel-closed");
                // log-policy: outside-contract
                error!("Failed to send exchange to pipeline: {}", e);
                // Don't break - continue processing items
            }
        }
    };

    tokio::select! {
        _ = cancel_token.cancelled() => {
            info!("Queue consumer received shutdown signal");
        }
        result = queue_session(
            &*topology,
            &mut *io,
            &params,
            &config.reconnect,
            &cancel_token,
            deliver,
        ) => {
            match result {
                Ok(()) => {
                    // Cancellation observed inside the session.
                    info!("Queue consumer received shutdown signal");
                }
                Err(e) => {
                    if is_transient_redis_error(&e) {
                        // Budget exhaustion — the task ends, supervision fires (ADR-0007).
                        runtime_err
                            .metrics()
                            .increment_errors(&route_id_err, "e:redis:message-transient-budget");
                        // log-policy: system-broken
                        error!(
                            command = %queue_cmd,
                            error = %e,
                            "Queue consumer terminated after transient-error budget exhausted"
                        );
                    } else {
                        // Non-transient error — the route terminates and
                        // supervision restarts it (ADR-0007).
                        runtime_err
                            .metrics()
                            .increment_errors(&route_id_err, "e:redis:message-non-transient");
                        // log-policy: system-broken
                        error!(command = %queue_cmd, error = %e, "Non-transient error");
                    }
                    return Err(e);
                }
            }
        }
    }

    Ok(())
}

fn build_pubsub_exchange(payload: String, channel: String, pattern: Option<String>) -> Exchange {
    let mut exchange = Exchange::new(Message::new(Body::Text(payload)));
    exchange
        .input
        .set_header("CamelRedis.Channel", serde_json::Value::String(channel));

    if let Some(pattern) = pattern {
        exchange
            .input
            .set_header("CamelRedis.Pattern", serde_json::Value::String(pattern));
    }

    exchange
}

/// Builds an Exchange from a Pub/Sub message.
///
/// Sets the following headers:
/// - `CamelRedis.Channel`: The channel the message was published to
/// - `CamelRedis.Pattern`: The pattern matched (if applicable, for PSUBSCRIBE)
fn build_exchange_from_pubsub(msg: Msg) -> Exchange {
    let payload: String = msg
        .get_payload()
        .unwrap_or_else(|_| "<error decoding payload>".to_string());
    let channel = msg.get_channel_name().to_string();
    let pattern = if msg.from_pattern() {
        msg.get_pattern::<String>().ok()
    } else {
        None
    };

    build_pubsub_exchange(payload, channel, pattern)
}

/// Builds an Exchange from a BLPOP result.
///
/// Sets the following headers:
/// - `CamelRedis.Key`: The list key the item was popped from
fn build_exchange_from_blpop(key: String, value: String) -> Exchange {
    let mut exchange = Exchange::new(Message::new(Body::Text(value)));

    // Set key header
    exchange
        .input
        .set_header("CamelRedis.Key", serde_json::Value::String(key));

    exchange
}

#[cfg(test)]
#[path = "consumer_tests.rs"]
mod tests;
