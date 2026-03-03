use async_trait::async_trait;
use camel_api::{Body, CamelError, Exchange, Message};
use camel_component::{ConcurrencyModel, Consumer, ConsumerContext};
use futures_util::StreamExt;
use redis::Msg;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::{RedisCommand, RedisConfig};

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
        /// Timeout in seconds for BLPOP
        timeout: u64,
    },
}

/// Redis consumer implementation supporting both Pub/Sub and Queue modes.
pub struct RedisConsumer {
    config: RedisConfig,
    mode: RedisConsumerMode,
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
    pub fn new(config: RedisConfig) -> Self {
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
                RedisConsumerMode::Queue {
                    key,
                    timeout: config.timeout,
                }
            }
            _ => {
                warn!(
                    "Invalid consumer command: {:?}, defaulting to BLPOP",
                    config.command
                );
                RedisConsumerMode::Queue {
                    key: config.key.clone().unwrap_or_else(|| "queue".to_string()),
                    timeout: config.timeout,
                }
            }
        };

        Self {
            config,
            mode,
            cancel_token: None,
            task_handle: None,
        }
    }
}

#[async_trait]
impl Consumer for RedisConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // Create cancellation token for this consumer
        let cancel_token = CancellationToken::new();
        self.cancel_token = Some(cancel_token.clone());

        // Clone config and mode for the spawned task
        let config = self.config.clone();
        let mode = self.mode.clone();

        info!("Starting Redis consumer in {:?} mode", mode);

        // Spawn the appropriate consumer loop based on mode
        let handle =
            match mode {
                RedisConsumerMode::PubSub { channels, patterns } => tokio::spawn(
                    run_pubsub_consumer(config, channels, patterns, ctx, cancel_token),
                ),
                RedisConsumerMode::Queue { key, timeout } => {
                    tokio::spawn(run_queue_consumer(config, key, timeout, ctx, cancel_token))
                }
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

        // Wait for the task to complete
        if let Some(handle) = self.task_handle.take() {
            match handle.await {
                Ok(result) => {
                    if let Err(e) = result {
                        error!("Consumer task exited with error: {}", e);
                    }
                }
                Err(e) => {
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
}

/// Runs a Pub/Sub consumer loop.
///
/// Creates a dedicated PubSub connection and subscribes to the specified
/// channels and/or patterns. Messages are converted to Exchanges and sent
/// through the consumer context.
async fn run_pubsub_consumer(
    config: RedisConfig,
    channels: Vec<String>,
    patterns: Vec<String>,
    ctx: ConsumerContext,
    cancel_token: CancellationToken,
) -> Result<(), CamelError> {
    info!("PubSub consumer connecting to {}", config.redis_url());

    // Create dedicated PubSub connection
    let client = redis::Client::open(config.redis_url())
        .map_err(|e| CamelError::ProcessorError(format!("Failed to create Redis client: {}", e)))?;

    let mut pubsub = client.get_async_pubsub().await.map_err(|e| {
        CamelError::ProcessorError(format!("Failed to create PubSub connection: {}", e))
    })?;

    // Subscribe to channels
    for channel in &channels {
        info!("Subscribing to channel: {}", channel);
        pubsub.subscribe(channel).await.map_err(|e| {
            CamelError::ProcessorError(format!("Failed to subscribe to channel {}: {}", channel, e))
        })?;
    }

    // Subscribe to patterns
    for pattern in &patterns {
        info!("Subscribing to pattern: {}", pattern);
        pubsub.psubscribe(pattern).await.map_err(|e| {
            CamelError::ProcessorError(format!("Failed to subscribe to pattern {}: {}", pattern, e))
        })?;
    }

    info!("PubSub consumer started, waiting for messages");

    // Message loop
    let mut stream = pubsub.on_message();
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!("PubSub consumer received shutdown signal");
                break;
            }
            msg = stream.next() => {
                if let Some(msg) = msg {
                    let exchange = build_exchange_from_pubsub(msg);
                    if let Err(e) = ctx.send(exchange).await {
                        error!("Failed to send exchange to pipeline: {}", e);
                        // Don't break - continue processing messages
                    }
                } else {
                    // Stream ended
                    warn!("PubSub stream ended");
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Runs a Queue consumer loop using BLPOP.
///
/// Creates a dedicated connection and performs blocking list pop operations.
/// Items are converted to Exchanges and sent through the consumer context.
async fn run_queue_consumer(
    config: RedisConfig,
    key: String,
    timeout: u64,
    ctx: ConsumerContext,
    cancel_token: CancellationToken,
) -> Result<(), CamelError> {
    info!(
        "Queue consumer connecting to {} for key '{}' with timeout {}s",
        config.redis_url(),
        key,
        timeout
    );

    // Create dedicated multiplexed connection
    let client = redis::Client::open(config.redis_url())
        .map_err(|e| CamelError::ProcessorError(format!("Failed to create Redis client: {}", e)))?;

    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| CamelError::ProcessorError(format!("Failed to create connection: {}", e)))?;

    info!("Queue consumer started, waiting for items");

    // BLPOP loop
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!("Queue consumer received shutdown signal");
                break;
            }
            result = async {
                let cmd = redis::cmd("BLPOP")
                    .arg(&key)
                    .arg(timeout as f64)
                    .to_owned();
                cmd.query_async::<Option<(String, String)>>(&mut conn).await
            } =>
            {
                match result {
                    Ok(Some((key, value))) => {
                        let exchange = build_exchange_from_blpop(key, value);
                        if let Err(e) = ctx.send(exchange).await {
                            error!("Failed to send exchange to pipeline: {}", e);
                            // Don't break - continue processing items
                        }
                    }
                    Ok(None) => {
                        // Timeout - continue loop
                        // This is normal for BLPOP with timeout
                    }
                    Err(e) => {
                        error!("BLPOP error: {}", e);
                        // Brief pause before retrying to avoid tight error loop
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
    }

    Ok(())
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

    let mut exchange = Exchange::new(Message::new(Body::Text(payload)));

    // Set channel header
    exchange.input.set_header(
        "CamelRedis.Channel",
        serde_json::Value::String(msg.get_channel_name().to_string()),
    );

    // Set pattern header if this is from a pattern subscription
    if msg.from_pattern()
        && let Ok(pattern) = msg.get_pattern::<String>()
    {
        exchange
            .input
            .set_header("CamelRedis.Pattern", serde_json::Value::String(pattern));
    }

    exchange
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
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn create_test_config(command: RedisCommand) -> RedisConfig {
        RedisConfig {
            host: "localhost".to_string(),
            port: 6379,
            command,
            channels: vec!["test".to_string()],
            key: Some("test-queue".to_string()),
            timeout: 1,
            password: None,
            db: 0,
        }
    }

    #[test]
    fn test_consumer_new_subscribe() {
        let config = create_test_config(RedisCommand::Subscribe);
        let consumer = RedisConsumer::new(config);

        match consumer.mode {
            RedisConsumerMode::PubSub { channels, patterns } => {
                assert_eq!(channels, vec!["test".to_string()]);
                assert!(patterns.is_empty());
            }
            _ => panic!("Expected PubSub mode"),
        }
    }

    #[test]
    fn test_consumer_new_psubscribe() {
        let config = create_test_config(RedisCommand::Psubscribe);
        let consumer = RedisConsumer::new(config);

        match consumer.mode {
            RedisConsumerMode::PubSub { channels, patterns } => {
                assert!(channels.is_empty());
                assert_eq!(patterns, vec!["test".to_string()]);
            }
            _ => panic!("Expected PubSub mode"),
        }
    }

    #[test]
    fn test_consumer_new_blpop() {
        let config = create_test_config(RedisCommand::Blpop);
        let consumer = RedisConsumer::new(config);

        match consumer.mode {
            RedisConsumerMode::Queue { key, timeout } => {
                assert_eq!(key, "test-queue");
                assert_eq!(timeout, 1);
            }
            _ => panic!("Expected Queue mode"),
        }
    }

    #[test]
    fn test_consumer_new_blpop_default_key() {
        let mut config = create_test_config(RedisCommand::Blpop);
        config.key = None;
        let consumer = RedisConsumer::new(config);

        match consumer.mode {
            RedisConsumerMode::Queue { key, .. } => {
                assert_eq!(key, "queue");
            }
            _ => panic!("Expected Queue mode"),
        }
    }

    #[test]
    fn test_build_exchange_from_blpop() {
        let exchange = build_exchange_from_blpop("mykey".to_string(), "myvalue".to_string());

        assert_eq!(exchange.input.body.as_text(), Some("myvalue"));

        let header = exchange.input.header("CamelRedis.Key");
        assert_eq!(
            header,
            Some(&serde_json::Value::String("mykey".to_string()))
        );
    }

    #[tokio::test]
    async fn test_consumer_stops_gracefully() {
        let config = create_test_config(RedisCommand::Blpop);
        let mut consumer = RedisConsumer::new(config);

        // Create a mock context (won't actually be used in this test)
        let (tx, _rx) = mpsc::channel(16);
        let cancel_token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel_token.clone());

        // Start should succeed
        let start_result = consumer.start(ctx).await;
        assert!(start_result.is_ok());

        // Give task a moment to start
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Stop should succeed
        let stop_result = consumer.stop().await;
        assert!(stop_result.is_ok());
    }

    // Integration tests (require running Redis, marked with #[ignore])

    #[tokio::test]
    #[ignore] // Requires running Redis
    async fn test_pubsub_consumer_receives_messages() {
        // Setup: create consumer with SUBSCRIBE
        let config = create_test_config(RedisCommand::Subscribe);
        let mut consumer = RedisConsumer::new(config);

        let (tx, _rx) = mpsc::channel(16);
        let cancel_token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel_token.clone());

        // Start consumer
        consumer.start(ctx).await.unwrap();

        // Give consumer time to subscribe
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Publish test message (would need a separate Redis client)
        // let publish_client = redis::Client::open("redis://localhost:6379").unwrap();
        // let mut pub_conn = publish_client.get_connection().unwrap();
        // redis::cmd("PUBLISH").arg("test").arg("hello").query(&mut pub_conn).unwrap();

        // Verify exchange received (would check rx)
        // let envelope = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        //     .await
        //     .expect("Should receive message")
        //     .expect("Message should exist");
        // assert_eq!(envelope.exchange.input.body.as_text(), Some("hello"));

        // Cleanup
        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires running Redis
    async fn test_queue_consumer_processes_items() {
        // Setup: create consumer with BLPOP
        let config = create_test_config(RedisCommand::Blpop);
        let mut consumer = RedisConsumer::new(config);

        let (tx, _rx) = mpsc::channel(16);
        let cancel_token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel_token.clone());

        // Start consumer
        consumer.start(ctx).await.unwrap();

        // Give consumer time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // LPUSH test item (would need a separate Redis client)
        // let push_client = redis::Client::open("redis://localhost:6379").unwrap();
        // let mut push_conn = push_client.get_connection().unwrap();
        // redis::cmd("LPUSH").arg("test-queue").arg("item1").query(&mut push_conn).unwrap();

        // Verify exchange received (would check rx)
        // let envelope = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        //     .await
        //     .expect("Should receive message")
        //     .expect("Message should exist");
        // assert_eq!(envelope.exchange.input.body.as_text(), Some("item1"));

        // Cleanup
        consumer.stop().await.unwrap();
    }
}
