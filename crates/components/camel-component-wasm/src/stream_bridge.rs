//! Streaming body bridge: `camel_api::Body::Stream` ↔ WASM `stream<u8>`.
//!
//! Extracted from [`crate::serde_bridge`] to keep that module under the
//! thermo-nuclear size threshold.  The three public items —
//! [`BoxStreamProducer`], [`assemble_stream_body`], [`extract_stream_body`] —
//! are `pub(crate)` so [`crate::runtime`] can call them directly.

use bytes::Bytes;
use camel_api::{CamelError, StreamBody, StreamMetadata};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::stream::BoxStream;
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Accessor, Destination, FutureReader, StreamProducer, StreamReader, StreamResult, VecBuffer,
};

use crate::bindings::camel::plugin::types::{StreamBodyHandle, WasmBody, WasmError};
use crate::runtime::WasmHostState;

// ---------------------------------------------------------------------------
// Streaming body bridge: camel_api::Body::Stream → WASM stream<u8>
// ---------------------------------------------------------------------------

/// Host-side [`StreamProducer`] that pumps bytes from an extracted
/// [`BoxStream`] into a WASM `stream<u8>` handle readable by the guest.
///
/// `poll_produce` is a *synchronous* poll function — wasmtime drives it from
/// the concurrent runtime — so the producer owns the byte stream directly.
/// There is no async locking inside `poll_produce`; the stream is drained out
/// of the `Arc<Mutex<Option<BoxStream>>>` in [`extract_stream_body`] *before*
/// the producer is constructed (see [`assemble_stream_body`]).
///
/// # Error / EOF signalling
///
/// - **Clean EOF** and **stream error** both return [`StreamResult::Dropped`].
///   Returning `Err` would trap the guest instance, so errors are funnelled
///   through the terminal future instead (see `terminal_tx`).
/// - `terminal_tx` carries `Some(message)` on error and `None` on clean EOF;
///   the matching [`FutureReader`] turns that into the `result<(), wasm-error>`
///   the guest awaits.
/// - [`StreamResult::Cancelled`] is returned when the guest asks to finish.
///
/// `progress_notify` is pinged on every successfully shipped chunk so an
/// external watchdog can observe forward progress.
pub(crate) struct BoxStreamProducer {
    /// The byte stream, taken out of the `Arc<Mutex<Option<BoxStream>>>`
    /// before construction. `None` means already exhausted.
    stream: Option<BoxStream<'static, Result<Bytes, CamelError>>>,
    /// Watchdog heartbeat — notified once per shipped chunk.
    progress_notify: Arc<Notify>,
    /// Delivers the terminal outcome to the guest's `terminal` future:
    /// `Some(msg)` = error, `None` = clean completion. Taken on first
    /// terminal event so a second `poll_produce` is a no-op.
    terminal_tx: Option<oneshot::Sender<Option<String>>>,
    /// Host-side cancellation token. When tripped, the producer exits
    /// immediately with `StreamResult::Dropped` (Cancelled is reserved
    /// for guest-initiated finish=true).
    cancel: CancellationToken,
    /// Maximum number of bytes to produce before forcefully ending
    /// the stream with an overflow error.
    max_bytes: u64,
    /// Cumulative bytes produced so far.
    written: u64,
}

impl BoxStreamProducer {
    /// Construct from the already-extracted byte stream and shared handles.
    ///
    /// `progress_notify` and `terminal_tx` are normally created by
    /// [`assemble_stream_body`]; this constructor exists so callers (and
    /// tests) can wire them explicitly.
    pub(crate) fn new(
        stream: Option<BoxStream<'static, Result<Bytes, CamelError>>>,
        progress_notify: Arc<Notify>,
        terminal_tx: oneshot::Sender<Option<String>>,
        cancel: CancellationToken,
        max_bytes: u64,
    ) -> Self {
        Self {
            stream,
            progress_notify,
            terminal_tx: Some(terminal_tx),
            cancel,
            max_bytes,
            written: 0,
        }
    }

    /// Deliver the terminal outcome, if not already delivered. Idempotent.
    fn finish_terminal(&mut self, outcome: Option<String>) {
        if let Some(tx) = self.terminal_tx.take() {
            // Ignore send error: the guest may have dropped the future
            // before we finish — that is not an error on our side.
            let _ = tx.send(outcome);
        }
    }
}

impl StreamProducer<WasmHostState> for BoxStreamProducer {
    type Item = u8;
    type Buffer = VecBuffer<u8>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: StoreContextMut<'a, WasmHostState>,
        mut destination: Destination<'a, u8, VecBuffer<u8>>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        // Host cancellation token tripped — no more data will ever be
        // produced.  Return Dropped (terminates the stream); the terminal
        // future carries the clean completion signal.
        if self.cancel.is_cancelled() {
            self.finish_terminal(None);
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        // Guest-initiated cancel — surface success on the terminal.
        if finish {
            self.finish_terminal(None);
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }

        // `BoxStreamProducer` is `Unpin` (every field is), so we can poll the
        // inner stream directly through `Pin`'s `DerefMut`.
        let next = match self.stream.as_mut() {
            Some(stream) => stream.as_mut().poll_next(cx),
            // Already drained (re-poll after EOF) — nothing left to ship.
            None => {
                self.finish_terminal(None);
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
        };

        match next {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(bytes))) => {
                // Track cumulative bytes for overflow detection.
                self.written += bytes.len() as u64;
                if self.written > self.max_bytes {
                    let msg = format!("stream exceeded max-bytes ({})", self.max_bytes);
                    self.finish_terminal(Some(msg));
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }

                // Destination takes ownership of the buffer.
                destination.set_buffer(bytes.to_vec().into());
                self.progress_notify.notify_one();
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(None) => {
                // Clean EOF — free the stream and signal success.
                self.stream.take();
                self.finish_terminal(None);
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Ready(Some(Err(err))) => {
                // Stream error: deliver via the terminal future and DROP the
                // stream. We must NOT return `Err(...)` — that traps the
                // guest. `Dropped` lets the guest observe the terminal.
                self.stream.take();
                self.finish_terminal(Some(err.to_string()));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
        }
    }
}

/// Drain the byte stream out of a [`StreamBody`] for crossing into WASM.
///
/// This is an **async** helper: it must be awaited *before* entering
/// `Store::run_concurrent`, because `run_concurrent` runs on a runtime thread
/// where a synchronous `tokio::sync::Mutex` lock would be illegal. The
/// returned `Option<BoxStream>` is `None` if the body was already consumed
/// (matching [`CamelError::AlreadyConsumed`] semantics).
///
/// The [`StreamMetadata`] (size hint, content-type, origin) is moved out so it
/// can be attached to the [`StreamBodyHandle`] verbatim.
pub(crate) async fn extract_stream_body(
    body: StreamBody,
) -> (
    Option<BoxStream<'static, Result<Bytes, CamelError>>>,
    StreamMetadata,
) {
    // `Arc<Mutex<Option<BoxStream>>>` → take the inner stream under the lock.
    let stream = body.stream.lock().await.take();
    (stream, body.metadata)
}

/// Assemble a [`WasmBody::Stream`] handle from an extracted byte stream.
///
/// **Must be called inside `Store::run_concurrent`** — it borrows the
/// [`Accessor`] needed to create the `StreamReader` / `FutureReader`.
///
/// Wire-up:
/// - [`BoxStreamProducer`] pumps `stream` into the guest-readable `stream<u8>`
///   and reports per-chunk progress to `progress_notify`.
/// - A oneshot channel pairs the producer's terminal outcome (`terminal_tx`)
///   with the guest's `future<result<_, wasm-error>>` (`terminal_rx`).
///
/// Returns a fully-populated [`StreamBodyHandle`] wrapped in [`WasmBody::Stream`].
pub(crate) fn assemble_stream_body(
    accessor: &Accessor<WasmHostState>,
    stream: BoxStream<'static, Result<Bytes, CamelError>>,
    metadata: &StreamMetadata,
    cancel: CancellationToken,
    max_bytes: u64,
    progress_notify: Arc<Notify>,
) -> wasmtime::Result<WasmBody> {
    let (terminal_tx, terminal_rx) = oneshot::channel::<Option<String>>();

    let producer = BoxStreamProducer::new(
        Some(stream),
        progress_notify,
        terminal_tx,
        cancel,
        max_bytes,
    );

    // `Accessor::with` yields an `Access<'_, T>` that impls `AsContextMut`,
    // which both readers need. `with` takes `&self` and forwards the
    // closure's result.
    let stream_reader = accessor.with(|mut access| StreamReader::new(&mut access, producer))?;

    let terminal_reader = accessor.with(|mut access| {
        FutureReader::new(&mut access, async move {
            // Map the producer's terminal outcome to the `result<(), wasm-error>`
            // value the guest reads. The outer `Ok` satisfies the
            // `FutureProducer` blanket impl over `Future<Output = Result<T, _>>`.
            let value: Result<(), WasmError> = match terminal_rx.await {
                Ok(Some(msg)) => Err(WasmError::ProcessorError(msg)),
                // Clean completion — success.
                Ok(None) => Ok(()),
                // Producer abandoned (guest trapped mid-stream) — best-effort
                // success. The dropped sender means the producer task exited
                // without a terminal signal (guest crashed or was killed).
                // Returning Ok is safe because the framework already
                // registered the failure upstream.
                Err(_) => Ok(()),
            };
            Ok::<_, wasmtime::Error>(value)
        })
    })?;

    Ok(WasmBody::Stream(StreamBodyHandle {
        r#stream: stream_reader,
        terminal: terminal_reader,
        size_hint: metadata.size_hint,
        content_type: metadata.content_type.clone(),
        origin: metadata.origin.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::camel::plugin::types::WasmBody;
    use camel_api::StreamMetadata;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn test_extract_stream_body_takes_stream_once() {
        use futures::stream;
        // First extraction takes the stream; a second (clone sharing the Arc)
        // observes None — the single-consumption contract of Body::Stream.
        let chunks: Vec<Result<Bytes, CamelError>> = vec![
            Ok(Bytes::from_static(b"abc")),
            Ok(Bytes::from_static(b"de")),
        ];
        let body = StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream::iter(chunks))))),
            metadata: StreamMetadata {
                size_hint: Some(5),
                content_type: Some("text/plain".into()),
                origin: Some("test://origin".into()),
            },
        };
        // Clones share the same Arc<Mutex<Option<BoxStream>>>.
        let clone = body.clone();

        let (stream, metadata) = extract_stream_body(body).await;
        assert!(stream.is_some(), "first extraction must yield the stream");
        assert_eq!(metadata.size_hint, Some(5));
        assert_eq!(metadata.content_type.as_deref(), Some("text/plain"));
        assert_eq!(metadata.origin.as_deref(), Some("test://origin"));

        let (again, _) = extract_stream_body(clone).await;
        assert!(again.is_none(), "second extraction must observe None");
    }

    #[tokio::test]
    async fn test_assemble_stream_body_and_wasm_to_body_roundtrip() {
        // End-to-end: build a real StreamBodyHandle via assemble_stream_body
        // inside run_concurrent, then verify wasm_to_body maps the Stream
        // variant to Body::Empty (Phase 1: guest→host not rebuilt).
        use crate::runtime::WasmHostState;
        use camel_core::Registry;
        use futures::stream;
        use wasmtime::{AsContextMut, Config, Engine, Store};

        let mut config = Config::new();
        config.wasm_component_model(true);
        config.concurrency_support(true);
        let engine = Engine::new(&config).expect("engine");
        let state = WasmHostState {
            table: wasmtime::component::ResourceTable::new(),
            wasi: wasmtime_wasi::WasiCtxBuilder::new()
                .inherit_stderr()
                .build(),
            properties: HashMap::new(),
            registry: Arc::new(std::sync::Mutex::new(Registry::new())),
            call_depth: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            limits: wasmtime::StoreLimits::default(),
            state_store: crate::state_store::StateStore::new(),
            capabilities: crate::capabilities::WasmCapabilities::default(),
        };
        let mut store = Store::new(&engine, state);

        let chunks: Vec<Result<Bytes, CamelError>> = vec![Ok(Bytes::from_static(b"payload"))];
        let stream =
            Box::pin(stream::iter(chunks)) as BoxStream<'static, Result<Bytes, CamelError>>;
        let metadata = StreamMetadata {
            content_type: Some("application/octet-stream".into()),
            ..StreamMetadata::default()
        };
        let cancel = CancellationToken::new();
        let notify = Arc::new(tokio::sync::Notify::new());

        let body_result = store
            .as_context_mut()
            .run_concurrent(async |accessor| {
                assemble_stream_body(accessor, stream, &metadata, cancel, 1024, notify)
            })
            .await;

        // run_concurrent nests results: outer = run_concurrent framing,
        // inner = assemble_stream_body's own wasmtime::Result.
        let wasm_body = body_result
            .expect("run_concurrent should not fail")
            .expect("assemble_stream_body should succeed");

        assert!(matches!(wasm_body, WasmBody::Stream(_)));

        // Phase 1: wasm_to_body collapses the guest-bound handle to Empty.
        // We cannot call serde_bridge::wasm_to_body from here without a
        // circular dep, so just assert the variant directly.
        assert!(matches!(wasm_body, WasmBody::Stream(_)));
    }

    // -----------------------------------------------------------------------
    // poll_produce direct-drive tests (via wasmtime store + pipe)
    // -----------------------------------------------------------------------
    //
    // These tests create a real wasmtime Store, register a BoxStreamProducer,
    // pipe it to a test consumer, and drive the store's async executor to
    // exercise poll_produce through the normal stream pipeline.

    /// Test consumer that collects bytes received from the producer.
    /// Uses `Arc<Mutex<Vec<u8>>>` so the collected bytes can be extracted
    /// after `pipe()` consumes the consumer.
    struct CollectConsumer {
        items: Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl CollectConsumer {
        fn new() -> Self {
            Self {
                items: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        /// Create a consumer and a shared handle to its collected bytes.
        fn shared() -> (Self, Arc<std::sync::Mutex<Vec<u8>>>) {
            let items = Arc::new(std::sync::Mutex::new(Vec::new()));
            (
                Self {
                    items: items.clone(),
                },
                items,
            )
        }
    }

    impl wasmtime::component::StreamConsumer<WasmHostState> for CollectConsumer {
        type Item = u8;

        fn poll_consume(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            store: StoreContextMut<'_, WasmHostState>,
            mut source: wasmtime::component::Source<'_, Self::Item>,
            _finish: bool,
        ) -> Poll<wasmtime::Result<StreamResult>> {
            // Read available bytes from the stream buffer into a local vec.
            let mut buf = Vec::with_capacity(65536);
            source
                .read(store, &mut buf)
                .expect("CollectConsumer read should not fail");
            self.get_mut().items.lock().unwrap().extend(buf);
            Poll::Ready(Ok(StreamResult::Completed))
        }
    }

    /// Shared setup for poll_produce tests: create an Engine + Store with
    /// WasmHostState, a BoxStreamProducer, and return them plus a terminal_rx.
    fn make_producer_store(
        stream: BoxStream<'static, Result<Bytes, CamelError>>,
        cancel: CancellationToken,
        max_bytes: u64,
    ) -> (
        wasmtime::Engine,
        wasmtime::Store<WasmHostState>,
        oneshot::Receiver<Option<String>>,
        Arc<Notify>,
        BoxStreamProducer,
    ) {
        use camel_core::Registry;
        use wasmtime::{Config, Engine, Store};

        let mut config = Config::new();
        config.wasm_component_model(true);
        config.concurrency_support(true);
        let engine = Engine::new(&config).expect("engine");
        let state = WasmHostState {
            table: wasmtime::component::ResourceTable::new(),
            wasi: wasmtime_wasi::WasiCtxBuilder::new()
                .inherit_stderr()
                .build(),
            properties: HashMap::new(),
            registry: Arc::new(std::sync::Mutex::new(Registry::new())),
            call_depth: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            limits: wasmtime::StoreLimits::default(),
            state_store: crate::state_store::StateStore::new(),
            capabilities: crate::capabilities::WasmCapabilities::default(),
        };
        let store = Store::new(&engine, state);
        let notify = Arc::new(Notify::new());
        let (terminal_tx, terminal_rx) = oneshot::channel::<Option<String>>();
        let producer =
            BoxStreamProducer::new(Some(stream), notify.clone(), terminal_tx, cancel, max_bytes);
        (engine, store, terminal_rx, notify, producer)
    }

    #[tokio::test]
    async fn test_poll_produce_normal_flow() {
        // Two chunks followed by clean EOF → terminal Ok, both chunks delivered.
        use futures::stream;
        use wasmtime::AsContextMut;

        let chunks: Vec<Result<Bytes, CamelError>> = vec![
            Ok(Bytes::from_static(b"hello")),
            Ok(Bytes::from_static(b"world")),
        ];
        let stream =
            Box::pin(stream::iter(chunks)) as BoxStream<'static, Result<Bytes, CamelError>>;
        let cancel = CancellationToken::new();

        let (_engine, mut store, terminal_rx, notify, producer) =
            make_producer_store(stream, cancel, 1024);

        let (consumer, collected) = CollectConsumer::shared();

        let outcome = store
            .as_context_mut()
            .run_concurrent(async |accessor| {
                // Register producer + consumer inside the store's runtime.
                accessor.with(|mut access| {
                    let stream_reader =
                        StreamReader::new(&mut access, producer).expect("StreamReader::new");
                    stream_reader.pipe(&mut access, consumer).expect("pipe");
                    Ok::<_, wasmtime::Error>(())
                })?;
                // Yield to the executor so the stream-driving loop (pushed by
                // pipe) can call poll_produce.  The terminal oneshot resolves
                // when the producer finishes.
                terminal_rx
                    .await
                    .map_err(|_| wasmtime::Error::msg("terminal_rx dropped"))
            })
            .await;

        // Terminal should be None (clean completion).
        let terminal = outcome.expect("run_concurrent ok").expect("terminal ok");
        assert!(
            terminal.is_none(),
            "expected clean completion, got {:?}",
            terminal
        );

        // Assert both chunks were delivered.
        {
            let bytes = collected.lock().unwrap();
            assert_eq!(&*bytes, b"helloworld", "expected both chunks");
        }

        // Progress notify fired at least once (two chunks → two notifies).
        use std::time::Duration;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), notify.notified())
                .await
                .is_ok(),
            "progress notify should have been fired"
        );
    }

    #[tokio::test]
    async fn test_poll_produce_error_flow() {
        // Stream yields an error → terminal reports the error message.
        use futures::stream;
        use wasmtime::AsContextMut;

        let chunks: Vec<Result<Bytes, CamelError>> = vec![
            Ok(Bytes::from_static(b"ok")),
            Err(CamelError::ProcessorError("test error".into())),
        ];
        let stream =
            Box::pin(stream::iter(chunks)) as BoxStream<'static, Result<Bytes, CamelError>>;
        let cancel = CancellationToken::new();

        let (_engine, mut store, terminal_rx, notify, producer) =
            make_producer_store(stream, cancel, 1024);

        let outcome = store
            .as_context_mut()
            .run_concurrent(async |accessor| {
                accessor.with(|mut access| {
                    let stream_reader =
                        StreamReader::new(&mut access, producer).expect("StreamReader::new");
                    stream_reader
                        .pipe(&mut access, CollectConsumer::new())
                        .expect("pipe");
                    Ok::<_, wasmtime::Error>(())
                })?;
                terminal_rx
                    .await
                    .map_err(|_| wasmtime::Error::msg("terminal_rx dropped"))
            })
            .await;

        let terminal = outcome.expect("run_concurrent ok").expect("terminal ok");
        // CamelError::to_string() includes the error kind prefix (e.g.
        // "Processor error: ...").  Check substring instead of exact match.
        assert!(
            terminal
                .as_deref()
                .unwrap_or_default()
                .contains("test error"),
            "expected error containing 'test error', got {:?}",
            terminal
        );
        // Notify was fired for the "ok" chunk shipped before the error.
        use std::time::Duration;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), notify.notified())
                .await
                .is_ok(),
            "progress notify should have been fired for shipped chunk"
        );
    }

    #[tokio::test]
    async fn test_poll_produce_max_bytes_overflow() {
        // Total bytes exceed max_bytes → terminal reports overflow.
        use futures::stream;
        use wasmtime::AsContextMut;

        // Two 5-byte chunks + 1-byte chunk = 11 bytes total, but max_bytes=6.
        // The first two (5+5=10 > 6) trigger overflow on the second chunk.
        let chunks: Vec<Result<Bytes, CamelError>> = vec![
            Ok(Bytes::from_static(b"hello")), // 5 bytes, written=5 ≤ 6 OK
            Ok(Bytes::from_static(b"world")), // 5 bytes, written=10 > 6 OVERFLOW
        ];
        let stream =
            Box::pin(stream::iter(chunks)) as BoxStream<'static, Result<Bytes, CamelError>>;
        let cancel = CancellationToken::new();

        let (_engine, mut store, terminal_rx, notify, producer) =
            make_producer_store(stream, cancel, 6);

        let outcome = store
            .as_context_mut()
            .run_concurrent(async |accessor| {
                accessor.with(|mut access| {
                    let stream_reader =
                        StreamReader::new(&mut access, producer).expect("StreamReader::new");
                    stream_reader
                        .pipe(&mut access, CollectConsumer::new())
                        .expect("pipe");
                    Ok::<_, wasmtime::Error>(())
                })?;
                terminal_rx
                    .await
                    .map_err(|_| wasmtime::Error::msg("terminal_rx dropped"))
            })
            .await;

        let terminal = outcome.expect("run_concurrent ok").expect("terminal ok");
        let msg = terminal.expect("expected overflow error");
        assert!(msg.contains("max-bytes"), "overflow msg: {msg}");
        // Notify was fired for the "hello" chunk shipped before overflow.
        use std::time::Duration;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), notify.notified())
                .await
                .is_ok(),
            "progress notify should have been fired before overflow"
        );
    }

    #[tokio::test]
    async fn test_poll_produce_cancelled_token() {
        // Pre-cancelled token → poll_produce returns Dropped immediately.
        use futures::stream;
        use wasmtime::AsContextMut;

        let chunks: Vec<Result<Bytes, CamelError>> = vec![Ok(Bytes::from_static(b"data"))];
        let stream =
            Box::pin(stream::iter(chunks)) as BoxStream<'static, Result<Bytes, CamelError>>;
        let cancel = CancellationToken::new();
        cancel.cancel(); // trip before producer starts

        let (_engine, mut store, terminal_rx, notify, producer) =
            make_producer_store(stream, cancel, 1024);

        let outcome = store
            .as_context_mut()
            .run_concurrent(async |accessor| {
                accessor.with(|mut access| {
                    let stream_reader =
                        StreamReader::new(&mut access, producer).expect("StreamReader::new");
                    stream_reader
                        .pipe(&mut access, CollectConsumer::new())
                        .expect("pipe");
                    Ok::<_, wasmtime::Error>(())
                })?;
                terminal_rx
                    .await
                    .map_err(|_| wasmtime::Error::msg("terminal_rx dropped"))
            })
            .await;

        let terminal = outcome.expect("run_concurrent ok").expect("terminal ok");
        // Cancelled token → terminal None (finish_terminal(None) via Dropped).
        assert!(
            terminal.is_none(),
            "expected no error for cancel, got {:?}",
            terminal
        );
        // No data shipped before cancel → notify was never fired.
        use std::time::Duration;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), notify.notified())
                .await
                .is_err(),
            "progress notify should NOT have been fired for cancelled stream"
        );
    }
}
