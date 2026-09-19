//! Stream (stdio) component for rust-camel — routes exchange bodies between
//! the process's standard streams and the integration runtime.
//!
//! Main types: `StreamComponent`, `StreamEndpoint`, `StreamConfig`,
//! `StreamTarget`, `StreamFrame`.
//! URI format: `stream:out|err|in?appendNewline=true&charset=utf-8&frame=line`.
//!
//! # Targets
//!
//! - **out** / **err**: producers that write exchange bodies to stdout/stderr.
//! - **in**: consumer that reads exchange bodies from stdin.
//!
//! UTF-8 is the only supported charset in v1.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use async_trait::async_trait;
use bytes::BytesMut;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tower::Service;
use tracing::debug;

use camel_component_api::parse_uri;
use camel_component_api::{BoxProcessor, CamelError, Exchange, Message};
use camel_component_api::{
    Component, ComponentMetadata, Consumer, ConsumerContext, Endpoint, ProducerContext,
    RuntimeObservability, UriConfig,
};

// ---------------------------------------------------------------------------
// StreamTarget
// ---------------------------------------------------------------------------

/// The standard stream an endpoint is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamTarget {
    /// Standard output (producer).
    Out,
    /// Standard error (producer).
    Err,
    /// Standard input (consumer).
    In,
}

impl fmt::Display for StreamTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamTarget::Out => write!(f, "out"),
            StreamTarget::Err => write!(f, "err"),
            StreamTarget::In => write!(f, "in"),
        }
    }
}

impl FromStr for StreamTarget {
    type Err = String;

    // Explicit `String`: `Self::Err` is ambiguous here because the enum has
    // an `Err` variant (ambiguous_associated_items).
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "out" => Ok(StreamTarget::Out),
            "err" => Ok(StreamTarget::Err),
            "in" => Ok(StreamTarget::In),
            _ => Err(format!(
                "unknown stream target: '{}'. Valid: out, err, in",
                s
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// StreamFrame
// ---------------------------------------------------------------------------

/// How the body bytes are framed on the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFrame {
    /// One exchange per line (newline-delimited). Default.
    Line,
    /// Bytes written/read verbatim, no framing.
    Raw,
    /// Fixed-size frames of `size` bytes.
    Fixed,
}

impl fmt::Display for StreamFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamFrame::Line => write!(f, "line"),
            StreamFrame::Raw => write!(f, "raw"),
            StreamFrame::Fixed => write!(f, "fixed"),
        }
    }
}

impl FromStr for StreamFrame {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "line" => Ok(StreamFrame::Line),
            "raw" => Ok(StreamFrame::Raw),
            "fixed" => Ok(StreamFrame::Fixed),
            _ => Err(format!(
                "unknown stream frame: '{}'. Valid: line, raw, fixed",
                s
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// StreamConfig
// ---------------------------------------------------------------------------

/// Configuration parsed from a stream URI.
///
/// Format: `stream:out|err|in?appendNewline=true&charset=utf-8&frame=line[&size=N]`
#[derive(Debug, Clone, UriConfig)]
#[uri_scheme = "stream"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "stream",
        description = "stdio data-plane adapter: out/err producers, in consumer",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub struct StreamConfig {
    /// Stream target (the path portion of the URI).
    pub target: StreamTarget,

    /// When true (default), append a trailing newline to each body written
    /// to `out`/`err`.
    #[uri_param(name = "appendNewline", default = "true")]
    pub append_newline: bool,

    /// Charset for the stream. Only `utf-8` is supported in v1.
    #[uri_param(name = "charset", default = "utf-8")]
    pub charset: String,

    /// Framing mode. Default: `line`.
    #[uri_param(name = "frame", kind = "enum:line,raw,fixed", default = "line")]
    pub frame: StreamFrame,

    /// Fixed frame size in bytes. Required (and must be greater than 0)
    /// when `frame = fixed`.
    #[uri_param(name = "size")]
    pub size: Option<u64>,
}

// Inherent validate — callable as StreamConfig::validate(&self)
impl StreamConfig {
    /// Validate the configuration without consuming self.
    ///
    /// The target needs no check here: `StreamTarget` is a closed enum whose
    /// `FromStr` rejects unknown paths, so any constructed config already
    /// holds a valid target by construction.
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.charset != "utf-8" {
            return Err(CamelError::Config(format!(
                "unsupported charset '{}': utf-8 is the only charset supported in v1",
                self.charset
            )));
        }
        if self.frame == StreamFrame::Fixed {
            match self.size {
                None => {
                    return Err(CamelError::InvalidUri(
                        "frame=fixed requires the size parameter".to_string(),
                    ));
                }
                Some(0) => {
                    return Err(CamelError::InvalidUri(
                        "frame=fixed requires a size greater than 0".to_string(),
                    ));
                }
                // The chunk buffer is allocated per frame, so `size` doubles
                // as a per-frame allocation from a URI parameter. Cap it at
                // the crate's max-materialization limit (100 MiB).
                Some(n) if n > MAX_MATERIALIZE_BYTES as u64 => {
                    return Err(CamelError::InvalidUri(format!(
                        "frame=fixed size {n} exceeds the maximum frame size of {MAX_MATERIALIZE_BYTES} bytes (100 MiB)"
                    )));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

impl UriConfig for StreamConfig {
    fn scheme() -> &'static str {
        "stream"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        Self::from_components(parts)
    }

    fn from_components(parts: camel_component_api::UriComponents) -> Result<Self, CamelError> {
        let config = Self::parse_uri_components(parts)?;
        StreamConfig::validate(&config)?;
        Ok(config)
    }

    fn validate(self) -> Result<Self, CamelError> {
        // Delegate to the inherent validate(&self)
        StreamConfig::validate(&self)?;
        Ok(self)
    }
}

// ---------------------------------------------------------------------------
// StreamComponent
// ---------------------------------------------------------------------------

/// The Stream component routes exchange bodies to/from the process's
/// standard streams.
pub struct StreamComponent;

impl StreamComponent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StreamComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for StreamComponent {
    fn scheme(&self) -> &str {
        "stream"
    }

    fn metadata(&self) -> ComponentMetadata {
        StreamConfig::metadata()
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = StreamConfig::from_uri(uri)?;
        Ok(Box::new(StreamEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

// ---------------------------------------------------------------------------
// StreamEndpoint
// ---------------------------------------------------------------------------

struct StreamEndpoint {
    uri: String,
    config: StreamConfig,
}

impl Endpoint for StreamEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        match self.config.target {
            StreamTarget::In => Ok(Box::new(StreamConsumer::new(
                self.config.clone(),
                Arc::clone(&rt),
            ))),
            // `stream:out`/`stream:err` are producer targets; consuming from
            // them has no meaning on the data plane.
            StreamTarget::Out | StreamTarget::Err => Err(CamelError::EndpointCreationFailed(
                "stream:out and stream:err do not support consumers; use stream:in".into(),
            )),
        }
    }

    fn create_producer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        match self.config.target {
            StreamTarget::Out | StreamTarget::Err => {
                Ok(BoxProcessor::new(StreamProducer::new(self.config.clone())))
            }
            // `stream:in` is a consumer target (Phase 2); producing to it
            // has no meaning on the data plane.
            StreamTarget::In => Err(CamelError::EndpointCreationFailed(
                "stream:in is a consumer target; producers require stream:out or stream:err".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// StreamProducer
// ---------------------------------------------------------------------------

/// Write end of a stdio stream, so tests can capture bytes instead of
/// touching the real process file descriptors.
#[async_trait]
trait Sink: Send + Sync {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
    async fn flush(&mut self) -> std::io::Result<()>;
}

struct StdoutSink(tokio::io::Stdout);

#[async_trait]
impl Sink for StdoutSink {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.write_all(buf).await
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush().await
    }
}

struct StderrSink(tokio::io::Stderr);

#[async_trait]
impl Sink for StderrSink {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.write_all(buf).await
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush().await
    }
}

/// One lock per file descriptor. This is THE serialization mechanism: the
/// whole write+flush of a single concatenated buffer (body + optional
/// newline) happens under the guard, so two `stream:out` endpoints can never
/// interleave a body and its trailing newline on fd 1 (nor on fd 2).
///
/// The `Arc<tokio::sync::Mutex<dyn Sink>>` on the producer is NOT a
/// serialization mechanism — it only gives `Clone` producers `&mut` access
/// to the shared sink.
static FD1_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static FD2_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Per-exchange materialization limit (100 MiB), the same data-plane limit
/// the file component enforces.
const MAX_MATERIALIZE_BYTES: usize = 100 * 1024 * 1024;

/// Producer that writes exchange bodies to stdout/stderr as raw data.
///
/// Semantics (binding): NO log levels, NO formatting, NO redaction — the
/// body is DATA. Nothing partial is written on error. Flush after every
/// exchange.
#[derive(Clone)]
struct StreamProducer {
    config: StreamConfig,
    sink: Arc<tokio::sync::Mutex<dyn Sink>>,
}

impl StreamProducer {
    /// Build a producer with the real process stream for `config.target`.
    pub(crate) fn new(config: StreamConfig) -> Self {
        let sink: Arc<tokio::sync::Mutex<dyn Sink>> = match config.target {
            StreamTarget::Out => Arc::new(tokio::sync::Mutex::new(StdoutSink(tokio::io::stdout()))),
            StreamTarget::Err => Arc::new(tokio::sync::Mutex::new(StderrSink(tokio::io::stderr()))),
            // Unreachable through `create_producer` (which rejects `In`);
            // fd 0 is stdin, so `StdoutSink` is never a silent fallback for
            // real output — it can only surface through direct construction.
            StreamTarget::In => Arc::new(tokio::sync::Mutex::new(StdoutSink(tokio::io::stdout()))),
        };
        Self { config, sink }
    }

    /// Build a producer around an injected sink (test seam).
    #[cfg(test)]
    pub(crate) fn with_sink(config: StreamConfig, sink: Arc<tokio::sync::Mutex<dyn Sink>>) -> Self {
        Self { config, sink }
    }

    /// File descriptor this producer's target maps to (1 = stdout,
    /// 2 = stderr, 0 = stdin for the consumer-only `In` target).
    #[cfg(test)]
    pub(crate) fn target_fd(&self) -> i32 {
        match self.config.target {
            StreamTarget::Out => 1,
            StreamTarget::Err => 2,
            StreamTarget::In => 0,
        }
    }
}

impl Service<Exchange> for StreamProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let sink = self.sink.clone();
        Box::pin(async move {
            // The body is DATA: materialize verbatim — no formatting, no
            // redaction. `into_bytes` serializes structured bodies to their
            // canonical byte form; the exchange itself is left untouched.
            let bytes = exchange
                .input
                .body
                .clone()
                .into_bytes(MAX_MATERIALIZE_BYTES)
                .await?;

            // One logical write: newline is appended to the SAME buffer
            // before the single `write_all` — never two separate writes.
            let mut buf = BytesMut::from(bytes);
            if config.append_newline {
                buf.extend_from_slice(b"\n");
            }

            let fd_guard = if config.target == StreamTarget::Err {
                FD2_LOCK.lock().await
            } else {
                FD1_LOCK.lock().await
            };

            let mut sink = sink.lock().await;
            sink.write_all(&buf).await?;
            sink.flush().await?;
            drop(sink);
            drop(fd_guard);

            Ok(exchange)
        })
    }
}

// ---------------------------------------------------------------------------
// StreamConsumer
// ---------------------------------------------------------------------------

/// Loud bound check for consumer-side frame accumulation: `len` is the
/// frame length an accumulation step would reach. Past the crate's
/// `MAX_MATERIALIZE_BYTES` limit the frame fails with the same
/// `CamelError::StreamLimitExceeded` variant the producer's
/// `Body::into_bytes` bound uses — a loud route failure, never a silent
/// skip and never an unbounded buffer (`cat /dev/zero | camel run`
/// must not OOM the process).
fn ensure_frame_bound(len: usize) -> Result<(), CamelError> {
    if len > MAX_MATERIALIZE_BYTES {
        return Err(CamelError::StreamLimitExceeded(MAX_MATERIALIZE_BYTES));
    }
    Ok(())
}

/// Async byte-source seam behind the consumer so tests can inject buffered
/// readers instead of the real stdin. The three methods mirror the three
/// framing modes.
///
/// All accumulation is bounded: a frame that would grow past the crate's
/// per-frame materialization limit fails with
/// `CamelError::StreamLimitExceeded` before the buffer grows past the
/// bound. EOF is `Ok(0)`, never an error (ruling R2).
#[async_trait]
pub(crate) trait StreamReader: Send + Sync {
    /// Line framing: read bytes up to and including the next `\n`, appending
    /// them to `buf`. Returns the byte count (`0` = EOF); an unterminated
    /// final line is still returned when EOF follows it. Byte-oriented on
    /// purpose (never `read_line`): input need not be valid UTF-8.
    async fn read_until_newline(&mut self, buf: &mut Vec<u8>) -> Result<usize, CamelError>;

    /// Fixed framing: read up to `n` bytes, appending them to `buf`.
    /// Returns the byte count (`0` = EOF); a short frame is a smaller count.
    async fn read_exact_chunk(&mut self, n: usize, buf: &mut Vec<u8>) -> Result<usize, CamelError>;

    /// Raw framing: read until EOF, appending everything to `buf`. Returns
    /// the byte count.
    async fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize, CamelError>;
}

/// Production reader over the process stdin. Line and raw framing
/// accumulate through bounded `fill_buf` chunks — never unbounded
/// `read_until`/`read_to_end` growth. Each chunk is bound-checked
/// BEFORE it extends the frame, so an endless source (`cat /dev/zero`)
/// fails the route instead of OOMing. The internal buffer is bounded
/// (8 KiB) and owned by this reader, so framing state is consistent
/// across all three modes.
struct StdinReader {
    stdin: BufReader<tokio::io::Stdin>,
}

#[async_trait]
impl StreamReader for StdinReader {
    async fn read_until_newline(&mut self, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        let mut appended = 0;
        loop {
            let chunk = self
                .stdin
                .fill_buf()
                .await
                .map_err(|e| CamelError::Io(e.to_string()))?;
            if chunk.is_empty() {
                return Ok(appended);
            }
            // One logical `read_until` step: up to and including the first
            // `\n` in this chunk, or the whole chunk when the line spans
            // chunks. Bound-checked before the frame grows.
            let upto = chunk
                .iter()
                .position(|&b| b == b'\n')
                .map_or(chunk.len(), |i| i + 1);
            ensure_frame_bound(buf.len() + upto)?;
            let hit_newline = chunk[upto - 1] == b'\n';
            buf.extend_from_slice(&chunk[..upto]);
            self.stdin.consume(upto);
            appended += upto;
            if hit_newline {
                return Ok(appended);
            }
        }
    }

    async fn read_exact_chunk(&mut self, n: usize, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        // One read, capped at `n`: stdin is not seekable, so capping the
        // window at the remaining frame bytes is what keeps BufReader from
        // stealing a byte past the frame boundary (over-read bytes stay in
        // its internal buffer for the next call). Accumulation across short
        // reads is `read_frame`'s job — it holds for every StreamReader
        // implementation, not just this one. `n` is config-validated to be
        // within `MAX_MATERIALIZE_BYTES`, so this window is bounded.
        let mut chunk = vec![0u8; n];
        let read = self
            .stdin
            .read(&mut chunk)
            .await
            .map_err(|e| CamelError::Io(e.to_string()))?;
        buf.extend_from_slice(&chunk[..read]);
        Ok(read)
    }

    async fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        let mut appended = 0;
        loop {
            let chunk = self
                .stdin
                .fill_buf()
                .await
                .map_err(|e| CamelError::Io(e.to_string()))?;
            if chunk.is_empty() {
                return Ok(appended);
            }
            // Bound-checked before the frame grows: an endless raw source
            // fails loudly instead of OOMing.
            ensure_frame_bound(buf.len() + chunk.len())?;
            let len = chunk.len();
            buf.extend_from_slice(chunk);
            self.stdin.consume(len);
            appended += len;
        }
    }
}

/// One decoded frame handed from the read loop to the send path.
enum Frame {
    /// Line mode: decoded UTF-8 text, terminators stripped.
    Line(String),
    /// Raw/fixed mode: verbatim bytes.
    Bytes(Vec<u8>),
    /// Line mode: frame bytes are not valid UTF-8 — skip without sending.
    InvalidUtf8,
    /// End of stream: no further frames will arrive.
    Eof,
}

/// Read the next frame from `reader` according to the configured framing.
///
/// EOF is a [`Frame::Eof`] outcome, never an error (ruling R2). IO
/// failures and bound violations (`CamelError::StreamLimitExceeded`)
/// propagate as loud route failures.
async fn read_frame(
    reader: &mut dyn StreamReader,
    mode: StreamFrame,
    size: Option<u64>,
) -> Result<Frame, CamelError> {
    let mut buf = Vec::new();
    match mode {
        StreamFrame::Line => {
            if reader.read_until_newline(&mut buf).await? == 0 {
                return Ok(Frame::Eof);
            }
            // Strip the trailing `\n`, then one preceding `\r` — but only
            // strip the `\r` when a `\n` was actually stripped: a lone
            // trailing `\r` is data, not a terminator. Line mode terminates
            // on `\n` only (Apache Camel's stream: also splits on `\r`;
            // documented divergence on the component page).
            if buf.last() == Some(&b'\n') {
                buf.pop();
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
            }
            // Validates and constructs in one step; an invalid frame takes
            // the decode-skip path.
            match String::from_utf8(buf) {
                Ok(text) => Ok(Frame::Line(text)),
                Err(_) => Ok(Frame::InvalidUtf8),
            }
        }
        StreamFrame::Raw => {
            // `read_to_end` is bound-checked inside the reader: it fails
            // with `StreamLimitExceeded` instead of growing past the cap.
            reader.read_to_end(&mut buf).await?;
            if buf.is_empty() {
                Ok(Frame::Eof)
            } else {
                Ok(Frame::Bytes(buf))
            }
        }
        StreamFrame::Fixed => {
            // `size` is validated present, non-zero, and within
            // `MAX_MATERIALIZE_BYTES` by `StreamConfig::validate`, so the
            // frame buffer is bounded by construction and this conversion
            // cannot fail on any supported platform.
            let n = usize::try_from(size.unwrap_or(0)).unwrap_or(0);
            // Accumulate across short reads until exactly `n` bytes are in
            // the frame: a trickling source may deliver fewer bytes per
            // read call. Only EOF (a read returning 0) may yield a partial
            // frame — a mid-stream short read never does.
            let start = buf.len();
            while buf.len() - start < n {
                let read = reader
                    .read_exact_chunk(n - (buf.len() - start), &mut buf)
                    .await?;
                if read == 0 {
                    break;
                }
            }
            if buf.len() == start {
                Ok(Frame::Eof)
            } else {
                Ok(Frame::Bytes(buf))
            }
        }
    }
}

/// Consumer reading exchange bodies from stdin (`stream:in`).
///
/// Binding semantics:
/// - EOF is graceful: `start` returns `Ok(())`, never an error (ruling R2).
/// - Sequential backpressure: the next frame is read only after the previous
///   send completes — never read ahead, no buffering queue.
/// - Frames are bounded at the crate's materialization limit
///   (`MAX_MATERIALIZE_BYTES`): a frame past the bound fails `start`
///   loudly with `CamelError::StreamLimitExceeded` — same posture as an
///   IO error, never a silent skip.
/// - Terminal send failures (`b-prime:stream:fire-send`) and invalid UTF-8
///   lines (`b-prime:stream:decode`) are counted per ADR-0012.
pub(crate) struct StreamConsumer {
    config: StreamConfig,
    /// Guard against double-start (TimerConsumer pattern).
    started: AtomicBool,
    /// `rt.metrics().increment_errors(...)` sink per ADR-0012.
    runtime: Arc<dyn RuntimeObservability>,
    reader: Box<dyn StreamReader>,
}

impl StreamConsumer {
    /// Build a consumer reading the process stdin (production constructor).
    pub(crate) fn new(config: StreamConfig, runtime: Arc<dyn RuntimeObservability>) -> Self {
        Self::with_reader(
            config,
            runtime,
            Box::new(StdinReader {
                stdin: BufReader::new(tokio::io::stdin()),
            }),
        )
    }

    /// Build a consumer around an injected reader (test seam).
    pub(crate) fn with_reader(
        config: StreamConfig,
        runtime: Arc<dyn RuntimeObservability>,
        reader: Box<dyn StreamReader>,
    ) -> Self {
        Self {
            config,
            started: AtomicBool::new(false),
            runtime,
            reader,
        }
    }

    /// Test helper: pre-set the started flag to simulate an already-running
    /// consumer (TimerConsumer pattern).
    #[cfg(test)]
    pub(crate) fn mark_started_for_test(&self) {
        self.started.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl Consumer for StreamConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        self.started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| {
                CamelError::EndpointCreationFailed("stream consumer already started".to_string())
            })?;

        let cancel_token = context.cancel_token();
        let frame_mode = self.config.frame;
        let frame_size = self.config.size;
        // Field-level borrow: the loop reads through `reader` while the
        // per-frame body touches `runtime` — disjoint fields.
        let reader = &mut *self.reader;

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    debug!(stream_target = %self.config.target, "stream consumer cancelled, stopping");
                    break;
                }
                frame = read_frame(reader, frame_mode, frame_size) => {
                    let message = match frame {
                        Ok(Frame::Line(text)) => Message::new(text),
                        Ok(Frame::Bytes(bytes)) => Message::new(bytes),
                        Ok(Frame::InvalidUtf8) => {
                            self.runtime
                                .metrics()
                                .increment_errors(context.route_id(), "b-prime:stream:decode");
                            continue;
                        }
                        Ok(Frame::Eof) => break,
                        Err(error) => {
                            // An IO error is NOT EOF (only EOF is
                            // graceful), and neither is a frame past the
                            // materialization bound
                            // (`StreamLimitExceeded`). Fail loudly like a
                            // genuine source failure: the runtime treats a
                            // `start` Err as an error event.
                            self.started.store(false, Ordering::SeqCst);
                            return Err(error);
                        }
                    };

                    if context.send(Exchange::new(message)).await.is_err() {
                        // b-prime: locally terminal fire-send — the route
                        // channel is closed and the loop exits, so this
                        // metric is the only signal (ADR-0012).
                        self.runtime
                            .metrics()
                            .increment_errors(context.route_id(), "b-prime:stream:fire-send");
                        break;
                    }
                    // Sequential backpressure: the next read starts only
                    // after this send completed — never read ahead.
                }
            }
        }

        // Reset so the consumer can be restarted after stop.
        self.started.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        self.started.store(false, Ordering::SeqCst);
        debug!(stream_target = %self.config.target, "stream consumer stopped");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod stream_producer_tests {
    use super::*;
    use camel_component_api::{Body, Message, StreamBody};
    use serde_json::json;

    /// Captures every logical write so tests can assert on exact bytes
    /// without touching the real process streams.
    #[derive(Default, Clone)]
    struct VecSink(Arc<std::sync::Mutex<Vec<u8>>>);

    impl VecSink {
        fn bytes(&self) -> Vec<u8> {
            self.0.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Sink for VecSink {
        async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(())
        }

        async fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Test sink that splits every logical write into two chunks with a
    /// `yield_now` between them. A per-chunk atomic append (like `VecSink`)
    /// would let two producers interleave at chunk granularity; only a guard
    /// held across the WHOLE logical write (the fd statics) prevents that.
    /// Two `ChunkSink`s sharing one inner buffer model two endpoints writing
    /// to the same fd through separate sink mutexes.
    struct ChunkSink(Arc<std::sync::Mutex<Vec<u8>>>);

    #[async_trait::async_trait]
    impl Sink for ChunkSink {
        async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
            let mid = buf.len() / 2;
            self.0.lock().unwrap().extend_from_slice(&buf[..mid]);
            tokio::task::yield_now().await;
            self.0.lock().unwrap().extend_from_slice(&buf[mid..]);
            Ok(())
        }

        async fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Build the erased test sink plus a reader handle over the same
    /// internal buffer (clones of `VecSink` share one `Vec<u8>`).
    fn test_sink() -> (Arc<tokio::sync::Mutex<dyn Sink>>, VecSink) {
        let vec_sink = VecSink::default();
        (
            Arc::new(tokio::sync::Mutex::new(vec_sink.clone())),
            vec_sink,
        )
    }

    #[tokio::test]
    async fn line_mode_appends_single_newline() {
        let (sink, reader) = test_sink();
        let config = StreamConfig::from_uri("stream:out").unwrap();
        let mut producer = StreamProducer::with_sink(config, sink);

        let exchange = Exchange::new(Message::new("hello"));
        let result = producer.call(exchange).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("hello"));
        assert_eq!(reader.bytes(), b"hello\n");
    }

    #[tokio::test]
    async fn raw_mode_writes_verbatim() {
        let (sink, reader) = test_sink();
        let config = StreamConfig::from_uri("stream:out?appendNewline=false").unwrap();
        let mut producer = StreamProducer::with_sink(config, sink);

        let exchange = Exchange::new(Message::new(b"a,b,c".to_vec()));
        producer.call(exchange).await.unwrap();

        assert_eq!(reader.bytes(), b"a,b,c");
    }

    #[tokio::test]
    async fn err_target_dispatches_stderr() {
        let config = StreamConfig::from_uri("stream:err").unwrap();
        assert_eq!(StreamProducer::new(config).target_fd(), 2);

        let config = StreamConfig::from_uri("stream:out").unwrap();
        assert_eq!(StreamProducer::new(config).target_fd(), 1);
    }

    #[tokio::test]
    async fn structured_body_materializes_to_bytes() {
        let (sink, reader) = test_sink();
        let config = StreamConfig::from_uri("stream:out?appendNewline=false").unwrap();
        let mut producer = StreamProducer::with_sink(config, sink);

        let value = json!({"name": "café", "count": 3, "nested": {"ok": true}});
        let mut exchange = Exchange::new(Message::new(""));
        exchange.input.body = Body::Json(value.clone());
        producer.call(exchange).await.unwrap();

        let expected = Body::Json(value)
            .into_bytes(100 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(reader.bytes(), expected.as_ref());
    }

    #[tokio::test]
    async fn credential_body_passes_verbatim() {
        let (sink, reader) = test_sink();
        let config = StreamConfig::from_uri("stream:out?appendNewline=false").unwrap();
        let mut producer = StreamProducer::with_sink(config, sink);

        let exchange = Exchange::new(Message::new("password=hunter2"));
        producer.call(exchange).await.unwrap();

        // Body is DATA: no redaction, no formatting — byte-exact passthrough.
        assert_eq!(reader.bytes(), b"password=hunter2");
    }

    #[tokio::test]
    async fn empty_body_line_mode_writes_just_newline() {
        let (sink, reader) = test_sink();
        let config = StreamConfig::from_uri("stream:out").unwrap();
        let mut producer = StreamProducer::with_sink(config, sink);

        let exchange = Exchange::new(Message::default());
        producer.call(exchange).await.unwrap();

        assert_eq!(reader.bytes(), b"\n");
    }

    #[tokio::test]
    async fn concurrent_producers_serialize_on_fd() {
        // Two producers with SEPARATE sink mutexes over ChunkSinks that
        // share one buffer: the only cross-producer serializer left is the
        // fd static guard. ChunkSink splits each logical write and yields
        // mid-write, so without the guard the two writes interleave at
        // chunk granularity and the assertion below fails.
        let shared = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink1: Arc<tokio::sync::Mutex<dyn Sink>> =
            Arc::new(tokio::sync::Mutex::new(ChunkSink(shared.clone())));
        let sink2: Arc<tokio::sync::Mutex<dyn Sink>> =
            Arc::new(tokio::sync::Mutex::new(ChunkSink(shared.clone())));

        let config = StreamConfig::from_uri("stream:out").unwrap();
        let mut p1 = StreamProducer::with_sink(config.clone(), sink1);
        let mut p2 = StreamProducer::with_sink(config, sink2);

        let t1 = tokio::spawn(async move { p1.call(Exchange::new(Message::new("aa\naa"))).await });
        let t2 = tokio::spawn(async move { p2.call(Exchange::new(Message::new("bb\nbb"))).await });
        t1.await.unwrap().unwrap();
        t2.await.unwrap().unwrap();

        // Each multi-line body must land as ONE intact write; the two
        // logical writes may appear in either order but never interleave.
        let out = shared.lock().unwrap().clone();
        let ok = out.as_slice() == b"aa\naa\nbb\nbb\n" || out.as_slice() == b"bb\nbb\naa\naa\n";
        assert!(ok, "interleaved or corrupted writes: {out:?}");
    }

    #[tokio::test]
    async fn materialization_error_writes_nothing() {
        let (sink, reader) = test_sink();
        let config = StreamConfig::from_uri("stream:out").unwrap();
        let mut producer = StreamProducer::with_sink(config, sink);

        // A consumed stream body fails materialization cheaply (no 100 MiB
        // allocation needed): `into_bytes` returns AlreadyConsumed at once.
        let body = Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(None)),
            metadata: Default::default(),
        });
        let exchange = Exchange::new(Message::new(body));

        let result = producer.call(exchange).await;
        assert!(
            matches!(result, Err(CamelError::AlreadyConsumed)),
            "expected AlreadyConsumed, got: {result:?}"
        );
        assert!(
            reader.bytes().is_empty(),
            "nothing may be written when materialization fails"
        );
    }
}

// ---------------------------------------------------------------------------
// StreamConsumer tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "consumer_tests.rs"]
mod stream_consumer_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parses_out_default() {
        let config = StreamConfig::from_uri("stream:out").unwrap();
        assert_eq!(config.target, StreamTarget::Out);
        assert!(config.append_newline);
        assert_eq!(config.charset, "utf-8");
        assert_eq!(config.frame, StreamFrame::Line);
    }

    #[test]
    fn config_parses_in_raw() {
        let config = StreamConfig::from_uri("stream:in?frame=raw").unwrap();
        assert_eq!(config.target, StreamTarget::In);
        assert_eq!(config.frame, StreamFrame::Raw);
    }

    #[test]
    fn config_rejects_unknown_path() {
        let result = StreamConfig::from_uri("stream:logfile");
        assert!(
            matches!(result, Err(CamelError::InvalidUri(_))),
            "unknown path must be rejected with InvalidUri, got: {result:?}"
        );
    }

    #[test]
    fn config_rejects_non_utf8_charset() {
        let result = StreamConfig::from_uri("stream:out?charset=latin-1");
        match result {
            Err(CamelError::Config(msg)) => {
                assert!(msg.contains("utf-8"), "error must name utf-8, got: {msg}")
            }
            other => panic!("expected CamelError::Config, got: {other:?}"),
        }
    }

    #[test]
    fn config_rejects_fixed_without_size() {
        let result = StreamConfig::from_uri("stream:in?frame=fixed");
        assert!(
            matches!(result, Err(CamelError::InvalidUri(_))),
            "frame=fixed without size must be rejected with InvalidUri, got: {result:?}"
        );
        let result = StreamConfig::from_uri("stream:in?frame=fixed&size=0");
        assert!(
            matches!(result, Err(CamelError::InvalidUri(_))),
            "frame=fixed with size=0 must be rejected with InvalidUri, got: {result:?}"
        );
    }

    /// `size` drives a per-frame allocation from a URI parameter, so an
    /// absurd value must be rejected up front instead of OOMing the process.
    #[test]
    fn config_rejects_huge_fixed_size() {
        let result = StreamConfig::from_uri("stream:in?frame=fixed&size=99999999999");
        match result {
            Err(CamelError::InvalidUri(msg)) => {
                assert!(
                    msg.contains("104857600"),
                    "error must name the size bound, got: {msg}"
                );
            }
            other => panic!("expected CamelError::InvalidUri, got: {other:?}"),
        }
    }

    #[test]
    fn component_scheme_is_stream() {
        assert_eq!(StreamComponent::new().scheme(), "stream");
    }

    /// At-cap frames are allowed: `Body::into_bytes` rejects strictly
    /// past the bound, and the consumer must match that posture.
    #[test]
    fn ensure_frame_bound_allows_up_to_cap() {
        assert!(ensure_frame_bound(MAX_MATERIALIZE_BYTES - 1).is_ok());
        assert!(ensure_frame_bound(MAX_MATERIALIZE_BYTES).is_ok());
    }

    #[test]
    fn ensure_frame_bound_rejects_past_cap() {
        match ensure_frame_bound(MAX_MATERIALIZE_BYTES + 1) {
            Err(CamelError::StreamLimitExceeded(max)) => {
                assert_eq!(max, MAX_MATERIALIZE_BYTES);
            }
            other => panic!("expected StreamLimitExceeded, got: {other:?}"),
        }
    }
}
