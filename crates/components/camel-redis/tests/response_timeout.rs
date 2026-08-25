//! Driver-level response-deadline tests for `MultiplexedExecutor`'s
//! `with_response_timeout` builder (task 1.2, redis-response-timeout).
//!
//! Exercises the public surface only: `MultiplexedExecutor::new`,
//! `with_response_timeout`, `get_conn`, `refresh`,
//! `RedisEndpointConfig::from_uri`, and `topology_from_config`.
//!
//! The silent stub answers the driver handshake (`CLIENT`/`SELECT`/`AUTH`
//! → `+OK`) and then consumes every application command without a reply,
//! so a connected client's command await pends until its configured
//! response deadline fires. A zero-byte silent peer is NOT enough: the
//! driver completes `setup_connection` before returning the connection,
//! so the connect would fail instead of the command deadline firing.
//!
//! Clock discipline: tests 1-3 connect in real time and call
//! `tokio::time::pause()` only for the measured query — `start_paused`
//! with pending real-socket handshakes lets virtual time auto-advance
//! spuriously past live timers. Test 4 has no real I/O that completes,
//! so `start_paused` is deterministic there.

use camel_api::CamelError;
use camel_component_redis::{MultiplexedExecutor, RedisEndpointConfig, topology_from_config};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// ------------------------------------------------------------------
// Handshake-completing silent stub (ported from camel-redis-repo's
// `FakeRedisServer::start_silent` + `StubConnection`).
// ------------------------------------------------------------------

/// Bind an ephemeral loopback port and serve the silent stub.
///
/// The address is bound before this call returns, so clients may connect
/// immediately. Each accepted connection is served on its own task until
/// the peer disconnects.
async fn silent_redis_peer() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut stub = StubConnection::new(stream);
                // Consume one complete RESP request frame at a time.
                // Handshake commands get `+OK`; every other frame is read
                // and dropped so the client's command await pends until
                // its response deadline fires.
                loop {
                    let Ok(Some(command)) = stub.next_command().await else {
                        break;
                    };
                    if !is_handshake_command(&command) {
                        continue;
                    }
                    if stub.write_reply(b"+OK\r\n").await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

/// True for the commands the redis driver issues while establishing a
/// connection (`CLIENT SETINFO`, `SELECT`, `AUTH`). These still get
/// replies in silent mode so the handshake completes and only
/// application commands hang.
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
struct StubConnection {
    stream: TcpStream,
    buf: Vec<u8>,
}

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

fn invalid_data(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

// ------------------------------------------------------------------
// Never-handshake peer: TCP accepted, handshake never answered.
// ------------------------------------------------------------------

/// Accept connections and park forever — never read, never write — so the
/// driver handshake never completes and only the component's
/// `connection_timeout_secs` wrapper can end the connect.
fn never_handshake_peer() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            // Hold the stream open without reading or writing.
            std::mem::forget(stream);
        }
    });
    addr
}

// ------------------------------------------------------------------
// Executor under test, built through the public surface.
// ------------------------------------------------------------------

/// Standalone executor pointing at `addr` with the configured driver
/// response deadline. `connection_timeout_secs` stays at its default.
fn executor_for(addr: SocketAddr, response_timeout: Duration) -> MultiplexedExecutor {
    let config = RedisEndpointConfig::from_uri(&format!("redis://{addr}"))
        .expect("valid standalone URI for the stub");
    let topology = topology_from_config(&config).expect("standalone topology");
    MultiplexedExecutor::new(config, topology).with_response_timeout(response_timeout)
}

#[tokio::test]
async fn configured_large_timeout_outlives_driver_default() {
    let addr = silent_redis_peer().await;
    let executor = executor_for(addr, Duration::from_secs(10));

    // Real time: the stub answers the handshake, so the connect succeeds.
    let mut conn = executor
        .get_conn()
        .await
        .expect("handshake completes against the stub");

    tokio::time::pause();

    // With the configured 10s driver deadline governing, PING must still
    // be Pending at the 1s guard boundary — the guard fires. Had the
    // driver's 500ms default governed, the command would have FAILED
    // inside the guard instead.
    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        redis::cmd("PING").query_async::<()>(&mut conn),
    )
    .await;
    let Err(_elapsed) = outcome else {
        panic!(
            "guard must fire: the command must still be Pending at 1s (the driver 500ms default governed?)"
        );
    };
}

#[tokio::test]
async fn configured_small_timeout_fires_before_driver_default() {
    let addr = silent_redis_peer().await;
    let executor = executor_for(addr, Duration::from_millis(100));

    // Real time: the handshake completes.
    let mut conn = executor
        .get_conn()
        .await
        .expect("handshake completes against the stub");

    tokio::time::pause();
    let started = tokio::time::Instant::now();

    let outcome = tokio::time::timeout(
        Duration::from_millis(600),
        redis::cmd("PING").query_async::<()>(&mut conn),
    )
    .await;
    let elapsed = started.elapsed();

    let Ok(query) = outcome else {
        panic!("configured 100ms deadline must fire inside the 600ms guard, not the guard itself");
    };
    assert!(
        query.is_err(),
        "PING against the silent stub must fail with the response-deadline error"
    );
    assert!(
        elapsed >= Duration::from_millis(100) && elapsed < Duration::from_millis(500),
        "deadline must fire in [100ms, 500ms); a regression to the 500ms driver default \
         lands exactly at 500ms and fails this bound; elapsed: {elapsed:?}"
    );
}

#[tokio::test]
async fn refresh_rebuild_carries_configured_deadline() {
    let addr = silent_redis_peer().await;
    let executor = executor_for(addr, Duration::from_millis(100));

    // Real time: the initial connect and the refresh rebuild both
    // complete against the stub (the accept loop serves per-connection).
    assert!(executor.get_conn().await.is_ok());
    let mut conn = executor
        .refresh()
        .await
        .expect("refresh rebuilds a live connection");

    tokio::time::pause();
    let started = tokio::time::Instant::now();

    let outcome = tokio::time::timeout(
        Duration::from_millis(600),
        redis::cmd("PING").query_async::<()>(&mut conn),
    )
    .await;
    let elapsed = started.elapsed();

    let Ok(query) = outcome else {
        panic!(
            "rebuilt connection must inherit the 100ms deadline and fail inside the 600ms guard"
        );
    };
    assert!(
        query.is_err(),
        "PING on the rebuilt connection must fail with the response-deadline error"
    );
    assert!(
        elapsed >= Duration::from_millis(100) && elapsed < Duration::from_millis(500),
        "rebuilt connection deadline must fire in [100ms, 500ms); a rebuild that drops \
         the config regresses to the 500ms driver default; elapsed: {elapsed:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn response_timeout_does_not_alter_connect_timeout() {
    let addr = never_handshake_peer();
    let mut config = RedisEndpointConfig::from_uri(&format!("redis://{addr}"))
        .expect("valid standalone URI for the peer");
    config.connection_timeout_secs = 1;
    let topology = topology_from_config(&config).expect("standalone topology");
    let executor = MultiplexedExecutor::new(config, topology)
        .with_response_timeout(Duration::from_millis(100));

    // The peer accepts TCP but never answers the handshake, and no real
    // I/O ever completes, so paused virtual time jumps deterministically
    // to the component's 1s connect-timeout timer. The response-timeout
    // builder must not shift this bound: the 100ms response deadline
    // governs command awaits, not the connect wrapper.
    let err = executor
        .get_conn()
        .await
        .expect_err("handshake never completes");
    match err {
        CamelError::ProcessorError(msg) => assert!(
            msg.contains("timed out after 1s"),
            "component connect-timeout message must govern, got: {msg}"
        ),
        other => panic!("expected ProcessorError, got: {other:?}"),
    }
}
