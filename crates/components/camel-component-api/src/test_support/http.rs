//! Shared HTTP/1.1 test harness.
//!
//! A small, dependency-free (tokio + std) listener for component and
//! integration tests that need a real HTTP peer without `hyper`. It parses
//! the request line and headers, drains the request body according to
//! `Content-Length`, records the exact wire head, and answers with a
//! scripted response. By default every response injects `Connection: close`
//! and the socket closes afterwards; `HttpTestServer::builder` enables
//! keep-alive, which serves sequential and pipelined requests on one socket.
//! Responses can be `Content-Length`, chunked, or staged over a channel, and
//! framing failures answer `400 Bad Request`.
//!
//! Gated by the parent module's `#[cfg(any(test, feature = "test-support"))]`.

#[cfg(test)]
#[path = "http_tests.rs"]
mod http_tests;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

/// Bounded budget for the explicit `shutdown` teardown path.
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

/// Request captured by the harness.
///
/// Both layers are exposed: parsed fields (method, target, lowercased
/// headers, body) and the exact request head bytes sent by the client
/// (request line + header block, including the terminating blank line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    /// Request method verbatim (`GET`, `POST`, ...).
    pub method: String,
    /// Unnormalized request target, including any query bytes.
    pub target: String,
    /// Header names lowercased, values trimmed.
    pub headers: Vec<(String, String)>,
    /// Request body read according to `Content-Length`.
    pub body: Vec<u8>,
    /// Exact request line + head bytes, including the final `\r\n\r\n`.
    pub wire_head: Vec<u8>,
}

/// Header block for [`Response::staged`].
///
/// [`ResponseHeaders::encode`] emits the status line, the supplied header
/// lines, one connection header chosen by the harness, and the terminating
/// blank line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHeaders {
    /// Status code.
    pub status: u16,
    /// Reason phrase.
    pub reason: String,
    /// Header name/value pairs, written in order.
    pub headers: Vec<(String, String)>,
}

impl ResponseHeaders {
    /// Build a header block.
    pub fn new(status: u16, reason: impl Into<String>, headers: Vec<(String, String)>) -> Self {
        Self {
            status,
            reason: reason.into(),
            headers,
        }
    }

    /// Encode the status line, headers, `connection_header` and blank line.
    ///
    /// `connection_header` is the full header line without its terminator, for
    /// example `"Connection: close"` or `"Connection: keep-alive"`. Passing an
    /// empty string omits it.
    pub fn encode(&self, connection_header: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", self.status, self.reason).as_bytes());
        for (name, value) in &self.headers {
            out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        if !connection_header.is_empty() {
            out.extend_from_slice(connection_header.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out
    }
}

/// Scripted response written by the harness.
#[derive(Debug, Clone)]
pub struct Response {
    kind: ResponseKind,
}

#[derive(Debug, Clone)]
enum ResponseKind {
    Standard {
        status: u16,
        reason: String,
        body: Vec<u8>,
    },
    Raw(Vec<u8>),
    Chunked(Vec<Vec<u8>>),
    Staged {
        headers: ResponseHeaders,
        chunks: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>>>,
    },
}

impl Response {
    /// `200 OK` with the given body.
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::with_status(200, "OK", body)
    }

    /// A `Content-Length`-framed response with an explicit status line.
    pub fn with_status(status: u16, reason: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            kind: ResponseKind::Standard {
                status,
                reason: reason.to_string(),
                body: body.into(),
            },
        }
    }

    /// Verbatim bytes. The harness injects its connection header before the
    /// blank line when the raw head carries no `connection:` header of its own.
    ///
    /// A raw head's own `connection:` header wins over the harness header: in
    /// keep-alive mode `Connection: close` makes the harness close the socket
    /// after the response instead of serving another request. (The default
    /// one-shot mode always closes, whatever the raw head declares.)
    pub fn raw(bytes: Vec<u8>) -> Self {
        Self {
            kind: ResponseKind::Raw(bytes),
        }
    }

    /// `200 OK` framed with `Transfer-Encoding: chunked`.
    ///
    /// Each element becomes one chunk; a terminal `0\r\n\r\n` closes the body.
    /// No `Content-Length` is emitted.
    pub fn chunked(chunks: Vec<Vec<u8>>) -> Self {
        Self {
            kind: ResponseKind::Chunked(chunks),
        }
    }

    /// Header block written immediately, then each channel chunk as a chunked
    /// continuation. The terminal chunk is written when the sender drops.
    ///
    /// The receiver is shared across clones of the response and can be
    /// consumed once.
    pub fn staged(headers: ResponseHeaders, chunks: tokio::sync::mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            kind: ResponseKind::Staged {
                headers,
                chunks: Arc::new(tokio::sync::Mutex::new(Some(chunks))),
            },
        }
    }

    /// Encode a self-contained response, injecting `connection_header`. Staged
    /// responses are streamed by [`Response::write_to`] and return `None`.
    fn encode(&self, connection_header: &str) -> Option<Vec<u8>> {
        match &self.kind {
            ResponseKind::Standard {
                status,
                reason,
                body,
            } => {
                let headers = ResponseHeaders::new(
                    *status,
                    reason.as_str(),
                    vec![("Content-Length".to_string(), body.len().to_string())],
                );
                let mut out = headers.encode(connection_header);
                out.extend_from_slice(body);
                Some(out)
            }
            ResponseKind::Raw(bytes) => Some(if has_connection_header(bytes) {
                bytes.clone()
            } else {
                inject_connection_header(bytes, connection_header)
            }),
            ResponseKind::Chunked(chunks) => Some(encode_chunked(chunks, connection_header)),
            ResponseKind::Staged { .. } => None,
        }
    }

    /// True when a raw response's own `Connection: close` must end the
    /// connection even in keep-alive mode.
    fn closes_connection(&self) -> bool {
        match &self.kind {
            ResponseKind::Raw(bytes) => raw_connection_header(bytes)
                .is_some_and(|value| value.eq_ignore_ascii_case("close")),
            _ => false,
        }
    }

    /// Write the response to `stream`, honoring the connection mode.
    async fn write_to<W>(&self, stream: &mut W, connection_header: &str) -> std::io::Result<()>
    where
        W: AsyncWrite + Unpin + ?Sized,
    {
        if let ResponseKind::Staged { headers, chunks } = &self.kind {
            // Take the one-shot receiver before writing anything: a reused
            // staged response cannot produce headers plus a body, and the
            // caller turns this error into a complete 500 instead of leaving
            // a half-written head and a hung client.
            let mut receiver = {
                let mut guard = chunks.lock().await;
                guard.take().ok_or_else(staged_consumed)?
            };
            stream.write_all(&headers.encode(connection_header)).await?;
            stream.flush().await?;

            while let Some(chunk) = receiver.recv().await {
                if chunk.is_empty() {
                    continue;
                }
                write_chunk(stream, &chunk).await?;
                stream.flush().await?;
            }
            stream.write_all(b"0\r\n\r\n").await?;
            stream.flush().await?;
            return Ok(());
        }

        let bytes = self
            .encode(connection_header)
            .ok_or_else(|| invalid_data("response encoding failed"))?;
        stream.write_all(&bytes).await?;
        stream.flush().await?;
        Ok(())
    }
}

fn encode_chunked(chunks: &[Vec<u8>], connection_header: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n");
    out.extend_from_slice(connection_header.as_bytes());
    out.extend_from_slice(b"\r\n\r\n");
    for chunk in chunks {
        if !chunk.is_empty() {
            append_chunk(&mut out, chunk);
        }
    }
    out.extend_from_slice(b"0\r\n\r\n");
    out
}

fn append_chunk(out: &mut Vec<u8>, chunk: &[u8]) {
    out.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
    out.extend_from_slice(chunk);
    out.extend_from_slice(b"\r\n");
}

async fn write_chunk<W>(stream: &mut W, chunk: &[u8]) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut out = Vec::with_capacity(chunk.len() + 16);
    append_chunk(&mut out, chunk);
    stream.write_all(&out).await
}

/// Response selection for each accepted request.
///
/// exhaustive-by-contract: test-support-only two-variant selector consumed exhaustively by the harness.
#[derive(Clone)]
pub enum Responder {
    /// Same response for every request.
    Fixed(Response),
    /// Response chosen from the parsed request.
    Route(Arc<dyn Fn(&RecordedRequest) -> Response + Send + Sync>),
}

impl Responder {
    /// Path-routed responder (`Responder::Route`). No free `route` function.
    pub fn route(f: impl Fn(&RecordedRequest) -> Response + Send + Sync + 'static) -> Self {
        Self::Route(Arc::new(f))
    }
}

/// Connection behavior passed to [`serve_connection`].
///
/// The default configuration closes the socket after every response. Keep-alive
/// mode serves sequential requests (and pipelined bytes) on one connection.
/// [`HttpTestServer`] `builder` produces one; the same value is also accepted by
/// [`serve_connection`] for callers that own the transport (for example a TLS
/// test wrapping its own acceptor stream).
#[derive(Clone)]
pub struct ConnectionConfig {
    keep_alive: bool,
    reason: &'static str,
    recorded: Option<Arc<Mutex<Vec<RecordedRequest>>>>,
}

impl std::fmt::Debug for ConnectionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionConfig")
            .field("keep_alive", &self.keep_alive)
            .field("reason", &self.reason)
            .finish()
    }
}

impl ConnectionConfig {
    /// Serve sequential requests on one connection and advertise
    /// `Connection: keep-alive`. `reason` documents why a test needs reuse and
    /// is surfaced through [`Debug`].
    pub fn keep_alive(mut self, reason: &'static str) -> Self {
        self.keep_alive = true;
        self.reason = reason;
        self
    }

    /// Attach the recorder that backs [`HttpTestServer::recorded`].
    fn recording(mut self, recorded: Arc<Mutex<Vec<RecordedRequest>>>) -> Self {
        self.recorded = Some(recorded);
        self
    }

    /// Start the listener with this connection behavior.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn start(self, responder: Responder) -> std::io::Result<HttpTestServer> {
        HttpTestServer::start_with(responder, self).await
    }
}

struct ServerState {
    recorded: Arc<Mutex<Vec<RecordedRequest>>>,
    conns: tokio::sync::Mutex<JoinSet<()>>,
}

impl ServerState {
    fn new(recorded: Arc<Mutex<Vec<RecordedRequest>>>) -> Self {
        Self {
            recorded,
            conns: tokio::sync::Mutex::new(JoinSet::new()),
        }
    }
}

/// Handle to a started harness listener.
///
/// `Drop` cancels the listener and detaches its tasks without blocking.
/// [`HttpTestServer::shutdown`] provides a bounded, awaitable teardown that
/// also aborts straggling connection tasks.
pub struct HttpTestServer {
    addr: SocketAddr,
    cancel: CancellationToken,
    state: Arc<ServerState>,
    listener_task: Option<JoinHandle<()>>,
}

impl HttpTestServer {
    /// Bind `127.0.0.1:0` and start accepting requests in the background.
    // Repeated test-only gate: `lint-cancel-tokens` scans this file standalone
    // and cannot see the parent module's gate, so the fresh lifecycle root
    // below would otherwise count as production inventory.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn start(responder: Responder) -> std::io::Result<Self> {
        Self::start_with(responder, Self::builder()).await
    }

    /// Start with an explicit connection mode. Default responses close after
    /// one exchange; keep-alive mode loops over requests on the same socket.
    // Repeated test-only gate: `lint-cancel-tokens` scans this file standalone
    // and cannot see the parent module's gate, so this lifecycle root needs the
    // explicit gate too.
    #[cfg(any(test, feature = "test-support"))]
    async fn start_with(responder: Responder, cfg: ConnectionConfig) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let cancel = CancellationToken::new();
        let recorded = cfg
            .recorded
            .clone()
            .unwrap_or_else(|| Arc::new(Mutex::new(Vec::new())));
        let cfg = cfg.recording(Arc::clone(&recorded));
        let state = Arc::new(ServerState::new(recorded));
        let listener_task = tokio::spawn(accept_loop(
            listener,
            responder,
            Arc::clone(&state),
            cancel.clone(),
            cfg,
        ));
        Ok(Self {
            addr,
            cancel,
            state,
            listener_task: Some(listener_task),
        })
    }

    /// Start a builder for non-default connection behavior.
    pub fn builder() -> ConnectionConfig {
        ConnectionConfig {
            keep_alive: false,
            reason: "",
            recorded: None,
        }
    }

    /// Address the listener is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Bound TCP port.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Snapshot of every request recorded so far, in acceptance order.
    pub fn recorded(&self) -> Vec<RecordedRequest> {
        self.state
            .recorded
            .lock()
            .expect("http test server recording lock") // allow-unwrap
            .clone()
    }

    /// Bounded teardown: cancel the listener, await it, then join active
    /// connections for at most `SHUTDOWN_BUDGET`, aborting stragglers.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();

        if let Some(mut task) = self.listener_task.take()
            && tokio::time::timeout(SHUTDOWN_BUDGET, &mut task)
                .await
                .is_err()
        {
            task.abort();
        }

        let deadline = std::time::Instant::now() + SHUTDOWN_BUDGET;
        let mut conns = self.state.conns.lock().await;
        while !conns.is_empty() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, conns.join_next()).await {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => break,
            }
        }
        conns.abort_all();
        while conns.join_next().await.is_some() {}
    }
}

impl Drop for HttpTestServer {
    fn drop(&mut self) {
        // Non-blocking teardown: cancel and detach. The listener task exits,
        // dropping the `JoinSet` which aborts its remaining connections.
        self.cancel.cancel();
    }
}

async fn accept_loop(
    listener: TcpListener,
    responder: Responder,
    state: Arc<ServerState>,
    cancel: CancellationToken,
    cfg: ConnectionConfig,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _peer)) => {
                        let responder = responder.clone();
                        let token = cancel.clone();
                        let cfg = cfg.clone();
                        state.conns.lock().await.spawn(async move {
                            tokio::select! {
                                _ = token.cancelled() => {}
                                _ = serve_connection(stream, responder, cfg) => {}
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

/// Serve HTTP/1.1 requests on an already-connected transport.
///
/// This is the per-connection loop behind [`HttpTestServer`], exposed so tests
/// that own the transport can drive it directly. A TLS test accepts a
/// `TcpStream`, wraps it with its own `TlsAcceptor`, and passes the resulting
/// stream here; a unit test can hand it one half of a `tokio::io::duplex`
/// pair. The generic bounds are the only transport requirement.
///
/// `cfg` selects the connection mode ([`ConnectionConfig::keep_alive`] or the
/// default one-shot behavior) and, when it carries a recorder, receives every
/// parsed request. Returns after the peer closes, the configured mode ends, or
/// a transport/framing failure is answered.
pub async fn serve_connection<IO>(
    io: IO,
    responder: Responder,
    cfg: ConnectionConfig,
) -> std::io::Result<()>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let connection_header = if cfg.keep_alive {
        "Connection: keep-alive"
    } else {
        "Connection: close"
    };
    // Bytes that arrived past the parsed head (body + pipelined requests).
    let mut pending = Vec::new();
    let mut io = io;

    loop {
        let wire_head = match read_head(&mut io, &mut pending).await {
            Ok(Some(head)) => head,
            Ok(None) => return Ok(()),
            Err(_) => return bad_request(&mut io).await,
        };

        let mut request = match parse_request(&wire_head) {
            Ok(request) => request,
            Err(_) => return bad_request(&mut io).await,
        };

        // Reject unsupported framing before reading a body that we cannot
        // delimit.
        if has_unsupported_transfer_encoding(&request) {
            return bad_request(&mut io).await;
        }

        let content_length = match declared_content_length(&request) {
            Ok(length) => length,
            Err(_) => return bad_request(&mut io).await,
        };

        request.body = match read_body(&mut io, &mut pending, content_length).await {
            Ok(body) => body,
            Err(_) => return bad_request(&mut io).await,
        };

        if let Some(recorded) = &cfg.recorded {
            recorded
                .lock()
                .expect("http test server recording lock") // allow-unwrap
                .push(request.clone());
        }

        let response = match &responder {
            Responder::Fixed(response) => response.clone(),
            Responder::Route(route) => route(&request),
        };

        // A consumed staged channel is not a transport failure: answer 500 so
        // the client always gets a response. Any other write error means the
        // peer went away; both cases close the socket and stop the loop.
        let close = !cfg.keep_alive || response.closes_connection();
        if let Err(err) = response.write_to(&mut io, connection_header).await {
            if is_staged_consumed(&err) {
                let _ = internal_error(&mut io).await;
            }
            return Ok(());
        }

        if close {
            return Ok(());
        }
    }
}

fn declared_content_length(request: &RecordedRequest) -> std::io::Result<usize> {
    request
        .headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| {
            value
                .parse::<usize>()
                .map_err(|_| invalid_data("invalid Content-Length header"))
        })
        .transpose()
        .map(|length| length.unwrap_or(0))
}

fn has_unsupported_transfer_encoding(request: &RecordedRequest) -> bool {
    request
        .headers
        .iter()
        .any(|(name, value)| name == "transfer-encoding" && !value.eq_ignore_ascii_case("identity"))
}

async fn bad_request<W>(stream: &mut W) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
{
    const RESPONSE: &[u8] =
        b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    // Best effort: a peer that already disappeared must not hang teardown.
    let _ = stream.write_all(RESPONSE).await;
    let _ = stream.flush().await;
    Ok(())
}

async fn internal_error<W>(stream: &mut W) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
{
    const RESPONSE: &[u8] =
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let _ = stream.write_all(RESPONSE).await;
    let _ = stream.flush().await;
    Ok(())
}

/// Marker error: the staged response's one-shot channel was already consumed.
#[derive(Debug)]
struct StagedConsumed;

impl std::fmt::Display for StagedConsumed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("staged response channel was already consumed")
    }
}

impl std::error::Error for StagedConsumed {}

fn staged_consumed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, StagedConsumed)
}

fn is_staged_consumed(err: &std::io::Error) -> bool {
    err.get_ref()
        .is_some_and(|inner| inner.downcast_ref::<StagedConsumed>().is_some())
}

fn invalid_data(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

/// Read through the end of the next header block.
///
/// `pending` holds bytes already received but not yet consumed (body bytes
/// that arrived with the head, or pipelined requests). Returns the exact head
/// bytes including `\r\n\r\n`, leaving unconsumed bytes in `pending`.
///
/// `Ok(None)` means the peer closed cleanly on a request boundary.
async fn read_head<R>(stream: &mut R, pending: &mut Vec<u8>) -> std::io::Result<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin + ?Sized,
{
    let mut chunk = [0u8; 8192];
    loop {
        if let Some(pos) = find_subslice(pending, b"\r\n\r\n") {
            let end = pos + 4;
            return Ok(Some(pending.drain(..end).collect()));
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return if pending.is_empty() {
                Ok(None)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed before the request head completed",
                ))
            };
        }
        pending.extend_from_slice(&chunk[..read]);
    }
}

/// Consume `content_length` body bytes, taking buffered bytes first and
/// reading the remainder from the socket. Extra buffered bytes are left for
/// the next pipelined request.
async fn read_body<R>(
    stream: &mut R,
    pending: &mut Vec<u8>,
    content_length: usize,
) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin + ?Sized,
{
    if pending.len() >= content_length {
        return Ok(pending.drain(..content_length).collect());
    }

    let mut body = std::mem::take(pending);
    let mut chunk = [0u8; 8192];
    while body.len() < content_length {
        let want = (content_length - body.len()).min(chunk.len());
        let read = stream.read(&mut chunk[..want]).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed before the request body completed",
            ));
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Ok(body)
}

fn parse_request(wire_head: &[u8]) -> std::io::Result<RecordedRequest> {
    let text = std::str::from_utf8(wire_head)
        .map_err(|_| invalid_data("request head is not valid UTF-8"))?;
    let head = text
        .strip_suffix("\r\n\r\n")
        .ok_or_else(|| invalid_data("request head terminator missing"))?;

    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| invalid_data("request line missing"))?;
    let mut parts = request_line.split(' ');
    let method = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| invalid_data("request method missing"))?;
    let target = parts
        .next()
        .ok_or_else(|| invalid_data("request target missing"))?;
    let version = parts
        .next()
        .ok_or_else(|| invalid_data("HTTP version missing"))?;
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err(invalid_data("malformed request line"));
    }

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid_data("malformed header line"))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }

    Ok(RecordedRequest {
        method: method.to_string(),
        target: target.to_string(),
        headers,
        body: Vec::new(),
        wire_head: wire_head.to_vec(),
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Value of the raw response's header block `connection:` header, if any
/// (case-insensitive header name).
fn raw_connection_header(bytes: &[u8]) -> Option<String> {
    let head_end = find_subslice(bytes, b"\r\n\r\n").unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..head_end])
        .split("\r\n")
        .skip(1)
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("connection")
                .then(|| value.trim().to_string())
        })
}

/// True when the raw response's header block already declares a
/// `connection:` header (case-insensitive header name).
fn has_connection_header(bytes: &[u8]) -> bool {
    raw_connection_header(bytes).is_some()
}

/// Insert `connection_header` as the last header line before the blank line.
fn inject_connection_header(bytes: &[u8], connection_header: &str) -> Vec<u8> {
    match find_subslice(bytes, b"\r\n\r\n") {
        Some(pos) => {
            let mut out = Vec::with_capacity(bytes.len() + connection_header.len() + 4);
            out.extend_from_slice(&bytes[..pos]);
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(connection_header.as_bytes());
            out.extend_from_slice(b"\r\n\r\n");
            out.extend_from_slice(&bytes[pos + 4..]);
            out
        }
        None => bytes.to_vec(),
    }
}
