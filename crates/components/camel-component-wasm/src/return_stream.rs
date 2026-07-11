//! Guest→host streaming return path.
//!
//! When a WASM guest returns a `WasmBody::Stream`, the host cannot materialize
//! it synchronously (reading a wasmtime `stream<u8>` requires `&mut Store`
//! inside `run_concurrent`). Instead a spawned drain task owns the moved
//! `Store`, drives the guest's `StreamReader` via `pipe`, and pushes `Bytes`
//! through a bounded channel. The returned `Body::Stream` adapts over the
//! receiver — see spec 2026-07-05-wasm-return-path-streaming-design.md.

use bytes::Bytes;
use camel_api::error::CamelError;
use futures::stream;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::{CancellationToken, PollSender};
use tracing::Instrument;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Accessor, FutureConsumer, FutureReader, Lift, Source, StreamConsumer, StreamReader,
    StreamResult,
};

use crate::error::WasmError;

/// Default bounded-channel slot count for the drain → pipeline bridge.
/// Bounds *in-flight chunks*, not total bytes (the watchdog bounds total bytes).
pub(crate) const DEFAULT_DRAIN_CHANNEL_BOUND: usize = 8;

/// What travels through the drain channel. Channel close = clean EOF.
#[derive(Debug)]
pub(crate) enum DrainEvent {
    Chunk(Bytes),
    Error(CamelError),
}

/// Trait for extracting a guest-emitted stream from a binding-specific
/// `WasmExchange`. Plugin (`bindings`) and bean (`bean_bindings`) generate
/// distinct `WasmExchange`/`WasmBody`/`StreamBodyHandle` types; this trait
/// abstracts over them so the drain spawn logic is binding-agnostic.
///
/// The `E` parameter is the binding-specific wasm-error type (plugin's
/// `WasmError` vs bean's `WasmError`) — it determines the terminal future's
/// error type.
pub(crate) trait StreamReturnable<E: 'static> {
    /// Extract the stream reader and terminal future from this exchange's
    /// output message, if the body is a `Stream` variant. Returns `None` if
    /// the output is `None` or the body is not a stream.
    ///
    /// The stream is moved out of the exchange (replaced with `WasmBody::Empty`)
    /// so the caller can drive it to completion.
    fn take_stream(&mut self) -> Option<GuestStreamParts<E>>;
}

pub(crate) type GuestStreamParts<E> = (
    StreamReader<u8>,
    FutureReader<Result<(), E>>,
    camel_api::StreamMetadata,
);

impl StreamReturnable<crate::bindings::camel::plugin::types::WasmError>
    for crate::bindings::camel::plugin::types::WasmExchange
{
    fn take_stream(
        &mut self,
    ) -> Option<GuestStreamParts<crate::bindings::camel::plugin::types::WasmError>> {
        use crate::bindings::camel::plugin::types::WasmBody;
        let msg = self.output.as_mut()?;
        if !matches!(msg.body, WasmBody::Stream(_)) {
            return None;
        }
        if let WasmBody::Stream(handle) = std::mem::replace(&mut msg.body, WasmBody::Empty) {
            let metadata = camel_api::StreamMetadata {
                size_hint: handle.size_hint,
                content_type: handle.content_type,
                origin: handle.origin,
            };
            Some((handle.r#stream, handle.terminal, metadata))
        } else {
            unreachable!()
        }
    }
}

impl StreamReturnable<crate::bean_bindings::camel::plugin::types::WasmError>
    for crate::bean_bindings::camel::plugin::types::WasmExchange
{
    fn take_stream(
        &mut self,
    ) -> Option<GuestStreamParts<crate::bean_bindings::camel::plugin::types::WasmError>> {
        use crate::bean_bindings::camel::plugin::types::WasmBody;
        // Bean convention: the guest transforms input.body in place (unlike
        // the plugin path which uses output). Check input.body for the stream.
        let msg = &mut self.input;
        if !matches!(msg.body, WasmBody::Stream(_)) {
            return None;
        }
        if let WasmBody::Stream(handle) = std::mem::replace(&mut msg.body, WasmBody::Empty) {
            let metadata = camel_api::StreamMetadata {
                size_hint: handle.size_hint,
                content_type: handle.content_type,
                origin: handle.origin,
            };
            Some((handle.r#stream, handle.terminal, metadata))
        } else {
            unreachable!()
        }
    }
}

impl StreamReturnable<crate::source_bindings::camel::plugin::types::WasmError>
    for crate::source_bindings::camel::plugin::types::WasmExchange
{
    fn take_stream(
        &mut self,
    ) -> Option<GuestStreamParts<crate::source_bindings::camel::plugin::types::WasmError>> {
        use crate::source_bindings::camel::plugin::types::WasmBody;
        // Source convention: the guest submits the body it wants pushed into
        // the pipeline on input.body (matching the bean convention). The host
        // import extracts the stream here and replaces the body with Empty.
        let msg = &mut self.input;
        if !matches!(msg.body, WasmBody::Stream(_)) {
            return None;
        }
        if let WasmBody::Stream(handle) = std::mem::replace(&mut msg.body, WasmBody::Empty) {
            let metadata = camel_api::StreamMetadata {
                size_hint: handle.size_hint,
                content_type: handle.content_type,
                origin: handle.origin,
            };
            Some((handle.r#stream, handle.terminal, metadata))
        } else {
            unreachable!()
        }
    }
}

/// Adapter turning a `mpsc::Receiver<DrainEvent>` into the inner `BoxStream`
/// that `camel_api::StreamBody` wraps. Clean channel close = stream end;
/// `DrainEvent::Error` = final `Err`. The caller wraps the result with
/// `Body::Stream(StreamBody { stream: Arc::new(Mutex::new(Some(Box::pin(s)))),
/// metadata: StreamMetadata { .. } })` — populated from the guest's StreamBodyHandle.
pub(crate) fn receiver_to_body_stream(
    mut rx: mpsc::Receiver<DrainEvent>,
) -> futures::stream::BoxStream<'static, Result<Bytes, CamelError>> {
    Box::pin(stream::poll_fn(move |cx| match rx.poll_recv(cx) {
        Poll::Ready(Some(DrainEvent::Chunk(b))) => Poll::Ready(Some(Ok(b))),
        Poll::Ready(Some(DrainEvent::Error(e))) => Poll::Ready(Some(Err(e))),
        Poll::Ready(None) => Poll::Ready(None),
        Poll::Pending => Poll::Pending,
    }))
}

/// `StreamConsumer` bridging the guest's emitted `stream<u8>` into a bounded
/// `mpsc::Sender<DrainEvent>`. Real backpressure: `PollSender::poll_reserve`
/// registers the waker and returns `Pending` when the channel is full, so
/// wasmtime stops driving the guest's writes until downstream drains.
///
/// Signals `progress` (an `Arc<Notify>` shared with the watchdog) on each
/// successful chunk send, so a legitimately slow consumer (backpressure stall)
/// does NOT false-trip the no-progress watchdog — only a truly wedged guest
/// (no chunks flowing) trips it.
pub(crate) struct ChannelConsumer<S> {
    sender: PollSender<DrainEvent>,
    cancel: CancellationToken,
    progress: Arc<Notify>,
    receiver_gone: Arc<Notify>, // fired when poll_reserve sees the receiver dropped
    _marker: std::marker::PhantomData<S>,
}

impl<S> ChannelConsumer<S> {
    pub(crate) fn new(
        tx: mpsc::Sender<DrainEvent>,
        cancel: CancellationToken,
        progress: Arc<Notify>,
        receiver_gone: Arc<Notify>,
    ) -> Self {
        Self {
            sender: PollSender::new(tx),
            cancel,
            progress,
            receiver_gone,
            _marker: std::marker::PhantomData,
        }
    }
}

// SAFETY: Unpin is implemented unconditionally because ChannelConsumer
// does not perform pin-projection; all fields (including PhantomData<S>)
// are safe to move after pinning regardless of S.
impl<S> Unpin for ChannelConsumer<S> {}

impl<S: Send + 'static> StreamConsumer<S> for ChannelConsumer<S> {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<'_, S>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        // Cancel-on-drop symmetry with BoxStreamProducer (stream_bridge.rs:118).
        if self.cancel.is_cancelled() {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        match self.sender.poll_reserve(cx) {
            Poll::Ready(Ok(())) => {
                let mut buf = Vec::with_capacity(65536);
                match source.read(store, &mut buf) {
                    Ok(()) => {}
                    Err(e) => {
                        // Do NOT propagate Err (would trap the guest); surface
                        // as a terminal error and drop, mirroring BoxStreamProducer
                        // discipline (stream_bridge.rs:161-168).
                        let _ =
                            self.sender
                                .send_item(DrainEvent::Error(CamelError::ProcessorError(
                                    e.to_string(),
                                )));
                        self.receiver_gone.notify_one();
                        return Poll::Ready(Ok(StreamResult::Dropped));
                    }
                }
                if buf.is_empty() {
                    // Guest-initiated EOF arrives as finish=true. finish=false
                    // + empty means "no data yet" — return Pending to avoid a
                    // busy-spin (NOT Dropped, which would propagate as a cancel).
                    if finish {
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }
                    return Poll::Pending;
                }
                self.sender
                    .send_item(DrainEvent::Chunk(Bytes::from(buf)))
                    .expect("reserved slot must accept send"); // allow-unwrap
                self.progress.notify_one();
                Poll::Ready(Ok(StreamResult::Completed))
            }
            // Receiver gone (pipeline dropped the Body::Stream) → signal the
            // drain task's select! so it can cancel promptly, then Dropped.
            Poll::Ready(Err(_)) => {
                self.receiver_gone.notify_one();
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Pending => Poll::Pending, // backpressure
        }
    }
}

/// Drive the guest's emitted stream to completion.
/// Pipes the `StreamReader` into a [`ChannelConsumer`] and the terminal
/// [`FutureReader`] into a [`TerminalFutureConsumer`]; on terminal `Err`,
/// pushes `DrainEvent::Error` and closes.
///
/// **Must be called inside `Store::run_concurrent`** (plugin/bean) **OR an
/// `Accessor::spawn` task** (source world) — anywhere an `&Accessor` is in scope.
///
/// Generic over `E` (binding-specific wasm-error) because plugin
/// (`bindings`) and bean (`bean_bindings`) generate distinct `StreamBodyHandle`
/// types whose cross-binding `From` omits `Stream`.
/// Generic over `S` (host-state type) so both `WasmHostState` (plugin/bean)
/// and `SourceHostState` (source world) can reuse this drain scaffold.
pub(crate) async fn drain_guest_stream<E, S>(
    accessor: &Accessor<S>,
    stream_reader: StreamReader<u8>,
    terminal: FutureReader<Result<(), E>>,
    tx: mpsc::Sender<DrainEvent>,
    cancel: CancellationToken,
    progress: Arc<Notify>,
    receiver_gone: Arc<Notify>,
) where
    E: Into<CamelError> + Send + Sync + 'static,
    S: Send + 'static,
    Result<(), E>: Lift,
{
    // Oneshot channel to receive the terminal result from the concurrent
    // execution context.
    let (terminal_tx, terminal_rx) = tokio::sync::oneshot::channel::<Result<(), E>>();

    // Register both the stream pump and the terminal consumer inside the
    // same accessor.with() call — both need the concurrent context.
    accessor.with(|mut access| {
        stream_reader
            .pipe(
                &mut access,
                ChannelConsumer::new(
                    tx.clone(),
                    cancel.clone(),
                    progress.clone(),
                    receiver_gone.clone(),
                ),
            )
            .expect("pipe registration"); // allow-unwrap

        terminal
            .pipe(
                &mut access,
                TerminalFutureConsumer {
                    tx: Some(terminal_tx),
                    _marker: std::marker::PhantomData,
                },
            )
            .expect("terminal pipe registration"); // allow-unwrap
    });

    // Wait for the terminal future to resolve.
    let outcome = match terminal_rx.await {
        Ok(v) => v,
        Err(_) => {
            // Terminal consumer dropped without resolving (guest protocol
            // violation or concurrent-execution teardown). Surface as a terminal
            // error rather than panicking — symmetric with BoxStreamProducer
            // (stream_bridge.rs tolerates sender-drop without erroring).
            let _ = tx
                .send(DrainEvent::Error(CamelError::ProcessorError(
                    "wasm guest terminal future dropped without resolving".into(),
                )))
                .await;
            drop(tx);
            return;
        }
    };
    if let Err(e) = outcome {
        let _ = tx.send(DrainEvent::Error(e.into())).await;
    }
    // Dropping tx here signals EOF to the receiver (clean close).
    drop(tx);
}

/// Consumer that captures the terminal future's resolved value and forwards
/// it through a oneshot channel.
struct TerminalFutureConsumer<E> {
    tx: Option<tokio::sync::oneshot::Sender<Result<(), E>>>,
    _marker: std::marker::PhantomData<E>,
}

// SAFETY: Unpin is implemented unconditionally because TerminalFutureConsumer
// does not perform pin-projection; the oneshot::Sender and PhantomData are safe
// to move after pinning regardless of E.
impl<E> Unpin for TerminalFutureConsumer<E> {}

// Manual impl to avoid requiring E: Debug for the auto-derived Debug.
impl<E> std::fmt::Debug for TerminalFutureConsumer<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalFutureConsumer")
            .field("tx", &self.tx.as_ref().map(|_| "Some(Sender)"))
            .finish()
    }
}

impl<E: Send + Sync + 'static, S> FutureConsumer<S> for TerminalFutureConsumer<E>
where
    Result<(), E>: Lift,
{
    type Item = Result<(), E>;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: StoreContextMut<'_, S>,
        mut source: Source<'_, Self::Item>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<()>> {
        let this = self.get_mut();
        let mut buf: Option<Result<(), E>> = None;
        source.read(store, &mut buf)?;
        if let Some(value) = buf
            && let Some(tx) = this.tx.take()
        {
            let _ = tx.send(value);
        }
        Poll::Ready(Ok(()))
    }
}

/// Rendezvous type: the spawned drain task sends the guest's exchange
/// (plus optional drain receiver and stream metadata) back to the caller via a oneshot.
/// Generic over `W` (the binding-specific `WasmExchange` type) so both
/// the plugin and bean paths share one scaffold.
pub(crate) type StreamHandoff<W> =
    Result<(W, Option<DrainReceiver>, camel_api::StreamMetadata), WasmError>;
pub(crate) type StreamHandoffSender<W> = oneshot::Sender<StreamHandoff<W>>;
pub(crate) type DrainReceiver = mpsc::Receiver<DrainEvent>;

/// Take the handoff sender out of the shared mutex (one-shot).
pub(crate) fn take_stream_handoff_sender<W>(
    handoff: &Arc<std::sync::Mutex<Option<StreamHandoffSender<W>>>>,
) -> Option<StreamHandoffSender<W>> {
    handoff
        .lock()
        .expect("stream handoff lock not poisoned") // allow-unwrap
        .take()
}

/// Shared spawn + rendezvous + watchdog scaffold for the return-path
/// streaming drain. Both the plugin path (`process_streaming_exchange`)
/// and the bean path (`WasmBean::call`) run identical scaffolding; this
/// helper avoids two copies of load-bearing cancellation/watchdog logic.
///
/// `make_drive` is a closure that builds the inner `drive` future. It
/// receives the handoff sender, drain channel endpoints, cancel token,
/// progress notify, and receiver-gone notify — everything the inner
/// future needs to run `run_concurrent`, call the guest, extract the
/// stream (if any), send the handoff, and drain.
///
/// `pending_permit` is `Some(permit)` when the caller holds a concurrency
/// limiter (plugin path) — the permit is held for the drain lifetime so
/// the semaphore bounds in-flight drains. The bean path passes `None`
/// (no concurrency limiter on bean invocations).
///
/// The helper:
/// 1. Creates the drain channel and handoff oneshot.
/// 2. Spawns a task that runs `make_drive(...)` wrapped in
///    `drive_with_watchdog`.
/// 3. On watchdog error, sends the error through the handoff and the
///    drain channel (best-effort).
/// 4. Awaits the handoff and returns the exchange + optional drain
///    receiver.
pub(crate) async fn spawn_return_drain<W, F, Fut>(
    pending_permit: Option<tokio::sync::OwnedSemaphorePermit>,
    cancel: CancellationToken,
    no_progress_timeout: Duration,
    completion_notify: Option<Arc<Notify>>,
    make_drive: F,
) -> Result<(W, Option<DrainReceiver>, camel_api::StreamMetadata), WasmError>
where
    W: Send + 'static,
    F: FnOnce(
            Arc<std::sync::Mutex<Option<StreamHandoffSender<W>>>>,
            mpsc::Sender<DrainEvent>,
            mpsc::Receiver<DrainEvent>,
            CancellationToken,
            Arc<Notify>,
            Arc<Notify>,
        ) -> Fut
        + Send
        + 'static,
    Fut: Future<Output = Result<(), WasmError>> + Send + 'static,
{
    let (handoff_tx, handoff_rx) = oneshot::channel::<StreamHandoff<W>>();
    let handoff_shared = Arc::new(std::sync::Mutex::new(Some(handoff_tx)));
    let cancel_child = cancel.child_token();
    let progress_notify = Arc::new(Notify::new());
    let receiver_gone = Arc::new(Notify::new());

    tokio::spawn(async move {
        let _permit = pending_permit; // held for drain lifetime
        let cancel_drain = cancel_child.clone();
        let progress = progress_notify.clone();
        let rx_gone = receiver_gone.clone();
        let (dtx, drx) = mpsc::channel::<DrainEvent>(DEFAULT_DRAIN_CHANNEL_BOUND);
        let late_error_tx = dtx.clone(); // held for watchdog Err arm
        let handoff_drive = handoff_shared.clone();

        let drive = make_drive(handoff_drive, dtx, drx, cancel_drain, progress, rx_gone);
        let span = tracing::info_span!("wasm return-stream drain");

        let drain_result = crate::runtime::WasmRuntime::drive_with_watchdog(
            drive,
            &progress_notify,
            no_progress_timeout,
        )
        .instrument(span.clone())
        .await;

        let outcome = match &drain_result {
            Ok(()) => {
                if cancel_child.is_cancelled() {
                    "cancelled"
                } else {
                    "eof"
                }
            }
            Err(_) => "error",
        };
        tracing::debug!(outcome, "wasm return-stream drain ended");

        match drain_result {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "wasm return-stream drain failed");
                if let Some(tx) = take_stream_handoff_sender(&handoff_shared) {
                    let _ = tx.send(Err(e.clone()));
                }
                let _ = late_error_tx.send(DrainEvent::Error(e.into())).await;
            }
        }
        // store + plugin + permit drop here → Store freed
        // Fire drain-completion lifecycle hook if installed (metrics /
        // readiness probes / integration-test timing assertions).
        if let Some(notify) = completion_notify {
            notify.notify_one();
        }
    });

    // Await the rendezvous (fires fast, right after the guest returns).
    let (exchange_out, drain_rx, metadata) = match handoff_rx.await {
        Ok(Ok((exchange_out, drain_rx, metadata))) => (exchange_out, drain_rx, metadata),
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(WasmError::InstantiationFailed("drain task panicked".into()));
        }
    };
    Ok((exchange_out, drain_rx, metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use camel_api::error::CamelError;
    use futures::StreamExt;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn receiver_to_body_stream_yields_chunks_then_eof() {
        let (tx, rx) = mpsc::channel::<DrainEvent>(8);
        tx.send(DrainEvent::Chunk(Bytes::from_static(b"hello")))
            .await
            .unwrap();
        tx.send(DrainEvent::Chunk(Bytes::from_static(b"world")))
            .await
            .unwrap();
        drop(tx); // EOF

        let mut s = receiver_to_body_stream(rx);
        assert_eq!(
            s.next().await.unwrap().unwrap(),
            Bytes::from_static(b"hello")
        );
        assert_eq!(
            s.next().await.unwrap().unwrap(),
            Bytes::from_static(b"world")
        );
        assert!(s.next().await.is_none()); // clean EOF
    }

    #[tokio::test]
    async fn receiver_to_body_stream_surfaces_error_as_final_item() {
        let (tx, rx) = mpsc::channel::<DrainEvent>(8);
        tx.send(DrainEvent::Chunk(Bytes::from_static(b"partial")))
            .await
            .unwrap();
        tx.send(DrainEvent::Error(CamelError::Io("guest boom".into())))
            .await
            .unwrap();
        drop(tx);

        let mut s = receiver_to_body_stream(rx);
        assert_eq!(
            s.next().await.unwrap().unwrap(),
            Bytes::from_static(b"partial")
        );
        assert!(s.next().await.unwrap().is_err()); // terminal error
        assert!(s.next().await.is_none());
    }
}
