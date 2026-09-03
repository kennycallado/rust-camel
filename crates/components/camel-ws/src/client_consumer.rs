//! Client-mode WebSocket consumer: connects out to a remote WebSocket
//! server and consumes frames as an inbound route source (`consumeAsClient`).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_component_api::{
    Body as CamelBody, CamelError, ConcurrencyModel, Consumer, ConsumerContext,
    ConsumerStartupMode, Exchange, ExchangeEnvelope, Message as CamelMessage, NetworkRetryPolicy,
    RuntimeObservability, retry_async_cancelable,
};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_util::sync::CancellationToken;

use crate::{WsClientConfig, is_retryable_ws_error, map_connect_error, redact_ws_url_for_log};

/// Connection lifecycle state published on the consumer's `watch` channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientConnState {
    /// Connect (or reconnect) attempt in flight.
    Connecting,
    /// WebSocket handshake completed; frames can flow.
    Connected,
    /// Connected previously; a transient failure triggered a reconnect.
    Reconnecting,
    /// Reconnect policy exhausted; consumer gave up connecting.
    Exhausted,
}

/// Concrete client-side stream type produced by
/// [`connect_ws_client_cancelable`].
type ClientWsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Everything the frame loop needs to re-establish the connection after a
/// disconnect. Owned by the spawned task; the request (URL +
/// `Sec-WebSocket-Protocol` headers) is cloned per reconnect so every
/// attempt re-sends the IDENTICAL upgrade request.
struct ReconnectPlan {
    request: http::Request<()>,
    url: String,
    connect_timeout: Duration,
    policy: NetworkRetryPolicy,
    connector: Option<Connector>,
}

/// Deps the detached frame loop owns for its entire lifetime, captured at
/// spawn time — once `background_task_handle` transfers the task, the loop
/// runs unowned by `start()`.
struct FrameLoopDeps {
    sender: mpsc::Sender<ExchangeEnvelope>,
    route_id: String,
    runtime: Arc<dyn RuntimeObservability>,
    max_message_size: u32,
    plan: ReconnectPlan,
    conn_state_tx: watch::Sender<ClientConnState>,
}

/// Private lifecycle gate: rejects double `start` and makes `stop` idempotent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifecycleState {
    Created,
    Running,
    Stopped,
}

/// Event-driven consumer bound to an upstream WebSocket server.
///
/// The consumer connects out to the configured server, publishes its
/// connection lifecycle on the shared `conn_state_tx` watch channel, and runs
/// its receive loop as a background task owned by the Runtime (ADR-0007
/// supervision via [`Consumer::background_task_handle`]).
pub struct WsClientConsumer {
    cfg: WsClientConfig,
    runtime: Arc<dyn RuntimeObservability>,
    conn_state_tx: watch::Sender<ClientConnState>,
    /// Optional TLS connector for `wss://` targets. `None` (production)
    /// makes tungstenite build a rustls config from the native root store
    /// (enabled via the `rustls-tls-native-roots` feature).
    connector: Option<Connector>,
    cancel: Option<CancellationToken>,
    task: Option<JoinHandle<Result<(), CamelError>>>,
    state: LifecycleState,
}

impl WsClientConsumer {
    /// Create a client consumer in the `Created` state, seeding the
    /// connection-state channel with [`ClientConnState::Connecting`].
    pub fn new(
        cfg: WsClientConfig,
        runtime: Arc<dyn RuntimeObservability>,
        conn_state_tx: watch::Sender<ClientConnState>,
    ) -> Self {
        // `send_replace` ignores receiver presence, so the seed never fails.
        conn_state_tx.send_replace(ClientConnState::Connecting);
        Self {
            cfg,
            runtime,
            conn_state_tx,
            connector: None,
            cancel: None,
            task: None,
            state: LifecycleState::Created,
        }
    }

    /// Test-only TLS connector injection (e.g. a rustls config trusting the
    /// committed test fixtures); production constructors leave the stored
    /// connector as `None`.
    #[cfg(test)]
    pub(crate) fn with_connector(mut self, connector: Connector) -> Self {
        self.connector = Some(connector);
        self
    }
}

#[async_trait]
impl Consumer for WsClientConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        if self.state == LifecycleState::Running {
            return Err(CamelError::EndpointCreationFailed(
                "WebSocket client consumer already started".into(),
            ));
        }

        tracing::debug!(
            url = %redact_ws_url_for_log(&format!(
                "{}://{}:{}{}",
                self.cfg.inner.scheme, self.cfg.inner.host, self.cfg.inner.port, self.cfg.inner.path
            )),
            "WebSocket client consumer connecting"
        );

        self.conn_state_tx.send_replace(ClientConnState::Connecting);

        let inner = &self.cfg.inner;
        let url = format!(
            "{}://{}:{}{}",
            inner.scheme, inner.host, inner.port, inner.path
        );
        let mut request = url.clone().into_client_request().map_err(|e| {
            CamelError::EndpointCreationFailed(format!("WebSocket request error: {e}"))
        })?;

        // Add Sec-WebSocket-Protocol header if subprotocols configured (WS-007)
        if !inner.subprotocols.is_empty() {
            let proto_value = inner.subprotocols.join(", ");
            if let (Ok(name), Ok(val)) = (
                http::header::HeaderName::from_bytes(b"Sec-WebSocket-Protocol"),
                http::header::HeaderValue::from_str(&proto_value),
            ) {
                request.headers_mut().insert(name, val);
            }
        }

        let cancel = ctx.cancel_token();
        let metrics = self.runtime.metrics();
        match connect_ws_client_cancelable(
            request.clone(),
            &url,
            inner.connect_timeout,
            &inner.reconnect_policy,
            &cancel,
            Some(metrics.as_ref()),
            self.connector.clone(),
        )
        .await
        {
            Err(err) if cancel.is_cancelled() => {
                // Shutdown raced the connect — that is not exhaustion.
                Err(err)
            }
            Err(err) => {
                // Fail-loud start: reset to Created so a fresh start can be
                // attempted after a transient outage.
                self.state = LifecycleState::Created;
                self.conn_state_tx.send_replace(ClientConnState::Exhausted);
                Err(err)
            }
            Ok(stream) => {
                ctx.mark_ready();
                self.conn_state_tx.send_replace(ClientConnState::Connected);
                let sender = ctx.sender();
                let route_id = ctx.route_id().to_string();
                let runtime = Arc::clone(&self.runtime);
                let max_message_size = self.cfg.inner.max_message_size;
                // The task owns the whole reconnect loop: it re-sends the
                // SAME request (URL + Sec-WebSocket-Protocol) on every
                // reconnect and publishes lifecycle transitions on the
                // cloned watch sender.
                let task: JoinHandle<Result<(), CamelError>> = tokio::spawn(run_frame_loop(
                    stream,
                    FrameLoopDeps {
                        sender,
                        route_id,
                        runtime,
                        max_message_size,
                        plan: ReconnectPlan {
                            request,
                            url,
                            connect_timeout: inner.connect_timeout,
                            policy: inner.reconnect_policy.clone(),
                            connector: self.connector.clone(),
                        },
                        conn_state_tx: self.conn_state_tx.clone(),
                    },
                    cancel.clone(),
                ));
                self.task = Some(task);
                self.cancel = Some(cancel);
                self.state = LifecycleState::Running;
                Ok(())
            }
        }
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if self.state == LifecycleState::Stopped {
            return Ok(());
        }
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        // Await the receive task only while it is still locally owned — once
        // `background_task_handle` transferred it, the Runtime supervises it.
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        self.state = LifecycleState::Stopped;
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.task.take()
    }
}

/// Connect to a WebSocket server as a client, honouring a
/// [`CancellationToken`] before the retry sequence starts and during
/// inter-attempt backoff (via
/// [`camel_component_api::retry_async_cancelable`]).
///
/// Mirrors the producer's [`crate::connect_ws_with_retry`] request-clone +
/// timeout + error-mapping pattern, but the caller owns cancellation and the
/// connect metrics (`attempts` + exhaustion error) ride exclusively on
/// `metrics` — no `record_component_operation` here.
pub(crate) async fn connect_ws_client_cancelable<R>(
    request: R,
    url: &str,
    connect_timeout: Duration,
    policy: &NetworkRetryPolicy,
    cancel: &CancellationToken,
    metrics: Option<&dyn camel_api::MetricsCollector>,
    connector: Option<Connector>,
) -> Result<ClientWsStream, CamelError>
where
    R: IntoClientRequest + Unpin + Clone,
{
    let url_owned = url.to_string();
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(CamelError::ProcessorError(
            "WebSocket client connect cancelled".into(),
        )),
        result = retry_async_cancelable(
            policy,
            "ws",
            "connect",
            || {
                let r = request.clone();
                let url = url_owned.clone();
                let connector = connector.clone();
                async move {
                    match tokio::time::timeout(
                        connect_timeout,
                        tokio_tungstenite::connect_async_tls_with_config(
                            r, None, false, connector,
                        ),
                    )
                    .await
                    {
                        Ok(Ok((stream, _))) => Ok(stream),
                        Ok(Err(e)) => Err(map_connect_error(e, &url)),
                        Err(_) => Err(CamelError::ProcessorError(format!(
                            "WebSocket connect timeout ({connect_timeout:?}) to {}",
                            redact_ws_url_for_log(&url)
                        ))),
                    }
                }
            },
            is_retryable_ws_error,
            cancel,
            metrics,
        ) => result,
    }
}

/// Frame receive loop with reconnect: reads WebSocket frames until
/// cancellation, EOF, a stream error, or a Close frame, mapping each data
/// frame to an `ExchangeEnvelope` dispatched through `sender`.
///
/// Every disconnect path (`None` / `Err` / `Close`) publishes `Reconnecting`
/// and runs ONE fresh bounded reconnect sequence through
/// [`connect_ws_client_cancelable`] with the SAME request (URL +
/// `Sec-WebSocket-Protocol`) every time. On success the loop publishes
/// `Connected` and re-enters the receive session with the new stream; on
/// exhaustion it publishes `Exhausted` and returns `Err` so route supervision
/// sees the failure (ADR-0007). Cancellation during the sequence maps to a
/// clean `Ok(())` shutdown exit — shutdown is not exhaustion.
async fn run_frame_loop(
    mut stream: ClientWsStream,
    deps: FrameLoopDeps,
    cancel: CancellationToken,
) -> Result<(), CamelError> {
    loop {
        let outcome = receive_until_disconnect(
            &mut stream,
            &deps.sender,
            &deps.route_id,
            deps.runtime.as_ref(),
            deps.max_message_size,
            &cancel,
        )
        .await;
        // Category (b′) per ADR-0012: locally terminal dispatch failure —
        // surface it to route supervision instead of reconnecting.
        outcome?;
        if cancel.is_cancelled() {
            return Ok(());
        }
        tracing::info!(
            url = %redact_ws_url_for_log(&deps.plan.url),
            "WebSocket client consumer disconnected; reconnecting"
        );
        deps.conn_state_tx
            .send_replace(ClientConnState::Reconnecting);
        let metrics = deps.runtime.metrics();
        match connect_ws_client_cancelable(
            deps.plan.request.clone(),
            &deps.plan.url,
            deps.plan.connect_timeout,
            &deps.plan.policy,
            &cancel,
            Some(metrics.as_ref()),
            deps.plan.connector.clone(),
        )
        .await
        {
            Ok(next) => {
                stream = next;
                deps.conn_state_tx.send_replace(ClientConnState::Connected);
            }
            Err(err) => {
                if cancel.is_cancelled() {
                    // Shutdown raced the reconnect sequence — that is not
                    // exhaustion.
                    return Ok(());
                }
                tracing::warn!(
                    error = %err,
                    "WebSocket client consumer reconnect policy exhausted"
                );
                deps.conn_state_tx.send_replace(ClientConnState::Exhausted);
                return Err(CamelError::ProcessorError(
                    "WebSocket client consumer reconnect policy exhausted".into(),
                ));
            }
        }
    }
}

/// One connected receive session: reads frames from `stream` until
/// cancellation, EOF, a stream error, or a Close frame. Returns `Ok` on all
/// disconnect paths (the caller decides between shutdown and reconnect);
/// `Err` only on locally terminal dispatch failure (ADR-0012 category b′).
///
/// Send shape (at-most-one in-flight send, no busy loop): each read frame is
/// parked in `pending` and the loop's `select!` drains exactly that one send
/// before the frame arm is re-enabled, so reads pause while the channel is
/// full (backpressure) and the next frame is never touched mid-delivery.
/// Cancellation wins over a pending send because the cancel arm is first
/// under `biased;`.
async fn receive_until_disconnect(
    stream: &mut ClientWsStream,
    sender: &mpsc::Sender<ExchangeEnvelope>,
    route_id: &str,
    runtime: &dyn RuntimeObservability,
    max_message_size: u32,
    cancel: &CancellationToken,
) -> Result<(), CamelError> {
    let mut pending: Option<ExchangeEnvelope> = None;
    loop {
        // Park at most one envelope; the select drains exactly this send.
        let to_send = pending.take();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let _ = stream.close(None).await;
                return Ok(());
            }
            send = async {
                match to_send {
                    Some(env) => sender.send(env).await,
                    None => std::future::pending().await,
                }
            } => match send {
                Ok(()) => {
                    runtime.component_metrics().observe("ws", "frame", false);
                }
                Err(_) => {
                    // Category (b′) per ADR-0012: locally terminal dispatch
                    // failure — the pipeline receiver is gone and this return
                    // is the only signal.
                    runtime.component_metrics().observe("ws", "frame", true);
                    runtime
                        .metrics()
                        .increment_errors(route_id, "ws_client_consumer");
                    return Err(CamelError::ChannelClosed);
                }
            },
            frame = stream.next(), if to_send.is_none() => match frame {
                None => return Ok(()),
                Some(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        "WebSocket client consumer stream error"
                    );
                    return Ok(());
                }
                Some(Ok(msg)) => {
                    let (body, message_type, len) = match msg {
                        Message::Text(t) => {
                            let len = t.len();
                            (CamelBody::Text(t.to_string()), "text", len)
                        }
                        Message::Binary(b) => {
                            let len = b.len();
                            (CamelBody::Bytes(b), "binary", len)
                        }
                        // Transparent: tungstenite answers pings while the stream
                        // is read, and control frames never become exchanges.
                        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
                        Message::Close(_) => return Ok(()),
                    };
                    if len > max_message_size as usize {
                        tracing::warn!(
                            size = len,
                            max = max_message_size,
                            "WebSocket client frame exceeds maxMessageSize; dropping frame"
                        );
                        runtime
                            .metrics()
                            .increment_errors(route_id, "ws_client_consumer");
                        continue;
                    }
                    let mut message = CamelMessage::new(body);
                    message.set_header("CamelWsMessageType", message_type);
                    pending = Some(ExchangeEnvelope {
                        exchange: Exchange::new(message),
                        reply_tx: None,
                    });
                },
            },
        }
    }
}

#[cfg(test)]
#[path = "client_consumer_tests.rs"]
mod tests;
