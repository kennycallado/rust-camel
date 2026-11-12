use crate::config::{RedisCommand, RedisEndpointConfig, is_idempotent_command};
use crate::executor::{MultiplexedExecutor, execute_with_retry};
use camel_component_api::{CamelError, Exchange, RuntimeObservability};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::Service;

/// Redis producer that implements Tower `Service<Exchange>` for integration
/// with rust-camel pipelines.
///
/// Routes every call through a [`MultiplexedExecutor`], which owns the shared
/// connection and re-resolves the master address through a [`RedisTopology`](crate::topology::RedisTopology)
/// on reconnect (enabling sentinel failover).
#[derive(Clone)]
pub struct RedisProducer {
    config: RedisEndpointConfig,
    executor: MultiplexedExecutor,
    /// Runtime observability handle powering the component-ops facade at
    /// the command boundary (`("redis","command")`, dashboard-observability
    /// Task 4.3). `execute_with_retry` is a custom loop (not retry_async)
    /// and never wires metrics, so nothing collides with `e:redis:command`.
    runtime: Arc<dyn RuntimeObservability>,
}

impl RedisProducer {
    /// Creates a new RedisProducer with the given configuration.
    ///
    /// Builds the topology from `config` (standalone, sentinel, or cluster) and
    /// wraps it in a [`MultiplexedExecutor`]. The connection is not established
    /// until the first call to `call()`.
    pub fn new(
        config: RedisEndpointConfig,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> Result<Self, CamelError> {
        let topology = crate::topology::topology_from_config(&config)?;
        let executor = MultiplexedExecutor::new(config.clone(), topology);
        Ok(Self {
            config,
            executor,
            runtime,
        })
    }

    /// Resolves the command to execute from the endpoint URI only.
    ///
    /// The endpoint URI is the sole command source (ADR-0032). A
    /// `CamelRedis.Command` header — with any value, parsable or not — is
    /// rejected so that message payloads cannot select a different Redis
    /// command than the one the endpoint declares.
    fn resolve_command(
        exchange: &Exchange,
        config: &RedisEndpointConfig,
    ) -> Result<RedisCommand, CamelError> {
        if exchange.input.header("CamelRedis.Command").is_some() {
            return Err(CamelError::ProcessorError(
                "untrusted header 'CamelRedis.Command' cannot select a Redis command; the endpoint URI 'command=' is the only command source (ADR-0032)"
                    .into(),
            ));
        }
        Ok(config.command.clone())
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
        let mut connection = self.executor.get_conn().await?;

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
        let mut executor = self.executor.clone();
        let component_metrics = self.runtime.component_metrics("redis");

        Box::pin(async move {
            // 1. Resolve command from the endpoint URI. A `CamelRedis.Command`
            //    header is rejected here, before any connection is attempted.
            let cmd = Self::resolve_command(&exchange, &config)?;

            // 2. Set defaults from config if missing in headers
            Self::apply_default_key(&mut exchange, &config);
            Self::apply_default_channels(&mut exchange, &config);

            // 3. Execute with retry on transient errors. The executor reconnects
            //    (re-resolving the master through the topology) before each retry.
            // ("redis","command") facade (dashboard-observability 4.3): the
            // command boundary observes the final outcome AFTER the retry
            // loop settles — one observe per exchange, not per attempt (the
            // custom loop in execute_with_retry never wires metrics).
            let command_outcome = execute_with_retry(
                &mut executor,
                &cmd,
                &mut exchange,
                is_idempotent_command(&cmd),
                &config.reconnect,
            )
            .await;
            component_metrics.observe("command", command_outcome.is_err());
            command_outcome?;

            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::test_support::NoopRuntimeObservability;

    fn test_rt() -> Arc<dyn RuntimeObservability> {
        Arc::new(NoopRuntimeObservability)
    }

    use camel_component_api::Message;
    #[test]
    fn test_producer_new() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt()).unwrap();
        // conn_arc() returns a clone, so the executor's reference + this one = 2.
        let conn = producer.executor.conn_arc();
        assert_eq!(Arc::strong_count(&conn), 2);
    }

    #[test]
    fn test_producer_clone_shares_connection() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt()).unwrap();
        let producer2 = producer.clone();

        // Both producers share the same connection Arc (via the executor)
        assert!(Arc::ptr_eq(
            &producer.executor.conn_arc(),
            &producer2.executor.conn_arc()
        ));
    }

    #[test]
    fn resolve_command_uses_uri_command() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET").unwrap();
        let exchange = Exchange::new(Message::default());

        let cmd = RedisProducer::resolve_command(&exchange, &config).unwrap();
        assert_eq!(cmd, RedisCommand::Get);
    }

    /// Regression: an injected `DEL` on a `GET` endpoint must be rejected,
    /// not executed.
    #[test]
    fn resolve_command_rejects_valid_header() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET").unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Command", serde_json::json!("DEL"));
        let exchange = Exchange::new(msg);

        let err = RedisProducer::resolve_command(&exchange, &config).unwrap_err();
        assert!(
            err.to_string().contains("CamelRedis.Command"),
            "error must name the rejected header, got: {err}"
        );
    }

    #[test]
    fn resolve_command_rejects_invalid_header() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET").unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Command", serde_json::json!("NOT_A_COMMAND"));
        let exchange = Exchange::new(msg);

        let err = RedisProducer::resolve_command(&exchange, &config).unwrap_err();
        assert!(
            err.to_string().contains("CamelRedis.Command"),
            "error must name the rejected header, got: {err}"
        );
    }

    #[test]
    fn resolve_command_rejects_non_string_header() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET").unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Command", serde_json::json!(123));
        let exchange = Exchange::new(msg);

        let err = RedisProducer::resolve_command(&exchange, &config).unwrap_err();
        assert!(
            err.to_string().contains("CamelRedis.Command"),
            "error must name the rejected header, got: {err}"
        );
    }

    /// Nothing listens on 127.0.0.1:1, so a connect attempt would fail with a
    /// connection error. The header-naming error proves rejection happens
    /// before dialing.
    #[tokio::test]
    async fn producer_rejects_command_header_before_connect() {
        let config = RedisEndpointConfig::from_uri("redis://127.0.0.1:1?command=GET").unwrap();
        let mut producer = RedisProducer::new(config, test_rt()).unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Command", serde_json::json!("DEL"));
        let exchange = Exchange::new(msg);

        let err = producer.call(exchange).await.unwrap_err();
        assert!(
            matches!(err, CamelError::ProcessorError(_)),
            "expected a processor error, got: {err:?}"
        );
        assert!(
            err.to_string().contains("CamelRedis.Command"),
            "rejection must happen before connecting, got: {err}"
        );
    }

    /// redis-geo MODIFIED scenario: the URI alone resolves a GEO command, and
    /// a `CamelRedis.Command` header on the same endpoint is rejected.
    #[tokio::test]
    async fn geo_uri_and_header_resolution_matches_spec() {
        let config =
            RedisEndpointConfig::from_uri("redis://localhost:6379?command=GEOSEARCH").unwrap();
        assert_eq!(config.command, RedisCommand::Geosearch);

        // Port 1 keeps the test hermetic; no connection is attempted anyway
        // because the header check runs first.
        let config =
            RedisEndpointConfig::from_uri("redis://127.0.0.1:1?command=GEOSEARCH").unwrap();
        let mut producer = RedisProducer::new(config, test_rt()).unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Command", serde_json::json!("GEOADD"));
        let exchange = Exchange::new(msg);

        let err = producer.call(exchange).await.unwrap_err();
        assert!(
            err.to_string().contains("CamelRedis.Command"),
            "header must be rejected before any connection, got: {err}"
        );
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
        let mut producer = RedisProducer::new(config, test_rt()).unwrap();
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
        let producer = RedisProducer::new(config, test_rt()).unwrap();
        let producer2 = producer.clone();

        // Both share the same connection Arc (via the executor)
        assert!(Arc::ptr_eq(
            &producer.executor.conn_arc(),
            &producer2.executor.conn_arc()
        ));

        // Cloning one doesn't affect the other's config
        assert_eq!(producer.config.command, producer2.config.command);
    }

    #[tokio::test]
    async fn test_producer_connection_is_none_initially() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt()).unwrap();

        let conn = producer.executor.conn_arc();
        let guard = conn.lock().await; // allow-test-wait: fresh producer; conn cell uncontended
        assert!(guard.is_none());
    }

    #[test]
    fn test_producer_clone_increments_arc_count() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt()).unwrap();
        // conn_arc() returns a clone, so the executor's reference + this one = 2.
        let conn = producer.executor.conn_arc();
        assert_eq!(Arc::strong_count(&conn), 2);

        let _producer2 = producer.clone();
        // The second producer's executor shares the same conn Arc.
        assert_eq!(Arc::strong_count(&conn), 3);
    }

    #[tokio::test]
    async fn test_producer_creates_connection_on_first_call() {
        // This test requires a real Redis server, so we mark it as a pattern test
        // In CI, this would be skipped unless Redis is available
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let producer = RedisProducer::new(config, test_rt()).unwrap();

        // Connection should be None initially
        {
            let conn = producer.executor.conn_arc();
            let guard = conn.lock().await; // allow-test-wait: fresh producer; conn cell uncontended
            assert!(guard.is_none());
        }

        // Note: We can't actually test the connection creation without a real Redis
        // This is documented for integration testing
    }

    // REDIS-010: Health check method exists and returns error without live Redis
    #[tokio::test]
    async fn test_check_connection_fails_without_redis() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:9933").unwrap();
        let producer = RedisProducer::new(config, test_rt()).unwrap();
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
        let producer = RedisProducer::new(config, test_rt()).unwrap();
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
