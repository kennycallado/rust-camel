use crate::config::{RedisCommand, RedisEndpointConfig, is_idempotent_command, is_script_command};
use crate::executor::{CommandContext, MultiplexedExecutor, execute_with_retry};
use crate::script::{ScriptKind, ScriptRuntime, load_script_source};
use crate::transport_error::RequestKind;
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
        let context = command_context(&config)?;
        let executor = MultiplexedExecutor::new(config.clone(), topology).with_context(context);
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
    /// command than the one the endpoint declares. Script text is operator
    /// config too: on a script endpoint a `CamelRedis.Script` header is
    /// rejected for the same reason (exchange data is untrusted; executable
    /// text is an interpretable sink).
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
        if is_script_command(&config.command)
            && exchange.input.header("CamelRedis.Script").is_some()
        {
            return Err(CamelError::ProcessorError(
                "untrusted header 'CamelRedis.Script' cannot supply script text; a script endpoint takes its source only from the URI 'script=' or 'scriptFile=' (ADR-0032)"
                    .into(),
            ));
        }
        Ok(config.command.clone())
    }

    /// Resolves the effective key: the URI `key=` wins, otherwise the
    /// `CamelRedis.Key` header, otherwise no key at all.
    ///
    /// A non-string header value yields `None` (the header is left untouched),
    /// so the command handlers surface their existing missing-header error.
    fn resolve_key(exchange: &Exchange, config: &RedisEndpointConfig) -> Option<String> {
        match config.key {
            Some(ref key) => Some(key.clone()),
            None => exchange
                .input
                .header("CamelRedis.Key")
                .and_then(|v| v.as_str())
                .map(str::to_string),
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
    /// exists yet, creates one (proving connectivity).
    ///
    /// On a PING failure the cached connection is invalidated when the error
    /// proves the socket is dead, and the error is returned with the
    /// `RedisError` preserved as its source.
    pub async fn check_connection(&self) -> Result<(), CamelError> {
        let endpoint = self.config.safe_endpoint();
        let (generation, mut connection) = self.executor.checkout().await?;

        if let Err(e) = redis::cmd("PING")
            .query_async::<String>(&mut connection)
            .await
        {
            // e_opus ruling (change redisdisc): keep the existing text but
            // flip the variant to `ProcessorErrorWithSource` so the transport
            // classifiers read the `redis::RedisError` structure instead of
            // sniffing prose. This is what lets a dead socket classify as
            // invalidating here.
            let err = CamelError::ProcessorErrorWithSource(
                format!("Redis health check PING failed for '{}': {}", endpoint, e),
                Arc::new(e),
            );
            // Compare-and-clear: only the generation this checkout observed is
            // evicted, so a fresher connection rebuilt by another clone
            // survives. Invalidation never dials by itself.
            if crate::transport_error::invalidates_connection(&err) {
                self.executor.invalidate_if_current(generation).await;
            }
            return Err(err);
        }

        Ok(())
    }
}

/// Builds the per-endpoint dispatch context: the script runtime for a
/// script command (validating and, for `scriptFile`, reading its source NOW,
/// so a missing/unreadable file fails at endpoint creation) plus the
/// `idempotent` replay opt-in.
fn command_context(config: &RedisEndpointConfig) -> Result<CommandContext, CamelError> {
    let script = if is_script_command(&config.command) {
        Some(Arc::new(ScriptRuntime::new(
            load_script_source(config)?,
            script_kind(config)?,
        )))
    } else {
        None
    };
    Ok(CommandContext {
        script,
        script_idempotent: config.idempotent,
        atomic: config.atomic,
        batch: None,
    })
}

/// Resolves which script command the runtime drives.
///
/// `from_uri` already validated the `function`/`script` pairing; a
/// programmatically built config that skips it fails closed here instead of
/// dispatching an `FCALL` without a name.
fn script_kind(config: &RedisEndpointConfig) -> Result<ScriptKind, CamelError> {
    match config.command {
        RedisCommand::Fcall => match config.function.clone() {
            Some(function) if !function.is_empty() => Ok(ScriptKind::Fcall { function }),
            _ => Err(CamelError::Config(
                "command=fcall requires 'function': set the registered function name to invoke"
                    .to_string(),
            )),
        },
        _ => Ok(ScriptKind::Eval),
    }
}

/// Aggregate deadline for a `WAIT` exchange: `CamelRedis.WaitTimeout` plus the
/// endpoint `timeout`.
///
/// `execute_with_retry` is not bounded by [`RedisEndpointConfig::timeout`], so
/// a WAIT exchange gets its own whole-exchange deadline that covers connection
/// checkout, every attempt, backoff and replies. The sum is computed with
/// checked arithmetic: an unrepresentable duration fails before send with a
/// named error, never a panic.
pub(crate) fn wait_aggregate_timeout(
    wait_timeout_ms: u64,
    endpoint_timeout_secs: u64,
) -> Result<std::time::Duration, CamelError> {
    std::time::Duration::from_millis(wait_timeout_ms)
        .checked_add(std::time::Duration::from_secs(endpoint_timeout_secs))
        .ok_or_else(unrepresentable_wait_deadline)
}

/// Named error for a WAIT deadline that cannot be represented.
///
/// Raised both when the summed [`std::time::Duration`] overflows and when the
/// sum fits a duration but not an [`tokio::time::Instant`]; either way the
/// exchange must fail before send and name `CamelRedis.WaitTimeout`.
fn unrepresentable_wait_deadline() -> CamelError {
    CamelError::ProcessorError(
        "WAIT: the aggregate deadline (CamelRedis.WaitTimeout + endpoint timeout) is unrepresentable; refusing to send"
            .to_string(),
    )
}

/// Named error for an exchange that outlived its WAIT aggregate deadline.
fn wait_deadline_exceeded() -> CamelError {
    CamelError::ProcessorError(
        "WAIT: aggregate deadline exceeded (CamelRedis.WaitTimeout + endpoint timeout)".to_string(),
    )
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

            // 2. Resolve the effective key and publish it, then set channel
            //    defaults. Publishing before execution keeps command handlers
            //    reading `CamelRedis.Key` unchanged.
            if let Some(key) = Self::resolve_key(&exchange, &config) {
                exchange
                    .input
                    .set_header("CamelRedis.Key", serde_json::Value::String(key));
            }
            Self::apply_default_channels(&mut exchange, &config);

            // 2b. WAIT is a blocking acknowledgement probe, so it gets its own
            //     whole-exchange deadline: `CamelRedis.WaitTimeout` + the
            //     endpoint timeout. The deadline covers connection checkout,
            //     every retry attempt, backoff and replies; `execute_with_retry`
            //     alone is not bounded by `RedisEndpointConfig::timeout`. Both
            //     the duration sum and the deadline instant are checked, so an
            //     unrepresentable deadline fails before send, never panics.
            let wait_deadline = if cmd == RedisCommand::Wait {
                let wait_timeout_ms = crate::commands::other::resolve_wait_timeout(&exchange)?;
                let aggregate = wait_aggregate_timeout(wait_timeout_ms, config.timeout)?;
                Some(
                    tokio::time::Instant::now()
                        .checked_add(aggregate)
                        .ok_or_else(unrepresentable_wait_deadline)?,
                )
            } else {
                None
            };

            // 3. Execute with retry. Retry eligibility is a producer-side
            //    preflight: the retry loop wraps dispatch, so the idempotence
            //    argument must be resolved BEFORE the loop runs. Script
            //    commands opt in through the endpoint's `idempotent` flag and
            //    use the script classifier, whose provenance pair resolves
            //    every reply before classification. A batch is replayable
            //    only when EVERY entry is idempotent, and its plan is built
            //    here — before any connection — so a rejected batch cannot
            //    send a partially-applied command list.
            // ("redis","command") facade (dashboard-observability 4.3): the
            // command boundary observes the final outcome AFTER the retry
            // loop settles — one observe per exchange, not per attempt (the
            // custom loop in execute_with_retry never wires metrics).
            let batch = if cmd == RedisCommand::Multi {
                Some(crate::commands::batch::plan_batch(&exchange)?)
            } else {
                None
            };
            let (is_idempotent, kind) = if is_script_command(&cmd) {
                (config.idempotent, RequestKind::Script)
            } else if let Some(plan) = &batch {
                (plan.idempotent, RequestKind::Plain)
            } else {
                (is_idempotent_command(&cmd), RequestKind::Plain)
            };
            if let Some(plan) = batch {
                executor = executor.with_batch(plan);
            }
            let command_outcome = {
                let execute = execute_with_retry(
                    &mut executor,
                    &cmd,
                    &mut exchange,
                    is_idempotent,
                    kind,
                    &config.reconnect,
                );
                match wait_deadline {
                    Some(deadline) => match tokio::time::timeout_at(deadline, execute).await {
                        Ok(outcome) => outcome,
                        Err(_) => Err(wait_deadline_exceeded()),
                    },
                    None => execute.await,
                }
            };
            component_metrics.observe("command", command_outcome.is_err());
            command_outcome?;

            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::test_stub;
    use crate::topology::FakeTopology;
    use camel_component_api::NetworkRetryPolicy;
    use camel_component_api::test_support::NoopRuntimeObservability;
    use std::error::Error as _;

    fn test_rt() -> Arc<dyn RuntimeObservability> {
        Arc::new(NoopRuntimeObservability)
    }

    /// Producer wired to `topology` instead of the config-derived topology.
    ///
    /// `tests` is a descendant of the `producer` module, so it may build the
    /// struct directly and inject the RESP stub topology.
    fn stub_producer(url: &str, topology: Arc<FakeTopology>) -> RedisProducer {
        let config = RedisEndpointConfig::from_uri(url).expect("valid stub redis URI");
        let context = command_context(&config).expect("script source resolves for the stub config");
        let executor = MultiplexedExecutor::new(config.clone(), topology).with_context(context);
        RedisProducer {
            config,
            executor,
            runtime: test_rt(),
        }
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
    fn uri_key_wins_over_exchange_header() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?key=out").unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("in"));
        let exchange = Exchange::new(msg);

        assert_eq!(
            RedisProducer::resolve_key(&exchange, &config),
            Some("out".to_string()),
            "the URI key must win over the CamelRedis.Key header"
        );
    }

    #[test]
    fn header_key_used_when_uri_declares_none() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("s2"));
        let exchange = Exchange::new(msg);

        assert_eq!(
            RedisProducer::resolve_key(&exchange, &config),
            Some("s2".to_string()),
            "without a URI key the header must supply the key"
        );
    }

    #[test]
    fn resolve_key_returns_uri_key() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?key=cfg-key").unwrap();
        let exchange = Exchange::new(Message::default());

        assert_eq!(
            RedisProducer::resolve_key(&exchange, &config),
            Some("cfg-key".to_string()),
            "the URI key must be the resolved key"
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
    fn resolve_key_returns_none_without_sources() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        let exchange = Exchange::new(Message::default());

        assert_eq!(
            RedisProducer::resolve_key(&exchange, &config),
            None,
            "no URI key and no header must resolve to no key"
        );
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

    /// redisdisc task 2.2: a PING failure on a dead socket must keep the
    /// existing text, preserve the `RedisError` as source (the classifiers
    /// read structure, not prose) and evict the cached connection.
    #[tokio::test]
    async fn check_connection_invalidates_on_killed_connection() {
        let stub = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let producer = stub_producer(&stub.url(), topology.clone());

        // First call primes the cache: the stub answers PING with +OK.
        producer
            .check_connection()
            .await
            .expect("the stub answers PING with +OK");
        assert_eq!(topology.resolve_call_count(), 1);
        let conn = producer.executor.conn_arc();
        let cached = conn.lock().await; // allow-test-wait: test-owned cache cell; lock is uncontended in-test
        assert!(
            cached.is_some(),
            "a successful PING must leave the connection cached"
        );
        drop(cached);

        stub.kill().await;

        let err = producer
            .check_connection()
            .await
            .expect_err("a killed socket must fail PING");
        let rendered = err.to_string();
        // `CamelError::ProcessorErrorWithSource` renders with the shared
        // "Processor error: " variant prefix; the health-check text follows it.
        assert!(
            rendered.starts_with("Processor error: Redis health check PING failed for '"),
            "the existing health-check text must be preserved, got: {rendered}"
        );
        assert!(
            err.source().is_some(),
            "the PING failure must keep the RedisError as source: {err:?}"
        );
        assert!(
            crate::transport_error::is_transient_redis_error(&err),
            "socket death must classify transient: {err}"
        );
        assert!(
            crate::transport_error::invalidates_connection(&err),
            "socket death must invalidate the cached connection: {err}"
        );

        let conn = producer.executor.conn_arc();
        let cached = conn.lock().await; // allow-test-wait: test-owned cache cell; lock is uncontended in-test
        assert!(
            cached.is_none(),
            "an invalidating PING failure must clear the cached connection"
        );
        drop(cached);
        assert_eq!(
            topology.resolve_call_count(),
            1,
            "an invalidation must never dial by itself"
        );
    }

    /// redisdisc task 2.2: after the invalidation the next health check lazily
    /// rebuilds through the topology and succeeds against the fresh node.
    #[tokio::test]
    async fn check_connection_recovers_after_kill() {
        let dead = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let fresh = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let topology = Arc::new(FakeTopology::addrs(vec![dead.url(), fresh.url()]));
        let producer = stub_producer(&dead.url(), topology.clone());

        producer
            .check_connection()
            .await
            .expect("the first PING answers on the first stub");
        assert_eq!(topology.resolve_call_count(), 1);

        dead.kill().await;
        producer
            .check_connection()
            .await
            .expect_err("a killed socket must fail PING");

        producer
            .check_connection()
            .await
            .expect("the post-invalidation call must dial the fresh stub");
        assert_eq!(
            topology.resolve_call_count(),
            2,
            "recovery must re-resolve exactly once"
        );
        let conn = producer.executor.conn_arc();
        let cached = conn.lock().await; // allow-test-wait: test-owned cache cell; lock is uncontended in-test
        assert_eq!(
            cached.as_ref().map(|(generation, _)| *generation),
            Some(2),
            "the rebuilt connection must carry a fresh generation"
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

    // ── eval script execution (redisfc 2.1) ────────────────────────────────

    /// Raw `EXEC` reply carrying the provenance pair `(exists, result)`.
    /// `exists` is `SCRIPT EXISTS <sha>`'s reply — real Redis answers one
    /// flag per requested SHA, so it is a one-element ARRAY (`*1\r\n:1\r\n`),
    /// never a bare integer — and `result` is `EVALSHA`'s.
    const PAIR_CACHED_ONE: &[u8] = b"*2\r\n*1\r\n:1\r\n:1\r\n";
    const PAIR_UNCACHED_NOSCRIPT: &[u8] =
        b"*2\r\n*1\r\n:0\r\n-NOSCRIPT No matching script. Please use EVAL.\r\n";
    const PAIR_CACHED_NOSCRIPT: &[u8] = b"*2\r\n*1\r\n:1\r\n-NOSCRIPT forged by the script\r\n";
    const PAIR_SCRIPT_BUSY: &[u8] = b"*2\r\n*1\r\n:1\r\n-BUSY fake script reply\r\n";

    fn eval_uri(stub: &test_stub::StubServer, script: &str) -> String {
        format!("{}?command=eval&script={script}", stub.url())
    }

    fn eval_sha(script: &str) -> String {
        redis::Script::new(script).get_hash().to_string()
    }

    fn eval_exchange() -> Exchange {
        use camel_component_api::Body;
        Exchange::new(Message::new(Body::Json(serde_json::json!({
            "keys": [],
            "args": [],
        }))))
    }

    fn recorded_text(stub: &test_stub::StubServer) -> Vec<String> {
        stub.recorded_frames()
            .iter()
            .map(|frame| String::from_utf8_lossy(frame).into_owned())
            .collect()
    }

    /// Cached connection generation of `producer`'s executor: `None` when the
    /// cache is empty (a failure invalidated it), `Some(n)` while generation
    /// `n` is cached.
    async fn cached_generation(producer: &RedisProducer) -> Option<u64> {
        let conn = producer.executor.conn_arc();
        let guard = conn.lock().await; // allow-test-wait: test-owned cache cell; lock is uncontended in-test
        guard.as_ref().map(|(generation, _)| *generation)
    }

    /// `command=eval` producer wired to `stub`'s topology.
    fn eval_producer(stub: &test_stub::StubServer) -> RedisProducer {
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        stub_producer(&eval_uri(stub, "return%201"), topology)
    }

    fn evalsha_count(frames: &[String]) -> usize {
        frames.iter().filter(|f| f.contains("EVALSHA")).count()
    }

    #[tokio::test]
    async fn eval_inline_script_runs_with_body_keys_and_args() {
        use camel_component_api::Body;
        let stub =
            test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Raw(PAIR_CACHED_ONE)])
                .await;
        let mut producer = eval_producer(&stub);
        stub.record_frames();

        let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({
            "keys": ["k1"],
            "args": ["a1"],
        }))));
        let out = producer
            .call(exchange)
            .await
            .expect("the script answers the integer 1");

        assert_eq!(
            out.input.body,
            Body::Json(serde_json::json!(1)),
            "the reply becomes the exchange body"
        );

        let frames = recorded_text(&stub);
        let evalsha = frames
            .iter()
            .find(|frame| frame.contains("EVALSHA"))
            .unwrap_or_else(|| panic!("the invocation must be an EVALSHA frame: {frames:?}"));
        let sha = eval_sha("return 1");
        assert!(
            evalsha.contains(&sha),
            "EVALSHA must carry the script SHA, got: {evalsha}"
        );
        assert!(
            evalsha.contains("k1"),
            "the body key must travel as the first key operand, got: {evalsha}"
        );
        assert!(
            evalsha.contains("a1"),
            "the body arg must travel as the first arg operand, got: {evalsha}"
        );
    }

    #[tokio::test]
    async fn eval_provenance_pair_is_atomic() {
        let stub =
            test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Raw(PAIR_CACHED_ONE)])
                .await;
        let mut producer = eval_producer(&stub);
        stub.record_frames();

        producer
            .call(eval_exchange())
            .await
            .expect("the cached script answers the integer 1");

        let frames = recorded_text(&stub);
        let sha = eval_sha("return 1");
        assert_eq!(
            frames.len(),
            4,
            "the provenance pair is one atomic pipeline: MULTI, SCRIPT EXISTS, EVALSHA, EXEC; got {frames:?}"
        );
        assert!(frames[0].contains("MULTI"), "pair must open with MULTI");
        assert!(
            frames[1].contains("SCRIPT")
                && frames[1].contains("EXISTS")
                && frames[1].contains(&sha),
            "the pair must observe SCRIPT EXISTS <sha>, got: {}",
            frames[1]
        );
        assert!(
            frames[2].contains("EVALSHA") && frames[2].contains(&sha),
            "the pair must invoke EVALSHA <sha>, got: {}",
            frames[2]
        );
        assert!(frames[3].contains("EXEC"), "pair must close with EXEC");
    }

    #[tokio::test]
    async fn eval_noscript_reloads_once_and_reinvokes() {
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Raw(PAIR_UNCACHED_NOSCRIPT),
            test_stub::AppReply::Ok,
            test_stub::AppReply::Raw(PAIR_CACHED_ONE),
        ])
        .await;
        let mut producer = eval_producer(&stub);
        stub.record_frames();

        let out = producer
            .call(eval_exchange())
            .await
            .expect("the reload recovers the missing script once");

        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(1)),
            "the second pair's result is the exchange result"
        );

        let frames = recorded_text(&stub);
        assert_eq!(
            frames.len(),
            9,
            "pair1, SCRIPT LOAD, pair2 as four frames each plus the reload; got {frames:?}"
        );
        assert!(
            frames[4].contains("SCRIPT") && frames[4].contains("LOAD"),
            "the reload must be a SCRIPT LOAD between the pairs, got: {}",
            frames[4]
        );
        assert_eq!(
            evalsha_count(&frames),
            2,
            "exactly one re-invocation after the reload"
        );
    }

    #[tokio::test]
    async fn eval_second_noscript_is_named_error() {
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Raw(PAIR_UNCACHED_NOSCRIPT),
            test_stub::AppReply::Ok,
            test_stub::AppReply::Raw(PAIR_CACHED_NOSCRIPT),
        ])
        .await;
        let mut producer = eval_producer(&stub);
        stub.record_frames();

        let err = producer
            .call(eval_exchange())
            .await
            .expect_err("a second NOSCRIPT is terminal");

        assert!(
            err.to_string().contains("NOSCRIPT"),
            "the terminal error must name NOSCRIPT, got: {err}"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            evalsha_count(&frames),
            2,
            "no third invocation after the second NOSCRIPT; frames: {frames:?}"
        );
    }

    #[tokio::test]
    async fn eval_forged_noscript_not_reinvoked_without_optin() {
        let stub = test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Raw(
            PAIR_CACHED_NOSCRIPT,
        )])
        .await;
        let mut producer = eval_producer(&stub);
        stub.record_frames();

        let err = producer
            .call(eval_exchange())
            .await
            .expect_err("a cached script cannot return NOSCRIPT, so the reply is forged");

        assert!(
            err.to_string().contains("NOSCRIPT"),
            "the forged reply surfaces as a named error, got: {err}"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            evalsha_count(&frames),
            1,
            "exactly one side effect: the forged reply must not be re-invoked; frames: {frames:?}"
        );
        assert!(
            !frames.iter().any(|f| f.contains("LOAD")),
            "a cached script must not be reloaded: {frames:?}"
        );
        assert_eq!(
            crate::config::retry_class_script(&err, crate::config::SendPhase::PostFlush),
            crate::config::RetryClass::NeverTransient,
            "a proven-forged reply is a business error"
        );
    }

    #[tokio::test]
    async fn eval_rejects_script_header() {
        let stub = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let mut producer = eval_producer(&stub);

        let mut msg = Message::new(camel_component_api::Body::Json(serde_json::json!({
            "keys": [],
            "args": [],
        })));
        msg.set_header(
            "CamelRedis.Script",
            serde_json::json!("return redis.call('GET', 'k')"),
        );

        let err = producer
            .call(Exchange::new(msg))
            .await
            .expect_err("script text from exchange data is rejected");

        assert!(
            err.to_string().contains("CamelRedis.Script"),
            "the error must name the rejected header, got: {err}"
        );
        assert_eq!(
            stub.command_count(),
            0,
            "the rejection must happen before send"
        );
    }

    #[tokio::test]
    async fn eval_malformed_body_operands_named_error() {
        use camel_component_api::Body;
        let stub = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let mut producer = eval_producer(&stub);

        let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({
            "keys": "k1",
        }))));

        let err = producer
            .call(exchange)
            .await
            .expect_err("a non-array keys field is rejected");

        assert!(
            err.to_string()
                .contains(r#"{"keys": [...], "args": [...]}"#),
            "the error must name the expected body shape, got: {err}"
        );
        assert_eq!(
            stub.command_count(),
            0,
            "the rejection must happen before send"
        );
    }

    #[tokio::test]
    async fn eval_rejects_missing_keys_field() {
        use camel_component_api::Body;
        let stub = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let mut producer = eval_producer(&stub);

        let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({
            "args": ["x"],
        }))));

        let err = producer
            .call(exchange)
            .await
            .expect_err("both operand arrays are required, so a missing keys field is rejected");

        assert!(
            err.to_string().contains("'keys' is required"),
            "the error must name the missing field, got: {err}"
        );
        assert!(
            err.to_string()
                .contains(r#"{"keys": [...], "args": [...]}"#),
            "the error must name the expected body shape, got: {err}"
        );
        assert_eq!(
            stub.command_count(),
            0,
            "the rejection must happen before send"
        );
    }

    #[tokio::test]
    async fn eval_rejects_non_string_key() {
        use camel_component_api::Body;
        let stub = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let mut producer = eval_producer(&stub);

        let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({
            "keys": [123],
            "args": [],
        }))));

        let err = producer
            .call(exchange)
            .await
            .expect_err("keys accept strings only, so a numeric key is rejected");

        assert!(
            err.to_string().contains("'keys[0]'"),
            "the error must name the offending element, got: {err}"
        );
        assert!(
            err.to_string().contains("must be a string key"),
            "the error must state the keys element rule, got: {err}"
        );
        assert!(
            err.to_string()
                .contains(r#"{"keys": [...], "args": [...]}"#),
            "the error must name the expected body shape, got: {err}"
        );
        assert_eq!(
            stub.command_count(),
            0,
            "the rejection must happen before send"
        );
    }

    #[tokio::test]
    async fn eval_faked_busy_reply_not_replayed_without_optin() {
        let stub =
            test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Raw(PAIR_SCRIPT_BUSY)])
                .await;
        let mut producer = eval_producer(&stub);
        stub.record_frames();

        let err = producer
            .call(eval_exchange())
            .await
            .expect_err("a cached script returning BUSY is a business error");

        assert!(
            err.to_string().contains("BUSY"),
            "the faked BUSY reply surfaces as a named business error, got: {err}"
        );
        assert_eq!(
            evalsha_count(&recorded_text(&stub)),
            1,
            "exactly one invocation: no replay without the idempotent opt-in"
        );
        assert_eq!(
            crate::config::retry_class_script(&err, crate::config::SendPhase::PostFlush),
            crate::config::RetryClass::NeverTransient,
            "an arbitrary script-forged code is a business error, never transient"
        );
    }

    #[tokio::test]
    async fn non_script_commands_keep_header_sourced_keys() {
        use camel_component_api::Body;
        let stub = test_stub::StubServer::start(test_stub::AppReply::Bulk("v")).await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let mut producer = stub_producer(&format!("{}?command=get", stub.url()), topology);
        stub.record_frames();

        let mut msg = Message::new(Body::Text("ignored".into()));
        msg.set_header("CamelRedis.Key", serde_json::json!("k1"));
        producer
            .call(Exchange::new(msg))
            .await
            .expect("GET answers from the stub");

        let frames = recorded_text(&stub);
        assert_eq!(frames.len(), 1, "one GET frame: {frames:?}");
        assert!(
            frames[0].contains("GET") && frames[0].contains("k1"),
            "the key still comes from CamelRedis.Key, got: {}",
            frames[0]
        );
    }

    /// The reactive `SCRIPT LOAD` after a `NOSCRIPT` dies on the socket: the
    /// terminal error must still evict the dead connection, so the next
    /// exchange loads on a fresh one instead of retrying against the poisoned
    /// socket.
    #[tokio::test]
    async fn eval_script_load_failure_invalidates_dead_connection() {
        let dead = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Raw(PAIR_UNCACHED_NOSCRIPT),
            test_stub::AppReply::Close,
        ])
        .await;
        let fresh = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Raw(PAIR_UNCACHED_NOSCRIPT),
            test_stub::AppReply::Ok,
            test_stub::AppReply::Raw(PAIR_CACHED_ONE),
        ])
        .await;
        let topology = Arc::new(FakeTopology::addrs(vec![dead.url(), fresh.url()]));
        let mut producer = stub_producer(&eval_uri(&dead, "return%201"), topology.clone());

        let err = producer
            .call(eval_exchange())
            .await
            .expect_err("the reload died on the socket, so the first exchange fails");

        assert!(
            err.to_string().contains("SCRIPT LOAD"),
            "the terminal error must name the failed reload, got: {err}"
        );
        assert_eq!(
            crate::config::retry_class_script(&err, crate::config::SendPhase::PostFlush),
            crate::config::RetryClass::NeverTransient,
            "the attempt is terminal: a failed load is never replayed: {err}"
        );
        assert!(
            crate::transport_error::invalidates_connection(&err),
            "the preserved driver error must reach the invalidation track: {err}"
        );
        assert!(
            crate::transport_error::is_transient_redis_error(&err),
            "invalidating evidence implies the transient class: {err}"
        );
        assert_eq!(
            cached_generation(&producer).await,
            None,
            "the dead generation must be evicted, not reused by later exchanges"
        );
        assert_eq!(
            dead.command_count(),
            5,
            "the pair's four frames plus the reload reached the dead stub"
        );

        let out = producer
            .call(eval_exchange())
            .await
            .expect("the second exchange redials and loads on a fresh connection");
        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(1)),
            "the fresh connection serves the invocation"
        );
        assert_eq!(
            cached_generation(&producer).await,
            Some(2),
            "the retry dialed a new generation"
        );
        assert_eq!(
            fresh.command_count(),
            9,
            "the fresh connection re-ran the pair, the reload, and the second pair"
        );
        assert_eq!(
            topology.resolve_call_count(),
            2,
            "exactly one redial: the dead connection was not reused"
        );
    }

    // ── fcall script execution (redisfc 2.3) ───────────────────────────────    /// The function-library source the fcall tests configure inline.
    const LOCK_LIBRARY: &str =
        "#!lua name=lock\nredis.register_function('lock', function(keys, args) return 'hi' end)";

    /// A distinct library source for the `scriptFile` test, so the wire log
    /// proves which implementation the first-use load bound.
    const CONFIGURED_LIBRARY: &str = "#!lua name=lock\nredis.register_function('lock', function(keys, args) return 'configured' end)";

    /// `$len\r\n<text>\r\n`: one RESP2 bulk string.
    fn bulk(text: &str) -> Vec<u8> {
        let mut out = format!("${}\r\n", text.len()).into_bytes();
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(b"\r\n");
        out
    }

    /// `*n\r\n<elements>`: one RESP2 array.
    fn array(elements: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
        let elements: Vec<Vec<u8>> = elements.into_iter().collect();
        let mut out = format!("*{}\r\n", elements.len()).into_bytes();
        for element in elements {
            out.extend_from_slice(&element);
        }
        out
    }

    /// A real RESP2 `FUNCTION LIST` reply: one library whose `functions`
    /// field carries one entry per name. Redis answers an array of
    /// libraries, each a flat alternating field/value array; a function
    /// entry has the same flat shape (RESP2, the driver default).
    fn function_list(names: &[&str]) -> Vec<u8> {
        let functions: Vec<Vec<u8>> = names
            .iter()
            .map(|name| array(vec![bulk("name"), bulk(name)]))
            .collect();
        let library = array(vec![
            bulk("library_name"),
            bulk("lock"),
            bulk("engine"),
            bulk("LUA"),
            bulk("functions"),
            array(functions),
            bulk("library_code"),
            bulk(LOCK_LIBRARY),
        ]);
        array(vec![library])
    }

    /// A provenance-pair `EXEC` reply with an explicit `FUNCTION LIST` entry.
    fn fcall_pair_with_list(list: Vec<u8>, result: Vec<u8>) -> Vec<u8> {
        array(vec![list, result])
    }

    /// A provenance-pair `EXEC` reply: `(FUNCTION LIST, FCALL)`.
    fn fcall_pair(present: bool, result: Vec<u8>) -> Vec<u8> {
        let names: &[&str] = if present { &["lock"] } else { &[] };
        fcall_pair_with_list(function_list(names), result)
    }

    /// `%n\r\n<field><value>…`: one RESP3 map.
    fn resp3_map(pairs: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut out = format!("%{}\r\n", pairs.len()).into_bytes();
        for (key, value) in pairs {
            out.extend_from_slice(&bulk(key));
            out.extend_from_slice(&value);
        }
        out
    }

    /// A RESP2 `FUNCTION LIST` reply whose function entry has a non-text
    /// `name`. Such a reply proves nothing about registration.
    fn function_list_with_non_text_name() -> Vec<u8> {
        let entry = array(vec![bulk("name"), b":7\r\n".to_vec()]);
        let library = array(vec![
            bulk("library_name"),
            bulk("lock"),
            bulk("engine"),
            bulk("LUA"),
            bulk("functions"),
            array(vec![entry]),
            bulk("library_code"),
            bulk(LOCK_LIBRARY),
        ]);
        array(vec![library])
    }

    /// The RESP3-map form of [`function_list_with_non_text_name`].
    fn resp3_function_list_with_non_text_name() -> Vec<u8> {
        let entry = resp3_map(vec![("name", b":7\r\n".to_vec())]);
        let library = resp3_map(vec![
            ("library_name", bulk("lock")),
            ("engine", bulk("LUA")),
            ("functions", array(vec![entry])),
            ("library_code", bulk(LOCK_LIBRARY)),
        ]);
        array(vec![library])
    }

    /// Leaks `bytes` so a dynamically built reply can ride [`AppReply::Raw`].
    fn raw_reply(bytes: Vec<u8>) -> test_stub::AppReply {
        test_stub::AppReply::Raw(Box::leak(bytes.into_boxed_slice()))
    }

    /// The `FCALL` reply for an unregistered function.
    fn function_not_found() -> Vec<u8> {
        b"-ERR Function not found\r\n".to_vec()
    }

    fn encode_query_value(value: &str) -> String {
        percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
    }

    fn fcall_uri(stub: &test_stub::StubServer, extra: &str) -> String {
        format!(
            "{}?command=fcall&function=lock&script={}{extra}",
            stub.url(),
            encode_query_value(LOCK_LIBRARY)
        )
    }

    fn fcall_producer(stub: &test_stub::StubServer) -> RedisProducer {
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        stub_producer(&fcall_uri(stub, ""), topology)
    }

    fn fcall_exchange(args: &[&str]) -> Exchange {
        use camel_component_api::Body;
        Exchange::new(Message::new(Body::Json(serde_json::json!({
            "keys": [],
            "args": args,
        }))))
    }

    /// `FUNCTION LOAD` frames: the first-use load and any recovery load.
    fn load_count(frames: &[String]) -> usize {
        frames
            .iter()
            .filter(|frame| frame.contains("FUNCTION") && frame.contains("LOAD"))
            .count()
    }

    fn fcall_count(frames: &[String]) -> usize {
        frames
            .iter()
            .filter(|frame| frame.contains("FCALL"))
            .count()
    }

    fn first_index(frames: &[String], needle: &str) -> Option<usize> {
        frames.iter().position(|frame| frame.contains(needle))
    }

    #[tokio::test]
    async fn fcall_loads_library_and_invokes() {
        use camel_component_api::Body;
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(true, bulk("hi"))),
        ])
        .await;
        let mut producer = fcall_producer(&stub);
        stub.record_frames();

        let first = producer
            .call(fcall_exchange(&["a1"]))
            .await
            .expect("the function library loads and the invocation answers");
        let second = producer
            .call(fcall_exchange(&[]))
            .await
            .expect("the repeat exchange reuses the loaded library");

        assert_eq!(
            first.input.body,
            Body::Text("hi".into()),
            "the FCALL reply becomes the exchange body"
        );
        assert_eq!(
            second.input.body,
            Body::Text("hi".into()),
            "the repeat exchange maps its reply the same way"
        );

        let frames = recorded_text(&stub);
        assert_eq!(
            load_count(&frames),
            1,
            "the first-use lazy load runs exactly once across both exchanges: {frames:?}"
        );
        assert_eq!(
            fcall_count(&frames),
            2,
            "one FCALL invocation per exchange: {frames:?}"
        );
        let load =
            first_index(&frames, "FUNCTION").expect("the first-use load must be on the wire");
        let invoke = first_index(&frames, "FCALL").expect("the invocation must be on the wire");
        assert!(
            load < invoke,
            "the library loads before the first invocation: {frames:?}"
        );
        assert!(
            frames[load].contains("REPLACE") && frames[load].contains("register_function"),
            "the load must be FUNCTION LOAD REPLACE with the configured source, got: {}",
            frames[load]
        );
    }

    #[tokio::test]
    async fn fcall_first_use_replaces_preexisting_function() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let library_path = dir.path().join("lock.lua");
        std::fs::write(&library_path, CONFIGURED_LIBRARY).expect("write the library source");

        // FUNCTION LIST already reports a `lock` function, so only the
        // first-use REPLACE can bind the name to the configured code.
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(true, bulk("hi"))),
        ])
        .await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let uri = format!(
            "{}?command=fcall&function=lock&scriptFile={}",
            stub.url(),
            encode_query_value(&library_path.display().to_string())
        );
        let mut producer = stub_producer(&uri, topology);
        stub.record_frames();

        producer
            .call(fcall_exchange(&[]))
            .await
            .expect("the configured implementation runs");
        let after_first = recorded_text(&stub);
        let invoke =
            first_index(&after_first, "FCALL").expect("the invocation must be on the wire");
        let load =
            first_index(&after_first, "FUNCTION").expect("the first-use load must be on the wire");
        assert!(
            load < invoke,
            "the configured source must load before the first invocation: {after_first:?}"
        );
        assert!(
            after_first[load].contains("REPLACE")
                && after_first[load].contains("return 'configured'"),
            "the first-use load must bind the configured implementation, got: {}",
            after_first[load]
        );

        stub.record_frames();
        producer
            .call(fcall_exchange(&[]))
            .await
            .expect("the repeat exchange reuses the loaded library");
        assert_eq!(
            load_count(&recorded_text(&stub)),
            0,
            "a second exchange sends no FUNCTION LOAD"
        );
    }

    #[tokio::test]
    async fn fcall_missing_function_reloads_once() {
        use camel_component_api::Body;
        // Pair 1 observes the function absent and answers "function not
        // found"; the recovery load succeeds and pair 2 runs the function.
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(false, function_not_found())),
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(true, bulk("again"))),
        ])
        .await;
        let mut producer = fcall_producer(&stub);
        stub.record_frames();

        let out = producer
            .call(fcall_exchange(&["again"]))
            .await
            .expect("the recovery load and single re-invocation succeed");

        assert_eq!(
            out.input.body,
            Body::Text("again".into()),
            "the second pair's result is the exchange result"
        );

        let frames = recorded_text(&stub);
        assert_eq!(
            load_count(&frames),
            2,
            "the first-use load plus exactly one recovery load: {frames:?}"
        );
        assert_eq!(
            fcall_count(&frames),
            2,
            "one initial invocation plus exactly one re-invocation: {frames:?}"
        );
        let loads: Vec<usize> = frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame.contains("FUNCTION") && frame.contains("LOAD"))
            .map(|(index, _)| index)
            .collect();
        let invokes: Vec<usize> = frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame.contains("FCALL"))
            .map(|(index, _)| index)
            .collect();
        assert!(
            loads[0] < invokes[0] && invokes[0] < loads[1] && loads[1] < invokes[1],
            "the loads are distinguishable: initial load, invocation, recovery load, re-invocation; got {frames:?}"
        );

        // A harness that keeps reporting the function absent and "not found"
        // exhausts the single reload: two invocations, then a terminal error.
        let stuck = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(false, function_not_found())),
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(false, function_not_found())),
        ])
        .await;
        let mut producer = fcall_producer(&stuck);
        stuck.record_frames();

        let err = producer
            .call(fcall_exchange(&[]))
            .await
            .expect_err("a repeated missing function is terminal");

        assert!(
            err.to_string().contains("function") && err.to_string().contains("lock"),
            "the terminal error must name the function, got: {err}"
        );
        assert_eq!(
            fcall_count(&recorded_text(&stuck)),
            2,
            "exactly one re-invocation before giving up"
        );
        assert_eq!(
            crate::config::retry_class_script(&err, crate::config::SendPhase::PostFlush),
            crate::config::RetryClass::NeverTransient,
            "an exhausted reload is terminal: {err}"
        );
    }

    /// A `FUNCTION LIST` reply with a non-text function `name` is malformed,
    /// not "absent": the request is terminal and nothing is re-invoked.
    async fn assert_malformed_function_name_is_terminal(list: Vec<u8>) {
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(true, bulk("hi"))),
            raw_reply(fcall_pair_with_list(list, function_not_found())),
        ])
        .await;
        let mut producer = fcall_producer(&stub);
        producer
            .call(fcall_exchange(&[]))
            .await
            .expect("the priming exchange completes the first-use load");
        stub.record_frames();

        let err = producer
            .call(fcall_exchange(&[]))
            .await
            .expect_err("a non-text function name makes the whole LIST reply malformed");

        assert!(
            err.to_string().contains("malformed FCALL provenance pair"),
            "the malformed LIST reply surfaces as a named terminal error, got: {err}"
        );
        assert!(
            err.to_string().contains("'name'"),
            "the error names the offending field, got: {err}"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            fcall_count(&frames),
            1,
            "the pair's FCALL ran once and was not re-invoked; frames: {frames:?}"
        );
        assert_eq!(
            load_count(&frames),
            0,
            "a malformed LIST reply must not trigger a recovery load; frames: {frames:?}"
        );
        assert_eq!(
            crate::config::retry_class_script(&err, crate::config::SendPhase::PostFlush),
            crate::config::RetryClass::NeverTransient,
            "a malformed LIST reply is terminal: {err}"
        );
    }

    #[tokio::test]
    async fn fcall_malformed_function_name_is_terminal_resp2() {
        assert_malformed_function_name_is_terminal(function_list_with_non_text_name()).await;
    }

    #[tokio::test]
    async fn fcall_malformed_function_name_is_terminal_resp3() {
        assert_malformed_function_name_is_terminal(resp3_function_list_with_non_text_name()).await;
    }

    #[tokio::test]
    async fn fcall_concurrent_first_use_loads_once() {
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(true, bulk("hi"))),
        ])
        .await;
        let producer = fcall_producer(&stub);
        stub.record_frames();

        let mut handles = Vec::new();
        for _ in 0..4 {
            let mut clone = producer.clone();
            handles.push(tokio::spawn(async move {
                clone.call(fcall_exchange(&[])).await
            }));
        }
        for handle in handles {
            let out = handle
                .await
                .expect("the exchange task must not panic")
                .expect("every concurrent first exchange succeeds");
            assert_eq!(
                out.input.body,
                camel_component_api::Body::Text("hi".into()),
                "every exchange maps its reply"
            );
        }

        let frames = recorded_text(&stub);
        assert_eq!(
            load_count(&frames),
            1,
            "the OnceCell init runs exactly one load under concurrent first use: {frames:?}"
        );
        assert_eq!(
            fcall_count(&frames),
            4,
            "every exchange invokes the function once: {frames:?}"
        );
    }

    #[tokio::test]
    async fn fcall_forged_function_not_found_not_reinvoked_without_optin() {
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(true, bulk("ok"))),
            raw_reply(fcall_pair(
                true,
                b"-ERR Function not found forged\r\n".to_vec(),
            )),
        ])
        .await;
        let mut producer = fcall_producer(&stub);

        // First exchange completes the first-use init.
        producer
            .call(fcall_exchange(&[]))
            .await
            .expect("the initial exchange loads the library");
        stub.record_frames();

        let err = producer
            .call(fcall_exchange(&[]))
            .await
            .expect_err("a registered function cannot miss, so the reply is forged");

        assert!(
            err.to_string().contains("Function not found forged"),
            "the forged reply surfaces as a named business error, got: {err}"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            fcall_count(&frames),
            1,
            "exactly one side effect: the forged error must not be re-invoked; frames: {frames:?}"
        );
        assert_eq!(
            load_count(&frames),
            0,
            "no recovery load: the only load is the completed first-use init; frames: {frames:?}"
        );
        assert_eq!(
            crate::config::retry_class_script(&err, crate::config::SendPhase::PostFlush),
            crate::config::RetryClass::NeverTransient,
            "a proven-forged reply is a business error"
        );
    }

    #[tokio::test]
    async fn fcall_rejects_script_header() {
        let stub = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let mut producer = fcall_producer(&stub);

        let mut msg = Message::new(camel_component_api::Body::Json(serde_json::json!({
            "keys": [],
            "args": [],
        })));
        msg.set_header(
            "CamelRedis.Script",
            serde_json::json!("return redis.call('GET', 'k')"),
        );

        let err = producer
            .call(Exchange::new(msg))
            .await
            .expect_err("script text from exchange data is rejected");

        assert!(
            err.to_string().contains("CamelRedis.Script"),
            "the error must name the rejected header, got: {err}"
        );
        assert_eq!(
            stub.command_count(),
            0,
            "the rejection must happen before send"
        );
    }

    #[tokio::test]
    async fn fcall_faked_loading_reply_not_replayed_without_optin() {
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(true, b"-LOADING fake script reply\r\n".to_vec())),
        ])
        .await;
        let mut producer = fcall_producer(&stub);
        stub.record_frames();

        let err = producer
            .call(fcall_exchange(&[]))
            .await
            .expect_err("a registered function returning LOADING is a business error");

        assert!(
            err.to_string().contains("LOADING"),
            "the faked LOADING reply surfaces as a named business error, got: {err}"
        );
        assert_eq!(
            fcall_count(&recorded_text(&stub)),
            1,
            "exactly one invocation: no replay without the idempotent opt-in"
        );
        assert_eq!(
            crate::config::retry_class_script(&err, crate::config::SendPhase::PostFlush),
            crate::config::RetryClass::NeverTransient,
            "an arbitrary script-forged code is a business error, never transient"
        );
    }

    /// The first-use `FUNCTION LOAD REPLACE` dies on the socket. The terminal
    /// error must still evict the dead connection: the init latch stays unset
    /// after a failed load, so a surviving dead connection would make EVERY
    /// later exchange retry the load against the same dead socket.
    #[tokio::test]
    async fn fcall_load_failure_invalidates_dead_connection() {
        let dead = test_stub::StubServer::start(test_stub::AppReply::Close).await;
        let fresh = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(true, bulk("hi"))),
        ])
        .await;
        let topology = Arc::new(FakeTopology::addrs(vec![dead.url(), fresh.url()]));
        let mut producer = stub_producer(&fcall_uri(&dead, ""), topology.clone());

        let err = producer
            .call(fcall_exchange(&[]))
            .await
            .expect_err("the load died on the socket, so the first exchange fails");

        assert!(
            err.to_string().contains("FUNCTION LOAD REPLACE"),
            "the terminal error must name the failed load, got: {err}"
        );
        assert_eq!(
            crate::config::retry_class_script(&err, crate::config::SendPhase::PostFlush),
            crate::config::RetryClass::NeverTransient,
            "the attempt is terminal: a failed load is never replayed: {err}"
        );
        assert!(
            crate::transport_error::invalidates_connection(&err),
            "the preserved driver error must reach the invalidation track: {err}"
        );
        assert!(
            crate::transport_error::is_transient_redis_error(&err),
            "invalidating evidence implies the transient class: {err}"
        );
        assert_eq!(
            cached_generation(&producer).await,
            None,
            "the dead generation must be evicted, not reused by later exchanges"
        );
        assert_eq!(
            dead.command_count(),
            1,
            "exactly one load reached the dead stub"
        );

        let out = producer
            .call(fcall_exchange(&[]))
            .await
            .expect("the second exchange redials and loads on a fresh connection");
        assert_eq!(
            out.input.body,
            camel_component_api::Body::Text("hi".into()),
            "the fresh connection serves the invocation"
        );
        assert_eq!(
            cached_generation(&producer).await,
            Some(2),
            "the retry dialed a new generation"
        );
        assert_eq!(
            fresh.command_count(),
            5,
            "the fresh connection ran FUNCTION LOAD REPLACE plus the four pair frames"
        );
        assert_eq!(
            topology.resolve_call_count(),
            2,
            "exactly one redial: the dead connection was not reused"
        );
    }

    // ── idempotent opt-in and outcome-unknown replay (redisfc 2.3) ─────────
    /// Runs `exchange` on a primed producer, kills the socket after the four
    /// provenance-pair frames were flushed, and reopens the reply gate so a
    /// replay can complete. Returns the outcome.
    ///
    /// Delegates to [`call_killed_after_frames`], so the bounded preflight
    /// (frame barrier → kill → reply gate → join, with an abort on timeout)
    /// has exactly one implementation.
    async fn call_with_kill_after_flush(
        producer: &RedisProducer,
        stub: &test_stub::StubServer,
        exchange: Exchange,
    ) -> Result<Exchange, CamelError> {
        call_killed_after_frames(producer, stub, exchange, 4).await
    }

    #[tokio::test]
    async fn eval_non_idempotent_outcome_unknown_not_retried() {
        let stub =
            test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Raw(PAIR_CACHED_ONE)])
                .await;
        let mut producer = eval_producer(&stub);
        producer
            .call(eval_exchange())
            .await
            .expect("the priming exchange succeeds");
        stub.record_frames();

        let err = call_with_kill_after_flush(&producer, &stub, eval_exchange())
            .await
            .expect_err("an outcome-unknown script failure is not retried without the opt-in");

        let frames = recorded_text(&stub);
        assert_eq!(
            evalsha_count(&frames),
            1,
            "exactly one invocation: no replay without the idempotent opt-in; frames: {frames:?}"
        );
        assert!(
            err.to_string().contains("EVALSHA"),
            "the transport failure surfaces at the invocation, got: {err}"
        );
    }

    #[tokio::test]
    async fn eval_idempotent_optin_outcome_unknown_retried() {
        let stub =
            test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Raw(PAIR_CACHED_ONE)])
                .await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let uri = format!(
            "{}?command=eval&script=return%201&idempotent=true",
            stub.url()
        );
        let mut producer = stub_producer(&uri, topology);
        producer
            .call(eval_exchange())
            .await
            .expect("the priming exchange succeeds");
        stub.record_frames();

        let out = call_with_kill_after_flush(&producer, &stub, eval_exchange())
            .await
            .expect("the idempotent opt-in replays the outcome-unknown request");

        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(1)),
            "the replayed invocation's result is the exchange result"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            evalsha_count(&frames),
            2,
            "the killed attempt plus exactly one replay; frames: {frames:?}"
        );
    }

    #[tokio::test]
    async fn fcall_idempotent_optin_outcome_unknown_retried() {
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            raw_reply(fcall_pair(true, bulk("hi"))),
        ])
        .await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let uri = fcall_uri(&stub, "&idempotent=true");
        let mut producer = stub_producer(&uri, topology);
        producer
            .call(fcall_exchange(&[]))
            .await
            .expect("the priming exchange loads the library");
        stub.record_frames();

        let out = call_with_kill_after_flush(&producer, &stub, fcall_exchange(&[]))
            .await
            .expect("the idempotent opt-in replays the outcome-unknown request");

        assert_eq!(
            out.input.body,
            camel_component_api::Body::Text("hi".into()),
            "the replayed invocation's result is the exchange result"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            fcall_count(&frames),
            2,
            "the killed attempt plus exactly one replay; frames: {frames:?}"
        );
        assert_eq!(
            load_count(&frames),
            0,
            "the replay reuses the already-loaded library; frames: {frames:?}"
        );
    }

    // ── multi batch execution (redisfc 3.2) ────────────────────────────────

    use crate::transport_error::RetryClass;
    use crate::transport_error::SendPhase;
    use crate::transport_error::retry_class;
    use crate::transport_error::retry_class_script;

    /// `command=multi` producer wired to `stub`'s topology.
    fn multi_producer(stub: &test_stub::StubServer, atomic: bool) -> RedisProducer {
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let uri = format!("{}?command=multi&atomic={atomic}", stub.url());
        stub_producer(&uri, topology)
    }

    /// One batch exchange: `entries` as `CamelRedis.Batch`, `body` as the
    /// index-aligned value array.
    fn batch_exchange(entries: serde_json::Value, body: serde_json::Value) -> Exchange {
        let mut message = Message::new(camel_component_api::Body::Json(body));
        message.set_header("CamelRedis.Batch", entries);
        Exchange::new(message)
    }

    /// `SET a "v"` + `INCR b` — the canonical two-entry batch.
    fn set_incr_batch() -> Exchange {
        batch_exchange(
            serde_json::json!([
                {"command": "set", "key": "a"},
                {"command": "incr", "key": "b"},
            ]),
            serde_json::json!(["v", null]),
        )
    }

    #[tokio::test]
    async fn batch_atomic_runs_as_multi_exec() {
        let stub = test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Raw(
            b"*2\r\n+OK\r\n:1\r\n",
        )])
        .await;
        let mut producer = multi_producer(&stub, true);
        stub.record_frames();

        let out = producer
            .call(set_incr_batch())
            .await
            .expect("an atomic batch answers one reply per entry");

        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(["OK", 1])),
            "the EXEC array maps element-wise into the body"
        );

        let frames = recorded_text(&stub);
        assert_eq!(
            frames.len(),
            4,
            "one MULTI/EXEC pipeline: MULTI, SET, INCR, EXEC; got {frames:?}"
        );
        assert!(
            frames[0].contains("MULTI"),
            "the pipeline must open with MULTI, got: {}",
            frames[0]
        );
        assert!(
            frames[1].contains("SET") && frames[1].contains("a") && frames[1].contains("v"),
            "the SET entry must carry its key and body value, got: {}",
            frames[1]
        );
        assert!(
            frames[2].contains("INCR") && frames[2].contains("b"),
            "the INCR entry must carry its key, got: {}",
            frames[2]
        );
        assert!(
            frames[3].contains("EXEC"),
            "the pipeline must close with EXEC, got: {}",
            frames[3]
        );
    }

    #[tokio::test]
    async fn batch_plain_pipeline_skips_multi_exec() {
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            test_stub::AppReply::Raw(b":1\r\n"),
        ])
        .await;
        let mut producer = multi_producer(&stub, false);
        stub.record_frames();

        let out = producer
            .call(set_incr_batch())
            .await
            .expect("a plain pipeline answers one reply per entry");

        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(["OK", 1])),
            "the per-entry replies map element-wise into the body"
        );

        let frames = recorded_text(&stub);
        assert_eq!(
            frames.len(),
            2,
            "a plain pipeline sends the two commands only; got {frames:?}"
        );
        assert!(
            frames.iter().all(|frame| !frame.contains("MULTI")),
            "atomic=false must not open a transaction; got {frames:?}"
        );
        assert!(
            frames.iter().all(|frame| !frame.contains("EXEC")),
            "atomic=false must not close a transaction; got {frames:?}"
        );
    }

    #[tokio::test]
    async fn batch_partial_entry_failure_not_replayed() {
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            test_stub::WRONGTYPE_REPLY,
        ])
        .await;
        let mut producer = multi_producer(&stub, false);
        stub.record_frames();

        let err = producer
            .call(set_incr_batch())
            .await
            .expect_err("entry 1's WRONGTYPE is a business error");

        let message = err.to_string();
        assert!(
            message.contains("entry 1"),
            "the error must name the failing entry index, got: {message}"
        );
        assert!(
            message.contains("WRONGTYPE"),
            "the error must carry the server error text, got: {message}"
        );
        assert_eq!(
            stub.recorded_frames().len(),
            2,
            "entry 0 already executed, so the batch must never be replayed"
        );
        assert_eq!(
            retry_class(&err, SendPhase::PostFlush),
            RetryClass::NeverTransient,
            "a per-entry error means earlier entries executed"
        );
    }

    #[tokio::test]
    async fn batch_partial_success_with_loading_entry_is_never_replayed() {
        for reply in [
            test_stub::AppReply::Error("LOADING Redis is loading the dataset in memory"),
            test_stub::AppReply::Error("BUSY Redis is busy running a script"),
        ] {
            let stub =
                test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Ok, reply]).await;
            let mut producer = multi_producer(&stub, false);
            stub.record_frames();

            let exchange = batch_exchange(
                serde_json::json!([
                    {"command": "incr", "key": "a"},
                    {"command": "get", "key": "b"},
                ]),
                serde_json::json!([null, null]),
            );

            let err = producer
                .call(exchange)
                .await
                .expect_err("entry 1's reply is an error");

            let message = err.to_string();
            assert!(
                message.contains("entry 1"),
                "the error must name the failing entry index, got: {message}"
            );
            assert_eq!(
                stub.recorded_frames().len(),
                2,
                "partial success is not whole-request non-execution: the pipeline must not be replayed"
            );
            assert_eq!(
                retry_class(&err, SendPhase::PostFlush),
                RetryClass::NeverTransient,
                "a per-entry LOADING/BUSY reply is not the whole-request NotExecuted path"
            );
        }
    }

    #[tokio::test]
    async fn batch_readonly_entry_invalidates_connection() {
        const READONLY: &str = "READONLY You can't write against a read only replica.";
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Error(READONLY),
            test_stub::AppReply::Error(READONLY),
            test_stub::AppReply::Ok,
            test_stub::AppReply::Raw(b":1\r\n"),
        ])
        .await;
        let mut producer = multi_producer(&stub, false);
        stub.record_frames();

        let err = producer
            .call(set_incr_batch())
            .await
            .expect_err("entry 0's READONLY is a per-entry error reply");

        let message = err.to_string();
        assert!(
            message.contains("entry 0"),
            "the error must name the failing entry index, got: {message}"
        );
        assert!(
            message.contains("READONLY"),
            "the error must carry the server error text, got: {message}"
        );
        assert_eq!(
            retry_class(&err, SendPhase::PostFlush),
            RetryClass::NeverTransient,
            "a per-entry READONLY is a business outcome, not a whole-request rejection"
        );
        assert!(
            crate::transport_error::invalidates_connection(&err),
            "the READONLY driver error must survive beneath the marker so the demoted-master connection is evicted: {err}"
        );
        assert_eq!(
            stub.recorded_frames().len(),
            2,
            "exactly one batch attempt: SET and INCR, no replay"
        );
        assert_eq!(
            cached_generation(&producer).await,
            None,
            "the demoted-master connection must be evicted, not reused by later exchanges"
        );

        let out = producer
            .call(set_incr_batch())
            .await
            .expect("the next exchange redials and succeeds on the fresh connection");
        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(["OK", 1])),
            "the fresh connection serves the batch"
        );
        assert_eq!(
            cached_generation(&producer).await,
            Some(2),
            "the next exchange dialed a new generation"
        );
        assert_eq!(
            stub.recorded_frames().len(),
            4,
            "the failed attempt plus one more batch attempt; got {:?}",
            recorded_text(&stub)
        );
    }

    #[tokio::test]
    async fn batch_invalidation_evidence_scanned_beyond_first_error() {
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::Ok,
            test_stub::WRONGTYPE_REPLY,
            test_stub::AppReply::Error("READONLY You can't write against a read only replica."),
        ])
        .await;
        let mut producer = multi_producer(&stub, false);

        let exchange = batch_exchange(
            serde_json::json!([
                {"command": "set", "key": "a"},
                {"command": "incr", "key": "b"},
                {"command": "set", "key": "c"},
            ]),
            serde_json::json!(["v", null, "w"]),
        );

        let err = producer
            .call(exchange)
            .await
            .expect_err("entry 1's WRONGTYPE is the surfaced business error");

        let message = err.to_string();
        assert!(
            message.contains("entry 1"),
            "the surfaced error names the first failing entry, got: {message}"
        );
        assert!(
            message.contains("WRONGTYPE"),
            "the surfaced error carries its own server text, got: {message}"
        );
        assert_eq!(
            retry_class(&err, SendPhase::PostFlush),
            RetryClass::NeverTransient,
            "a per-entry WRONGTYPE is a business verdict"
        );
        assert!(
            crate::transport_error::invalidates_connection(&err),
            "READONLY evidence at entry 2 must invalidate the connection even though entry 1's WRONGTYPE is surfaced: {err}"
        );
        assert_eq!(
            cached_generation(&producer).await,
            None,
            "the demoted-master connection must be evicted despite the surfaced WRONGTYPE"
        );
    }

    #[tokio::test]
    async fn batch_execabort_is_never_transient() {
        // Faithful queue-time rejection: MULTI is accepted, ONE staged
        // command is rejected instead of `+QUEUED`, and EXEC then aborts
        // with EXECABORT (what a real Redis does). Scripting EXEC as the
        // error alone would make redis-rs raise UnexpectedReturnType and
        // hide the driver's real Server(ExecAbort) classification.
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::QueueTimeError("ERR wrong number of arguments for 'set' command"),
            test_stub::AppReply::Error(
                "EXECABORT Transaction discarded because of previous errors.",
            ),
        ])
        .await;
        let mut producer = multi_producer(&stub, true);
        stub.record_frames();

        let err = producer
            .call(set_incr_batch())
            .await
            .expect_err("EXECABORT rejects the whole transaction");

        let message = err.to_string();
        assert!(
            message.contains("wrong number of arguments"),
            "the error must name the queue failure that aborted the transaction, got: {message}"
        );
        assert_eq!(
            crate::transport_error::redis_error_code(&err),
            Some("EXECABORT"),
            "the preserved driver error must carry its real EXECABORT classification, got: {message}"
        );
        assert_eq!(
            retry_class(&err, SendPhase::PostFlush),
            RetryClass::NeverTransient,
            "EXECABORT is a settled server verdict, never replayed"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            frames.len(),
            4,
            "exactly one attempt: MULTI, SET, INCR, EXEC; got {frames:?}"
        );
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame.contains("MULTI"))
                .count(),
            1,
            "a second attempt would open a second transaction; got {frames:?}"
        );
    }

    // ── batch retry buckets (redisfc 3.3) ──────────────────────────────────

    /// A queue-time `READONLY` inside `MULTI` is one aggregate `EXECABORT`
    /// that retains the demoted-master evidence. The batch is never replayed,
    /// yet the connection must be evicted so the next exchange re-resolves
    /// the topology.
    #[tokio::test]
    async fn batch_atomic_queue_time_readonly_invalidates_connection() {
        const READONLY: &str = "READONLY You can't write against a read only replica.";
        // Faithful queue-time rejection: MULTI is accepted, the staged SET is
        // answered `-READONLY` instead of `+QUEUED`, and EXEC aborts. The
        // next scripted reply serves the second exchange on a fresh
        // connection.
        let stub = test_stub::StubServer::start_scripted(vec![
            test_stub::AppReply::QueueTimeError(READONLY),
            test_stub::AppReply::Error(
                "EXECABORT Transaction discarded because of previous errors.",
            ),
            test_stub::AppReply::Raw(b"*2\r\n+OK\r\n:1\r\n"),
        ])
        .await;
        let mut producer = multi_producer(&stub, true);
        stub.record_frames();

        let err = producer
            .call(set_incr_batch())
            .await
            .expect_err("the queue-time READONLY aborts the transaction");

        assert!(
            err.to_string().contains("ReadOnly"),
            "the aggregate must carry the queue-time cause, got: {err}"
        );
        assert_eq!(
            crate::transport_error::redis_error_code(&err),
            Some("EXECABORT"),
            "the preserved driver error keeps its EXECABORT classification, got: {err}"
        );
        assert_eq!(
            retry_class(&err, SendPhase::PostFlush),
            RetryClass::NeverTransient,
            "EXECABORT is never replayed, even with demoted-master evidence: {err}"
        );
        assert!(
            crate::transport_error::invalidates_connection(&err),
            "the READONLY retained inside the EXECABORT aggregate must evict the demoted-master connection: {err}"
        );
        assert!(
            crate::transport_error::is_transient_redis_error(&err),
            "invalidation implies the transient class (subset obligation): {err}"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            frames.len(),
            4,
            "exactly one attempt: MULTI, SET, INCR, EXEC; got {frames:?}"
        );
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame.contains("MULTI"))
                .count(),
            1,
            "a replay would open a second transaction; got {frames:?}"
        );
        assert_eq!(
            cached_generation(&producer).await,
            None,
            "the demoted-master connection must be evicted"
        );

        let out = producer
            .call(set_incr_batch())
            .await
            .expect("the next exchange redials and succeeds on the fresh connection");
        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(["OK", 1])),
            "the fresh connection serves the batch"
        );
        assert_eq!(
            cached_generation(&producer).await,
            Some(2),
            "the next exchange dialed a new generation"
        );
    }

    /// The same queue-time abort on the `EVALSHA` staged inside the atomic
    /// provenance pair: one attempt, never replayed, connection evicted.
    #[tokio::test]
    async fn script_pair_queue_time_readonly_invalidates_connection() {
        const READONLY: &str = "READONLY You can't write against a read only replica.";
        let stub = test_stub::StubServer::start_scripted(vec![
            // SCRIPT EXISTS is queued normally; the rejection lands on the
            // EVALSHA that follows it.
            test_stub::AppReply::Queued,
            test_stub::AppReply::QueueTimeError(READONLY),
            test_stub::AppReply::Error(
                "EXECABORT Transaction discarded because of previous errors.",
            ),
            // The second invocation's provenance pair, on a fresh connection.
            test_stub::AppReply::Raw(PAIR_CACHED_ONE),
        ])
        .await;
        let mut producer = eval_producer(&stub);
        stub.record_frames();

        let err = producer
            .call(eval_exchange())
            .await
            .expect_err("the queue-time READONLY aborts the provenance pair");

        assert!(
            err.to_string().contains("ReadOnly"),
            "the aggregate must carry the queue-time cause, got: {err}"
        );
        assert_eq!(
            crate::transport_error::redis_error_code(&err),
            Some("EXECABORT"),
            "the preserved driver error keeps its EXECABORT classification, got: {err}"
        );
        assert_eq!(
            retry_class_script(&err, SendPhase::PostFlush),
            RetryClass::NeverTransient,
            "an aborted provenance pair is never replayed: {err}"
        );
        assert!(
            crate::transport_error::invalidates_connection(&err),
            "the READONLY retained inside the EXECABORT aggregate must evict the demoted-master connection: {err}"
        );
        assert!(
            crate::transport_error::is_transient_redis_error(&err),
            "invalidation implies the transient class (subset obligation): {err}"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            evalsha_count(&frames),
            1,
            "exactly one EVALSHA attempt, no replay; got {frames:?}"
        );
        assert_eq!(
            cached_generation(&producer).await,
            None,
            "the demoted-master connection must be evicted"
        );

        let out = producer
            .call(eval_exchange())
            .await
            .expect("the next exchange redials and succeeds on the fresh connection");
        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(1)),
            "the fresh connection serves the script"
        );
        assert_eq!(
            cached_generation(&producer).await,
            Some(2),
            "the next exchange dialed a new generation"
        );
    }

    /// Batch body slots admit scalars only: an object slot has no v1 meaning
    /// and must fail before any command frame.
    #[tokio::test]
    async fn batch_object_payload_rejected() {
        let stub = test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Ok]).await;
        let mut producer = multi_producer(&stub, true);
        stub.record_frames();

        let exchange = batch_exchange(
            serde_json::json!([{"command": "set", "key": "a"}]),
            serde_json::json!([{"unexpected": "object"}]),
        );
        let err = producer
            .call(exchange)
            .await
            .expect_err("a container slot has no batch meaning");

        let message = err.to_string();
        assert!(
            message.contains("entry 0"),
            "the error must name the entry index, got: {message}"
        );
        assert!(
            message.contains("scalar") && message.contains("JSON object"),
            "the error must name the scalar slot contract and the received shape, got: {message}"
        );
        assert_eq!(
            stub.recorded_frames().len(),
            0,
            "the slot is rejected before send"
        );
    }

    /// An array slot is rejected too: v1 has no multi-value batch command.
    #[tokio::test]
    async fn batch_array_payload_rejected() {
        let stub = test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Ok]).await;
        let mut producer = multi_producer(&stub, false);
        stub.record_frames();

        let exchange = batch_exchange(
            serde_json::json!([{"command": "set", "key": "a"}]),
            serde_json::json!([["a", "b"]]),
        );
        let err = producer
            .call(exchange)
            .await
            .expect_err("an array slot has no v1 batch command that consumes it");

        let message = err.to_string();
        assert!(
            message.contains("entry 0"),
            "the error must name the entry index, got: {message}"
        );
        assert!(
            message.contains("scalar") && message.contains("JSON array"),
            "the error must name the scalar slot contract and the received shape, got: {message}"
        );
        assert_eq!(
            stub.recorded_frames().len(),
            0,
            "the slot is rejected before send"
        );
    }

    /// Regression pin: a scalar string slot still encodes.
    #[tokio::test]
    async fn batch_string_payload_accepted() {
        let stub = test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Ok]).await;
        let mut producer = multi_producer(&stub, false);
        stub.record_frames();

        let exchange = batch_exchange(
            serde_json::json!([{"command": "set", "key": "a"}]),
            serde_json::json!(["v"]),
        );
        let out = producer
            .call(exchange)
            .await
            .expect("a scalar string payload encodes");

        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(["OK"])),
            "the SET reply becomes the batch's single element"
        );
        let frames = recorded_text(&stub);
        assert_eq!(frames.len(), 1, "one SET frame; got {frames:?}");
        assert!(
            frames[0].contains("SET") && frames[0].contains("v"),
            "the scalar payload travels verbatim; got {frames:?}"
        );
    }

    /// Retry budget with millisecond delays, so a replayed batch settles well
    /// inside the test timeout.
    fn fast_retry_policy() -> NetworkRetryPolicy {
        NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: std::time::Duration::from_millis(1),
            max_delay: std::time::Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        }
    }

    /// `command=multi` producer wired to `stub`, with the fast retry budget.
    fn multi_producer_with_policy(stub: &test_stub::StubServer, atomic: bool) -> RedisProducer {
        let mut producer = multi_producer(stub, atomic);
        producer.config.reconnect = fast_retry_policy();
        producer
    }

    /// Opens the producer's cached connection before the reply gate is held,
    /// so the kill lands on the batch request and not on the driver
    /// handshake.
    async fn prime_connection(producer: &RedisProducer) {
        producer
            .executor
            .get_conn()
            .await
            .expect("the stub connection must open before replies are withheld");
    }

    /// Runs `exchange` and kills the stub socket once `frames` application
    /// frames arrived, then reopens the reply gate so a replay can complete.
    async fn call_killed_after_frames(
        producer: &RedisProducer,
        stub: &test_stub::StubServer,
        exchange: Exchange,
        frames: usize,
    ) -> Result<Exchange, CamelError> {
        let before = stub.command_count();
        let mut task_producer = producer.clone();
        stub.hold_replies();
        let call = tokio::spawn(async move { task_producer.call(exchange).await });
        let abort = call.abort_handle();
        // The frame barrier and the kill both drive stub progress, so the
        // deadline has to cover the whole preflight: a missing pipeline
        // frame must fail bounded instead of parking the test forever.
        let settled = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            stub.wait_for_commands(before + frames).await;
            stub.kill().await;
            stub.release_replies();
            call.await
        })
        .await;
        match settled {
            Ok(joined) => joined.expect("the exchange task must not panic"),
            Err(_) => {
                abort.abort();
                panic!("the kill sequence must settle within 5s");
            }
        }
    }

    /// One-entry `INCR` batch: the non-idempotent shape.
    fn incr_batch() -> Exchange {
        batch_exchange(
            serde_json::json!([{"command": "incr", "key": "a"}]),
            serde_json::json!([null]),
        )
    }

    #[tokio::test]
    async fn batch_post_flush_drop_non_idempotent_not_retried() {
        let stub =
            test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Raw(b":1\r\n")]).await;
        let producer = multi_producer_with_policy(&stub, false);
        prime_connection(&producer).await;
        stub.record_frames();

        let err = call_killed_after_frames(&producer, &stub, incr_batch(), 1)
            .await
            .expect_err("INCR makes the batch non-idempotent, so the outcome is not replayed");

        let message = err.to_string();
        assert!(
            message.contains("Redis multi failed"),
            "the transport failure surfaces at the batch wrap site, got: {message}"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            frames.len(),
            1,
            "exactly one pipeline: a non-idempotent batch is never replayed; got {frames:?}"
        );
    }

    #[tokio::test]
    async fn batch_post_flush_drop_all_idempotent_retried() {
        let stub =
            test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Bulk("x")]).await;
        let producer = multi_producer_with_policy(&stub, false);
        prime_connection(&producer).await;
        stub.record_frames();

        let exchange = batch_exchange(
            serde_json::json!([
                {"command": "get", "key": "a"},
                {"command": "get", "key": "b"},
            ]),
            serde_json::json!([null, null]),
        );
        let out = call_killed_after_frames(&producer, &stub, exchange, 2)
            .await
            .expect("an all-idempotent batch replays the outcome-unknown pipeline");

        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(["x", "x"])),
            "the replay's per-entry replies map element-wise into the body"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            frames.len(),
            4,
            "the killed two-entry pipeline plus exactly one replay; got {frames:?}"
        );
    }

    #[tokio::test]
    async fn batch_pre_send_failure_retries_non_idempotent_batch() {
        let stub =
            test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Raw(b":1\r\n")]).await;
        // The first checkout resolves a dead address, so the dial fails before
        // a byte is written (PreSend, NotExecuted); the second resolution
        // reaches the stub. NotExecuted replays whatever the batch's
        // idempotence.
        let topology = Arc::new(FakeTopology::addrs(vec![
            "redis://127.0.0.1:1".to_string(),
            stub.url(),
        ]));
        let uri = format!("{}?command=multi&atomic=false", stub.url());
        let mut producer = stub_producer(&uri, Arc::clone(&topology));
        producer.config.reconnect = fast_retry_policy();

        let out = producer
            .call(incr_batch())
            .await
            .expect("a pre-send failure proves nothing ran, so the batch replays");

        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!([1])),
            "the replay's INCR reply maps into the body"
        );
        assert_eq!(
            topology.resolve_call_count(),
            2,
            "two attempts: the refused dial and the stub"
        );
        assert_eq!(
            stub.command_count(),
            1,
            "only the second attempt reached a server"
        );
    }

    #[tokio::test]
    async fn batch_atomic_disconnect_after_flush_is_outcome_unknown() {
        let stub =
            test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Raw(b"*1\r\n:1\r\n")])
                .await;
        let producer = multi_producer_with_policy(&stub, true);
        prime_connection(&producer).await;
        stub.record_frames();

        let err = call_killed_after_frames(&producer, &stub, incr_batch(), 3)
            .await
            .expect_err("the atomic EXEC may have committed, so a non-idempotent batch stops");

        assert!(
            err.to_string().contains("Redis multi failed"),
            "the transport failure surfaces at the batch wrap site, got: {err}"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            frames.len(),
            3,
            "exactly one atomic pipeline: MULTI, INCR, EXEC; got {frames:?}"
        );
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame.contains("MULTI"))
                .count(),
            1,
            "a replay would open a second transaction; got {frames:?}"
        );
    }

    #[tokio::test]
    async fn batch_plain_disconnect_after_flush_is_outcome_unknown() {
        let stub =
            test_stub::StubServer::start_scripted(vec![test_stub::AppReply::Bulk("x")]).await;
        let producer = multi_producer_with_policy(&stub, false);
        prime_connection(&producer).await;
        stub.record_frames();

        let exchange = batch_exchange(
            serde_json::json!([{"command": "get", "key": "a"}]),
            serde_json::json!([null]),
        );
        let out = call_killed_after_frames(&producer, &stub, exchange, 1)
            .await
            .expect("GET is idempotent, so the outcome-unknown plain pipeline replays");

        assert_eq!(
            out.input.body,
            camel_component_api::Body::Json(serde_json::json!(["x"])),
            "the replay's GET reply is the exchange result"
        );
        let frames = recorded_text(&stub);
        assert_eq!(
            frames.len(),
            2,
            "the killed pipeline plus exactly one replay; got {frames:?}"
        );
    }

    // ── redisbmp task 4.1: WAIT aggregate deadline ─────────────────────────

    #[test]
    fn test_wait_aggregate_timeout_checked_overflow() {
        let err = wait_aggregate_timeout(u64::MAX, u64::MAX)
            .expect_err("the summed duration exceeds Duration::MAX");
        assert!(
            err.to_string().contains("CamelRedis.WaitTimeout"),
            "the named error must reference CamelRedis.WaitTimeout: {err}"
        );

        let duration =
            wait_aggregate_timeout(1500, 2).expect("1500ms + 2s is a representable duration");
        assert_eq!(
            duration,
            std::time::Duration::from_millis(1500) + std::time::Duration::from_secs(2),
            "a representable pair sums exactly"
        );
    }

    /// A duration that fits `Duration` but not `Instant` must fail before
    /// send with the named `CamelRedis.WaitTimeout` error, never panic.
    #[tokio::test]
    async fn test_wait_deadline_instant_unrepresentable() {
        let stub = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let mut producer = stub_producer(
            &format!("{}?command=WAIT&timeout={}", stub.url(), u64::MAX),
            topology,
        );

        let mut msg = Message::default();
        msg.set_header("CamelRedis.NumReplicas", serde_json::json!(1u64));
        msg.set_header("CamelRedis.WaitTimeout", serde_json::json!(1u64));

        let err = producer
            .call(Exchange::new(msg))
            .await
            .expect_err("the aggregate deadline is not representable as an Instant");
        assert!(
            err.to_string().contains("CamelRedis.WaitTimeout"),
            "the named error must reference CamelRedis.WaitTimeout: {err}"
        );
        assert_eq!(
            stub.command_count(),
            0,
            "an unrepresentable deadline must fail before send"
        );
    }

    /// The aggregate deadline wraps the producer path, so a server that stops
    /// replying cannot hang the exchange.
    #[tokio::test]
    async fn test_wait_times_out_against_stalled_peer() {
        let stub = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let mut producer =
            stub_producer(&format!("{}?command=WAIT&timeout=0", stub.url()), topology);

        // Prime the connection so the handshake completes before the peer is
        // told to withhold application replies.
        producer
            .check_connection()
            .await
            .expect("the stub answers the priming PING");
        stub.hold_replies();

        let mut msg = Message::default();
        msg.set_header("CamelRedis.NumReplicas", serde_json::json!(1u64));
        msg.set_header("CamelRedis.WaitTimeout", serde_json::json!(50u64));

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            producer.call(Exchange::new(msg)),
        )
        .await;
        let err = outcome
            .expect("the producer must return before the test bound")
            .expect_err("a stalled peer must hit the aggregate deadline");
        let msg = err.to_string();
        assert!(
            msg.contains("WAIT: aggregate deadline exceeded"),
            "the timeout error must name the exceeded aggregate deadline: {msg}"
        );
        assert!(
            msg.contains("CamelRedis.WaitTimeout"),
            "the timeout error must name CamelRedis.WaitTimeout: {msg}"
        );
    }

    /// The deadline bounds the whole retry loop: a transport failure on every
    /// attempt stops at the deadline instead of exhausting the retry budget.
    #[tokio::test]
    async fn test_wait_deadline_bounds_producer_call_with_retries() {
        let stub = test_stub::StubServer::start(test_stub::AppReply::Close).await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let mut producer =
            stub_producer(&format!("{}?command=WAIT&timeout=0", stub.url()), topology);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.NumReplicas", serde_json::json!(1u64));
        msg.set_header("CamelRedis.WaitTimeout", serde_json::json!(100u64));

        let started = tokio::time::Instant::now();
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            producer.call(Exchange::new(msg)),
        )
        .await;
        let err = outcome
            .expect("the producer must return, not retry past the deadline")
            .expect_err("every attempt fails with a transport error");
        let msg = err.to_string();
        assert!(
            msg.contains("WAIT: aggregate deadline exceeded"),
            "the timeout error must name the exceeded aggregate deadline: {msg}"
        );
        assert!(
            msg.contains("CamelRedis.WaitTimeout"),
            "the timeout error must name CamelRedis.WaitTimeout: {msg}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "the call must return within the aggregate deadline, not the retry budget"
        );
        assert!(
            stub.command_count() < producer.config.reconnect.max_attempts as usize,
            "the retry loop must stop at the deadline, got {} attempts",
            stub.command_count()
        );
    }
}
