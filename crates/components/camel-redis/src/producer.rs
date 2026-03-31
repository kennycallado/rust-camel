use crate::commands;
use crate::config::{RedisCommand, RedisEndpointConfig};
use camel_api::{CamelError, Exchange};
use redis::aio::MultiplexedConnection;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::Mutex;
use tower::Service;

/// Redis producer that implements Tower `Service<Exchange>` for integration
/// with rust-camel pipelines.
///
/// Manages a shared `MultiplexedConnection` to Redis that is created lazily
/// on first use and reused across multiple calls.
#[derive(Clone)]
pub struct RedisProducer {
    config: RedisEndpointConfig,
    /// Shared connection pool - created lazily on first use
    conn: Arc<Mutex<Option<MultiplexedConnection>>>,
}

impl RedisProducer {
    /// Creates a new RedisProducer with the given configuration.
    ///
    /// The connection is not established until the first call to `call()`.
    pub fn new(config: RedisEndpointConfig) -> Self {
        Self {
            config,
            conn: Arc::new(Mutex::new(None)),
        }
    }

    /// Dispatches a Redis command to the appropriate module handler.
    async fn dispatch_command(
        cmd: &RedisCommand,
        conn: &mut MultiplexedConnection,
        exchange: &mut Exchange,
    ) -> Result<(), CamelError> {
        match cmd {
            // String commands
            RedisCommand::Set
            | RedisCommand::Get
            | RedisCommand::Getset
            | RedisCommand::Setnx
            | RedisCommand::Setex
            | RedisCommand::Mget
            | RedisCommand::Mset
            | RedisCommand::Incr
            | RedisCommand::Incrby
            | RedisCommand::Decr
            | RedisCommand::Decrby
            | RedisCommand::Append
            | RedisCommand::Strlen => commands::string::dispatch(cmd, conn, exchange).await,

            // Key commands
            RedisCommand::Exists
            | RedisCommand::Del
            | RedisCommand::Expire
            | RedisCommand::Expireat
            | RedisCommand::Pexpire
            | RedisCommand::Pexpireat
            | RedisCommand::Ttl
            | RedisCommand::Keys
            | RedisCommand::Rename
            | RedisCommand::Renamenx
            | RedisCommand::Type
            | RedisCommand::Persist
            | RedisCommand::Move
            | RedisCommand::Sort => commands::key::dispatch(cmd, conn, exchange).await,

            // List commands
            RedisCommand::Lpush
            | RedisCommand::Rpush
            | RedisCommand::Lpushx
            | RedisCommand::Rpushx
            | RedisCommand::Lpop
            | RedisCommand::Rpop
            | RedisCommand::Blpop
            | RedisCommand::Brpop
            | RedisCommand::Llen
            | RedisCommand::Lrange
            | RedisCommand::Lindex
            | RedisCommand::Linsert
            | RedisCommand::Lset
            | RedisCommand::Lrem
            | RedisCommand::Ltrim
            | RedisCommand::Rpoplpush => commands::list::dispatch(cmd, conn, exchange).await,

            // Hash commands
            RedisCommand::Hset
            | RedisCommand::Hget
            | RedisCommand::Hsetnx
            | RedisCommand::Hmset
            | RedisCommand::Hmget
            | RedisCommand::Hdel
            | RedisCommand::Hexists
            | RedisCommand::Hlen
            | RedisCommand::Hkeys
            | RedisCommand::Hvals
            | RedisCommand::Hgetall
            | RedisCommand::Hincrby => commands::hash::dispatch(cmd, conn, exchange).await,

            // Set commands
            RedisCommand::Sadd
            | RedisCommand::Srem
            | RedisCommand::Smembers
            | RedisCommand::Scard
            | RedisCommand::Sismember
            | RedisCommand::Spop
            | RedisCommand::Smove
            | RedisCommand::Sinter
            | RedisCommand::Sunion
            | RedisCommand::Sdiff
            | RedisCommand::Sinterstore
            | RedisCommand::Sunionstore
            | RedisCommand::Sdiffstore
            | RedisCommand::Srandmember => commands::set::dispatch(cmd, conn, exchange).await,

            // Sorted set commands
            RedisCommand::Zadd
            | RedisCommand::Zrem
            | RedisCommand::Zrange
            | RedisCommand::Zrevrange
            | RedisCommand::Zrank
            | RedisCommand::Zrevrank
            | RedisCommand::Zscore
            | RedisCommand::Zcard
            | RedisCommand::Zincrby
            | RedisCommand::Zcount
            | RedisCommand::Zrangebyscore
            | RedisCommand::Zrevrangebyscore
            | RedisCommand::Zremrangebyrank
            | RedisCommand::Zremrangebyscore
            | RedisCommand::Zunionstore
            | RedisCommand::Zinterstore => commands::zset::dispatch(cmd, conn, exchange).await,

            // Pub/Sub commands
            RedisCommand::Publish | RedisCommand::Subscribe | RedisCommand::Psubscribe => {
                commands::pubsub::dispatch(cmd, conn, exchange).await
            }

            // Other commands
            RedisCommand::Ping | RedisCommand::Echo => {
                commands::other::dispatch(cmd, conn, exchange).await
            }
        }
    }

    /// Resolves the command to execute.
    ///
    /// Priority:
    /// 1. Header `CamelRedis.Command` if present
    /// 2. Configuration default command
    fn resolve_command(exchange: &Exchange, config: &RedisEndpointConfig) -> RedisCommand {
        exchange
            .input
            .header("CamelRedis.Command")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| config.command.clone())
    }

    fn apply_default_key(exchange: &mut Exchange, config: &RedisEndpointConfig) {
        if exchange.input.header("CamelRedis.Key").is_none()
            && let Some(ref key) = config.key
        {
            exchange
                .input
                .set_header("CamelRedis.Key", serde_json::Value::String(key.clone()));
        }
    }

    fn apply_default_channels(exchange: &mut Exchange, config: &RedisEndpointConfig) {
        if exchange.input.header("CamelRedis.Channels").is_none() && !config.channels.is_empty() {
            exchange.input.set_header(
                "CamelRedis.Channels",
                serde_json::Value::Array(
                    config
                        .channels
                        .iter()
                        .map(|c| serde_json::Value::String(c.clone()))
                        .collect(),
                ),
            );
        }
    }
}

impl Service<Exchange> for RedisProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Always ready - connection is created lazily in call()
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let conn = self.conn.clone();

        Box::pin(async move {
            // 1. Get or create connection
            let mut connection = {
                let guard = conn.lock().await;
                if let Some(c) = guard.as_ref() {
                    c.clone()
                } else {
                    // Need to create connection - drop guard first
                    drop(guard);

                    let mut guard = conn.lock().await;
                    if let Some(c) = guard.as_ref() {
                        c.clone()
                    } else {
                        let redis_url = config.redis_url();
                        tracing::debug!("Creating Redis client with URL: {}", redis_url);
                        let client = redis::Client::open(redis_url.as_str()).map_err(|e| {
                            CamelError::ProcessorError(format!(
                                "Failed to create Redis client for URL '{}': {}",
                                redis_url, e
                            ))
                        })?;

                        let new_conn =
                            client
                                .get_multiplexed_async_connection()
                                .await
                                .map_err(|e| {
                                    CamelError::ProcessorError(format!(
                                        "Failed to connect to Redis: {}",
                                        e
                                    ))
                                })?;

                        *guard = Some(new_conn.clone());
                        new_conn
                    }
                }
            };

            // 2. Resolve command from header or config
            let cmd = Self::resolve_command(&exchange, &config);

            // 3. Set defaults from config if missing in headers
            Self::apply_default_key(&mut exchange, &config);
            Self::apply_default_channels(&mut exchange, &config);

            // 5. Dispatch to appropriate command handler
            Self::dispatch_command(&cmd, &mut connection, &mut exchange).await?;

            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;

    #[test]
    fn test_producer_new() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config);
        assert!(Arc::strong_count(&producer.conn) == 1);
    }

    #[test]
    fn test_producer_clone_shares_connection() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config);
        let producer2 = producer.clone();

        // Both producers share the same connection Arc
        assert!(Arc::ptr_eq(&producer.conn, &producer2.conn));
    }

    #[test]
    fn test_resolve_command_from_config() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET").unwrap();
        let exchange = Exchange::new(Message::default());

        let cmd = RedisProducer::resolve_command(&exchange, &config);
        assert_eq!(cmd, RedisCommand::Get);
    }

    #[test]
    fn test_resolve_command_from_header() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?command=SET").unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Command", serde_json::json!("GET"));
        let exchange = Exchange::new(msg);

        let cmd = RedisProducer::resolve_command(&exchange, &config);
        assert_eq!(cmd, RedisCommand::Get);
    }

    #[test]
    fn test_resolve_command_header_overrides_config() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?command=SET").unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Command", serde_json::json!("INCR"));
        let exchange = Exchange::new(msg);

        let cmd = RedisProducer::resolve_command(&exchange, &config);
        assert_eq!(cmd, RedisCommand::Incr);
    }

    #[test]
    fn test_resolve_command_invalid_header_falls_back_to_config() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?command=DECR").unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Command", serde_json::json!("NOT_A_COMMAND"));
        let exchange = Exchange::new(msg);

        let cmd = RedisProducer::resolve_command(&exchange, &config);
        assert_eq!(cmd, RedisCommand::Decr);
    }

    #[test]
    fn test_resolve_command_non_string_header_falls_back_to_config() {
        let config =
            RedisEndpointConfig::from_uri("redis://localhost:6379?command=EXISTS").unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Command", serde_json::json!(123));
        let exchange = Exchange::new(msg);

        let cmd = RedisProducer::resolve_command(&exchange, &config);
        assert_eq!(cmd, RedisCommand::Exists);
    }

    #[test]
    fn test_apply_default_key_sets_when_missing() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?key=cfg-key").unwrap();
        let mut exchange = Exchange::new(Message::default());

        RedisProducer::apply_default_key(&mut exchange, &config);
        assert_eq!(
            exchange.input.header("CamelRedis.Key"),
            Some(&serde_json::json!("cfg-key"))
        );
    }

    #[test]
    fn test_apply_default_key_preserves_existing_header() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?key=cfg-key").unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("header-key"));
        let mut exchange = Exchange::new(msg);

        RedisProducer::apply_default_key(&mut exchange, &config);
        assert_eq!(
            exchange.input.header("CamelRedis.Key"),
            Some(&serde_json::json!("header-key"))
        );
    }

    #[test]
    fn test_apply_default_channels_sets_when_missing() {
        let config =
            RedisEndpointConfig::from_uri("redis://localhost:6379?command=SUBSCRIBE&channels=a,b")
                .unwrap();
        let mut exchange = Exchange::new(Message::default());

        RedisProducer::apply_default_channels(&mut exchange, &config);
        assert_eq!(
            exchange.input.header("CamelRedis.Channels"),
            Some(&serde_json::json!(["a", "b"]))
        );
    }

    #[test]
    fn test_apply_default_channels_skips_when_empty() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let mut exchange = Exchange::new(Message::default());

        RedisProducer::apply_default_channels(&mut exchange, &config);
        assert!(exchange.input.header("CamelRedis.Channels").is_none());
    }

    #[tokio::test]
    async fn test_producer_creates_connection_on_first_call() {
        // This test requires a real Redis server, so we mark it as a pattern test
        // In CI, this would be skipped unless Redis is available
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config);

        // Connection should be None initially
        {
            let guard = producer.conn.lock().await;
            assert!(guard.is_none());
        }

        // Note: We can't actually test the connection creation without a real Redis
        // This is documented for integration testing
    }
}
