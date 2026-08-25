//! Shared helpers for the camel-component-redis integration test
//! targets (`response_timeout`, `single_flight`).
//!
//! This module is included by each test target via `mod common;`. It is
//! NOT a test target itself and contains no `#[test]` functions.
//! Because every target compiles this module independently, items used
//! by only some targets carry `#[allow(dead_code)]` naming their users.
//!
//! Two peer flavors live here:
//!
//! - [`HandshakeStub`] — a loopback TCP server that speaks just enough
//!   RESP to complete the redis driver handshake (`CLIENT`/`SELECT`/
//!   `AUTH` → `+OK`) and then consumes every application command
//!   without a reply. A zero-byte silent peer is NOT enough: the driver
//!   completes `setup_connection` before returning the connection, so
//!   the connect would fail instead of a command deadline firing. The
//!   stub additionally supports a hold gate: handshake replies can be
//!   withheld (parking the driver's connect mid-handshake) and later
//!   released — or the connection rejected outright — to stage
//!   concurrent-connect scenarios deterministically.
//! - [`never_handshake_peer`] — accepts TCP and never answers, so only
//!   the component's `connection_timeout_secs` wrapper can end the
//!   connect.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, watch};

// ------------------------------------------------------------------
// Handshake-completing silent stub (ported from camel-redis-repo's
// `FakeRedisServer::start_silent` + `StubConnection`), extended with
// the hold gate used by the single-flight tests.
// ------------------------------------------------------------------

/// Loopback RESP handshake stub with an optional hold gate.
///
/// The three constructors:
///
/// - [`HandshakeStub::start_silent`] — handshake answered immediately;
///   application commands consumed silently.
/// - [`HandshakeStub::start_silent_held`] — identical, except handshake
///   replies are withheld from the start (until `release()`).
/// - [`HandshakeStub::start_rejecting_after_hold`] — holds handshakes;
///   on `release()` every held connection is closed without replying
///   and every future accepted connection is closed immediately, so
///   every connect attempt after the hold fails.
pub struct HandshakeStub;

impl HandshakeStub {
    /// Silent stub: handshake commands are answered `+OK` immediately,
    /// application commands are read and dropped.
    pub fn start_silent() -> std::io::Result<(SocketAddr, HandshakeStubHandle)> {
        Self::start(HoldState::Free, false)
    }

    /// Silent stub with the hold gate engaged from the start: handshake
    /// replies are withheld until [`HandshakeStubHandle::release`].
    // Each integration-test target compiles `common` independently;
    // used by the single_flight target only.
    #[allow(dead_code)]
    pub fn start_silent_held() -> std::io::Result<(SocketAddr, HandshakeStubHandle)> {
        Self::start(HoldState::Held, false)
    }

    /// Rejecting stub: accepts connections and holds their handshakes;
    /// on [`HandshakeStubHandle::release`] every held connection is
    /// closed without replying and every future accepted connection is
    /// closed immediately — every connect attempt after the hold fails.
    // Each integration-test target compiles `common` independently;
    // used by the single_flight target only.
    #[allow(dead_code)]
    pub fn start_rejecting_after_hold() -> std::io::Result<(SocketAddr, HandshakeStubHandle)> {
        Self::start(HoldState::Held, true)
    }

    /// Bind an ephemeral loopback port and serve the stub. The address
    /// is bound before this call returns, so clients may connect
    /// immediately. Each accepted connection is served on its own task
    /// until the peer disconnects.
    ///
    /// Must be called from inside a tokio runtime (the listener is
    /// registered with the runtime's I/O driver and the accept loop is
    /// spawned onto it).
    fn start(
        initial: HoldState,
        reject_on_release: bool,
    ) -> std::io::Result<(SocketAddr, HandshakeStubHandle)> {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        std_listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(std_listener)?;
        let addr = listener.local_addr()?;

        let (hold, _) = watch::channel(initial);
        let inner = Arc::new(StubInner {
            hold,
            notify: Notify::new(),
            reject: AtomicBool::new(false),
            reject_on_release,
        });

        let accept_inner = Arc::clone(&inner);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                // Rejecting stub after release: every future connection
                // is closed at once — every connect attempt fails.
                if accept_inner.reject.load(Ordering::Acquire) {
                    close_clean(stream).await;
                    continue;
                }
                let inner = Arc::clone(&accept_inner);
                tokio::spawn(async move {
                    serve_connection(stream, inner).await;
                });
            }
        });

        Ok((addr, HandshakeStubHandle { inner }))
    }
}

/// Handle to a running [`HandshakeStub`]; controls the hold gate.
#[derive(Clone)]
pub struct HandshakeStubHandle {
    inner: Arc<StubInner>,
}

impl HandshakeStubHandle {
    /// Latch: handshake replies for NEW connections are withheld from
    /// now on. Connections already past their handshake are unaffected
    /// (application commands are never gated).
    // Each integration-test target compiles `common` independently;
    // used by the single_flight target only.
    #[allow(dead_code)]
    pub fn hold(&self) {
        self.inner.hold.send_replace(HoldState::Held);
    }

    /// One-shot unlock: sets the shared hold state back to `Free`
    /// (`watch::send`) AND wakes all currently-withheld handshakes
    /// (`Notify::notify_waiters`). Both actions are required:
    /// connections that open AFTER the release read `Free` and proceed
    /// without waiting; connections already parked on the notify are
    /// woken. On a stub started with
    /// [`HandshakeStub::start_rejecting_after_hold`] the release
    /// instead closes every held connection without replying and
    /// rejects every future accepted connection.
    // Each integration-test target compiles `common` independently;
    // used by the single_flight target only.
    #[allow(dead_code)]
    pub fn release(&self) {
        if self.inner.reject_on_release {
            self.inner.reject.store(true, Ordering::Release);
        }
        self.inner.hold.send_replace(HoldState::Free);
        self.inner.notify.notify_waiters();
    }
}

/// State shared between the accept loop and every connection task.
struct StubInner {
    /// Hold gate state, broadcast to every connection task.
    hold: watch::Sender<HoldState>,
    /// Wakes every handshake parked on `Held` at release time.
    notify: Notify,
    /// Set by `release()` on a rejecting stub: close held and future
    /// connections instead of answering them.
    reject: AtomicBool,
    /// Whether `release()` arms `reject` (rejecting stub flavor).
    reject_on_release: bool,
}

/// Hold-gate state: `Free` answers handshakes, `Held` withholds the
/// replies until released.
// Each integration-test target compiles `common` independently; `Held`
// is constructed only through the single_flight-gated constructors.
#[allow(dead_code)]
enum HoldState {
    Free,
    Held,
}

/// Serve one accepted connection: answer handshake commands (subject to
/// the hold gate), consume application commands silently.
async fn serve_connection(stream: TcpStream, inner: Arc<StubInner>) {
    let mut stub = StubConnection::new(stream);
    let mut hold_rx = inner.hold.subscribe();
    // Consume one complete RESP request frame at a time. Handshake
    // commands get `+OK` (gated); every other frame is read and dropped
    // so the client's command await pends until its response deadline
    // fires.
    loop {
        let Ok(Some(command)) = stub.next_command().await else {
            break;
        };
        if !is_handshake_command(&command) {
            continue;
        }
        if !wait_until_released(&inner, &mut hold_rx).await {
            // Rejecting stub released under us: close without replying.
            break;
        }
        if stub.write_reply(b"+OK\r\n").await.is_err() {
            break;
        }
    }
}

/// Park while the hold gate is `Held`.
///
/// Returns `true` when the handshake may be answered. Returns `false`
/// when the connection must be closed instead (a rejecting stub was
/// released).
///
/// Uses tokio's `Notified::enable()` pattern: the `Notified` future is
/// created and `enable()`d — registering interest — BEFORE the watch
/// state is re-checked, and only then polled. A `release()` landing
/// between the state check and the poll is therefore never missed,
/// regardless of runtime flavor.
async fn wait_until_released(inner: &StubInner, hold_rx: &mut watch::Receiver<HoldState>) -> bool {
    loop {
        let mut notified = std::pin::pin!(inner.notify.notified());
        notified.as_mut().enable();
        if matches!(*hold_rx.borrow_and_update(), HoldState::Free) {
            return !inner.reject.load(Ordering::Acquire);
        }
        notified.await;
    }
}

/// Close a TCP connection RST-free: shut the write side down (the
/// peer's reads see EOF), then drain the read side until the peer
/// closes, so the kernel never discards unread buffered data (which
/// would emit an RST). If the peer never closes, the task simply parks
/// here; the test process exit reaps it.
async fn close_clean(mut stream: TcpStream) {
    let _ = stream.shutdown().await;
    let mut drain = [0u8; 512];
    loop {
        match stream.read(&mut drain).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
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
// Each integration-test target compiles `common` independently; used by
// the response_timeout target only.
#[allow(dead_code)]
pub fn never_handshake_peer() -> SocketAddr {
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
