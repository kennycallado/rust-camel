use async_trait::async_trait;
use camel_component_api::{CamelError, Exchange, NetworkRetryPolicy};
// retry_async is used in tests (the regression test in this file).
#[cfg(test)]
use camel_component_api::retry_async;
use redis::AsyncConnectionConfig;
use redis::aio::MultiplexedConnection;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
                    Err(crate::transport_error::marker_camel(
                        format!("Connection error: {}", fake_err.message),
                        crate::transport_error::TransientByProse {
                            site: "FakeExecutor transient seam",
                        },
                    ))
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
        | RedisCommand::Strlen
        | RedisCommand::Getdel
        | RedisCommand::Getex
        | RedisCommand::Setrange
        | RedisCommand::Getrange => commands::string::dispatch(cmd, conn, exchange).await,

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
        | RedisCommand::Sort
        | RedisCommand::Unlink
        | RedisCommand::Copy
        | RedisCommand::Touch => commands::key::dispatch(cmd, conn, exchange).await,

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
        | RedisCommand::Rpoplpush
        | RedisCommand::Lmpop
        | RedisCommand::Blmpop
        | RedisCommand::Lmove
        | RedisCommand::Blmove
        | RedisCommand::Lpos => commands::list::dispatch(cmd, conn, exchange).await,

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
        | RedisCommand::Hincrby
        | RedisCommand::Hrandfield
        | RedisCommand::Hincrbyfloat => commands::hash::dispatch(cmd, conn, exchange).await,

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
        | RedisCommand::Srandmember
        | RedisCommand::Smismember
        | RedisCommand::Sintercard => commands::set::dispatch(cmd, conn, exchange).await,

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
        | RedisCommand::Zinterstore
        | RedisCommand::Zmpop
        | RedisCommand::Bzmpop
        | RedisCommand::Zrandmember
        | RedisCommand::Zdiff
        | RedisCommand::Zinter
        | RedisCommand::Zunion
        | RedisCommand::Zrangebylex => commands::zset::dispatch(cmd, conn, exchange).await,

        // Geo commands
        RedisCommand::Geoadd
        | RedisCommand::Geopos
        | RedisCommand::Geodist
        | RedisCommand::Geosearch
        | RedisCommand::Geohash => commands::geo::dispatch(cmd, conn, exchange).await,

        // Stream producer commands
        RedisCommand::Xadd | RedisCommand::Xlen => {
            commands::stream::dispatch(cmd, conn, exchange).await
        }

        // Stream commands are consumer-only; the producer path rejects them.
        RedisCommand::Xreadgroup => Err(CamelError::InvalidUri(
            "XREADGROUP is a consumer-only command".into(),
        )),

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

/// Driver response-deadline policy for a newly built connection.
///
/// Spells out the three distinct meanings that a bare `Option<Duration>`
/// conflates: deadline-free (blocking pops must wait without any driver
/// deadline), an explicit deadline, and the redis driver default.
enum Deadline {
    /// No driver response deadline at all; blocking pops rely on their own
    /// Redis-side timeout (or wait forever when it is 0).
    Free,
    /// Explicit driver response deadline.
    Fixed(Duration),
    /// Keep the redis driver's built-in response deadline.
    DriverDefault,
}

/// A real [`RedisCommandExecutor`] backed by a lazily-created multiplexed
/// connection whose target is re-resolved through a [`RedisTopology`].
///
/// The connection is created on first use and cached together with the
/// generation it was built under.
/// [`RedisCommandExecutor::reconnect`] drops the cached connection — when it is
/// still the generation this instance last used — and rebuilds it, which
/// re-resolves the master address through the topology — this is what
/// enables sentinel failover detection.
///
/// # Generation tagging
///
/// Every stored connection carries a monotonically increasing generation
/// drawn from an allocator shared by all clones ([`Self::next_gen`]);
/// generation `0` is reserved for "never used a connection", so the first
/// stored generation is `1`. The pairing lets a failure clear the cache by
/// compare-and-clear instead of unconditionally: a clone that observed a
/// transport failure on generation `N` only evicts the cache while it still
/// holds generation `N`, so it can never clobber a fresher connection
/// another clone already rebuilt.
///
// log-policy: the transient retry `warn!` in `execute_with_retry` is
// category (e) outside-contract (ADR-0012). Error messages here are kept
// redaction-safe via `config.redis_url_safe()`.
#[derive(Clone)]
pub struct MultiplexedExecutor {
    config: RedisEndpointConfig,
    topology: Arc<dyn RedisTopology>,
    /// Cached connection tagged with the generation it was built under.
    conn: Arc<Mutex<Option<(u64, MultiplexedConnection)>>>,
    /// Generation allocator shared by every clone, so a generation is never
    /// reused across the clone family. Starts at 0; [`Self::checkout`] adds
    /// 1 before storing, keeping generation 0 for "never used".
    next_gen: Arc<AtomicU64>,
    /// Generation of the connection THIS instance last used for a command.
    ///
    /// Deliberately a plain `u64`, not shared state: the derived `Clone`
    /// copies it, so each clone remembers the generation it last observed.
    /// [`RedisCommandExecutor::reconnect`] compares it against the cached
    /// generation so a stale clone does not evict a fresher connection.
    last_used_gen: u64,
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
            next_gen: Arc::new(AtomicU64::new(0)),
            last_used_gen: 0,
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
    /// A blocking endpoint command (`BLPOP`/`BRPOP`/`BLMOVE`/`BLMPOP`/`BZMPOP`)
    /// always builds deadline-free connections and ignores this seam value: its
    /// blocking duration is governed by the Redis-side timeout, not by a driver
    /// deadline.
    ///
    /// This does not affect `connection_timeout_secs` (TCP connect only).
    /// It is distinct from camel-redis-repo's
    /// `MultiplexedRepoExecutor::with_response_timeout` test seam: that sizes
    /// the LOCAL backstop, this sizes the DRIVER deadline.
    pub fn with_response_timeout(mut self, timeout: Duration) -> Self {
        self.response_timeout = Some(timeout);
        self
    }

    /// Response-deadline policy for connections built by [`Self::get_conn`].
    ///
    /// Blocking endpoints are always [`Deadline::Free`]; otherwise the
    /// `with_response_timeout` seam selects a fixed deadline, and its absence
    /// leaves the driver default in place.
    fn deadline(&self) -> Deadline {
        match self.effective_response_timeout() {
            Some(t) => Deadline::Fixed(t),
            None if self.is_blocking_endpoint() => Deadline::Free,
            None => Deadline::DriverDefault,
        }
    }

    /// Whether this endpoint issues a blocking command (`BLPOP`, `BRPOP`,
    /// `BLMOVE`, `BLMPOP`, `BZMPOP`).
    fn is_blocking_endpoint(&self) -> bool {
        matches!(
            self.config.command,
            RedisCommand::Blpop
                | RedisCommand::Brpop
                | RedisCommand::Blmove
                | RedisCommand::Blmpop
                | RedisCommand::Bzmpop
        )
    }

    /// Effective driver response timeout for this endpoint.
    ///
    /// Returns `None` for a blocking endpoint (deadline-free, NOT the driver
    /// default) and otherwise mirrors the `with_response_timeout` seam.
    fn effective_response_timeout(&self) -> Option<Duration> {
        if self.is_blocking_endpoint() {
            None
        } else {
            self.response_timeout
        }
    }

    /// Return a cached connection, or resolve and build one on first use.
    ///
    /// `pub` so out-of-crate services (repository service) can reuse the same
    /// lazily-cached connection as the producer's `check_connection`.
    pub async fn get_conn(&self) -> Result<MultiplexedConnection, CamelError> {
        self.checkout().await.map(|(_, conn)| conn)
    }

    /// Return the cached connection together with the generation it was
    /// built under, or resolve and build one on first use.
    ///
    /// Crate-visible seam: callers that must invalidate a failed connection
    /// (producer health check, executor dispatch) need the generation to
    /// compare-and-clear instead of unconditionally dropping whatever is
    /// cached by the time the failure surfaces.
    pub(crate) async fn checkout(&self) -> Result<(u64, MultiplexedConnection), CamelError> {
        // Step A (fast path): reuse the cached connection. Only `conn` is
        // locked here — a cache hit never touches the connect gate.
        {
            let guard = self.conn.lock().await;
            if let Some((generation, c)) = guard.as_ref() {
                return Ok((*generation, c.clone()));
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
            if let Some((generation, c)) = guard.as_ref() {
                return Ok((*generation, c.clone()));
            }
        }

        // Step D (leader): resolve the master address through the
        // topology, then connect.
        let client = self.topology.resolve(ServerKind::Master).await?;
        let redis_url_safe = self.config.redis_url_safe();
        let timeout_secs = self.config.connection_timeout_secs;

        let connect = async {
            match self.deadline() {
                Deadline::Free => {
                    client
                        .get_multiplexed_async_connection_with_config(
                            &AsyncConnectionConfig::new()
                                .set_response_timeout(None)
                                .set_connection_timeout(None),
                        )
                        .await
                }
                Deadline::Fixed(t) => {
                    client
                        .get_multiplexed_async_connection_with_config(
                            &AsyncConnectionConfig::new()
                                .set_response_timeout(Some(t))
                                .set_connection_timeout(None),
                        )
                        .await
                }
                Deadline::DriverDefault => client.get_multiplexed_async_connection().await,
            }
        };

        let new_conn = tokio::time::timeout(Duration::from_secs(timeout_secs), connect)
            .await
            .map_err(|_| {
                crate::transport_error::marker_camel(
                    format!(
                        "Redis connection to '{}' timed out after {}s",
                        redis_url_safe, timeout_secs
                    ),
                    crate::transport_error::TransportTimeout {
                        stage: "executor connect",
                    },
                )
            })?
            .map_err(|e| {
                // rc-swzq: name the credential plane on data-node auth
                // failures so a sentinel_password/password mixup is
                // diagnosable from the error alone.
                CamelError::ProcessorErrorWithSource(
                    crate::config::enrich_data_auth_error(format!(
                        "Failed to connect to Redis at '{}': {}",
                        redis_url_safe, e
                    )),
                    Arc::new(e),
                )
            })?;

        // First stored generation is 1: the allocator starts at 0 and is
        // incremented before storing, so 0 stays reserved for instances
        // that never used a connection.
        let generation = self.next_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let mut guard = self.conn.lock().await;
        *guard = Some((generation, new_conn.clone()));
        Ok((generation, new_conn))
    }

    /// Drop the cached connection when it is still the generation that
    /// `failed_gen` observed.
    ///
    /// Never dials: compare-and-clear only. A failure that surfaced on an
    /// older generation must not evict the connection a concurrent clone
    /// has already rebuilt.
    pub(crate) async fn invalidate_if_current(&self, failed_gen: u64) {
        let mut guard = self.conn.lock().await;
        if guard
            .as_ref()
            .is_some_and(|(generation, _)| *generation == failed_gen)
        {
            *guard = None;
        }
    }

    /// Clear the cached connection and resolve a fresh one through the
    /// topology.
    ///
    /// Inherent `&self` counterpart of [`RedisCommandExecutor::reconnect`]
    /// for callers that hold a shared (non-`mut`) executor, e.g. the
    /// repository service. Unlike the trait method it is UNCONDITIONAL: the
    /// cache is dropped regardless of any instance's last-used generation,
    /// then a new connection is built via [`Self::get_conn`], which
    /// re-resolves the master address. An explicit refresh is a caller
    /// decision, so no compare-and-clear applies.
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
    pub(crate) fn conn_arc(&self) -> Arc<Mutex<Option<(u64, MultiplexedConnection)>>> {
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
        let (generation, mut conn) = self.checkout().await?;
        // Record the generation this instance last used: `reconnect` uses it
        // to compare-and-clear instead of blindly evicting.
        self.last_used_gen = generation;
        match dispatch_command(cmd, &mut conn, exchange).await {
            Ok(()) => Ok(()),
            Err(err) => {
                // A transport failure is proof the cached connection is
                // dead: evict it by compare-and-clear so a fresher
                // connection rebuilt by another clone survives. The error is
                // rethrown unchanged — no dial, no retry here.
                if crate::transport_error::invalidates_connection(&err) {
                    self.invalidate_if_current(generation).await;
                }
                Err(err)
            }
        }
    }

    /// Reconnect this instance's connection.
    ///
    /// Generation-aware clear: the cache is dropped only when it still holds
    /// the generation THIS instance last used for a command. `last_used_gen`
    /// is per-instance (the derived `Clone` copies it, without sharing it),
    /// so a clone that has never issued a command keeps generation 0 and its
    /// `reconnect` reuses whatever its siblings cached instead of clobbering
    /// it. Production reconnects always follow this instance's failed command
    /// (`execute_with_retry` only reconnects before a retry, after its own
    /// `execute_command` failed), so the generations match and the stale
    /// connection is dropped as before.
    async fn reconnect(&mut self) -> Result<(), CamelError> {
        let last_used_gen = self.last_used_gen;
        {
            let mut guard = self.conn.lock().await;
            if guard
                .as_ref()
                .is_some_and(|(generation, _)| *generation == last_used_gen)
            {
                *guard = None;
            }
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
/// Invalidation happens inside [`RedisCommandExecutor::execute_command`] (or
/// the producer's health check) on the FIRST failure, regardless of retry
/// policy: a transport failure evicts the cached connection immediately, so a
/// retry only reconnects when the policy actually allows it, and a
/// non-retryable failure leaves the cache already clean for the next command.
///
/// Uses a manual retry loop calling [`NetworkRetryPolicy::should_retry`] and
/// [`NetworkRetryPolicy::delay_for`] rather than the shared [`retry_async`](camel_component_api::retry_async)
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

/// Shared RESP server stub for this crate's synchronous tests.
///
/// Extracted from `cached_fast_path_skips_gate` so the producer tests
/// (task 2.2 of change redisdisc) import the same harness instead of
/// growing a second copy. Three capabilities:
///
/// - **reply script** — application commands get a scripted reply ([`AppReply`]);
///   the driver handshake (`CLIENT`/`AUTH`/`SELECT`/`HELLO`) always gets
///   `+OK` and is never counted as an application command;
/// - **kill handle** — [`StubServer::kill`] drops every accepted socket and
///   waits until the stub confirms that both halves are closed, so a test
///   can fail an in-flight command deterministically;
/// - **command barrier** — [`StubServer::hold_replies`] withholds application
///   command replies while [`StubServer::wait_for_commands`] observes that a
///   command frame arrived. Frames keep being read while replies are held, so
///   several commands can be in flight at once. No release step exists: the
///   tests that use the barrier kill the socket, which fails the held
///   commands deterministically.
#[cfg(test)]
pub(crate) mod test_stub {
    use std::io;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::net::TcpStream;
    use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::sync::Notify;
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;

    /// Reply a stub connection returns for one application command.
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum AppReply {
        /// `+OK` — accepted by `SET` and other unit-expecting commands.
        Ok,
        /// A RESP error reply; the line is sent with a leading `-` and a
        /// trailing CRLF, e.g. `WRONGTYPE Operation against a key holding
        /// the wrong kind of value`.
        Error(&'static str),
    }

    impl AppReply {
        fn wire(self) -> Vec<u8> {
            match self {
                AppReply::Ok => b"+OK\r\n".to_vec(),
                AppReply::Error(line) => format!("-{line}\r\n").into_bytes(),
            }
        }
    }

    /// The `WRONGTYPE` server reply: a business error that is neither
    /// transient nor invalidating.
    pub(crate) const WRONGTYPE_REPLY: AppReply =
        AppReply::Error("WRONGTYPE Operation against a key holding the wrong kind of value");

    /// Driver handshake verbs; answered `+OK` and excluded from the command
    /// count.
    const HANDSHAKE_VERBS: [&str; 4] = ["CLIENT", "AUTH", "SELECT", "HELLO"];

    fn is_handshake_command(verb: &str) -> bool {
        HANDSHAKE_VERBS.contains(&verb)
    }

    /// Loopback RESP stub bound to an ephemeral port.
    pub(crate) struct StubServer {
        addr: SocketAddr,
        state: Arc<StubState>,
        accept: JoinHandle<()>,
    }

    impl StubServer {
        /// Bind an ephemeral loopback port and start serving `reply`.
        pub(crate) async fn start(reply: AppReply) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind loopback stub listener");
            let addr = listener.local_addr().expect("stub listener local addr");
            let state = Arc::new(StubState {
                reply,
                commands: AtomicUsize::new(0),
                command_notify: Notify::new(),
                hold: AtomicBool::new(false),
                gate_notify: Notify::new(),
                connections: Mutex::new(Vec::new()),
            });
            let accept_state = Arc::clone(&state);
            let accept = tokio::spawn(async move {
                loop {
                    // allow-test-wait: spawned test-server accept loop; teardown-bounded by test runtime (ADR-0069 §13.2 R1)
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let conn = Arc::new(ConnKill::default());
                    accept_state
                        .connections
                        .lock()
                        .expect("stub connection registry poisoned")
                        .push(Arc::clone(&conn));
                    tokio::spawn(serve_connection(stream, Arc::clone(&accept_state), conn));
                }
            });
            Self {
                addr,
                state,
                accept,
            }
        }

        /// `redis://` URL of the stub, for `RedisEndpointConfig::from_uri`
        /// and `FakeTopology::addrs`.
        pub(crate) fn url(&self) -> String {
            format!("redis://{}", self.addr)
        }

        /// Application commands received so far (handshake excluded).
        pub(crate) fn command_count(&self) -> usize {
            self.state.commands.load(Ordering::SeqCst)
        }

        /// Wait until `n` application commands have arrived.
        pub(crate) async fn wait_for_commands(&self, n: usize) {
            wait_until(&self.state.command_notify, || self.command_count() >= n).await;
        }

        /// Withhold every application-command reply. Command frames keep
        /// being read, and killing the socket fails the held commands.
        pub(crate) fn hold_replies(&self) {
            self.state.hold.store(true, Ordering::SeqCst);
        }

        /// Drop every accepted socket and wait until the stub confirms that
        /// both halves are closed.
        pub(crate) async fn kill(&self) {
            let conns: Vec<Arc<ConnKill>> = self
                .state
                .connections
                .lock()
                .expect("stub connection registry poisoned")
                .clone();
            for conn in &conns {
                conn.kill();
            }
            for conn in &conns {
                conn.wait_closed().await;
            }
        }
    }

    impl Drop for StubServer {
        fn drop(&mut self) {
            self.accept.abort();
        }
    }

    /// State shared by the accept loop and every connection task.
    struct StubState {
        reply: AppReply,
        commands: AtomicUsize,
        command_notify: Notify,
        hold: AtomicBool,
        gate_notify: Notify,
        connections: Mutex<Vec<Arc<ConnKill>>>,
    }

    /// Per-connection kill/closed handshake.
    #[derive(Default)]
    struct ConnKill {
        killed: AtomicBool,
        closed: AtomicBool,
        kill_notify: Notify,
        closed_notify: Notify,
    }

    impl ConnKill {
        fn kill(&self) {
            self.killed.store(true, Ordering::SeqCst);
            self.kill_notify.notify_waiters();
        }

        fn is_killed(&self) -> bool {
            self.killed.load(Ordering::SeqCst)
        }

        async fn wait_kill(&self) {
            wait_until(&self.kill_notify, || self.is_killed()).await;
        }

        fn mark_closed(&self) {
            self.closed.store(true, Ordering::SeqCst);
            self.closed_notify.notify_waiters();
        }

        async fn wait_closed(&self) {
            wait_until(&self.closed_notify, || self.closed.load(Ordering::SeqCst)).await;
        }
    }

    /// Waits until `predicate` holds, registering with `notify` before the
    /// check so a concurrent `notify_waiters` cannot be missed.
    async fn wait_until(notify: &Notify, predicate: impl Fn() -> bool) {
        loop {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if predicate() {
                return;
            }
            notified.await; // allow-test-wait: stub-internal gate; the notifier fires in-test and the predicate re-checks it
        }
    }

    /// Serve one accepted connection: reads frames on one task and writes
    /// replies on another, so the reader can keep consuming command frames
    /// while the reply gate holds the writer.
    async fn serve_connection(stream: TcpStream, state: Arc<StubState>, conn: Arc<ConnKill>) {
        let (read_half, write_half) = stream.into_split();
        let (reply_tx, reply_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let writer = tokio::spawn(write_replies(
            write_half,
            reply_rx,
            Arc::clone(&state),
            Arc::clone(&conn),
        ));
        read_frames(read_half, reply_tx, Arc::clone(&state), Arc::clone(&conn)).await;
        // The peer is gone (or the socket was killed): stop the writer too so
        // both halves are dropped and the closure becomes observable.
        conn.kill();
        let _ = writer.await; // allow-test-wait: writer wakes on the kill just signalled; join asserted by closure ack
        conn.mark_closed();
    }

    async fn read_frames(
        mut reader: OwnedReadHalf,
        reply_tx: mpsc::UnboundedSender<Vec<u8>>,
        state: Arc<StubState>,
        conn: Arc<ConnKill>,
    ) {
        // Persistent read buffer: one TCP read can carry several pipelined
        // frames.
        let mut buf: Vec<u8> = Vec::with_capacity(64);
        let mut chunk = [0u8; 256];
        loop {
            tokio::select! {
                _ = conn.wait_kill() => break,
                read = reader.read(&mut chunk) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                },
            }
            loop {
                match parse_frame(&buf) {
                    Ok(Some((frame_len, verb))) => {
                        let _ = buf.drain(..frame_len);
                        let bytes = if is_handshake_command(&verb) {
                            b"+OK\r\n".to_vec()
                        } else {
                            state.commands.fetch_add(1, Ordering::SeqCst);
                            state.command_notify.notify_waiters();
                            state.reply.wire()
                        };
                        if reply_tx.send(bytes).is_err() {
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => return,
                }
            }
        }
    }

    async fn write_replies(
        mut writer: OwnedWriteHalf,
        mut replies: mpsc::UnboundedReceiver<Vec<u8>>,
        state: Arc<StubState>,
        conn: Arc<ConnKill>,
    ) {
        loop {
            tokio::select! {
                _ = conn.wait_kill() => break,
                reply = replies.recv() => {
                    let Some(bytes) = reply else {
                        break;
                    };
                    if !wait_gate_open(&state, &conn).await {
                        break;
                    }
                    if writer.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    /// Waits until the reply gate is open. Returns `false` when the
    /// connection was killed while waiting.
    async fn wait_gate_open(state: &StubState, conn: &ConnKill) -> bool {
        loop {
            let notified = state.gate_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !state.hold.load(Ordering::SeqCst) {
                return true;
            }
            if conn.is_killed() {
                return false;
            }
            tokio::select! {
                _ = notified => {}
                _ = conn.wait_kill() => return false,
            }
        }
    }

    fn invalid_data(message: impl Into<String>) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, message.into())
    }

    /// Parse `<marker><digits>\r\n` starting at `offset`. Returns the parsed
    /// integer and the byte offset just past the header line, `Ok(None)` when
    /// the header is not fully buffered yet.
    fn parse_integer_header(
        buf: &[u8],
        offset: usize,
        marker: u8,
    ) -> io::Result<Option<(usize, usize)>> {
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

    /// Parse one complete RESP array-of-bulk-strings frame from `buf`.
    ///
    /// Returns `Ok(Some((total_len, verb)))` when a full frame is buffered
    /// (with its first bulk string, uppercased, as the command verb),
    /// `Ok(None)` when more bytes are needed, `Err` on malformed input.
    fn parse_frame(buf: &[u8]) -> io::Result<Option<(usize, String)>> {
        let Some((elements, mut cursor)) = parse_integer_header(buf, 0, b'*')? else {
            return Ok(None);
        };
        let mut verb = String::new();
        for index in 0..elements {
            let Some((len, payload_at)) = parse_integer_header(buf, cursor, b'$')? else {
                return Ok(None);
            };
            let Some(end) = payload_at.checked_add(len).and_then(|e| e.checked_add(2)) else {
                return Err(invalid_data("bulk string length overflow"));
            };
            if buf.len() < end {
                return Ok(None);
            }
            if index == 0 {
                verb = String::from_utf8_lossy(&buf[payload_at..payload_at + len]).to_uppercase();
            }
            cursor = end;
        }
        Ok(Some((cursor, verb)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::is_idempotent_command;
    use crate::executor::test_stub;
    use crate::topology::FakeTopology;
    use camel_component_api::{Body, Message};
    use std::error::Error as _;
    use std::time::Duration;

    /// Exchange carrying the `CamelRedis.Key` header and a body, so `SET`
    /// reaches the wire (and therefore the stub).
    fn set_exchange(key: &str) -> Exchange {
        let mut exchange = Exchange::new(Message::new(Body::Text("v".into())));
        exchange
            .input
            .set_header("CamelRedis.Key", serde_json::json!(key));
        exchange
    }

    fn stub_executor(url: &str, topology: Arc<FakeTopology>) -> MultiplexedExecutor {
        let config = RedisEndpointConfig::from_uri(url).expect("valid stub redis URI");
        MultiplexedExecutor::new(config, topology)
    }

    /// Generation currently cached, or `None` when the cache is empty.
    async fn cached_generation(executor: &MultiplexedExecutor) -> Option<u64> {
        let conn = executor.conn_arc();
        let guard = conn.lock().await; // allow-test-wait: test-owned cache cell; lock is uncontended in-test
        guard.as_ref().map(|(generation, _)| *generation)
    }

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

    // Regression: a retried Rename/Renamenx/Move after an unconfirmed success
    // fails visibly (ERR no such key / 0), so the retry flag must come from
    // the classifier. One exchange per command, mirroring the spec scenario.
    #[tokio::test]
    async fn non_idempotent_command_is_attempted_once() {
        for cmd in [
            RedisCommand::Rename,
            RedisCommand::Renamenx,
            RedisCommand::Move,
            // Reclassified (design D3): the option-blind classifier makes the
            // worst-case SET option form govern.
            RedisCommand::Set,
        ] {
            let executor = FakeExecutor::new(vec![transient_err("connection reset")]);
            let call_count = executor.call_count();
            let reconnect_count = executor.reconnect_count();

            let mut exchange = Exchange::default();

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
                is_idempotent_command(&cmd),
                &policy,
            )
            .await;

            assert!(
                result.is_err(),
                "{cmd:?} must be attempted once and fail: {result:?}"
            );
            assert_eq!(
                call_count.load(Ordering::SeqCst),
                1,
                "non-idempotent {cmd:?} must not be retried"
            );
            assert_eq!(
                reconnect_count.load(Ordering::SeqCst),
                0,
                "non-idempotent {cmd:?} must not reconnect"
            );
        }
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
        let cmd = RedisCommand::Setrange;

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
            is_idempotent_command(&cmd),
            &policy,
        )
        .await;

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
        // Both wraps are acceptable: the auth-enriched connect error carries
        // the inner redis error as source (ProcessorErrorWithSource).
        assert!(
            matches!(
                result,
                Err(CamelError::ProcessorError(_))
                    | Err(CamelError::ProcessorErrorWithSource(_, _))
            ),
            "expected ProcessorError connecting to dead port, got: {result:?}"
        );
    }

    #[test]
    fn connect_timeout_classifies_transient() {
        // Same marker_camel call shape as the production get_conn
        // connect-timeout wrap.
        let redis_url_safe = "redis://localhost:6379";
        let timeout_secs = 1u64;
        let err = crate::transport_error::marker_camel(
            format!(
                "Redis connection to '{}' timed out after {}s",
                redis_url_safe, timeout_secs
            ),
            crate::transport_error::TransportTimeout {
                stage: "executor connect",
            },
        );
        assert!(
            is_transient_redis_error(&err),
            "executor connect timeout must classify transient: {err}"
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

    // Task 2.1 (redisdisc): a clone created before it issues any command
    // keeps generation 0, so its `reconnect` compares 0 against the cached
    // generation (>= 1), finds a mismatch, and reuses the connection its
    // parent primed instead of unconditionally evicting it.
    #[tokio::test]
    async fn blind_reconnect_on_unused_clone_keeps_cache() {
        let stub = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let mut parent = stub_executor(&stub.url(), topology.clone());

        // Clone BEFORE any command: the copy inherits the unused generation.
        let mut blind = parent.clone();
        assert_eq!(
            blind.last_used_gen, 0,
            "an unused instance starts at generation 0"
        );

        // The parent primes the shared cache.
        let mut exchange = set_exchange("k");
        parent
            .execute_command(&RedisCommand::Set, &mut exchange)
            .await
            .expect("the stub answers SET with +OK");
        assert_eq!(parent.last_used_gen, 1);
        assert_eq!(cached_generation(&parent).await, Some(1));
        assert_eq!(topology.resolve_call_count(), 1);

        // A blind reconnect (never used a generation) must not drop the
        // live cache, hence no re-resolution.
        blind
            .reconnect()
            .await
            .expect("a blind reconnect must reuse the live connection");
        assert_eq!(
            topology.resolve_call_count(),
            1,
            "an unused clone's reconnect must not clear the parent's connection"
        );
        assert_eq!(cached_generation(&parent).await, Some(1));
    }

    #[tokio::test]
    async fn execute_command_then_reconnect_reresolves() {
        let first = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let second = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let topology = Arc::new(FakeTopology::addrs(vec![first.url(), second.url()]));
        let mut executor = stub_executor(&first.url(), topology.clone());

        let mut exchange = set_exchange("k");
        executor
            .execute_command(&RedisCommand::Set, &mut exchange)
            .await
            .expect("the stub answers SET with +OK");
        assert_eq!(executor.last_used_gen, 1);
        assert_eq!(topology.resolve_call_count(), 1);

        // The last-used generation matches the cached one, so reconnect
        // clears and rebuilds — one more resolve, a new generation.
        executor
            .reconnect()
            .await
            .expect("reconnect must rebuild against the second stub");
        assert_eq!(
            topology.resolve_call_count(),
            2,
            "a matched last-used generation must clear and re-resolve"
        );
        assert_eq!(cached_generation(&executor).await, Some(2));
    }

    #[tokio::test]
    async fn killed_connection_invalidated_and_next_command_reconnects() {
        let dead = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let fresh = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let topology = Arc::new(FakeTopology::addrs(vec![dead.url(), fresh.url()]));
        let mut executor = stub_executor(&dead.url(), topology.clone());

        // Command #1 answers on the live stub: generation 1 gets cached.
        let mut exchange = set_exchange("k");
        executor
            .execute_command(&RedisCommand::Set, &mut exchange)
            .await
            .expect("the stub answers SET with +OK");
        assert_eq!(
            executor.last_used_gen, 1,
            "the first stored generation must be 1"
        );
        assert_eq!(cached_generation(&executor).await, Some(1));

        // Kill the accepted socket and wait for the stub to confirm closure.
        dead.kill().await;

        // The next command on the cached generation fails with an
        // invalidating io error and evicts the cache.
        let mut exchange = set_exchange("k");
        let err = executor
            .execute_command(&RedisCommand::Set, &mut exchange)
            .await
            .expect_err("a killed socket must fail the command");
        assert!(
            crate::transport_error::invalidates_connection(&err),
            "socket death must be an invalidating io error: {err}"
        );
        assert!(
            crate::transport_error::is_transient_redis_error(&err),
            "socket death is transient too: {err}"
        );
        assert_eq!(
            cached_generation(&executor).await,
            None,
            "an invalidating failure must clear the cached connection"
        );
        assert_eq!(
            topology.resolve_call_count(),
            1,
            "invalidating a connection must never dial by itself"
        );

        // The next command lazily rebuilds through the topology: one more
        // resolve, a fresh generation, and the fresh stub answers.
        let mut exchange = set_exchange("k");
        executor
            .execute_command(&RedisCommand::Set, &mut exchange)
            .await
            .expect("the lazy rebuild must reconnect to the fresh stub");
        assert_eq!(
            topology.resolve_call_count(),
            2,
            "the lazy rebuild must re-resolve exactly once"
        );
        assert_eq!(executor.last_used_gen, 2);
        assert_eq!(cached_generation(&executor).await, Some(2));
    }

    #[tokio::test]
    async fn non_invalidating_error_keeps_cached_connection() {
        let stub = test_stub::StubServer::start(test_stub::WRONGTYPE_REPLY).await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let mut executor = stub_executor(&stub.url(), topology.clone());

        // Prime the cache without issuing a command.
        executor
            .get_conn()
            .await
            .expect("the stub answers the handshake; connection cached");
        let generation = cached_generation(&executor)
            .await
            .expect("connection cached after get_conn");
        assert_eq!(generation, 1);

        let mut exchange = set_exchange("k");
        let err = executor
            .execute_command(&RedisCommand::Set, &mut exchange)
            .await
            .expect_err("a server error reply must surface");
        assert!(
            err.to_string().contains("WRONGTYPE"),
            "the server error text must come back unchanged: {err}"
        );
        assert!(
            err.source().is_some(),
            "the wrap site must preserve the RedisError as source: {err}"
        );
        assert!(
            !crate::transport_error::invalidates_connection(&err),
            "a business server reply must not invalidate: {err}"
        );
        assert_eq!(
            cached_generation(&executor).await,
            Some(generation),
            "the cache must keep the SAME generation after a server reply"
        );
        assert_eq!(
            topology.resolve_call_count(),
            1,
            "a server reply must not trigger a reconnect"
        );
    }

    #[tokio::test]
    async fn clone_reconnect_does_not_clobber_fresh_connection() {
        let dead = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let fresh = test_stub::StubServer::start(test_stub::AppReply::Ok).await;

        // Two resolves: the initial build, then the post-kill rebuild.
        let topology = Arc::new(FakeTopology::addrs(vec![dead.url(), fresh.url()]));
        let mut alive = stub_executor(&dead.url(), topology.clone());

        // A drives the first command: generation 1, recorded in A.
        let mut exchange = set_exchange("k");
        alive
            .execute_command(&RedisCommand::Set, &mut exchange)
            .await
            .expect("the stub answers SET with +OK");
        assert_eq!(alive.last_used_gen, 1);

        // B is cloned from A: the copy carries A's last-used generation 1.
        let mut clone_b = alive.clone();
        assert_eq!(
            clone_b.last_used_gen, 1,
            "the derived Clone must copy (not share) the last-used generation"
        );

        // Barrier: hold every application reply so both commands can be in
        // flight at once. The stub keeps reading frames while replies are
        // withheld.
        dead.hold_replies();

        let task_a = tokio::spawn(async move {
            let mut exchange = set_exchange("k");
            let result = alive
                .execute_command(&RedisCommand::Set, &mut exchange)
                .await;
            (alive, result)
        });
        let task_b = tokio::spawn(async move {
            let mut exchange = set_exchange("k");
            let result = clone_b
                .execute_command(&RedisCommand::Set, &mut exchange)
                .await;
            (clone_b, result)
        });

        // Command #1 was A's answered command; #2 and #3 are the two held
        // commands, one per clone.
        dead.wait_for_commands(3).await;

        // Kill the socket: both in-flight commands fail with an io-kind,
        // invalidating error.
        dead.kill().await;

        let (mut alive, a_result) = task_a.await.expect("A task must not panic"); // allow-test-wait: socket killed; join returns the in-flight failure
        let (mut clone_b, b_result) = task_b.await.expect("B task must not panic"); // allow-test-wait: socket killed; join returns the in-flight failure
        for (who, result) in [("A", a_result), ("B", b_result)] {
            let err = result.expect_err("a killed socket must fail the command");
            assert!(
                crate::transport_error::invalidates_connection(&err),
                "{who}: socket death must be an invalidating io error: {err}"
            );
            assert!(
                crate::transport_error::is_transient_redis_error(&err),
                "{who}: socket death is transient too: {err}"
            );
        }
        assert_eq!(
            cached_generation(&alive).await,
            None,
            "the two compare-and-clear invalidations must leave the cache empty"
        );
        assert_eq!(
            topology.resolve_call_count(),
            1,
            "neither invalidation may dial"
        );

        // A's next command lazily rebuilds under the gate: ONE new resolve,
        // generation 2, served by the fresh stub.
        let mut exchange = set_exchange("k");
        alive
            .execute_command(&RedisCommand::Set, &mut exchange)
            .await
            .expect("A must lazily rebuild against the fresh stub");
        assert_eq!(
            topology.resolve_call_count(),
            2,
            "the rebuild must re-resolve exactly once"
        );
        assert_eq!(alive.last_used_gen, 2);
        assert_eq!(cached_generation(&alive).await, Some(2));

        // B's reconnect compares its stale generation 1 against the cached
        // generation 2: NO clear, NO extra resolve.
        clone_b
            .reconnect()
            .await
            .expect("a stale-generation reconnect must reuse the fresh connection");
        assert_eq!(
            topology.resolve_call_count(),
            2,
            "B's stale generation must not evict A's fresh connection"
        );
        assert_eq!(cached_generation(&clone_b).await, Some(2));

        // Explicit stale-generation negative: compare-and-clear never drops
        // a newer generation.
        clone_b.invalidate_if_current(1).await;
        assert_eq!(
            cached_generation(&clone_b).await,
            Some(2),
            "invalidate_if_current must not clear a newer generation"
        );

        // B's next command is served the gen-2 connection.
        let mut exchange = set_exchange("k");
        clone_b
            .execute_command(&RedisCommand::Set, &mut exchange)
            .await
            .expect("B must be served the fresh connection");
        assert_eq!(clone_b.last_used_gen, 2);
        assert_eq!(
            topology.resolve_call_count(),
            2,
            "B's command must not resolve again"
        );
    }

    // The wrap site must stay byte-identical and keep the RedisError as its
    // source: the invalidation seam must not reclassify or re-wrap.
    #[tokio::test]
    async fn rethrow_original_error_unchanged() {
        let stub = test_stub::StubServer::start(test_stub::WRONGTYPE_REPLY).await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let mut executor = stub_executor(&stub.url(), topology);

        let mut exchange = set_exchange("k");
        let err = executor
            .execute_command(&RedisCommand::Set, &mut exchange)
            .await
            .expect_err("a server error reply must surface");
        let source = err
            .source()
            .expect("the wrap site must preserve the RedisError as source");
        assert_eq!(
            err.to_string(),
            format!("Processor error: Redis SET failed: {source}"),
            "the error must be the CamelError variant prefix plus the existing wrap-site text, \
             not re-wrapped"
        );
        assert!(
            source.to_string().contains("WRONGTYPE"),
            "the source must be the server reply: {source}"
        );
        assert_eq!(
            crate::transport_error::redis_error_code(&err),
            Some("WRONGTYPE"),
            "the inner RedisError must survive in the source chain (its code is readable)"
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
        use tokio::sync::Notify;

        let stub = test_stub::StubServer::start(test_stub::AppReply::Ok).await;
        let topology = Arc::new(FakeTopology::addrs(vec![stub.url()]));
        let executor = stub_executor(&stub.url(), topology);

        // Build the executor and prime the cache: the stub answers the
        // full handshake, so this connect succeeds and caches.
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
            let _g = gate.lock().await; // allow-test-wait: gate uncontended at spawn; bounded probe follows
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

    #[test]
    fn effective_response_timeout_blocking_endpoint_ignores_seam() {
        let blocking_config =
            RedisEndpointConfig::from_uri("redis://127.0.0.1:6379?command=BLPOP&key=k")
                .expect("BLPOP endpoint URI must parse");
        let blocking = MultiplexedExecutor::new(
            blocking_config,
            Arc::new(FakeTopology::addrs(vec![
                "redis://127.0.0.1:6379".to_string(),
            ])),
        )
        .with_response_timeout(Duration::from_millis(100));
        assert_eq!(
            blocking.effective_response_timeout(),
            None,
            "a blocking-pop endpoint must be deadline-free regardless of the seam"
        );

        let get_config = RedisEndpointConfig::from_uri("redis://127.0.0.1:6379?command=GET")
            .expect("GET endpoint URI must parse");
        let get = MultiplexedExecutor::new(
            get_config,
            Arc::new(FakeTopology::addrs(vec![
                "redis://127.0.0.1:6379".to_string(),
            ])),
        )
        .with_response_timeout(Duration::from_millis(100));
        assert_eq!(
            get.effective_response_timeout(),
            Some(Duration::from_millis(100)),
            "a non-blocking endpoint must keep the seam value"
        );
    }

    #[test]
    fn blocking_endpoint_includes_new_blocking_commands() {
        let executor_for = |command: &str| {
            let config = RedisEndpointConfig::from_uri(&format!(
                "redis://127.0.0.1:6379?command={command}&key=k"
            ))
            .unwrap_or_else(|e| panic!("{command} endpoint URI must parse: {e}"));
            MultiplexedExecutor::new(
                config,
                Arc::new(FakeTopology::addrs(vec![
                    "redis://127.0.0.1:6379".to_string(),
                ])),
            )
        };

        for command in ["BLMOVE", "BLMPOP", "BZMPOP"] {
            let executor = executor_for(command);
            assert!(
                executor.is_blocking_endpoint(),
                "{command} must be a blocking endpoint"
            );
            assert!(
                matches!(executor.deadline(), Deadline::Free),
                "{command} must build deadline-free connections"
            );
        }

        let non_blocking = executor_for("LMOVE");
        assert!(
            !non_blocking.is_blocking_endpoint(),
            "LMOVE is non-blocking and must keep the driver deadline"
        );
        assert!(
            !matches!(non_blocking.deadline(), Deadline::Free),
            "LMOVE must not get the blocking bypass"
        );
    }
}
