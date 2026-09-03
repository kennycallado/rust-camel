//! Tests for the WebSocket client consumer (extracted from
//! `client_consumer.rs` per the 1k-line rule; keeps `super::` access to
//! the consumer internals).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{
    Body as CamelBody, Consumer, ConsumerContext, ExchangeEnvelope, NetworkRetryPolicy,
    RuntimeObservability, StartupReceiver, StartupSignal,
};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_util::sync::CancellationToken;

use super::{ClientConnState, WsClientConsumer, connect_ws_client_cancelable};
use crate::test_doubles::{CountingRuntime, RecordingMetrics};
use crate::{WsClientConfig, WsEndpointConfig};

/// A port on an ephemeral address that nothing listens on: bind, grab the
/// port, drop the listener.
fn unreachable_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// A URL on an ephemeral port that nothing listens on.
fn unreachable_url() -> String {
    format!("ws://127.0.0.1:{}/", unreachable_port())
}

#[tokio::test]
async fn connect_cancelable_returns_clean_on_cancel() {
    let policy = NetworkRetryPolicy {
        initial_delay: Duration::from_secs(5),
        ..NetworkRetryPolicy::default()
    };
    let cancel = CancellationToken::new();
    cancel.cancel();
    let url = unreachable_url();
    let start = Instant::now();
    let result = connect_ws_client_cancelable(
        url.clone(),
        &url,
        Duration::from_secs(1),
        &policy,
        &cancel,
        None,
        None,
    )
    .await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(100),
        "cancel must win before the 5s backoff, took {elapsed:?}"
    );
    let err = result.expect_err("cancelled connect must return Err");
    assert!(
        err.to_string().contains("cancelled"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn connect_cancelable_exhausts_to_err() {
    let policy = NetworkRetryPolicy {
        max_attempts: 2,
        initial_delay: Duration::from_millis(10),
        ..NetworkRetryPolicy::default()
    };
    let cancel = CancellationToken::new();
    let url = unreachable_url();
    let metrics = RecordingMetrics::new();
    let result = connect_ws_client_cancelable(
        url.clone(),
        &url,
        Duration::from_secs(1),
        &policy,
        &cancel,
        Some(&metrics),
        None,
    )
    .await;
    let err = result.expect_err("unreachable connect must exhaust to Err");
    assert!(
        err.to_string().contains("connection refused"),
        "expected the last attempt's error, got: {err}"
    );
    assert_eq!(
        *metrics.attempts.lock().unwrap(),
        vec![
            ("ws".to_string(), "connect".to_string()),
            ("ws".to_string(), "connect".to_string()),
        ],
        "max_attempts: 2 must record exactly two connect attempts"
    );
}

#[tokio::test]
async fn connect_cancelable_connects_when_reachable() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
    });
    let url = format!("ws://127.0.0.1:{port}/");
    let cancel = CancellationToken::new();
    let result = connect_ws_client_cancelable(
        url.clone(),
        &url,
        Duration::from_secs(2),
        &NetworkRetryPolicy::default(),
        &cancel,
        None,
        None,
    )
    .await;
    server.abort();
    let _stream = result.expect("reachable connect must return Ok(stream)");
}

// --- Task 1.3: WsClientConsumer lifecycle ---

/// Spawn a plain WebSocket server that accepts any number of connections,
/// holds each open until the peer disconnects, and never sends frames.
async fn spawn_push_ws_server(bind: &str) -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut ws = match tokio_tungstenite::accept_async(stream).await {
                    Ok(ws) => ws,
                    Err(_) => return,
                };
                // Hold the connection open until the peer goes away.
                while let Some(Ok(msg)) = ws.next().await {
                    if msg.is_close() {
                        break;
                    }
                }
            });
        }
    });
    (addr, handle)
}

/// Server variant that captures the upgrade request's
/// `Sec-WebSocket-Protocol` header before completing the handshake.
async fn spawn_protocol_capturing_server(
    bind: &str,
    captured: Arc<Mutex<Option<String>>>,
) -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let mut ws = match tokio_tungstenite::accept_hdr_async(stream, {
                let captured = Arc::clone(&captured);
                // The callback's Err type is the API-forced
                // `http::Response<Option<String>>`; the large Err is
                // inherent to the accept_hdr_async signature.
                #[allow(clippy::result_large_err)]
                move |req: &http::Request<()>, mut resp: http::Response<()>| {
                    if let Some(proto) = req.headers().get("Sec-WebSocket-Protocol") {
                        let value = proto.to_str().unwrap_or_default().to_string();
                        *captured.lock().unwrap() = Some(value.clone());
                        // Echo the selected protocol like a real server:
                        // tungstenite's client rejects a 101 that offers
                        // no subprotocol after the client proposed one.
                        if let Ok(val) = http::HeaderValue::from_str(&value) {
                            resp.headers_mut().insert("Sec-WebSocket-Protocol", val);
                        }
                    }
                    Ok(resp)
                }
            })
            .await
            {
                Ok(ws) => ws,
                Err(_) => return,
            };
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_close() {
                    break;
                }
            }
        }
    });
    (addr, handle)
}

/// Loopback client config with small timeouts and a fast bounded retry
/// policy so lifecycle tests stay quick and deterministic.
fn client_cfg(port: u16, subprotocols: Vec<String>) -> WsClientConfig {
    WsEndpointConfig {
        scheme: "ws".into(),
        host: "127.0.0.1".into(),
        port,
        path: "/".into(),
        connect_timeout: Duration::from_millis(500),
        subprotocols,
        reconnect_policy: NetworkRetryPolicy {
            enabled: true,
            max_attempts: 2,
            initial_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        },
        ..WsEndpointConfig::default()
    }
    .client_config()
}

/// Build a `ConsumerContext` exactly as the runtime does: channel +
/// cancel token + route id, with the controller's startup signal installed.
fn make_ctx(token: CancellationToken) -> (ConsumerContext, StartupReceiver) {
    let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let (signal, receiver) = StartupSignal::pair();
    let ctx = ConsumerContext::new(tx, token, "r1".into()).with_startup(signal);
    (ctx, receiver)
}

#[tokio::test]
async fn client_consumer_start_marks_ready_and_connects() {
    let (addr, server) = spawn_push_ws_server("127.0.0.1:0").await;
    let (state_tx, mut state_rx) = watch::channel(ClientConnState::Connecting);
    let mut consumer = WsClientConsumer::new(
        client_cfg(addr.port(), Vec::new()),
        Arc::new(NoopRuntimeObservability),
        state_tx,
    );
    assert_eq!(
        *state_rx.borrow(),
        ClientConnState::Connecting,
        "conn state must be seeded Connecting"
    );

    let (ctx, receiver) = make_ctx(CancellationToken::new());
    consumer.start(ctx).await.expect("start must succeed");

    tokio::time::timeout(Duration::from_secs(1), async move {
        while *state_rx.borrow_and_update() != ClientConnState::Connected {
            state_rx.changed().await.expect("conn_state_tx alive");
        }
    })
    .await
    .expect("Connected must be observed within 1s");

    tokio::time::timeout(Duration::from_secs(1), receiver.await_ready())
        .await
        .expect("startup signal must resolve within 1s")
        .expect("startup must be Ok after mark_ready");

    server.abort();
}

#[tokio::test]
async fn client_consumer_unreachable_fails_start() {
    // Grab a free port, then drop the listener so nothing accepts.
    let port = unreachable_port();

    let (state_tx, state_rx) = watch::channel(ClientConnState::Connecting);
    let mut consumer = WsClientConsumer::new(
        client_cfg(port, Vec::new()),
        Arc::new(NoopRuntimeObservability),
        state_tx,
    );

    let (ctx, _receiver) = make_ctx(CancellationToken::new());
    let err = consumer
        .start(ctx)
        .await
        .expect_err("unreachable start must fail");
    assert!(
        !err.to_string().contains("already started"),
        "unexpected double-start rejection: {err}"
    );
    assert_eq!(
        *state_rx.borrow(),
        ClientConnState::Exhausted,
        "failed start must publish Exhausted"
    );

    // A failed start resets to Created: a second start re-attempts the
    // connect (and fails again) instead of being rejected as a double start.
    let (ctx, _receiver) = make_ctx(CancellationToken::new());
    let err2 = consumer
        .start(ctx)
        .await
        .expect_err("second start must re-attempt and fail");
    assert!(
        !err2.to_string().contains("already started"),
        "second start after failure must be allowed, got: {err2}"
    );
    assert_eq!(*state_rx.borrow(), ClientConnState::Exhausted);
}

#[tokio::test]
async fn client_consumer_double_start_rejected() {
    let (addr, server) = spawn_push_ws_server("127.0.0.1:0").await;
    let (state_tx, _state_rx) = watch::channel(ClientConnState::Connecting);
    let mut consumer = WsClientConsumer::new(
        client_cfg(addr.port(), Vec::new()),
        Arc::new(NoopRuntimeObservability),
        state_tx,
    );
    let (ctx, _receiver) = make_ctx(CancellationToken::new());
    consumer.start(ctx).await.expect("first start must succeed");

    let (ctx2, _receiver2) = make_ctx(CancellationToken::new());
    let err = consumer
        .start(ctx2)
        .await
        .expect_err("double start must be rejected");
    assert!(
        err.to_string().contains("already started"),
        "unexpected error: {err}"
    );

    // The first task is unaffected: still transferable for Runtime
    // supervision. (Since Task 1.4 the task is a live receive loop, so
    // it exits only on shutdown — verified via stop() below.)
    assert!(consumer.background_task_handle().is_some());
    consumer.stop().await.expect("stop must cancel the loop");
    server.abort();
}

#[tokio::test]
async fn client_consumer_stop_idempotent() {
    let (addr, server) = spawn_push_ws_server("127.0.0.1:0").await;
    let (state_tx, _state_rx) = watch::channel(ClientConnState::Connecting);
    let mut consumer = WsClientConsumer::new(
        client_cfg(addr.port(), Vec::new()),
        Arc::new(NoopRuntimeObservability),
        state_tx,
    );
    let (ctx, _receiver) = make_ctx(CancellationToken::new());
    consumer.start(ctx).await.expect("start must succeed");

    // At-most-once transfer of the background task handle.
    assert!(consumer.background_task_handle().is_some());
    assert!(consumer.background_task_handle().is_none());

    consumer.stop().await.expect("first stop must succeed");
    consumer
        .stop()
        .await
        .expect("second stop must be an idempotent Ok");
    server.abort();
}

#[tokio::test]
async fn client_consumer_shutdown_while_receiving() {
    let (addr, server) = spawn_push_ws_server("127.0.0.1:0").await;
    let (state_tx, state_rx) = watch::channel(ClientConnState::Connecting);
    let mut consumer = WsClientConsumer::new(
        client_cfg(addr.port(), Vec::new()),
        Arc::new(NoopRuntimeObservability),
        state_tx,
    );
    let token = CancellationToken::new();
    let (ctx, _receiver) = make_ctx(token.clone());
    consumer.start(ctx).await.expect("start must succeed");

    let handle = consumer
        .background_task_handle()
        .expect("task handle must be present");
    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("consumer task must exit within 1s of cancellation")
        .expect("no join error");
    result.expect("consumer task must exit Ok on shutdown");
    assert_eq!(*state_rx.borrow(), ClientConnState::Connected);
    server.abort();
}

#[tokio::test]
async fn subprotocol_header_sent_on_connect() {
    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let (addr, server) =
        spawn_protocol_capturing_server("127.0.0.1:0", Arc::clone(&captured)).await;
    let (state_tx, _state_rx) = watch::channel(ClientConnState::Connecting);
    let mut consumer = WsClientConsumer::new(
        client_cfg(addr.port(), vec!["vt1".to_string()]),
        Arc::new(NoopRuntimeObservability),
        state_tx,
    );
    let (ctx, _receiver) = make_ctx(CancellationToken::new());
    consumer
        .start(ctx)
        .await
        .expect("handshake must complete with subprotocol configured");

    assert_eq!(
        captured.lock().unwrap().as_deref(),
        Some("vt1"),
        "upgrade request must carry Sec-WebSocket-Protocol: vt1"
    );
    server.abort();
}

// --- Task 1.4: frame receive loop, mapping, backpressure, limits ---

/// Push-server variant: after the first connection completes its
/// handshake, sends `frames` (after an optional `delay` so tests can
/// arrange local state first), then holds the connection open until the
/// peer disconnects.
async fn spawn_frame_push_server(
    bind: &str,
    frames: Vec<Message>,
    delay: Duration,
) -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let mut ws = match tokio_tungstenite::accept_async(stream).await {
                Ok(ws) => ws,
                Err(_) => return,
            };
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            for frame in frames {
                if ws.send(frame).await.is_err() {
                    return;
                }
            }
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_close() {
                    break;
                }
            }
        }
    });
    (addr, handle)
}

/// `make_ctx` variant that keeps the route-channel receiver (frame-loop
/// tests read the envelopes the consumer dispatches) and takes the route
/// id and channel capacity.
fn make_ctx_with_rx(
    token: CancellationToken,
    route_id: &str,
    capacity: usize,
) -> (ConsumerContext, mpsc::Receiver<ExchangeEnvelope>) {
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(capacity);
    let (signal, _receiver) = StartupSignal::pair();
    let ctx = ConsumerContext::new(tx, token, route_id.into()).with_startup(signal);
    (ctx, rx)
}

/// Start a connected client consumer against `port` and hand back the
/// consumer plus its route-channel receiver.
async fn start_client_consumer(
    port: u16,
    runtime: Arc<dyn RuntimeObservability>,
    token: &CancellationToken,
    route_id: &str,
    capacity: usize,
    max_message_size: u32,
) -> (WsClientConsumer, mpsc::Receiver<ExchangeEnvelope>) {
    let (state_tx, _state_rx) = watch::channel(ClientConnState::Connecting);
    let mut cfg = client_cfg(port, Vec::new());
    cfg.inner.max_message_size = max_message_size;
    let mut consumer = WsClientConsumer::new(cfg, runtime, state_tx);
    let (ctx, rx) = make_ctx_with_rx(token.clone(), route_id, capacity);
    consumer.start(ctx).await.expect("start must succeed");
    (consumer, rx)
}

#[tokio::test]
async fn frames_become_exchanges() {
    let frames = vec![
        Message::text("f1"),
        Message::text("f2"),
        Message::text("f3"),
    ];
    let (addr, server) = spawn_frame_push_server("127.0.0.1:0", frames, Duration::ZERO).await;
    let token = CancellationToken::new();
    let (mut consumer, mut rx) = start_client_consumer(
        addr.port(),
        Arc::new(NoopRuntimeObservability),
        &token,
        "r1",
        16,
        65536,
    )
    .await;
    let handle = consumer.background_task_handle().expect("handle present");

    let mut bodies = Vec::new();
    for _ in 0..3 {
        let env = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("envelope within 2s")
            .expect("route channel open");
        assert!(
            env.reply_tx.is_none(),
            "client-consumer envelopes are fire-and-forget"
        );
        assert_eq!(
            env.exchange.input.header("CamelWsMessageType"),
            Some(&serde_json::Value::String("text".into())),
            "text frame must carry the text type header"
        );
        match &env.exchange.input.body {
            CamelBody::Text(s) => bodies.push(s.clone()),
            other => panic!("expected Text body, got {other:?}"),
        }
    }
    assert_eq!(bodies, vec!["f1", "f2", "f3"]);

    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("task must exit within 1s of cancellation")
        .expect("no join error");
    result.expect("task must exit Ok");
    server.abort();
}

#[tokio::test]
async fn binary_frame_maps_to_bytes() {
    let frames = vec![Message::binary(vec![1u8, 2, 3])];
    let (addr, server) = spawn_frame_push_server("127.0.0.1:0", frames, Duration::ZERO).await;
    let token = CancellationToken::new();
    let (mut consumer, mut rx) = start_client_consumer(
        addr.port(),
        Arc::new(NoopRuntimeObservability),
        &token,
        "r1",
        16,
        65536,
    )
    .await;
    let handle = consumer.background_task_handle().expect("handle present");

    let env = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("envelope within 2s")
        .expect("route channel open");
    assert_eq!(
        env.exchange.input.header("CamelWsMessageType"),
        Some(&serde_json::Value::String("binary".into())),
        "binary frame must carry the binary type header"
    );
    match &env.exchange.input.body {
        CamelBody::Bytes(b) => assert_eq!(&b[..], &[1u8, 2, 3]),
        other => panic!("expected Bytes body, got {other:?}"),
    }

    token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    server.abort();
}

#[tokio::test]
async fn ping_pong_transparent() {
    let frames = vec![
        Message::Ping(vec![].into()),
        Message::Pong(vec![].into()),
        Message::text("only"),
    ];
    let (addr, server) = spawn_frame_push_server("127.0.0.1:0", frames, Duration::ZERO).await;
    let token = CancellationToken::new();
    let (mut consumer, mut rx) = start_client_consumer(
        addr.port(),
        Arc::new(NoopRuntimeObservability),
        &token,
        "r1",
        16,
        65536,
    )
    .await;
    let handle = consumer.background_task_handle().expect("handle present");

    let env = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("envelope within 2s")
        .expect("route channel open");
    match &env.exchange.input.body {
        CamelBody::Text(s) => assert_eq!(s, "only", "the single envelope is the text frame"),
        other => panic!("expected Text body, got {other:?}"),
    }

    // Exactly 1 envelope: after cancel the loop exits, dropping the last
    // sender, so recv must return None with no further envelope.
    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("task must exit within 1s")
        .expect("no join error");
    result.expect("task must exit Ok");
    assert!(
        rx.recv().await.is_none(),
        "ping/pong frames must not produce envelopes"
    );
    server.abort();
}

#[tokio::test]
async fn oversized_frame_dropped_flow_continues() {
    let frames = vec![Message::text("x".repeat(2048)), Message::text("0123456789")];
    let (addr, server) = spawn_frame_push_server("127.0.0.1:0", frames, Duration::ZERO).await;
    let token = CancellationToken::new();
    let metrics = Arc::new(RecordingMetrics::new());
    let runtime: Arc<dyn RuntimeObservability> = Arc::new(CountingRuntime {
        metrics: Arc::clone(&metrics),
    });
    let (mut consumer, mut rx) =
        start_client_consumer(addr.port(), runtime, &token, "r", 16, 1024).await;
    let handle = consumer.background_task_handle().expect("handle present");

    let env = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("envelope within 2s")
        .expect("route channel open");
    match &env.exchange.input.body {
        CamelBody::Text(s) => assert_eq!(s, "0123456789", "only the small frame flows"),
        other => panic!("expected Text body, got {other:?}"),
    }
    // The oversized frame produced no envelope: after cancel the channel
    // closes empty.
    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("task must exit within 1s")
        .expect("no join error");
    result.expect("task must exit Ok");
    assert!(rx.recv().await.is_none(), "no second envelope expected");

    assert_eq!(
        *metrics.errors.lock().unwrap(),
        vec![("r".to_string(), "ws_client_consumer".to_string())],
        "oversized drop must record exactly one error metric"
    );
    server.abort();
}

#[tokio::test]
async fn backpressure_pauses_reads() {
    let frames = vec![Message::text("aaa"), Message::text("bbb")];
    let (addr, server) = spawn_frame_push_server("127.0.0.1:0", frames, Duration::ZERO).await;
    let token = CancellationToken::new();
    // Capacity 1, receiver NOT drained: the second send must block.
    let (mut consumer, mut rx) = start_client_consumer(
        addr.port(),
        Arc::new(NoopRuntimeObservability),
        &token,
        "r1",
        1,
        65536,
    )
    .await;
    let handle = consumer.background_task_handle().expect("handle present");

    // Both socket writes complete into kernel buffers while the loop is
    // blocked delivering the first envelope into the full channel.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let env1 = rx.try_recv().expect("first envelope buffered");
    match &env1.exchange.input.body {
        CamelBody::Text(s) => assert_eq!(s, "aaa"),
        other => panic!("expected Text body, got {other:?}"),
    }
    match rx.try_recv() {
        Err(TryRecvError::Empty) => {}
        Ok(_) => panic!("loop must be blocked in send with a full channel"),
        Err(e) => panic!("unexpected try_recv error: {e}"),
    }

    // Drain: the blocked send completes and the second envelope arrives.
    let env2 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("second envelope within 2s of draining")
        .expect("route channel open");
    match &env2.exchange.input.body {
        CamelBody::Text(s) => assert_eq!(s, "bbb", "reads resumed after drain"),
        other => panic!("expected Text body, got {other:?}"),
    }
    assert!(
        rx.try_recv().is_err(),
        "exactly two envelopes must have been dispatched"
    );

    token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    server.abort();
}

#[tokio::test]
async fn shutdown_while_backpressured() {
    let frames = vec![Message::text("aaa"), Message::text("bbb")];
    let (addr, server) = spawn_frame_push_server("127.0.0.1:0", frames, Duration::ZERO).await;
    let token = CancellationToken::new();
    let (mut consumer, _rx) = start_client_consumer(
        addr.port(),
        Arc::new(NoopRuntimeObservability),
        &token,
        "r1",
        1,
        65536,
    )
    .await;
    let handle = consumer.background_task_handle().expect("handle present");

    // Let the loop block in send on a full channel, then cancel: the
    // task must still exit Ok within 1s (cancel wins over the send).
    tokio::time::sleep(Duration::from_millis(200)).await;
    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("task must exit within 1s even while backpressured")
        .expect("no join error");
    result.expect("task must exit Ok on shutdown");
    server.abort();
}

#[tokio::test]
async fn frame_metrics_via_facade() {
    let frames = vec![Message::text("a"), Message::text("b")];
    let (addr, server) = spawn_frame_push_server("127.0.0.1:0", frames, Duration::ZERO).await;
    let token = CancellationToken::new();
    let metrics = Arc::new(RecordingMetrics::new());
    let runtime: Arc<dyn RuntimeObservability> = Arc::new(CountingRuntime {
        metrics: Arc::clone(&metrics),
    });
    let (mut consumer, mut rx) =
        start_client_consumer(addr.port(), runtime, &token, "r1", 16, 65536).await;
    let handle = consumer.background_task_handle().expect("handle present");

    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("envelope within 2s")
            .expect("route channel open");
    }
    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("task must exit within 1s")
        .expect("no join error");
    result.expect("task must exit Ok");

    assert_eq!(
        *metrics.component_ops.lock().unwrap(),
        vec![
            ("ws".to_string(), "frame".to_string(), "success".to_string()),
            ("ws".to_string(), "frame".to_string(), "success".to_string()),
        ],
        "each delivered frame observes one ws/frame/success via the facade; \
         connect must emit no raw component operation"
    );
    server.abort();
}

#[tokio::test]
async fn dispatch_failure_emits_both() {
    // Delay the push so the receiver is provably gone before the frame
    // arrives and the send fails.
    let frames = vec![Message::text("doomed")];
    let (addr, server) =
        spawn_frame_push_server("127.0.0.1:0", frames, Duration::from_millis(300)).await;
    let token = CancellationToken::new();
    let metrics = Arc::new(RecordingMetrics::new());
    let runtime: Arc<dyn RuntimeObservability> = Arc::new(CountingRuntime {
        metrics: Arc::clone(&metrics),
    });
    let (mut consumer, rx) =
        start_client_consumer(addr.port(), runtime, &token, "r9", 16, 65536).await;
    let handle = consumer.background_task_handle().expect("handle present");
    drop(rx);

    let result = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("task must exit after the send fails")
        .expect("no join error");
    let err = result.expect_err("dispatch failure must surface an error");
    assert!(
        matches!(err, camel_component_api::CamelError::ChannelClosed),
        "expected ChannelClosed, got: {err}"
    );

    assert_eq!(
        *metrics.component_ops.lock().unwrap(),
        vec![("ws".to_string(), "frame".to_string(), "failure".to_string())],
        "exactly one ws/frame/failure component operation"
    );
    assert_eq!(
        *metrics.errors.lock().unwrap(),
        vec![
            // The facade's failure emission (never lever-gated,
            // camel-api ComponentMetrics::observe) plus the loop's own
            // category-(b′) error-family entry.
            ("ws".to_string(), "e:ws:frame".to_string()),
            ("r9".to_string(), "ws_client_consumer".to_string()),
        ],
        "exactly one facade error emission and one dispatch-failure metric"
    );
    server.abort();
}

// --- Task 1.5: reconnect loop and exhaustion ---

/// Reconnect fixture: connection 1 is handshaked, pushed `first`, and held
/// open until `drop_gate` fires, then dropped abruptly (client-side EOF).
/// Connection 2's TCP connect is accepted but its HANDSHAKE is withheld
/// until `resume_gate` fires — no `Connected` can publish in between, so
/// the client's `Reconnecting` watch state is stable until the test
/// releases it.
async fn spawn_gated_reconnect_server(
    bind: &str,
    first: Message,
    second: Message,
    drop_gate: tokio::sync::oneshot::Receiver<()>,
    resume_gate: tokio::sync::oneshot::Receiver<()>,
) -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        // Connection 1: handshake, push one frame, hold until told to drop.
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = match tokio_tungstenite::accept_async(stream).await {
            Ok(ws) => ws,
            Err(_) => return,
        };
        if ws.send(first).await.is_err() {
            return;
        }
        if drop_gate.await.is_err() {
            return;
        }
        drop(ws); // abrupt EOF: forces the client's reconnect path
        // Connection 2: accept the TCP connect, withhold the handshake.
        let (stream, _) = listener.accept().await.unwrap();
        if resume_gate.await.is_err() {
            return;
        }
        let mut ws = match tokio_tungstenite::accept_async(stream).await {
            Ok(ws) => ws,
            Err(_) => return,
        };
        if ws.send(second).await.is_err() {
            return;
        }
        // Hold the second connection open until the peer goes away.
        while let Some(Ok(msg)) = ws.next().await {
            if msg.is_close() {
                break;
            }
        }
    });
    (addr, handle)
}

/// Server variant used for death-after-first-connection tests: accept one
/// connection, complete the handshake, hold it briefly so `start()` finishes,
/// then drop BOTH the connection and the listener — reconnect attempts must
/// find nothing listening.
async fn spawn_dying_ws_server(bind: &str) -> (u16, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws = match tokio_tungstenite::accept_async(stream).await {
            Ok(ws) => ws,
            Err(_) => return,
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(ws);
        drop(listener);
    });
    (port, handle)
}

#[tokio::test]
async fn disconnect_reconnect_resumes_delivery() {
    let (drop_tx, drop_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    let (addr, server) = spawn_gated_reconnect_server(
        "127.0.0.1:0",
        Message::text("one"),
        Message::text("two"),
        drop_rx,
        resume_rx,
    )
    .await;
    let (state_tx, mut state_rx) = watch::channel(ClientConnState::Connecting);
    let mut cfg = client_cfg(addr.port(), Vec::new());
    cfg.inner.reconnect_policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 5,
        initial_delay: Duration::from_millis(20),
        ..NetworkRetryPolicy::default()
    };
    let mut consumer = WsClientConsumer::new(cfg, Arc::new(NoopRuntimeObservability), state_tx);
    let token = CancellationToken::new();
    let (ctx, mut rx) = make_ctx_with_rx(token.clone(), "r1", 16);
    consumer.start(ctx).await.expect("start must succeed");
    let handle = consumer.background_task_handle().expect("handle present");

    // First delivery on connection 1.
    let env1 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("first envelope within 2s")
        .expect("route channel open");
    match &env1.exchange.input.body {
        CamelBody::Text(s) => assert_eq!(s, "one"),
        other => panic!("expected Text body, got {other:?}"),
    }

    // Force the drop; the client must observe EOF, publish Reconnecting,
    // and park in its (gated) reconnect attempt.
    drop_tx.send(()).expect("server task alive");
    tokio::time::timeout(Duration::from_secs(2), async {
        while *state_rx.borrow_and_update() != ClientConnState::Reconnecting {
            state_rx.changed().await.expect("conn_state_tx alive");
        }
    })
    .await
    .expect("Reconnecting must be observed after the drop");

    // Release the withheld handshake: the reconnect attempt succeeds.
    resume_tx.send(()).expect("server task alive");
    tokio::time::timeout(Duration::from_secs(2), async {
        while *state_rx.borrow_and_update() != ClientConnState::Connected {
            state_rx.changed().await.expect("conn_state_tx alive");
        }
    })
    .await
    .expect("Connected must be observed after reconnect");

    // Second delivery resumes on connection 2.
    let env2 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("second envelope within 2s")
        .expect("route channel open");
    match &env2.exchange.input.body {
        CamelBody::Text(s) => assert_eq!(s, "two", "delivery resumed after reconnect"),
        other => panic!("expected Text body, got {other:?}"),
    }
    assert!(
        rx.try_recv().is_err(),
        "exactly two envelopes must have been dispatched"
    );

    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("task must exit within 1s of cancellation")
        .expect("no join error");
    result.expect("task must exit Ok on shutdown");
    server.abort();
}

#[tokio::test]
async fn reconnect_exhaustion_returns_err() {
    let (port, server) = spawn_dying_ws_server("127.0.0.1:0").await;
    let (state_tx, state_rx) = watch::channel(ClientConnState::Connecting);
    // client_cfg policy: max_attempts=2, initial_delay=10ms.
    let mut consumer = WsClientConsumer::new(
        client_cfg(port, Vec::new()),
        Arc::new(NoopRuntimeObservability),
        state_tx,
    );
    let token = CancellationToken::new();
    let (ctx, _rx) = make_ctx_with_rx(token.clone(), "r1", 16);
    consumer
        .start(ctx)
        .await
        .expect("initial connect must succeed");
    let handle = consumer.background_task_handle().expect("handle present");

    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("task must terminate after reconnect exhaustion")
        .expect("no join error");
    let err = result.expect_err("reconnect exhaustion must surface an error");
    assert!(
        err.to_string().contains("exhausted"),
        "expected the exhaustion error, got: {err}"
    );
    assert_eq!(
        *state_rx.borrow(),
        ClientConnState::Exhausted,
        "watch must end Exhausted"
    );
    server.abort();
}

#[tokio::test]
async fn shutdown_during_reconnect_backoff() {
    let (port, server) = spawn_dying_ws_server("127.0.0.1:0").await;
    let (state_tx, mut state_rx) = watch::channel(ClientConnState::Connecting);
    let mut cfg = client_cfg(port, Vec::new());
    cfg.inner.reconnect_policy.initial_delay = Duration::from_secs(5);
    let mut consumer = WsClientConsumer::new(cfg, Arc::new(NoopRuntimeObservability), state_tx);
    let token = CancellationToken::new();
    let (ctx, _rx) = make_ctx_with_rx(token.clone(), "r1", 16);
    consumer
        .start(ctx)
        .await
        .expect("initial connect must succeed");
    let handle = consumer.background_task_handle().expect("handle present");

    // Deterministic: the reconnect sequence has begun (it cannot complete —
    // nothing is listening and the backoff is 5s).
    tokio::time::timeout(Duration::from_secs(2), async {
        while *state_rx.borrow_and_update() != ClientConnState::Reconnecting {
            state_rx.changed().await.expect("conn_state_tx alive");
        }
    })
    .await
    .expect("Reconnecting must be observed after the drop");

    token.cancel();
    let result = tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("cancel must preempt the 5s backoff (exit within 500ms)")
        .expect("no join error");
    result.expect("task must exit Ok on shutdown during reconnect");
    server.abort();
}

#[tokio::test]
async fn exhaustion_error_recorded_once() {
    let port = unreachable_port();
    let metrics = Arc::new(RecordingMetrics::new());
    let runtime: Arc<dyn RuntimeObservability> = Arc::new(CountingRuntime {
        metrics: Arc::clone(&metrics),
    });
    let (state_tx, state_rx) = watch::channel(ClientConnState::Connecting);
    let mut cfg = client_cfg(port, Vec::new());
    cfg.inner.reconnect_policy.max_attempts = 3;
    let mut consumer = WsClientConsumer::new(cfg, runtime, state_tx);
    let (ctx, _receiver) = make_ctx(CancellationToken::new());
    let _err = consumer
        .start(ctx)
        .await
        .expect_err("unreachable start must fail");

    assert_eq!(
        metrics.attempts.lock().unwrap().len(),
        3,
        "exactly three connect attempts must be recorded"
    );
    assert_eq!(
        *metrics.errors.lock().unwrap(),
        vec![("connect".to_string(), "e:ws:connect".to_string())],
        "exactly one connect-exhaustion error must be recorded"
    );
    assert!(
        metrics.component_ops.lock().unwrap().is_empty(),
        "connect must emit no raw record_component_operation calls"
    );
    assert_eq!(*state_rx.borrow(), ClientConnState::Exhausted);
}

// --- Task 1.7: TLS (wss) client consumer ---

/// Absolute path of a committed TLS test fixture (generated once with
/// openssl at development time, see `tests/fixtures/`).
fn tls_fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Client TLS config whose root store contains ONLY the fixture CA, built
/// on the ring provider explicitly (immune to provider-feature unification
/// in the workspace).
fn fixture_client_config() -> rustls::ClientConfig {
    let ca_pem = std::fs::read(tls_fixture_path("ws-test-ca.crt"))
        .expect("fixture CA cert must be readable");
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &ca_pem[..]) {
        roots
            .add(cert.expect("fixture CA must parse as a certificate"))
            .expect("fixture CA must be accepted as a root");
    }
    rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("default protocol versions must be supported")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// Server TLS config loaded from the committed fixture cert/key via
/// `rustls-pemfile`.
fn fixture_server_config() -> rustls::ServerConfig {
    let cert_pem = std::fs::read(tls_fixture_path("ws-test-server.crt"))
        .expect("fixture server cert must be readable");
    let key_pem = std::fs::read(tls_fixture_path("ws-test-server.key"))
        .expect("fixture server key must be readable");
    let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_pem[..])
        .map(|c| c.expect("fixture server cert must parse"))
        .collect();
    let key = rustls_pemfile::private_key(&mut &key_pem[..])
        .expect("fixture server key must parse")
        .expect("fixture server key must be present");
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("default protocol versions must be supported")
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("fixture server cert+key must form a valid chain")
}

/// rustls-terminating push server: TLS-accepts ONE connection, upgrades it
/// to WebSocket, pushes `frames`, then holds the connection until the peer
/// disconnects.
async fn spawn_tls_push_server(bind: &str, frames: Vec<Message>) -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(fixture_server_config()));
    let handle = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let tls = match acceptor.accept(stream).await {
                Ok(tls) => tls,
                Err(_) => return,
            };
            let mut ws = match tokio_tungstenite::accept_async(tls).await {
                Ok(ws) => ws,
                Err(_) => return,
            };
            for frame in frames {
                if ws.send(frame).await.is_err() {
                    return;
                }
            }
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_close() {
                    break;
                }
            }
        }
    });
    (addr, handle)
}

#[tokio::test]
async fn wss_client_consumer_frames_flow() {
    let frames = vec![Message::text("tls-hello")];
    let (addr, server) = spawn_tls_push_server("127.0.0.1:0", frames).await;
    let cfg = WsEndpointConfig {
        scheme: "wss".into(),
        host: "localhost".into(),
        port: addr.port(),
        path: "/feed".into(),
        connect_timeout: Duration::from_secs(2),
        reconnect_policy: NetworkRetryPolicy {
            enabled: true,
            max_attempts: 2,
            initial_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        },
        ..WsEndpointConfig::default()
    }
    .client_config();
    let (state_tx, _state_rx) = watch::channel(ClientConnState::Connecting);
    let mut consumer = WsClientConsumer::new(cfg, Arc::new(NoopRuntimeObservability), state_tx)
        .with_connector(tokio_tungstenite::Connector::Rustls(Arc::new(
            fixture_client_config(),
        )));
    let token = CancellationToken::new();
    let (ctx, mut rx) = make_ctx_with_rx(token.clone(), "r1", 16);
    consumer.start(ctx).await.expect("wss start must succeed");

    let env = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("envelope within 2s")
        .expect("route channel open");
    assert_eq!(
        env.exchange.input.header("CamelWsMessageType"),
        Some(&serde_json::Value::String("text".into())),
        "TLS server text frame must carry the text type header"
    );
    match &env.exchange.input.body {
        CamelBody::Text(s) => assert_eq!(s, "tls-hello", "TLS server frame must flow"),
        other => panic!("expected Text body, got {other:?}"),
    }

    token.cancel();
    server.abort();
}
