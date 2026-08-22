//! Command executor seam for Redis-backed repositories.
//!
//! [`RepoCommandExecutor`] narrows the transport surface the repositories
//! need — execute one command, refresh the connection — so repository logic
//! can be unit-tested against [`FakeRepoExecutor`] without a live Redis.
//!
//! The production [`MultiplexedRepoExecutor`] wraps the component's
//! [`MultiplexedExecutor`], remaps every connection failure to
//! [`CamelError::Io`], and bounds every command round-trip with a response
//! timeout, enforcing the repository transport contract: transport
//! failures must classify as `"io"` (`is_transient_redis_error`), never
//! surface as the component's `ProcessorError`, and never park an
//! Exchange-processing future forever on a silent peer.

use crate::is_transient_redis_error;
use crate::to_camel_error;
use async_trait::async_trait;
use camel_api::CamelError;
use camel_component_redis::MultiplexedExecutor;
#[cfg(test)]
use camel_component_redis::RedisTopology;
#[cfg(test)]
use camel_component_redis::ServerKind;
use redis::from_redis_value;
#[cfg(test)]
use std::collections::VecDeque;
#[cfg(test)]
use std::net::SocketAddr;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
#[cfg(test)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(test)]
use tokio::net::{TcpListener, TcpStream};
#[cfg(test)]
use tokio::task::JoinHandle;

/// Transport seam used by the Redis-backed repositories.
///
/// `execute` sends one command and returns the raw RESP value. `refresh`
/// drops any cached connection and resolves a fresh one through the
/// topology, which is how sentinel failover is detected.
#[async_trait]
pub(crate) trait RepoCommandExecutor: Send + Sync {
    /// Send one Redis command and return the raw RESP reply value.
    async fn execute(&self, cmd: redis::Cmd) -> Result<redis::Value, CamelError>;

    /// Drop the cached connection and resolve a fresh one.
    async fn refresh(&self) -> Result<(), CamelError>;
}

/// Crate-owned backstop on one command round-trip. The redis driver's
/// own per-request response timeout (500 ms by default in redis 1.6.0)
/// fires first on a silent or slow peer; this wrap makes the deadline a
/// crate contract instead of a driver default. It bounds `query_async`
/// only — topology resolves in `get_conn`/`refresh` run outside it.
/// `connection_timeout_secs` guards only the TCP connect.
const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Production [`RepoCommandExecutor`] over the component's cached
/// multiplexed connection.
pub(crate) struct MultiplexedRepoExecutor {
    inner: MultiplexedExecutor,
    response_timeout: Duration,
}

impl MultiplexedRepoExecutor {
    /// Wrap a component [`MultiplexedExecutor`].
    pub(crate) fn new(inner: MultiplexedExecutor) -> Self {
        Self {
            inner,
            response_timeout: DEFAULT_RESPONSE_TIMEOUT,
        }
    }

    /// Test seam: shrink the per-command response timeout so timeout
    /// behavior is observable in unit tests without waiting 30 seconds.
    /// Production executors always use [`DEFAULT_RESPONSE_TIMEOUT`].
    #[cfg(test)]
    pub(crate) fn with_response_timeout(mut self, response_timeout: Duration) -> Self {
        self.response_timeout = response_timeout;
        self
    }
}

/// Execute one retry-safe command with a single refresh-and-retry on a
/// transient transport failure.
///
/// On a transient error the executor is refreshed (which is how sentinel
/// failover is picked up) and the SAME command is re-issued exactly once;
/// a second failure surfaces unchanged. Non-transient errors return
/// immediately. Only call this with commands that are safe to re-issue
/// (last-writer-wins SET, idempotent GET/SCAN/UNLINK).
pub(crate) async fn execute_retry_safe(
    ex: &Arc<dyn RepoCommandExecutor>,
    cmd: redis::Cmd,
) -> Result<redis::Value, CamelError> {
    match ex.execute(cmd.clone()).await {
        Ok(value) => Ok(value),
        Err(err) if is_transient_redis_error(&err) => {
            ex.refresh().await?;
            ex.execute(cmd).await
        }
        Err(err) => Err(err),
    }
}

/// SCAN-page over every key matching `pattern` and UNLINK it in batches,
/// returning the total removed-key count.
///
/// SCAN iterations restart from a fixed cursor after a retry (cursor-based
/// iteration may re-visit a key; UNLINK is idempotent), and each UNLINK
/// batch goes through [`execute_retry_safe`] so a failed batch re-issues
/// unchanged. `clear`/`invalidate_prefix` scope the pattern so this NEVER
/// issues FLUSHDB/FLUSHALL.
pub(crate) async fn scan_unlink_pattern(
    ex: &Arc<dyn RepoCommandExecutor>,
    pattern: &str,
) -> Result<u64, CamelError> {
    const PAGE: usize = 100;
    let mut cursor: u64 = 0;
    let mut removed: u64 = 0;
    loop {
        let mut scan = redis::Cmd::new();
        scan.arg("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(PAGE);
        let reply = execute_retry_safe(ex, scan).await?;
        let (next, keys): (u64, Vec<String>) = from_redis_value(reply)
            .map_err(|e| CamelError::Io(format!("SCAN reply parse: {e}")))?;
        for batch in keys.chunks(PAGE) {
            if batch.is_empty() {
                continue;
            }
            let mut unlink = redis::Cmd::new();
            unlink.arg("UNLINK");
            for key in batch {
                unlink.arg(key);
            }
            let reply = execute_retry_safe(ex, unlink).await?;
            removed += from_redis_value::<u64>(reply)
                .map_err(|e| CamelError::Io(format!("UNLINK reply parse: {e}")))?;
        }
        cursor = next;
        if cursor == 0 {
            return Ok(removed);
        }
    }
}

#[async_trait]
impl RepoCommandExecutor for MultiplexedRepoExecutor {
    async fn execute(&self, cmd: redis::Cmd) -> Result<redis::Value, CamelError> {
        // The component maps connection failures to ProcessorError; the
        // repository contract requires transport failures to be Io so they
        // classify as "io" for transient-error handling. Remap ALL errors
        // from get_conn, not just RedisError-typed ones.
        let mut conn = self
            .inner
            .get_conn()
            .await
            .map_err(|e| CamelError::Io(e.to_string()))?;
        // `connection_timeout_secs` (and the driver's own defaults) bound
        // the connect, not this crate's command contract: a silent peer
        // must never park an Exchange-processing future. The elapsed
        // timeout maps to transient Io so retry-safe operations refresh
        // and re-resolve, and `add` surfaces Err without a re-issue (C1).
        match tokio::time::timeout(self.response_timeout, cmd.query_async(&mut conn)).await {
            Ok(result) => result.map_err(to_camel_error),
            Err(_elapsed) => Err(CamelError::Io(format!(
                "redis command response timed out after {:?}",
                self.response_timeout
            ))),
        }
    }

    async fn refresh(&self) -> Result<(), CamelError> {
        self.inner
            .refresh()
            .await
            .map(|_| ())
            .map_err(|e| CamelError::Io(e.to_string()))
    }
}

/// In-crate fake: records executed commands and replays scripted results.
///
/// The scripted queue is consumed front-to-back; an empty queue returns
/// `Ok(redis::Value::Nil)`. `refresh` always succeeds and is only counted.
#[cfg(test)]
pub(crate) struct FakeRepoExecutor {
    commands: Mutex<Vec<redis::Cmd>>,
    results: Mutex<VecDeque<Result<redis::Value, CamelError>>>,
    execute_count: AtomicUsize,
    refresh_count: AtomicUsize,
}

#[cfg(test)]
impl Default for FakeRepoExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl FakeRepoExecutor {
    /// Create a fake with an empty scripted-result queue.
    pub(crate) fn new() -> Self {
        Self {
            commands: Mutex::new(Vec::new()),
            results: Mutex::new(VecDeque::new()),
            execute_count: AtomicUsize::new(0),
            refresh_count: AtomicUsize::new(0),
        }
    }

    /// Queue the next result returned by `execute`.
    pub(crate) fn push_result(&self, result: Result<redis::Value, CamelError>) {
        self.results
            .lock()
            .expect("results mutex")
            .push_back(result);
    }

    /// Snapshot of every command passed to `execute`, in call order.
    pub(crate) fn commands(&self) -> Vec<redis::Cmd> {
        self.commands.lock().expect("commands mutex").clone()
    }

    /// Number of times `execute` was called.
    pub(crate) fn execute_count(&self) -> usize {
        self.execute_count.load(Ordering::SeqCst)
    }

    /// Number of times `refresh` was called.
    pub(crate) fn refresh_count(&self) -> usize {
        self.refresh_count.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[async_trait]
impl RepoCommandExecutor for FakeRepoExecutor {
    async fn execute(&self, cmd: redis::Cmd) -> Result<redis::Value, CamelError> {
        self.execute_count.fetch_add(1, Ordering::SeqCst);
        self.commands.lock().expect("commands mutex").push(cmd);
        self.results
            .lock()
            .expect("results mutex")
            .pop_front()
            .unwrap_or(Ok(redis::Value::Nil))
    }

    async fn refresh(&self) -> Result<(), CamelError> {
        self.refresh_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Shared test-support helpers for the in-crate unit tests (beside
/// [`FakeRepoExecutor`]). The cache and idempotent test modules both
/// assert against recorded commands; these helpers live once here so the
/// two modules cannot drift.
#[cfg(test)]
pub(crate) mod test_support {
    /// Flattened command arguments (command name included) for assertions.
    pub(crate) fn cmd_args(cmd: &redis::Cmd) -> Vec<Vec<u8>> {
        cmd.args_iter()
            .map(|arg| match arg {
                redis::Arg::Simple(bytes) => bytes.to_vec(),
                redis::Arg::Cursor => Vec::new(),
                _ => Vec::new(),
            })
            .collect()
    }

    /// SCAN-reply-shaped RESP value: bulk-string cursor + key array (the
    /// shape a real server returns, unlike an Int cursor).
    pub(crate) fn scan_reply(cursor: u64, keys: &[&str]) -> redis::Value {
        redis::Value::Array(vec![
            redis::Value::BulkString(cursor.to_string().into_bytes()),
            redis::Value::Array(
                keys.iter()
                    .map(|k| redis::Value::BulkString(k.as_bytes().to_vec()))
                    .collect(),
            ),
        ])
    }

    /// Position of the argument right after `marker` in a flattened command.
    pub(crate) fn arg_after(args: &[Vec<u8>], marker: &[u8]) -> Vec<u8> {
        let at = args
            .iter()
            .position(|arg| arg == marker)
            .unwrap_or_else(|| panic!("no {marker:?} argument in {args:?}"));
        args[at + 1].clone()
    }

    /// True when `needle` appears as a substring of any argument of any
    /// recorded command.
    pub(crate) fn any_arg_contains(commands: &[redis::Cmd], needle: &str) -> bool {
        commands.iter().any(|cmd| {
            cmd_args(cmd)
                .iter()
                .any(|arg| String::from_utf8_lossy(arg).contains(needle))
        })
    }
}

/// Topology fake that always resolves to one scripted [`redis::Client`].
///
/// The component's own `FakeTopology` is `#[cfg(test)]`-gated and cannot be
/// imported, so repositories need this in-crate equivalent. Clones share the
/// resolve counter, so a clone handed to the executor can still be observed
/// by the test.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct FakeStaticTopology {
    client: Arc<Mutex<Option<redis::Client>>>,
    resolve_count: Arc<AtomicUsize>,
}

#[cfg(test)]
impl FakeStaticTopology {
    /// Create a topology whose every `resolve` returns `client`.
    pub(crate) fn with_client(client: redis::Client) -> Self {
        Self {
            client: Arc::new(Mutex::new(Some(client))),
            resolve_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Number of times `resolve` has been called (shared across clones).
    pub(crate) fn resolve_count(&self) -> usize {
        self.resolve_count.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[async_trait]
impl RedisTopology for FakeStaticTopology {
    async fn resolve(&self, _kind: ServerKind) -> Result<redis::Client, CamelError> {
        self.resolve_count.fetch_add(1, Ordering::SeqCst);
        self.client
            .lock()
            .expect("client mutex")
            .clone()
            .ok_or_else(|| {
                CamelError::Config("test setup: no client scripted in FakeStaticTopology".into())
            })
    }
}

/// Minimal in-process Redis stub: binds an ephemeral loopback port and
/// answers every complete RESP request frame with a success reply.
///
/// `PING` is answered with `+PONG`; every other command (including the
/// driver's `CLIENT SETINFO` handshake) is answered with `+OK`. The
/// silent variant answers only the handshake and then swallows commands.
/// The stub keeps no state — it exists to give tests a deterministic
/// healthy or silent endpoint without Docker or a real server.
/// `#[cfg(test)]`-gated but shared crate-wide: `cfg(test)` is set for the
/// whole crate in test builds, so other modules' unit tests reach it
/// through the module path.
#[cfg(test)]
pub(crate) struct FakeRedisServer {
    /// When true, only handshake commands are answered; every later
    /// frame is read and dropped, modeling a peer that accepts commands
    /// but never replies (half-open socket, black-holed network).
    silent: bool,
}

#[cfg(test)]
impl FakeRedisServer {
    /// Bind `127.0.0.1:0` eagerly and spawn the accept loop.
    ///
    /// The address is bound before this call returns, so clients may
    /// connect immediately. The accept loop runs until its task is aborted
    /// or the listener errors; each accepted connection is served on its
    /// own task until the peer disconnects.
    pub(crate) async fn start() -> std::io::Result<(SocketAddr, JoinHandle<()>)> {
        Self { silent: false }.serve().await
    }

    /// Silent variant: the driver handshake still completes (commands
    /// like `CLIENT SETINFO` get replies), then every later command frame
    /// is consumed without a reply, so a connected client's command await
    /// pends until its response timeout fires.
    pub(crate) async fn start_silent() -> std::io::Result<(SocketAddr, JoinHandle<()>)> {
        Self { silent: true }.serve().await
    }

    /// Shared accept loop for both modes.
    async fn serve(self) -> std::io::Result<(SocketAddr, JoinHandle<()>)> {
        let silent = self.silent;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut stub = StubConnection::new(stream);
                    // Serve until the peer disconnects: consume one complete
                    // RESP request frame, answer it, repeat. In silent mode
                    // non-handshake frames are consumed but never answered.
                    loop {
                        let Ok(Some(command)) = stub.next_command().await else {
                            break;
                        };
                        if silent && !is_handshake_command(&command) {
                            continue;
                        }
                        let reply: &[u8] = if command == b"PING" {
                            b"+PONG\r\n"
                        } else {
                            b"+OK\r\n"
                        };
                        if stub.write_reply(reply).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Ok((addr, handle))
    }
}

/// True for the commands the redis-rs driver issues while establishing a
/// connection (`CLIENT SETINFO`, `SELECT`, `AUTH`). These still get
/// replies in silent mode so the handshake completes and only
/// application commands hang.
#[cfg(test)]
fn is_handshake_command(command: &[u8]) -> bool {
    [b"CLIENT".as_slice(), b"SELECT", b"AUTH"]
        .iter()
        .any(|handshake| handshake.eq_ignore_ascii_case(command))
}

/// One accepted stub connection: the TCP stream plus a persistent read
/// buffer.
///
/// The buffer must outlive a single request: redis-rs pipelines its
/// handshake commands, so one TCP read can carry several complete RESP
/// frames. A per-request buffer would discard the buffered-but-unparsed
/// frames and deadlock waiting for bytes already consumed.
#[cfg(test)]
struct StubConnection {
    stream: TcpStream,
    buf: Vec<u8>,
}

#[cfg(test)]
impl StubConnection {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            buf: Vec::with_capacity(64),
        }
    }

    /// Consume one complete RESP request frame (array of bulk strings).
    ///
    /// Returns `Ok(Some(first_argument))` after a full frame was consumed,
    /// `Ok(None)` on clean EOF before any byte, `Err` on I/O failure or
    /// malformed input.
    async fn next_command(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let mut chunk = [0u8; 256];
        loop {
            if let Some((frame_len, first_argument)) = parse_frame(&self.buf)? {
                let _ = self.buf.drain(..frame_len);
                return Ok(Some(first_argument));
            }
            let n = self.stream.read(&mut chunk).await?;
            if n == 0 {
                return if self.buf.is_empty() {
                    Ok(None)
                } else {
                    Err(invalid_data("client closed mid-frame"))
                };
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// Write one complete RESP reply. Returns `Err` on I/O failure.
    async fn write_reply(&mut self, reply: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(reply).await?;
        self.stream.flush().await
    }
}

/// Try to parse one complete RESP array-of-bulk-strings frame from `buf`.
///
/// Returns `Ok(Some((total_len, first_argument)))` when a full frame is
/// buffered, `Ok(None)` when more bytes are needed, `Err` on malformed
/// input. Element counts and lengths can span reads — parsing restarts on
/// every appended chunk, which is fine for test-sized frames.
#[cfg(test)]
fn parse_frame(buf: &[u8]) -> std::io::Result<Option<(usize, Vec<u8>)>> {
    let Some((elements, mut pos)) = parse_integer_header(buf, 0, b'*')? else {
        return Ok(None);
    };
    let mut first_argument = Vec::new();
    for index in 0..elements {
        let Some((len, payload_at)) = parse_integer_header(buf, pos, b'$')? else {
            return Ok(None);
        };
        let Some(end) = payload_at.checked_add(len).and_then(|e| e.checked_add(2)) else {
            return Err(invalid_data("bulk string length overflow"));
        };
        if buf.len() < end {
            return Ok(None);
        }
        if index == 0 {
            first_argument = buf[payload_at..payload_at + len].to_vec();
        }
        pos = end;
    }
    Ok(Some((pos, first_argument)))
}

/// Parse `<marker><digits>\r\n` starting at `offset`.
///
/// Returns the parsed integer and the byte offset just past the header
/// line, `Ok(None)` when the header is not fully buffered yet.
#[cfg(test)]
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
                Some(_) => return Err(invalid_data("expected \\n after \\r in header")),
                None => return Ok(None),
            },
            Some(_) => return Err(invalid_data("unexpected byte in RESP header")),
            None => return Ok(None),
        }
    }
}

#[cfg(test)]
fn invalid_data(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::FakeRedisServer;
    use super::FakeRepoExecutor;
    use super::FakeStaticTopology;
    use super::MultiplexedRepoExecutor;
    use super::RepoCommandExecutor;
    use super::execute_retry_safe;
    use super::is_transient_redis_error;
    use camel_api::CamelError;
    use camel_component_redis::MultiplexedExecutor;
    use camel_component_redis::RedisEndpointConfig;
    use std::sync::Arc;
    use std::time::Duration;

    /// Standalone config with a short connect timeout so dead-address tests
    /// fail fast (same pattern as the component's executor tests).
    fn short_timeout_config() -> RedisEndpointConfig {
        let mut config =
            RedisEndpointConfig::from_uri("redis://localhost:6379").expect("valid standalone URI");
        config.connection_timeout_secs = 1;
        config
    }

    /// Build an owned single-argument command (`Cmd::arg` borrows, so a
    /// chain cannot be passed by value).
    fn cmd(name: &str) -> redis::Cmd {
        let mut cmd = redis::Cmd::new();
        cmd.arg(name);
        cmd
    }

    #[tokio::test]
    async fn fake_records_commands_and_counts() {
        let fake = FakeRepoExecutor::new();
        fake.push_result(Ok(redis::Value::SimpleString("PONG".into())));
        fake.push_result(Ok(redis::Value::Int(1)));

        let first = fake
            .execute(cmd("PING"))
            .await
            .expect("first scripted execute");
        let second = fake
            .execute(cmd("PING"))
            .await
            .expect("second scripted execute");
        let third = fake
            .execute(cmd("GET"))
            .await
            .expect("exhausted queue defaults to Ok(Nil)");

        assert!(
            matches!(&first, redis::Value::SimpleString(s) if s == "PONG"),
            "first result should come from the scripted queue, got: {first:?}"
        );
        assert_eq!(second, redis::Value::Int(1));
        assert_eq!(third, redis::Value::Nil);

        let commands = fake.commands();
        assert_eq!(commands.len(), 3, "every executed command is recorded");
        assert_eq!(fake.execute_count(), 3);
        assert_eq!(fake.refresh_count(), 0);
    }

    #[tokio::test]
    async fn fake_counts_refreshes() {
        let fake = FakeRepoExecutor::new();

        fake.refresh().await.expect("fake refresh always succeeds");

        assert_eq!(fake.refresh_count(), 1);
        assert_eq!(fake.execute_count(), 0);
        assert!(fake.commands().is_empty());
    }

    #[tokio::test]
    async fn multiplexed_execute_maps_transport_error_to_io() {
        // Client::open only parses the URL; the TCP connection to port 1 is
        // refused deterministically at execute time.
        let topology = FakeStaticTopology::with_client(
            redis::Client::open("redis://127.0.0.1:1/0").expect("client opens without network"),
        );
        let executor = MultiplexedRepoExecutor::new(MultiplexedExecutor::new(
            short_timeout_config(),
            Arc::new(topology.clone()),
        ));

        let result = executor.execute(cmd("PING")).await;

        match result {
            Err(CamelError::Io(_)) => {}
            other => panic!("expected CamelError::Io, got: {other:?}"),
        }
        assert_eq!(
            topology.resolve_count(),
            1,
            "execute must consult the topology exactly once"
        );
    }

    // A silent peer accepts the handshake and the command, then never
    // replies. Without the per-command response timeout this await would
    // pend forever; the meta-guard turns that into a 5s test failure.
    #[tokio::test]
    async fn execute_times_out_on_silent_peer() {
        let (addr, _server) = FakeRedisServer::start_silent()
            .await
            .expect("silent stub server binds an ephemeral loopback port");
        let topology = FakeStaticTopology::with_client(
            redis::Client::open(format!("redis://{addr}/0")).expect("client opens without network"),
        );
        let executor = MultiplexedRepoExecutor::new(MultiplexedExecutor::new(
            short_timeout_config(),
            Arc::new(topology),
        ))
        .with_response_timeout(Duration::from_millis(150));

        let outcome =
            tokio::time::timeout(Duration::from_secs(5), executor.execute(cmd("PING"))).await;
        let result = outcome.expect("execute must return within the meta-guard window");
        match result {
            Err(CamelError::Io(message)) => assert!(
                message.contains("redis command response timed out after 150ms"),
                "error must be the executor's response timeout: {message}"
            ),
            other => panic!("expected CamelError::Io timeout, got: {other:?}"),
        }
    }

    // Pins the coupling between the timeout error wording and the
    // transient classifier, then proves the retry-safe wrapper acts on it.
    #[tokio::test]
    async fn response_timeout_is_transient_and_drives_refresh() {
        let timeout_error = CamelError::Io(format!(
            "redis command response timed out after {:?}",
            Duration::from_secs(30)
        ));
        assert!(
            is_transient_redis_error(&timeout_error),
            "response-timeout Io must classify as transient"
        );

        let fake = Arc::new(FakeRepoExecutor::new());
        fake.push_result(Err(timeout_error));
        fake.push_result(Ok(redis::Value::SimpleString("PONG".into())));
        let ex: Arc<dyn RepoCommandExecutor> = fake.clone();

        let reply = execute_retry_safe(&ex, cmd("PING"))
            .await
            .expect("retry-safe re-issues once after the transient timeout");
        assert!(
            matches!(reply, redis::Value::SimpleString(ref s) if s == "PONG"),
            "expected the re-issued command's reply, got: {reply:?}"
        );
        assert_eq!(fake.execute_count(), 2, "command issued twice");
        assert_eq!(fake.refresh_count(), 1, "refresh fired once");
    }

    // Smoke test: the stub server must accept a real redis-rs multiplexed
    // connection and answer PING before later modules rely on it.
    #[tokio::test]
    async fn fake_redis_server_answers_ping() {
        let (addr, _server) = FakeRedisServer::start()
            .await
            .expect("stub server binds an ephemeral loopback port");
        let client =
            redis::Client::open(format!("redis://{addr}/0")).expect("client opens without network");
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .expect("stub server accepts connections");

        let reply: redis::Value = redis::Cmd::new()
            .arg("PING")
            .query_async(&mut conn)
            .await
            .expect("stub server answers PING");
        assert!(
            matches!(reply, redis::Value::SimpleString(ref s) if s == "PONG"),
            "expected +PONG, got: {reply:?}"
        );
    }
}
