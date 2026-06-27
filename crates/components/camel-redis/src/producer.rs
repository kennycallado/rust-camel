use crate::commands;
use crate::config::{
    RedisCommand, RedisEndpointConfig, is_idempotent_command, is_transient_redis_error,
};
use camel_component_api::{CamelError, Exchange, RuntimeObservability};
use redis::aio::MultiplexedConnection;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::Mutex;
use tower::Service;
use tracing::{debug, info, warn};

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
    /// Runtime observability handle. Currently unused — Redis producer
    /// errors are handler-owned per ADR-0012 (route ErrorHandler catches
    /// them). Retained for Phase A API consistency. Crash-health pinning
    /// stays owned by Runtime supervision per CONTEXT-MAP 'ConsumerRestart'.
    #[allow(dead_code)]
    runtime: Arc<dyn RuntimeObservability>,
}

impl RedisProducer {
    /// Creates a new RedisProducer with the given configuration.
    ///
    /// The connection is not established until the first call to `call()`.
    pub fn new(config: RedisEndpointConfig, runtime: Arc<dyn RuntimeObservability>) -> Self {
        Self {
            config,
            conn: Arc::new(Mutex::new(None)),
            runtime,
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

    /// Health check: PINGs Redis and returns Ok(()) if reachable.
    ///
    /// Uses the same shared connection as normal operations. If no connection
    /// exists yet, creates one (proving connectivity). On failure, returns
    /// a `CamelError::ProcessorError`.
    pub async fn check_connection(&self) -> Result<(), CamelError> {
        let endpoint = self.config.safe_endpoint();
        let mut connection = get_or_create_connection(&self.config, &self.conn, &endpoint).await?;

        redis::cmd("PING")
            .query_async::<String>(&mut connection)
            .await
            .map_err(|e| {
                CamelError::ProcessorError(format!(
                    "Redis health check PING failed for '{}': {}",
                    endpoint, e
                ))
            })?;

        Ok(())
    }
}

/// Get an existing cached connection or create a new one.
/// Clears and recreates the connection if the cached one is stale.
async fn get_or_create_connection(
    config: &RedisEndpointConfig,
    conn: &Arc<Mutex<Option<MultiplexedConnection>>>,
    endpoint: &str,
) -> Result<MultiplexedConnection, CamelError> {
    // Fast path: try to use cached connection
    {
        let guard = conn.lock().await;
        if let Some(c) = guard.as_ref() {
            return Ok(c.clone());
        }
    }

    // Need to create connection — double-check under exclusive lock
    let mut guard = conn.lock().await;
    if let Some(c) = guard.as_ref() {
        return Ok(c.clone());
    }

    debug!(endpoint = %endpoint, "Creating new Redis connection");
    let redis_url_safe = config.redis_url_safe();
    let client = redis::Client::open(config.redis_url().as_str()).map_err(|e| {
        CamelError::ProcessorError(format!(
            "Failed to create Redis client for endpoint '{}': {}",
            redis_url_safe, e
        ))
    })?;

    let new_conn = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| {
            CamelError::ProcessorError(format!(
                "Failed to connect to Redis at '{}': {}",
                redis_url_safe, e
            ))
        })?;

    *guard = Some(new_conn.clone());
    info!(endpoint = %endpoint, "Redis connection established");
    Ok(new_conn)
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
            let endpoint = config.safe_endpoint();

            // 1. Get or create connection (with reconnect on stale connection)
            let mut connection = get_or_create_connection(&config, &conn, &endpoint).await?;

            // 2. Resolve command from header or config
            let cmd = Self::resolve_command(&exchange, &config);

            // 3. Set defaults from config if missing in headers
            Self::apply_default_key(&mut exchange, &config);
            Self::apply_default_channels(&mut exchange, &config);

            // 4. Dispatch to appropriate command handler with retry for transient errors
            let result = Self::dispatch_command(&cmd, &mut connection, &mut exchange).await;

            // 5. On transient error, clear stale connection and retry with NetworkRetryPolicy
            if let Err(ref e) = result
                && is_transient_redis_error(e)
                && is_idempotent_command(&cmd)
            {
                warn!(
                    endpoint = %endpoint,
                    command = ?cmd,
                    error = %e,
                    "Transient error on idempotent command, reconnecting with bounded retry"
                );
                let mut attempt: u32 = 0;
                let mut last_err = e.clone();

                // Manual retry loop (not retry_async) because:
                // - Between attempts we must clear the stale connection
                //   (*guard = None) and explicitly reconnect via
                //   get_or_create_connection — side effects that
                //   retry_async's "always invoke op" model cannot express
                //   without duplicating the connect logic inside the closure.
                // - The retried operation (dispatch_command) borrows
                //   &mut exchange via async move; the resulting Future
                //   holds the borrow past the FnMut closure boundary,
                //   which the borrow checker rejects. Workarounds
                //   (Arc<Mutex<Exchange>>, clone-in-closure) are heavier
                //   than keeping the manual loop.
                loop {
                    attempt += 1;
                    if !config.reconnect.should_retry(attempt) {
                        break;
                    }
                    // Clear stale connection
                    {
                        let mut guard = conn.lock().await;
                        *guard = None;
                    }

                    let delay = config.reconnect.delay_for(attempt - 1);
                    debug!(
                        endpoint = %endpoint,
                        command = ?cmd,
                        attempt,
                        delay_ms = delay.as_millis(),
                        "Waiting before reconnect attempt"
                    );
                    tokio::time::sleep(delay).await;

                    // Reconnect
                    match get_or_create_connection(&config, &conn, &endpoint).await {
                        Ok(mut reconnected) => {
                            // Retry the command
                            match Self::dispatch_command(&cmd, &mut reconnected, &mut exchange)
                                .await
                            {
                                Ok(()) => return Ok(exchange),
                                Err(retry_err) => {
                                    if is_transient_redis_error(&retry_err) {
                                        warn!(
                                            endpoint = %endpoint,
                                            command = ?cmd,
                                            attempt,
                                            error = %retry_err,
                                            "Retry failed with transient error"
                                        );
                                        last_err = retry_err;
                                        continue;
                                    } else {
                                        // Non-transient error on retry — propagate immediately
                                        return Err(retry_err);
                                    }
                                }
                            }
                        }
                        Err(conn_err) => {
                            if is_transient_redis_error(&conn_err) {
                                warn!(
                                    endpoint = %endpoint,
                                    attempt,
                                    error = %conn_err,
                                    "Reconnect failed with transient error"
                                );
                                last_err = conn_err;
                                continue;
                            } else {
                                return Err(conn_err);
                            }
                        }
                    }
                }

                // Exhausted all retries
                return Err(CamelError::ProcessorError(format!(
                    "Command {:?} failed after {} attempts: {}",
                    cmd, config.reconnect.max_attempts, last_err
                )));
            }

            result?;
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::Message;
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

    #[test]
    fn test_producer_new() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt());
        assert!(Arc::strong_count(&producer.conn) == 1);
    }

    #[test]
    fn test_producer_clone_shares_connection() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt());
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
    async fn test_poll_ready_always_returns_ready() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let mut producer = RedisProducer::new(config, test_rt());
        let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_apply_default_key_does_nothing_when_config_key_is_none() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let mut exchange = Exchange::new(Message::default());

        RedisProducer::apply_default_key(&mut exchange, &config);
        assert!(exchange.input.header("CamelRedis.Key").is_none());
    }

    #[test]
    fn test_apply_default_channels_preserves_existing_header() {
        let config =
            RedisEndpointConfig::from_uri("redis://localhost:6379?command=SUBSCRIBE&channels=a,b")
                .unwrap();
        let mut msg = Message::default();
        msg.set_header(
            "CamelRedis.Channels",
            serde_json::json!(["existing-channel"]),
        );
        let mut exchange = Exchange::new(msg);

        RedisProducer::apply_default_channels(&mut exchange, &config);
        assert_eq!(
            exchange.input.header("CamelRedis.Channels"),
            Some(&serde_json::json!(["existing-channel"]))
        );
    }

    #[test]
    fn test_producer_clone_is_independent_for_async_state() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt());
        let producer2 = producer.clone();

        // Both share the same Arc
        assert!(Arc::ptr_eq(&producer.conn, &producer2.conn));

        // Cloning one doesn't affect the other's config
        assert_eq!(producer.config.command, producer2.config.command);
    }

    #[tokio::test]
    async fn test_producer_connection_is_none_initially() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt());

        let guard = producer.conn.lock().await;
        assert!(guard.is_none());
    }

    #[test]
    fn test_producer_clone_increments_arc_count() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt());
        assert_eq!(Arc::strong_count(&producer.conn), 1);

        let _producer2 = producer.clone();
        assert_eq!(Arc::strong_count(&producer.conn), 2);
    }

    #[tokio::test]
    async fn test_producer_creates_connection_on_first_call() {
        // This test requires a real Redis server, so we mark it as a pattern test
        // In CI, this would be skipped unless Redis is available
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt());

        // Connection should be None initially
        {
            let guard = producer.conn.lock().await;
            assert!(guard.is_none());
        }

        // Note: We can't actually test the connection creation without a real Redis
        // This is documented for integration testing
    }

    // REDIS-010: Health check method exists and returns error without live Redis
    #[tokio::test]
    async fn test_check_connection_fails_without_redis() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:9933").unwrap();
        let producer = RedisProducer::new(config, test_rt());
        let result = producer.check_connection().await;
        // Without a Redis on port 9933, this should fail
        // The error may come from connection failure or PING failure
        assert!(
            result.is_err(),
            "check_connection should fail without live Redis"
        );
    }

    // REDIS-010: Verify check_connection method is callable on cloned producer
    #[test]
    fn test_check_connection_available_on_clone() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt());
        let _clone = producer.clone();
        // Verify the method exists and compiles — actual call requires live Redis
    }

    #[test]
    fn test_reconnect_policy_defaults_to_enabled() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        // Default reconnect policy should be enabled with the standard defaults
        assert!(config.reconnect.enabled);
        assert_eq!(config.reconnect.max_attempts, 10);
    }

    #[test]
    fn test_reconnect_disabled_policy_never_retries() {
        let mut config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        config.reconnect.enabled = false;
        // When reconnect is disabled, should_retry returns false even on attempt 0
        assert!(!config.reconnect.should_retry(0));
    }

    #[test]
    fn test_reconnect_policy_respects_max_attempts() {
        let mut config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        config.reconnect.max_attempts = 3;
        // Zero-based: 0 = first attempt, 1 = first retry, etc.
        assert!(config.reconnect.should_retry(0));
        assert!(config.reconnect.should_retry(1));
        assert!(config.reconnect.should_retry(2));
        assert!(!config.reconnect.should_retry(3));
    }
}
