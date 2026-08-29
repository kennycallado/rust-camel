use async_trait::async_trait;
use camel_component_api::{CamelError, Exchange, NetworkRetryPolicy};
// retry_async is used in tests (the regression test in this file).
#[cfg(test)]
use camel_component_api::retry_async;
use redis::AsyncConnectionConfig;
use redis::aio::MultiplexedConnection;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;

use crate::commands;
use crate::config::{RedisCommand, RedisEndpointConfig, is_transient_redis_error};
use crate::topology::{RedisTopology, ServerKind};

/// Abstraction over a Redis connection that can execute commands.
///
/// This trait enables testing retry/reconnect behavior without a live Redis
/// by allowing injection of a fake implementation.
#[async_trait]
pub trait RedisCommandExecutor: Send + Sync {
    /// Execute a Redis command against the given exchange.
    async fn execute_command(
        &mut self,
        cmd: &RedisCommand,
        exchange: &mut Exchange,
    ) -> Result<(), CamelError>;

    /// Reconnect the underlying connection.
    async fn reconnect(&mut self) -> Result<(), CamelError>;
}

/// Fake executor for testing retry behavior.
///
/// Configured with a sequence of results to return. Each call to
/// `execute_command` consumes the next result. After exhausting the
/// sequence, returns `Ok(())`.
pub struct FakeExecutor {
    /// Pre-programmed results: each call pops the next one.
    results: Arc<Mutex<Vec<Result<(), FakeError>>>>,
    /// Counter of how many times `execute_command` was called.
    pub call_count: Arc<AtomicUsize>,
    /// Counter of how many times `reconnect` was called.
    pub reconnect_count: Arc<AtomicUsize>,
}

/// Error type for the fake executor, can be transient or non-transient.
#[derive(Debug, Clone)]
pub struct FakeError {
    pub message: String,
    pub is_transient: bool,
}

impl std::fmt::Display for FakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl FakeExecutor {
    /// Creates a new fake executor with the given result sequence.
    pub fn new(results: Vec<Result<(), FakeError>>) -> Self {
        Self {
            results: Arc::new(Mutex::new(results)),
            call_count: Arc::new(AtomicUsize::new(0)),
            reconnect_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns a shared reference to the call counter.
    pub fn call_count(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.call_count)
    }

    /// Returns a shared reference to the reconnect counter.
    pub fn reconnect_count(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.reconnect_count)
    }
}

#[async_trait]
impl RedisCommandExecutor for FakeExecutor {
    async fn execute_command(
        &mut self,
        _cmd: &RedisCommand,
        _exchange: &mut Exchange,
    ) -> Result<(), CamelError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        let result = {
            let mut results = self.results.lock().await;
            results.pop().unwrap_or(Ok(()))
        };

        match result {
            Ok(()) => Ok(()),
            Err(fake_err) => {
                if fake_err.is_transient {
                    Err(CamelError::ProcessorError(format!(
                        "Connection error: {}",
                        fake_err.message
                    )))
                } else {
                    Err(CamelError::ProcessorError(fake_err.message))
                }
            }
        }
    }

    async fn reconnect(&mut self) -> Result<(), CamelError> {
        self.reconnect_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Dispatches a Redis command to the appropriate module handler.
///
/// Moved here from `RedisProducer` so the producer no longer owns the
/// command-to-module mapping. The producer (and any executor) calls this free
/// function with a live connection.
pub async fn dispatch_command(
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

/// A real [`RedisCommandExecutor`] backed by a lazily-created multiplexed
/// connection whose target is re-resolved through a [`RedisTopology`].
///
/// The connection is created on first use and cached.
/// [`RedisCommandExecutor::reconnect`] drops the cached connection and rebuilds
/// it, which re-resolves the master address through the topology — this is what
/// enables sentinel failover detection.
///
// log-policy: the transient retry `warn!` in `execute_with_retry` is
// category (e) outside-contract (ADR-0012). Error messages here are kept
// redaction-safe via `config.redis_url_safe()`.
#[derive(Clone)]
pub struct MultiplexedExecutor {
    config: RedisEndpointConfig,
    topology: Arc<dyn RedisTopology>,
    conn: Arc<Mutex<Option<MultiplexedConnection>>>,
    /// Single-flight guard collapsing concurrent cache-miss builds to one
    /// leader (storm elimination at failover). Waiters double-check the
    /// cache after the gate; cancellation-safe — a dropped leader releases
    /// it.
    // Lock ordering: the gate is always acquired BEFORE `conn` for the
    // double-check and store sections; the fast path takes only `conn`;
    // the reverse nesting never occurs.
    connect_gate: std::sync::Arc<tokio::sync::Mutex<()>>,
    response_timeout: Option<Duration>,
}

impl MultiplexedExecutor {
    /// Create a new executor that resolves connections through `topology`.
    pub fn new(config: RedisEndpointConfig, topology: Arc<dyn RedisTopology>) -> Self {
        Self {
            config,
            topology,
            conn: Arc::new(Mutex::new(None)),
            connect_gate: Arc::new(tokio::sync::Mutex::new(())),
            response_timeout: None,
        }
    }

    /// Set the driver-level per-command response deadline.
    ///
    /// When `Some`, [`Self::get_conn`] builds connections with
    /// [`AsyncConnectionConfig::set_response_timeout`], so each command that
    /// awaits a Redis reply fails once `timeout` elapses. The default `None`
    /// keeps the redis driver default (500ms in 1.6.0).
    ///
    /// This does not affect `connection_timeout_secs` (TCP connect only).
    /// It is distinct from camel-redis-repo's
    /// `MultiplexedRepoExecutor::with_response_timeout` test seam: that sizes
    /// the LOCAL backstop, this sizes the DRIVER deadline.
    pub fn with_response_timeout(mut self, timeout: Duration) -> Self {
        self.response_timeout = Some(timeout);
        self
    }

    /// Return a cached connection, or resolve and build one on first use.
    ///
    /// `pub` so out-of-crate services (repository service) can reuse the same
    /// lazily-cached connection as the producer's `check_connection`.
    pub async fn get_conn(&self) -> Result<MultiplexedConnection, CamelError> {
        // Step A (fast path): reuse the cached connection. Only `conn` is
        // locked here — a cache hit never touches the connect gate.
        {
            let guard = self.conn.lock().await;
            if let Some(c) = guard.as_ref() {
                return Ok(c.clone());
            }
        }

        // Step B: single-flight gate, held through the double-check and
        // the leader build below so concurrent cache misses collapse to
        // one connector.
        let _gate = self.connect_gate.lock().await;

        // Step C (double-check): a leader may have stored a connection
        // while this caller waited for the gate.
        {
            let guard = self.conn.lock().await;
            if let Some(c) = guard.as_ref() {
                return Ok(c.clone());
            }
        }

        // Step D (leader): resolve the master address through the
        // topology, then connect.
        let client = self.topology.resolve(ServerKind::Master).await?;
        let redis_url_safe = self.config.redis_url_safe();
        let timeout_secs = self.config.connection_timeout_secs;

        let connect = async {
            match self.response_timeout {
                Some(t) => {
                    client
                        .get_multiplexed_async_connection_with_config(
                            &AsyncConnectionConfig::new()
                                .set_response_timeout(Some(t))
                                .set_connection_timeout(None),
                        )
                        .await
                }
                None => client.get_multiplexed_async_connection().await,
            }
        };

        let new_conn = tokio::time::timeout(Duration::from_secs(timeout_secs), connect)
            .await
            .map_err(|_| {
                CamelError::ProcessorError(format!(
                    "Redis connection to '{}' timed out after {}s",
                    redis_url_safe, timeout_secs
                ))
            })?
            .map_err(|e| {
                CamelError::ProcessorError(format!(
                    "Failed to connect to Redis at '{}': {}",
                    redis_url_safe, e
                ))
            })?;

        let mut guard = self.conn.lock().await;
        *guard = Some(new_conn.clone());
        Ok(new_conn)
    }

    /// Clear the cached connection and resolve a fresh one through the
    /// topology.
    ///
    /// Inherent `&self` counterpart of [`RedisCommandExecutor::reconnect`]
    /// for callers that hold a shared (non-`mut`) executor, e.g. the
    /// repository service. Mirrors the trait method: drops the cached
    /// connection, then builds a new one via [`Self::get_conn`], which
    /// re-resolves the master address.
    pub async fn refresh(&self) -> Result<MultiplexedConnection, CamelError> {
        // Drop the cached connection so the get_conn below re-resolves.
        {
            let mut guard = self.conn.lock().await;
            *guard = None;
        }
        self.get_conn().await
    }

    /// Expose the shared connection Arc so sibling-module tests (producer) can
    /// assert that clones share the same underlying connection.
    #[cfg(test)]
    pub(crate) fn conn_arc(&self) -> Arc<Mutex<Option<MultiplexedConnection>>> {
        Arc::clone(&self.conn)
    }

    /// Expose the single-flight gate Arc so in-module tests can hold or
    /// probe the gate directly.
    #[cfg(test)]
    pub(crate) fn gate_arc(&self) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        std::sync::Arc::clone(&self.connect_gate)
    }
}

#[async_trait]
impl RedisCommandExecutor for MultiplexedExecutor {
    async fn execute_command(
        &mut self,
        cmd: &RedisCommand,
        exchange: &mut Exchange,
    ) -> Result<(), CamelError> {
        let mut conn = self.get_conn().await?;
        dispatch_command(cmd, &mut conn, exchange).await
    }

    async fn reconnect(&mut self) -> Result<(), CamelError> {
        // Drop the cached connection so the next get_conn re-resolves.
        {
            let mut guard = self.conn.lock().await;
            *guard = None;
        }
        self.get_conn().await?;
        Ok(())
    }
}

/// Executes a command with retry on transient errors, using `NetworkRetryPolicy`.
///
/// This is the core retry logic extracted for testability.
///
/// - `policy`: reconnection policy (max attempts, backoff, jitter, etc.)
/// - `is_idempotent`: whether the command is safe to retry
///
/// # Implementation note
///
/// Uses a manual retry loop calling [`NetworkRetryPolicy::should_retry`] and
/// [`NetworkRetryPolicy::delay_for`] rather than the shared [`retry_async`]
/// helper. Manual loop needed because: (a) `executor.reconnect()` must run
/// before each retry attempt (not the first attempt), so `retry_async`'s
/// "always invoke op" model doesn't fit; (b) `&mut executor` / `&mut exchange`
/// borrows cannot be re-borrowed through `FnMut() -> async move { ... }` —
/// the Future returned by the closure holds the borrow past the closure body,
/// which the borrow checker rejects. `retry_async_cancelable` has the same
/// FnMut constraint so it is also excluded.
pub async fn execute_with_retry<E: RedisCommandExecutor>(
    executor: &mut E,
    cmd: &RedisCommand,
    exchange: &mut Exchange,
    is_idempotent: bool,
    policy: &NetworkRetryPolicy,
) -> Result<(), CamelError> {
    // Non-idempotent commands must not be retried — use a disabled policy.
    let effective_policy = if is_idempotent {
        policy.clone()
    } else {
        NetworkRetryPolicy::disabled()
    };

    let mut attempt: u32 = 0;

    loop {
        // Reconnect before retries (attempt > 0), not on the initial try.
        // This preserves the existing reconnect-before-retry semantics.
        if attempt > 0 {
            executor.reconnect().await?;
        }

        match executor.execute_command(cmd, exchange).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                if !is_transient_redis_error(&err) || !effective_policy.should_retry(attempt + 1) {
                    return Err(err);
                }
                let delay = effective_policy.delay_for(attempt);
                // log-policy: outside-contract
                tracing::warn!(
                    attempt,
                    delay_ms = delay.as_millis(),
                    error = %err,
                    "transient error — retrying"
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::FakeTopology;
    use std::time::Duration;

    fn transient_err(msg: &str) -> Result<(), FakeError> {
        Err(FakeError {
            message: msg.to_string(),
            is_transient: true,
        })
    }

    fn non_transient_err(msg: &str) -> Result<(), FakeError> {
        Err(FakeError {
            message: msg.to_string(),
            is_transient: false,
        })
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_transient_failures() {
        // 3 transient failures, then success
        // Results are popped from the back, so we reverse: success first in vec = last popped
        let executor = FakeExecutor::new(vec![
            Ok(()),
            transient_err("connection reset"),
            transient_err("connection reset"),
            transient_err("connection reset"),
        ]);
        let call_count = executor.call_count();
        let reconnect_count = executor.reconnect_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Get; // idempotent

        let policy = NetworkRetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            true, // is_idempotent
            &policy,
        )
        .await;

        assert!(result.is_ok(), "should succeed after retries: {:?}", result);
        // 4 execute calls total: 1 initial + 3 retries
        assert_eq!(call_count.load(Ordering::SeqCst), 4);
        // 3 reconnects (one per retry attempt)
        assert_eq!(reconnect_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted_after_max_retries() {
        // 10 transient failures (exhausts all retries)
        let mut results = vec![transient_err("connection refused"); 10];
        results.push(transient_err("final failure")); // one more for the initial call
        let executor = FakeExecutor::new(results);
        let call_count = executor.call_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Get;

        let policy = NetworkRetryPolicy {
            max_attempts: 11, // 1 initial + 10 retries = 11 total
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result =
            execute_with_retry(&mut { executor }, &cmd, &mut exchange, true, &policy).await;

        assert!(result.is_err(), "should fail after exhausting retries");
        // 1 initial + 10 retries = 11 calls
        assert_eq!(call_count.load(Ordering::SeqCst), 11);
    }

    #[tokio::test]
    async fn test_non_idempotent_command_not_retried() {
        let executor = FakeExecutor::new(vec![transient_err("connection reset")]);
        let call_count = executor.call_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Incr; // NOT idempotent

        let policy = NetworkRetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            false, // NOT idempotent
            &policy,
        )
        .await;

        assert!(result.is_err(), "non-idempotent should not be retried");
        // Only 1 call — no retries
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_non_transient_error_not_retried() {
        let executor = FakeExecutor::new(vec![non_transient_err("WRONGTYPE")]);
        let call_count = executor.call_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Get; // idempotent, but error is NOT transient

        let policy = NetworkRetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            true, // is_idempotent
            &policy,
        )
        .await;

        assert!(result.is_err(), "non-transient error should not be retried");
        // Only 1 call — no retries for non-transient
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_idempotent_command_retried_on_transient() {
        let executor = FakeExecutor::new(vec![
            Ok(()),
            transient_err("EOF"),
            transient_err("timed out"),
        ]);
        let call_count = executor.call_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Set; // idempotent write

        let policy = NetworkRetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result =
            execute_with_retry(&mut { executor }, &cmd, &mut exchange, true, &policy).await;

        assert!(result.is_ok());
        // 1 initial + 2 retries = 3 calls
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_immediate_success_no_retries() {
        let executor = FakeExecutor::new(vec![Ok(())]);
        let call_count = executor.call_count();
        let reconnect_count = executor.reconnect_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Get;

        let policy = NetworkRetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result =
            execute_with_retry(&mut { executor }, &cmd, &mut exchange, true, &policy).await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert_eq!(reconnect_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn network_retry_policy_retries_on_transient() {
        let policy = NetworkRetryPolicy {
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };

        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_async::<(), _, _, _, CamelError>(
            &policy,
            "t",
            "op",
            || {
                let c = attempts_clone.clone();
                async move {
                    let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        Err(CamelError::ProcessorError("transient".into()))
                    } else {
                        Ok(())
                    }
                }
            },
            |_| true,
            None,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    fn test_config() -> RedisEndpointConfig {
        let mut config = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        // Keep the connect timeout short so dead-address tests fail fast.
        config.connection_timeout_secs = 1;
        config
    }

    #[tokio::test]
    async fn multiplexed_executor_lazy_connects_via_topology() {
        let topology = Arc::new(FakeTopology::addrs(vec!["redis://127.0.0.1:1".into()]));
        let mut executor = MultiplexedExecutor::new(test_config(), topology.clone());

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Get;
        let result = executor.execute_command(&cmd, &mut exchange).await;

        // The topology must have been consulted to resolve the master address.
        assert!(
            topology.resolve_call_count() >= 1,
            "topology should be resolved on first execute_command"
        );
        // Connecting to a dead port (127.0.0.1:1) must fail deterministically.
        assert!(
            matches!(result, Err(CamelError::ProcessorError(_))),
            "expected ProcessorError connecting to dead port, got: {result:?}"
        );
    }

    // Failure-path only: get_conn failures are never cached (each call
    // re-resolves), and refresh() clears the cache and re-resolves exactly
    // once even when the reconnect attempt fails.
    #[tokio::test]
    async fn refresh_triggers_reresolution_and_failure_is_not_cached() {
        let topology = Arc::new(FakeTopology::addrs(vec!["redis://127.0.0.1:1".into()]));
        let executor = MultiplexedExecutor::new(test_config(), topology.clone());

        // First get_conn: resolves (count 1), connect to dead port fails.
        assert!(executor.get_conn().await.is_err());
        assert_eq!(topology.resolve_call_count(), 1);

        // Second get_conn: failure was not cached, so it re-resolves.
        assert!(executor.get_conn().await.is_err());
        assert_eq!(
            topology.resolve_call_count(),
            2,
            "failed connections must not be cached"
        );

        // refresh(): clears the cache, re-resolves exactly once, and fails
        // without caching anything.
        assert!(executor.refresh().await.is_err());
        assert_eq!(
            topology.resolve_call_count(),
            3,
            "refresh should re-resolve exactly once more"
        );
    }

    #[tokio::test]
    async fn multiplexed_executor_reconnect_reresolves() {
        let topology = Arc::new(FakeTopology::addrs(vec![
            "redis://a".into(),
            "redis://b".into(),
        ]));
        let mut executor = MultiplexedExecutor::new(test_config(), topology.clone());

        // Each reconnect drops the cached connection and re-resolves via the
        // topology. Connecting to the fake addresses fails, which is fine.
        let _ = executor.reconnect().await;
        let _ = executor.reconnect().await;

        assert_eq!(
            topology.resolve_call_count(),
            2,
            "each reconnect should re-resolve the topology"
        );
    }

    // Task 1.1 (redis-single-flight): the cached fast path in `get_conn`
    // must NOT take the connect gate. A foreign task holds the gate for
    // the whole probe; a cache hit must still be served immediately. A
    // gate-contending fast path would park on the held gate and the 5s
    // probe would time out instead.
    #[tokio::test]
    async fn cached_fast_path_skips_gate() {
        use std::sync::atomic::AtomicBool;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::Notify;

        // ------------------------------------------------------------------
        // Full-handshake answering stub (frame parser adapted from
        // tests/response_timeout.rs).
        // ------------------------------------------------------------------

        fn invalid_data(message: impl Into<String>) -> std::io::Error {
            std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
        }

        /// Parse `<marker><digits>\r\n` starting at `offset`. Returns the
        /// parsed integer and the byte offset just past the header line,
        /// `Ok(None)` when the header is not fully buffered yet.
        fn parse_integer_header(
            buf: &[u8],
            offset: usize,
            marker: u8,
        ) -> std::io::Result<Option<(usize, usize)>> {
            let Some(&first) = buf.get(offset) else {
                return Ok(None);
            };
            if first != marker {
                return Err(invalid_data(format!(
                    "expected '{}' RESP marker, got {:?}",
                    marker as char, first as char
                )));
            }
            let mut cursor = offset + 1;
            loop {
                match buf.get(cursor) {
                    Some(byte) if byte.is_ascii_digit() => cursor += 1,
                    Some(b'\r') => match buf.get(cursor + 1) {
                        Some(b'\n') => {
                            let digits = &buf[offset + 1..cursor];
                            let text = std::str::from_utf8(digits)
                                .map_err(|_| invalid_data("non-UTF-8 header digits"))?;
                            let value: usize = text
                                .parse()
                                .map_err(|_| invalid_data("header integer overflow"))?;
                            return Ok(Some((value, cursor + 2)));
                        }
                        Some(_) => return Err(invalid_data("expected \n after \r in header")),
                        None => return Ok(None),
                    },
                    Some(_) => return Err(invalid_data("unexpected byte in RESP header")),
                    None => return Ok(None),
                }
            }
        }

        /// Try to parse one complete RESP array-of-bulk-strings frame from
        /// `buf`. Returns `Ok(Some(total_len))` when a full frame is
        /// buffered, `Ok(None)` when more bytes are needed, `Err` on
        /// malformed input.
        fn parse_frame(buf: &[u8]) -> std::io::Result<Option<usize>> {
            let Some((elements, mut pos)) = parse_integer_header(buf, 0, b'*')? else {
                return Ok(None);
            };
            for _ in 0..elements {
                let Some((len, payload_at)) = parse_integer_header(buf, pos, b'$')? else {
                    return Ok(None);
                };
                let Some(end) = payload_at.checked_add(len).and_then(|e| e.checked_add(2)) else {
                    return Err(invalid_data("bulk string length overflow"));
                };
                if buf.len() < end {
                    return Ok(None);
                }
                pos = end;
            }
            Ok(Some(pos))
        }

        // Every complete request frame gets `+OK` — the driver pipelines
        // its handshake (CLIENT SETINFO ×2 etc.), so a single-frame reply
        // would break the handshake.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let stub_addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    // Persistent read buffer: one TCP read can carry
                    // several pipelined frames.
                    let mut buf: Vec<u8> = Vec::with_capacity(64);
                    let mut chunk = [0u8; 256];
                    loop {
                        match parse_frame(&buf) {
                            Ok(Some(frame_len)) => {
                                let _ = buf.drain(..frame_len);
                                if stream.write_all(b"+OK\r\n").await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) => {
                                let n = match stream.read(&mut chunk).await {
                                    Ok(0) => break, // clean EOF
                                    Ok(n) => n,
                                    Err(_) => break,
                                };
                                buf.extend_from_slice(&chunk[..n]);
                            }
                            Err(_) => break,
                        }
                    }
                });
            }
        });

        // Build the executor and prime the cache: the stub answers the
        // full handshake, so this connect succeeds and caches.
        let config = RedisEndpointConfig::from_uri(&format!("redis://{stub_addr}"))
            .expect("valid standalone URI for the stub");
        let topology = Arc::new(FakeTopology::addrs(vec![format!("redis://{stub_addr}")]));
        let executor = MultiplexedExecutor::new(config, topology);
        executor
            .get_conn()
            .await
            .expect("handshake answered; connection cached");

        // Foreign task grabs the gate and parks on a notify.
        let gate = executor.gate_arc();
        let acquired = Arc::new(AtomicBool::new(false));
        let notify = Arc::new(Notify::new());
        let holder_acquired = Arc::clone(&acquired);
        let holder_notify = Arc::clone(&notify);
        let holder = tokio::spawn(async move {
            let _g = gate.lock().await;
            holder_acquired.store(true, Ordering::SeqCst);
            holder_notify.notified().await;
        });

        // PROOF the gate is held before the probe: poll until the holder
        // reports acquisition.
        while !acquired.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }

        // Probe: the cached fast path must be served without the gate.
        let _conn = tokio::time::timeout(Duration::from_secs(5), executor.get_conn())
            .await
            .expect("cached fast path must not wait on the connect gate")
            .expect("cache hit must serve Ok");

        // Deterministic cleanup: release the holder and join bounded.
        notify.notify_one();
        let joined = tokio::time::timeout(Duration::from_secs(5), holder).await;
        assert!(joined.is_ok(), "holder must release the gate after notify");
    }
}
