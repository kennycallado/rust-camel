//! End-to-end decode-limit coverage for bridge→Rust `JmsMessage` streams.
//!
//! Spawns an in-process tonic server implementing the bridge `Subscribe`
//! stream (the `spawn_mock_bridge` pattern from
//! `crates/components/camel-cxf/tests/support/mock_bridge.rs`) and connects
//! through the production-path client constructor. A ~15 MiB body sits below
//! the Java-side 16 MiB body cap but exceeds tonic's 4 MiB default decode
//! limit, so intact delivery is only possible when the bridge decode limit is
//! applied.

use std::collections::HashSet;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_component_api::test_support::timeout_wait;
use camel_component_api::{Body, Exchange, Message, Value};
use futures::Stream;
use tonic::{Request, Response, Status};
use tower::Service;

use crate::component::bridge_service_client;
use crate::config::JmsEndpointConfig;
use crate::producer::JmsProducer;
use crate::proto::{
    CompletionRequest, CompletionResponse, HealthRequest, HealthResponse, JmsMessage, SendRequest,
    SendResponse, SubscribeRequest,
    bridge_service_server::{BridgeService, BridgeServiceServer},
};

const NEAR_CAP_BODY_SIZE: usize = 15 * 1024 * 1024;

/// Restores an environment variable to its prior value when dropped.
///
/// Shared by the `component.rs` and `consumer.rs` test modules so every test
/// that mutates the process environment while holding [`crate::BRIDGE_ENV_LOCK`]
/// uses one restore implementation.
pub(crate) struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(v) = &self.prev {
            // SAFETY: restoring process env in test scope.
            unsafe { std::env::set_var(self.key, v) };
        } else {
            // SAFETY: restoring process env in test scope.
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

/// Captures an environment variable's prior value and restores it on drop.
pub(crate) fn env_guard(key: &'static str) -> EnvGuard {
    EnvGuard {
        key,
        prev: std::env::var_os(key),
    }
}

/// The two copies of the JMS bridge wire contract. Task 3.1 keeps them
/// byte-identical except the Java `option` lines in the bridge copy.
const CRATE_PROTO: &str = include_str!("../proto/jms_bridge.proto");
const BRIDGE_PROTO: &str = include_str!("../../../../bridges/jms/src/main/proto/jms_bridge.proto");

/// Apply the same normalization as the task's `diff` command: drop `option `
/// lines and blank lines.
fn normalize_proto_wire(src: &str) -> String {
    src.lines()
        .filter(|line| !line.starts_with("option ") && !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn protos_have_identical_wire_additions() {
    assert_eq!(
        normalize_proto_wire(CRATE_PROTO),
        normalize_proto_wire(BRIDGE_PROTO),
        "crate-local and Java bridge protos must be identical except option lines"
    );
}

/// Deterministic non-uniform pattern so the assertion compares real bytes
/// instead of an all-zero or all-same payload.
fn near_cap_body() -> Vec<u8> {
    (0..NEAR_CAP_BODY_SIZE).map(|i| (i % 251) as u8).collect()
}

#[derive(Clone)]
struct MockJmsBridge {
    subscribe_message: Option<JmsMessage>,
    active_ids: Arc<Mutex<HashSet<String>>>,
    captured_send: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>,
    captured_send_request: Arc<Mutex<Option<SendRequest>>>,
    captured_subscribe: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>,
    captured_health: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>,
}

#[async_trait::async_trait]
impl BridgeService for MockJmsBridge {
    async fn send(&self, request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        *self
            .captured_send
            .lock()
            .expect("captured_send mutex poisoned") = Some(request.metadata().clone());
        *self
            .captured_send_request
            .lock()
            .expect("captured_send_request mutex poisoned") = Some(request.into_inner());
        Ok(Response::new(SendResponse::default()))
    }

    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

    async fn subscribe(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        *self
            .captured_subscribe
            .lock()
            .expect("captured_subscribe mutex poisoned") = Some(request.metadata().clone());
        let subscription_id = request.into_inner().subscription_id;
        if !self
            .active_ids
            .lock()
            .expect("active_ids mutex poisoned")
            .insert(subscription_id)
        {
            return Err(Status::already_exists("subscription_id already active"));
        }
        let msg = self
            .subscribe_message
            .clone()
            .ok_or_else(|| Status::internal("no prepared subscribe message"))?;
        Ok(Response::new(Box::pin(futures::stream::once(async move {
            Ok(msg)
        }))))
    }

    async fn health(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        *self
            .captured_health
            .lock()
            .expect("captured_health mutex poisoned") = Some(request.metadata().clone());
        Ok(Response::new(HealthResponse {
            healthy: true,
            broker_connected: true,
            message: "ok".to_string(),
            status: 0,
            age_ms: 0,
            ..Default::default()
        }))
    }

    async fn complete(
        &self,
        _request: Request<CompletionRequest>,
    ) -> Result<Response<CompletionResponse>, Status> {
        // Task 3.1: the mock bridge accepts every completion. Real settlement
        // semantics arrive with the Java owner thread in task 3.2.
        Ok(Response::new(CompletionResponse { accepted: true }))
    }
}

/// Metadata captured by the mock bridge per RPC shape, so tests can assert on
/// the exact gRPC metadata the client attached to each request.
#[cfg_attr(not(feature = "otel"), allow(dead_code))]
struct CapturedMetadata {
    port: u16,
    send: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>,
    send_request: Arc<Mutex<Option<SendRequest>>>,
    subscribe: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>,
    health: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>,
}

async fn spawn_mock_bridge(message: JmsMessage) -> std::io::Result<CapturedMetadata> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let captured_send = Arc::new(Mutex::new(None));
    let captured_send_request = Arc::new(Mutex::new(None));
    let captured_subscribe = Arc::new(Mutex::new(None));
    let captured_health = Arc::new(Mutex::new(None));

    let mock_send = captured_send.clone();
    let mock_send_request = captured_send_request.clone();
    let mock_subscribe = captured_subscribe.clone();
    let mock_health = captured_health.clone();

    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(BridgeServiceServer::new(MockJmsBridge {
                subscribe_message: Some(message),
                active_ids: Arc::new(Mutex::new(HashSet::new())),
                captured_send: mock_send,
                captured_send_request: mock_send_request,
                captured_subscribe: mock_subscribe,
                captured_health: mock_health,
            }))
            .serve_with_incoming(incoming)
            .await;
    });

    // Give the spawned accept loop a moment before the client dials.
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(CapturedMetadata {
        port,
        send: captured_send,
        send_request: captured_send_request,
        subscribe: captured_subscribe,
        health: captured_health,
    })
}

#[tokio::test]
async fn near_cap_body_decodes_end_to_end() {
    let body = near_cap_body();
    let message = JmsMessage {
        message_id: "msg-near-cap".to_string(),
        correlation_id: String::new(),
        timestamp: 0,
        destination: "queue.decode.limit.test".to_string(),
        body,
        headers: std::collections::HashMap::new(),
        content_type: "application/octet-stream".to_string(),
        ..Default::default()
    };

    let captured = spawn_mock_bridge(message).await.expect("spawn mock bridge");
    let port = captured.port;

    // Same channel-construction shape the bridge pool produces, connected to
    // the in-process mock instead of the Java bridge process.
    let channel = timeout_wait(
        "bridge connect",
        Duration::from_secs(30),
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .expect("valid endpoint uri")
            .connect(),
    )
    .await
    .expect("connect to mock bridge");
    let mut client = bridge_service_client(channel);

    let resp = client
        .subscribe(SubscribeRequest {
            destination: "queue.decode.limit.test".to_string(),
            subscription_id: "near-cap-test".to_string(),
            message_selector: String::new(),
        })
        .await
        .expect("subscribe accepted");

    let mut stream = resp.into_inner();
    let delivered = stream
        .message()
        .await
        .expect("jms message decodes")
        .expect("stream yields one jms message");
    assert_eq!(delivered.body.len(), NEAR_CAP_BODY_SIZE);
    assert_eq!(
        delivered.body,
        near_cap_body(),
        "body must arrive byte-intact below the 16 MiB cap"
    );
}

#[tokio::test]
async fn content_type_round_trips_through_stream() {
    let message = JmsMessage {
        content_type: "application/xml".to_string(),
        body: b"<a/>".to_vec(),
        ..Default::default()
    };

    let captured = spawn_mock_bridge(message).await.expect("spawn mock bridge");
    let port = captured.port;

    let channel = timeout_wait(
        "bridge connect",
        Duration::from_secs(30),
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .expect("valid endpoint uri")
            .connect(),
    )
    .await
    .expect("connect to mock bridge");
    let mut client = bridge_service_client(channel);

    let resp = client
        .subscribe(SubscribeRequest {
            destination: "queue.ct.test".to_string(),
            subscription_id: "ct-round-trip".to_string(),
            message_selector: String::new(),
        })
        .await
        .expect("subscribe accepted");

    let mut stream = resp.into_inner();
    let delivered = stream
        .message()
        .await
        .expect("jms message decodes")
        .expect("stream yields one jms message");
    assert_eq!(delivered.content_type, "application/xml");
    assert_eq!(delivered.body, b"<a/>", "body must arrive byte-intact");
}

#[tokio::test]
async fn duplicate_subscription_id_surfaces_already_exists() {
    let message = JmsMessage {
        content_type: "application/octet-stream".to_string(),
        body: b"dup".to_vec(),
        ..Default::default()
    };

    let captured = spawn_mock_bridge(message).await.expect("spawn mock bridge");
    let port = captured.port;

    let channel = timeout_wait(
        "bridge connect",
        Duration::from_secs(30),
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .expect("valid endpoint uri")
            .connect(),
    )
    .await
    .expect("connect to mock bridge");
    let mut first = bridge_service_client(channel.clone());
    let mut second = bridge_service_client(channel);

    first
        .subscribe(SubscribeRequest {
            destination: "queue.dup.test".to_string(),
            subscription_id: "dup-test".to_string(),
            message_selector: String::new(),
        })
        .await
        .expect("first subscribe accepted");

    let err = match second
        .subscribe(SubscribeRequest {
            destination: "queue.dup.test".to_string(),
            subscription_id: "dup-test".to_string(),
            message_selector: String::new(),
        })
        .await
    {
        Ok(_) => panic!("duplicate subscription_id must be rejected"),
        Err(e) => e,
    };
    assert_eq!(err.code(), tonic::Code::AlreadyExists);
}

#[test]
fn bridge_decode_limit_above_cap() {
    assert!(
        crate::component::bridge_decode_limit() > 19 * 1024 * 1024,
        "decode limit must exceed the 19 MiB JMS_MAX_BODY_BYTES ceiling"
    );
}

/// Task 3.1: the generated proto module exposes the completion symbols and the
/// mock bridge service implements the new `Complete` RPC.
#[tokio::test]
async fn mock_services_implement_complete() {
    use crate::proto::CompletionOutcome;

    // Compile-time symbols: request default, enum variant, response type.
    let request = CompletionRequest {
        outcome: CompletionOutcome::Commit as i32,
        ..Default::default()
    };

    let mock = MockJmsBridge {
        subscribe_message: None,
        active_ids: Arc::new(Mutex::new(HashSet::new())),
        captured_send: Arc::new(Mutex::new(None)),
        captured_send_request: Arc::new(Mutex::new(None)),
        captured_subscribe: Arc::new(Mutex::new(None)),
        captured_health: Arc::new(Mutex::new(None)),
    };
    let response = BridgeService::complete(&mock, Request::new(request))
        .await
        .expect("mock complete must be accepted");
    assert!(
        response.into_inner().accepted,
        "mock bridge must accept the completion"
    );
}

/// True when `v` is a well-formed W3C traceparent: version `00`, a 32-hex-char
/// trace-id, a 16-hex-char span-id, and a 2-hex-char flags field, all lowercase.
#[cfg(all(test, feature = "otel"))]
fn well_formed_traceparent(v: &str) -> bool {
    let parts: Vec<&str> = v.split('-').collect();
    parts.len() == 4
        && parts[0] == "00"
        && parts[1].len() == 32
        && parts[2].len() == 16
        && parts[3].len() == 2
        && parts[1..].iter().all(|p| {
            p.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
}

/// The traceparent the tests attach as the active context; the interceptor
/// under test must forward it verbatim on every RPC shape.
#[cfg(all(test, feature = "otel"))]
const ACTIVE_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

#[cfg(all(test, feature = "otel"))]
#[tokio::test]
async fn traceparent_injected_on_every_rpc_shape() {
    use std::collections::HashMap;

    // Build an active OTel context from a W3C traceparent header (camel-otel
    // propagation idiom, mirroring camel-http's otel tests) and keep the guard
    // alive for the whole test body so every RPC below runs under it.
    let mut headers = HashMap::new();
    headers.insert("traceparent".to_string(), ACTIVE_TRACEPARENT.to_string());
    let ctx = camel_otel::propagation::extract_context(&headers);
    let _guard = ctx.attach();

    let message = JmsMessage {
        content_type: "application/octet-stream".to_string(),
        body: vec![1],
        ..Default::default()
    };
    let captured = spawn_mock_bridge(message).await.expect("spawn mock bridge");

    let channel = timeout_wait(
        "bridge connect",
        Duration::from_secs(30),
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", captured.port))
            .expect("valid endpoint uri")
            .connect(),
    )
    .await
    .expect("connect to mock bridge");
    let mut client = bridge_service_client(channel);

    client
        .send(SendRequest {
            destination: "queue.tp.test".into(),
            body: vec![1],
            headers: HashMap::new(),
            content_type: "application/octet-stream".into(),
        })
        .await
        .expect("send accepted");

    client
        .health(HealthRequest {})
        .await
        .expect("health accepted");

    client
        .subscribe(SubscribeRequest {
            destination: "queue.tp.test".into(),
            subscription_id: "tp-test".into(),
            message_selector: String::new(),
        })
        .await
        .expect("subscribe accepted");

    for (name, slot) in [
        ("send", &captured.send),
        ("health", &captured.health),
        ("subscribe", &captured.subscribe),
    ] {
        let md = slot.lock().expect("metadata mutex poisoned");
        let md = md.as_ref().expect("metadata captured");
        let tp = md
            .get("traceparent")
            .expect("traceparent key present")
            .to_str()
            .expect("traceparent is ascii");
        assert_eq!(
            tp, ACTIVE_TRACEPARENT,
            "{name} rpc must carry the active traceparent"
        );
        assert!(
            well_formed_traceparent(tp),
            "{name} rpc traceparent must be well-formed"
        );
    }
}

#[cfg(all(test, feature = "otel"))]
#[tokio::test]
async fn no_traceparent_without_active_context() {
    use std::collections::HashMap;

    let message = JmsMessage {
        content_type: "application/octet-stream".to_string(),
        body: vec![1],
        ..Default::default()
    };
    let captured = spawn_mock_bridge(message).await.expect("spawn mock bridge");

    let channel = timeout_wait(
        "bridge connect",
        Duration::from_secs(30),
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", captured.port))
            .expect("valid endpoint uri")
            .connect(),
    )
    .await
    .expect("connect to mock bridge");
    let mut client = bridge_service_client(channel);

    client
        .send(SendRequest {
            destination: "queue.tp.test".into(),
            body: vec![1],
            headers: HashMap::new(),
            content_type: "application/octet-stream".into(),
        })
        .await
        .expect("send accepted");

    client
        .health(HealthRequest {})
        .await
        .expect("health accepted");

    client
        .subscribe(SubscribeRequest {
            destination: "queue.tp.test".into(),
            subscription_id: "tp-fallback".into(),
            message_selector: String::new(),
        })
        .await
        .expect("subscribe accepted");

    for (name, slot) in [
        ("send", &captured.send),
        ("health", &captured.health),
        ("subscribe", &captured.subscribe),
    ] {
        let md = slot.lock().expect("metadata mutex poisoned");
        let md = md.as_ref().expect("metadata captured");
        assert!(
            md.get("traceparent").is_none(),
            "{name} rpc must not carry traceparent without an active context"
        );
    }
}

/// QoS headers are written to the exchange *after* header extraction, so an
/// empty allowlist must not drop them even though the user header beside them
/// is dropped. Drives the real `JmsProducer::call` against a mock that captures
/// the actual `SendRequest`, so the assertion is about the wire payload, not a
/// helper's return value.
#[tokio::test]
async fn qos_headers_set_after_extraction() {
    let captured = spawn_mock_bridge(JmsMessage::default())
        .await
        .expect("spawn mock bridge");
    let endpoint =
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", captured.port))
            .expect("valid endpoint uri");
    let channel = tokio::time::timeout(Duration::from_secs(5), endpoint.connect())
        .await
        .expect("connect to mock bridge within 5s")
        .expect("mock bridge connection ok");

    let config =
        JmsEndpointConfig::from_uri("jms:queue:orders?timeToLive=30000&priority=5").unwrap();
    assert!(
        config.jms_header_allowlist.is_empty(),
        "precondition: no allowlist configured"
    );
    let mut producer = JmsProducer::with_concurrency(channel, config, 1);

    let mut ex = Exchange::new(Message::new(Body::Empty));
    ex.input
        .set_header("x-custom", Value::String("val".to_string()));
    let result = producer.call(ex).await.expect("send accepted");

    assert_eq!(
        result.input.header("JMSExpiration").unwrap().as_str(),
        Some("30000")
    );
    assert_eq!(
        result.input.header("JMSPriority").unwrap().as_str(),
        Some("5")
    );

    let sent = captured.send_request.lock().expect("send_request mutex");
    let sent = sent.as_ref().expect("send request captured");
    assert!(
        !sent.headers.contains_key("JMSExpiration") && !sent.headers.contains_key("JMSPriority"),
        "QoS headers must be written after extraction, not sent as properties: {:?}",
        sent.headers
    );
    assert!(
        !sent.headers.contains_key("x-custom"),
        "empty allowlist must drop the user header"
    );
}

/// Credential absence on the wire: a non-empty allowlist naming both a benign
/// header and credential-shaped names forwards only the benign one, and no
/// credential name or value appears in the actual `SendRequest`.
#[tokio::test]
async fn wire_payload_omits_credentials_and_keeps_trace_id() {
    let captured = spawn_mock_bridge(JmsMessage::default())
        .await
        .expect("spawn mock bridge");
    let endpoint =
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", captured.port))
            .expect("valid endpoint uri");
    let channel = tokio::time::timeout(Duration::from_secs(5), endpoint.connect())
        .await
        .expect("connect to mock bridge within 5s")
        .expect("mock bridge connection ok");

    let config = JmsEndpointConfig::from_uri(
        "jms:queue:orders?jmsHeaderAllowlist=password,Authorization,api-key,traceId",
    )
    .unwrap();
    let mut producer = JmsProducer::with_concurrency(channel, config, 1);

    let mut ex = Exchange::new(Message::new(Body::Empty));
    ex.input
        .set_header("password", Value::String("hunter2".to_string()));
    ex.input
        .set_header("Authorization", Value::String("Bearer hunter2".to_string()));
    ex.input
        .set_header("api-key", Value::String("k-hunter2".to_string()));
    ex.input
        .set_header("traceId", Value::String("trace-1".to_string()));
    producer.call(ex).await.expect("send accepted");

    let sent = captured.send_request.lock().expect("send_request mutex");
    let sent = sent.as_ref().expect("send request captured");
    assert_eq!(
        sent.headers.get("traceId"),
        Some(&"trace-1".to_string()),
        "the listed benign header must reach the wire"
    );
    for denied in ["password", "Authorization", "api-key"] {
        assert!(
            !sent.headers.contains_key(denied),
            "denied credential name reached the wire: {denied}"
        );
    }
    assert!(
        !sent.headers.values().any(|v| v.contains("hunter2")),
        "credential value reached the wire: {:?}",
        sent.headers
    );
    let payload = format!("{sent:?}");
    assert!(
        !payload.contains("hunter2"),
        "denied value appears in the serialized SendRequest: {payload}"
    );
}

// ── Task 2.4: scripted health fixtures for the real monitor ──────────────────

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::component::{
    BridgeSlot, BridgeState, JmsBridgePool, ProbeProcessed, RefreshAttempt, ReplacementProcessed,
};

/// One scripted reply for a single health call. The server pops one entry per
/// call; an empty queue answers `healthy=true, broker_connected=true,
/// message="ok"` so a test can enqueue only the exceptional prefix.
pub(crate) enum ScriptedHealth {
    /// Reply with the given response.
    Respond(HealthResponse),
    /// Enter `gate` before replying: signal `entered`, then wait for
    /// `release`, then reply. The test holds `release` to force a timeout.
    Gate {
        response: HealthResponse,
        gate: Arc<ProbeGate>,
    },
    /// Fail the RPC with the given status.
    Error(Status),
}

/// Gate coordinating one delayed health reply between test and server.
pub(crate) struct ProbeGate {
    /// Signalled by the server once the request entered the handler.
    pub entered: Arc<tokio::sync::Notify>,
    /// Signalled by the test (or teardown) to let the handler reply.
    pub release: Arc<tokio::sync::Notify>,
}

/// Builds a fresh gate pair for [`ScriptedHealth::Gate`].
pub(crate) fn probe_gate() -> Arc<ProbeGate> {
    Arc::new(ProbeGate {
        entered: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
    })
}

/// Server-side events for one health call. These coordinate gates only; they
/// are NOT proof that the client monitor processed the response — assertions
/// await monitor acknowledgements instead.
pub(crate) enum HealthEvent {
    Started,
    Completed,
}

/// Owns the scripted health server task and its shutdown path; no detached
/// server survives a test.
pub(crate) struct ScriptedHealthHandle {
    pub port: u16,
    pub calls: Arc<AtomicUsize>,
    pub events: tokio::sync::mpsc::UnboundedReceiver<HealthEvent>,
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    /// Gates entered by in-flight handlers; released on shutdown so no
    /// handler stays blocked.
    gates: Arc<std::sync::Mutex<Vec<Arc<ProbeGate>>>>,
}

impl ScriptedHealthHandle {
    /// Releases every entered gate, signals shutdown, and joins the server
    /// under a bounded timeout, aborting if the bound expires.
    pub async fn shutdown(self) {
        let Self {
            shutdown,
            mut task,
            gates,
            ..
        } = self;
        {
            let entered = gates.lock().expect("gates mutex poisoned");
            for gate in entered.iter() {
                gate.release.notify_one();
            }
        }
        let _ = shutdown.send(());
        if tokio::time::timeout(Duration::from_secs(2), &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

#[derive(Clone)]
struct ScriptedHealthService {
    queue: Arc<std::sync::Mutex<VecDeque<ScriptedHealth>>>,
    calls: Arc<AtomicUsize>,
    events: tokio::sync::mpsc::UnboundedSender<HealthEvent>,
    gates: Arc<std::sync::Mutex<Vec<Arc<ProbeGate>>>>,
}

#[async_trait::async_trait]
impl BridgeService for ScriptedHealthService {
    async fn send(&self, _request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        Err(Status::unimplemented(
            "scripted health fixture serves Health only",
        ))
    }

    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

    async fn subscribe(
        &self,
        _request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        Err(Status::unimplemented(
            "scripted health fixture serves Health only",
        ))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let _ = self.events.send(HealthEvent::Started);
        let entry = self
            .queue
            .lock()
            .expect("script queue mutex poisoned")
            .pop_front()
            .unwrap_or_else(|| {
                ScriptedHealth::Respond(HealthResponse {
                    healthy: true,
                    broker_connected: true,
                    message: "ok".to_string(),
                    status: 0,
                    age_ms: 0,
                    ..Default::default()
                })
            });
        // The gate is applied before the reply; the queue lock is never held
        // across the wait.
        let outcome = match entry {
            ScriptedHealth::Respond(response) => Ok(response),
            ScriptedHealth::Error(status) => Err(status),
            ScriptedHealth::Gate { response, gate } => {
                self.gates
                    .lock()
                    .expect("gates mutex poisoned")
                    .push(Arc::clone(&gate));
                gate.entered.notify_one();
                gate.release.notified().await;
                Ok(response)
            }
        };
        let _ = self.events.send(HealthEvent::Completed);
        outcome.map(Response::new)
    }

    async fn complete(
        &self,
        _request: Request<CompletionRequest>,
    ) -> Result<Response<CompletionResponse>, Status> {
        Err(Status::unimplemented(
            "scripted health fixture serves Health only",
        ))
    }
}

/// Spawns the scripted health server. Each call pops one `ScriptedHealth`
/// entry and increments `calls`.
pub(crate) async fn spawn_scripted_health(script: Vec<ScriptedHealth>) -> ScriptedHealthHandle {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind scripted health listener");
    let port = listener.local_addr().expect("local addr").port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let calls = Arc::new(AtomicUsize::new(0));
    let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let gates = Arc::new(std::sync::Mutex::new(Vec::new()));
    let queue = Arc::new(std::sync::Mutex::new(VecDeque::from(script)));
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let calls_for_service = Arc::clone(&calls);
    let gates_for_service = Arc::clone(&gates);
    let task = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(BridgeServiceServer::new(ScriptedHealthService {
                queue,
                calls: calls_for_service,
                events: events_tx,
                gates: gates_for_service,
            }))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    ScriptedHealthHandle {
        port,
        calls,
        events: events_rx,
        shutdown: shutdown_tx,
        task,
        gates,
    }
}

/// Connects a real (handshaked) tonic channel to the scripted server.
pub(crate) async fn connect_scripted(port: u16) -> tonic::transport::Channel {
    let endpoint = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("valid scripted endpoint");
    tokio::time::timeout(Duration::from_secs(5), endpoint.connect())
        .await
        .expect("scripted server connect within 5s")
        .expect("connected scripted channel")
}

// ── Task 1.2: selector-gate slot fixtures (shared by component/consumer tests) ──

/// Hand-built Ready slot pointed at the in-process mock plus a fast health
/// monitor: the selector-gate tests need real monitor probes without a
/// process launch.
pub(crate) async fn selector_monitor_fixture(port: u16) -> (Arc<JmsBridgePool>, Arc<BridgeSlot>) {
    let endpoint = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("valid endpoint uri");
    let channel = tokio::time::timeout(Duration::from_secs(5), endpoint.connect())
        .await
        .expect("connect must not time out")
        .expect("connect to mock bridge");
    let pool = Arc::new(
        JmsBridgePool::from_config(crate::config::JmsPoolConfig {
            health_check_interval_ms: 20,
            health_timeout_ms: 500,
            health_failure_threshold: 5,
            ..crate::config::JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                crate::BrokerType::Generic,
            )
        })
        .expect("selector monitor pool config must be valid"),
    );
    let (state_tx, state_rx) = tokio::sync::watch::channel(BridgeState::Ready { channel });
    let slot = Arc::new(BridgeSlot {
        name: "default".to_string(),
        broker_url: "tcp://localhost:61616".to_string(),
        broker_type: crate::BrokerType::Generic,
        credentials: None,
        state_rx,
        state_tx,
        process: Arc::new(tokio::sync::Mutex::new(None)),
        health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
        probe_wake: Arc::new(tokio::sync::Notify::new()),
        last_broker_failure: std::sync::Mutex::new(None),
        selector_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    pool.slots.insert("default".to_string(), Arc::clone(&slot));
    pool.spawn_health_monitor(Arc::clone(&slot)).await;
    (pool, slot)
}

/// Waits (bounded) until the slot publishes `Parked`, returning the reason.
pub(crate) async fn wait_parked(slot: &Arc<BridgeSlot>) -> String {
    let mut rx = slot.state_rx.clone();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let BridgeState::Parked(reason) = &*rx.borrow() {
                return reason.clone();
            }
            rx.changed().await.expect("slot state channel open");
        }
    })
    .await
    .expect("slot must park within 5s")
}

/// Stops the monitor and any installed child of a test slot.
pub(crate) async fn shutdown_test_slot(slot: &Arc<BridgeSlot>) {
    let _ = slot.state_tx.send_replace(BridgeState::Stopped);
    if let Some(mut handle) = slot.health_monitor_handle.lock().await.take()
        && tokio::time::timeout(Duration::from_secs(2), &mut handle)
            .await
            .is_err()
    {
        handle.abort();
        let _ = handle.await;
    }
    stop_slot_process(slot).await;
}

/// Senders backing the open subscribe streams of the selector-gate mock
/// ([`SelectorGateBridge`]): dropping one ends that subscription's stream.
type SelectorStreamSenders =
    Arc<Mutex<Vec<tokio::sync::mpsc::UnboundedSender<Result<JmsMessage, Status>>>>>;

/// Task 1.2 fixture: one bridge mock carrying BOTH selector-gate surfaces —
/// a Health response whose `capabilities`/`status` the test controls per
/// phase, and recorded `Subscribe` traffic. Ownership follows the
/// [`spawn_scripted_health`] shape: no detached server survives a test. Every
/// subscribe stream stays open until [`SelectorGateBridge::end_streams`] drops
/// its sender, so "no further Subscribe calls" is observable without sleeps.
pub(crate) struct SelectorGateBridge {
    pub port: u16,
    /// Capabilities advertised by the next Health responses. Flipping this
    /// models a replaced (older) bridge instance mid-test.
    pub capabilities: Arc<Mutex<Vec<String>>>,
    /// Raw `HealthResponse.status` (task 1.2: broker status never gates).
    pub status: Arc<Mutex<i32>>,
    /// Every `SubscribeRequest` received, in arrival order.
    pub subscribes: Arc<Mutex<Vec<SubscribeRequest>>>,
    /// One event per received Subscribe (bounded waits in tests).
    subscribe_events: tokio::sync::mpsc::UnboundedReceiver<()>,
    /// Senders backing the open subscribe streams.
    stream_senders: SelectorStreamSenders,
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl SelectorGateBridge {
    /// Awaits the next Subscribe under a 5s bound.
    pub(crate) async fn next_subscribe(&mut self) {
        tokio::time::timeout(Duration::from_secs(5), self.subscribe_events.recv())
            .await
            .expect("subscribe event within 5s")
            .expect("subscribe events channel open");
    }

    /// Replaces the advertised capabilities (model a new bridge instance).
    pub(crate) fn set_capabilities(&self, caps: &[&str]) {
        *self
            .capabilities
            .lock()
            .expect("capabilities mutex poisoned") =
            caps.iter().map(|c| (*c).to_string()).collect();
    }

    /// Subscribe requests recorded so far.
    pub(crate) fn subscribe_count(&self) -> usize {
        self.subscribes
            .lock()
            .expect("subscribes mutex poisoned")
            .len()
    }

    /// Ends every open subscribe stream (the consumer then re-attaches or
    /// terminates).
    pub(crate) fn end_streams(&self) {
        self.stream_senders
            .lock()
            .expect("stream senders mutex poisoned")
            .clear();
    }

    /// Shuts the server down under a bounded timeout.
    pub(crate) async fn shutdown(self) {
        let Self {
            shutdown, mut task, ..
        } = self;
        let _ = shutdown.send(());
        if tokio::time::timeout(Duration::from_secs(2), &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

#[derive(Clone)]
struct SelectorGateService {
    capabilities: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<i32>>,
    subscribes: Arc<Mutex<Vec<SubscribeRequest>>>,
    subscribe_tx: tokio::sync::mpsc::UnboundedSender<()>,
    stream_senders: SelectorStreamSenders,
}

#[async_trait::async_trait]
impl BridgeService for SelectorGateService {
    async fn send(&self, _request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        Err(Status::unimplemented(
            "selector-gate fixture serves Subscribe and Health",
        ))
    }

    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

    async fn subscribe(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        self.subscribes
            .lock()
            .expect("subscribes mutex poisoned")
            .push(request.into_inner());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.stream_senders
            .lock()
            .expect("stream senders mutex poisoned")
            .push(tx);
        let _ = self.subscribe_tx.send(());
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
        )))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse {
            healthy: true,
            broker_connected: true,
            message: "ok".to_string(),
            status: *self.status.lock().expect("status mutex poisoned"),
            age_ms: 0,
            capabilities: self
                .capabilities
                .lock()
                .expect("capabilities mutex poisoned")
                .clone(),
            ..Default::default()
        }))
    }

    async fn complete(
        &self,
        _request: Request<CompletionRequest>,
    ) -> Result<Response<CompletionResponse>, Status> {
        Ok(Response::new(CompletionResponse { accepted: true }))
    }
}

/// Spawns the selector-gate mock advertising `capabilities` until the test
/// flips them.
pub(crate) async fn spawn_selector_gate_bridge(capabilities: &[&str]) -> SelectorGateBridge {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind selector-gate listener");
    let port = listener.local_addr().expect("local addr").port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let caps = Arc::new(Mutex::new(
        capabilities.iter().map(|c| (*c).to_string()).collect(),
    ));
    let status = Arc::new(Mutex::new(0i32));
    let subscribes = Arc::new(Mutex::new(Vec::new()));
    let (subscribe_tx, subscribe_events) = tokio::sync::mpsc::unbounded_channel();
    let stream_senders = Arc::new(Mutex::new(Vec::new()));
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let service = SelectorGateService {
        capabilities: Arc::clone(&caps),
        status: Arc::clone(&status),
        subscribes: Arc::clone(&subscribes),
        subscribe_tx,
        stream_senders: Arc::clone(&stream_senders),
    };
    let task = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(BridgeServiceServer::new(service))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    SelectorGateBridge {
        port,
        capabilities: caps,
        status,
        subscribes,
        subscribe_events,
        stream_senders,
        shutdown: shutdown_tx,
        task,
    }
}

/// Test fixture owning the real health monitor, the scripted health server,
/// and the optional real stub child installed in the slot
/// ([`camel_bridge::process::BridgeProcess`] wraps a `tokio::process::Child`
/// with `kill_on_drop(true)`; `shutdown` reclaims and stops it).
pub(crate) struct TestBridgeFixture {
    pub pool: Arc<JmsBridgePool>,
    pub slot: Arc<BridgeSlot>,
    pub probe_rx: tokio::sync::mpsc::UnboundedReceiver<ProbeProcessed>,
    pub replacement_rx: tokio::sync::mpsc::UnboundedReceiver<ReplacementProcessed>,
    monitor: Option<tokio::task::JoinHandle<()>>,
    server: Option<ScriptedHealthHandle>,
}

impl TestBridgeFixture {
    /// Installs the optional initial child as the slot's Ready process (or
    /// publishes Ready with the scripted channel alone), installs the test
    /// notification channels, and starts the real health monitor.
    pub(crate) async fn start(
        pool: Arc<JmsBridgePool>,
        slot: Arc<BridgeSlot>,
        server: ScriptedHealthHandle,
        initial_child: Option<camel_bridge::process::BridgeProcess>,
    ) -> TestBridgeFixture {
        let channel = connect_scripted(server.port).await;
        match initial_child {
            Some(child) => {
                JmsBridgePool::apply_initial_start_result(slot.as_ref(), Ok((child, channel)))
                    .await;
            }
            None => {
                let _ = slot.state_tx.send_replace(BridgeState::Ready { channel });
            }
        }

        let (probe_tx, probe_rx) = tokio::sync::mpsc::unbounded_channel();
        *pool
            .probe_processed
            .lock()
            .expect("probe_processed mutex poisoned") = Some(probe_tx);
        let (replacement_tx, replacement_rx) = tokio::sync::mpsc::unbounded_channel();
        *pool
            .replacement_processed
            .lock()
            .expect("replacement_processed mutex poisoned") = Some(replacement_tx);

        pool.spawn_health_monitor(Arc::clone(&slot)).await;
        let monitor = slot
            .health_monitor_handle
            .lock()
            .await
            .take()
            .expect("health monitor handle stored");

        TestBridgeFixture {
            pool,
            slot,
            probe_rx,
            replacement_rx,
            monitor: Some(monitor),
            server: Some(server),
        }
    }

    /// Awaits the next monitor probe acknowledgement under a 5s bound.
    pub(crate) async fn next_probe(&mut self) -> ProbeProcessed {
        tokio::time::timeout(Duration::from_secs(5), self.probe_rx.recv())
            .await
            .expect("monitor probe acknowledgement within 5s")
            .expect("probe_processed channel open")
    }

    /// Awaits the next replacement-processed notification under a 5s bound.
    pub(crate) async fn next_replacement(&mut self) -> ReplacementProcessed {
        tokio::time::timeout(Duration::from_secs(5), self.replacement_rx.recv())
            .await
            .expect("replacement-processed notification within 5s")
            .expect("replacement_processed channel open")
    }

    /// Awaits the next server-side event under a 5s bound.
    pub(crate) async fn next_server_event(&mut self) -> HealthEvent {
        let server = self.server.as_mut().expect("server still attached");
        tokio::time::timeout(Duration::from_secs(5), server.events.recv())
            .await
            .expect("server event within 5s")
            .expect("server events channel open")
    }

    /// Awaits server-side completion of a health call under a 5s bound.
    pub(crate) async fn await_server_completed(&mut self) {
        loop {
            match self.next_server_event().await {
                HealthEvent::Completed => return,
                HealthEvent::Started => {}
            }
        }
    }

    /// Health calls observed by the scripted server so far.
    pub(crate) fn server_calls(&self) -> usize {
        self.server
            .as_ref()
            .expect("server still attached")
            .calls
            .load(Ordering::SeqCst)
    }

    /// Stops the monitor, the server, the installed child, and any unused
    /// prepared processes, all under bounded timeouts.
    pub(crate) async fn shutdown(mut self) {
        // The monitor only exits on a terminal state; publish it first so the
        // join is prompt, then abort if the bound expires.
        let _ = self.slot.state_tx.send_replace(BridgeState::Stopped);
        if let Some(mut monitor) = self.monitor.take()
            && tokio::time::timeout(Duration::from_secs(2), &mut monitor)
                .await
                .is_err()
        {
            monitor.abort();
            let _ = monitor.await;
        }
        if let Some(server) = self.server.take() {
            server.shutdown().await;
        }
        let process = self.slot.process.lock().await.take();
        if let Some(process) = process {
            let _ = tokio::time::timeout(Duration::from_secs(2), process.stop()).await;
        }
        let mut queue = self.pool.replacement_outcomes.lock().await;
        while let Some(prepared) = queue.pop_front() {
            if let Ok((process, _channel)) = prepared {
                let _ = tokio::time::timeout(Duration::from_secs(2), process.stop()).await;
            }
        }
    }
}

impl Drop for TestBridgeFixture {
    fn drop(&mut self) {
        // Panic path: abort everything so a panicking test leaks no task.
        // The child is killed by `BridgeProcess`'s kill-on-drop child when the
        // slot/pool drop during unwind.
        if let Some(monitor) = self.monitor.take() {
            monitor.abort();
        }
        if let Some(server) = self.server.take() {
            let _ = server.shutdown.send(());
            server.task.abort();
        }
    }
}

/// Task 2.4 test helper: reads the slot's installed child PID.
pub(crate) async fn slot_child_pid(slot: &BridgeSlot) -> Option<u32> {
    slot.process
        .lock()
        .await
        .as_ref()
        .and_then(|p| p.child_pid())
}

/// Task 2.4 test helper: queues one prepared replacement launch result.
pub(crate) async fn push_replacement(
    pool: &JmsBridgePool,
    process: camel_bridge::process::BridgeProcess,
    channel: tonic::transport::Channel,
) {
    pool.replacement_outcomes
        .lock()
        .await
        .push_back(Ok((process, channel)));
}

/// Task 2.4 test helper: replacement attempts observed so far.
pub(crate) fn replacement_attempts(pool: &JmsBridgePool) -> usize {
    pool.replacement_attempts.load(Ordering::SeqCst)
}

/// Task 2.5 test helper: every refresh attempt recorded so far.
pub(crate) fn refresh_attempts(pool: &JmsBridgePool) -> Vec<RefreshAttempt> {
    pool.refresh_attempts
        .lock()
        .expect("refresh_attempts mutex poisoned")
        .clone()
}

/// Task 2.5 test helper: waits for the `start`-indexed refresh attempt under a
/// bounded wait, driven by the pool's post-recording notification (no sleep or
/// polling).
pub(crate) async fn next_refresh_attempt(pool: &JmsBridgePool, start: usize) -> RefreshAttempt {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(attempt) = refresh_attempts(pool).get(start) {
            return attempt.clone();
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("refresh attempt {start} not observed within 3s");
        }
        // `notify_one` stores a permit, so a recording between the check and
        // this wait cannot be lost.
        let _ = tokio::time::timeout(remaining, pool.refresh_observed.notified()).await;
    }
}

/// Task 2.5 test helper: starts a real ready-handshake child that announces
/// `port` and sleeps, so `refresh_slot_channel` selects a real process endpoint.
#[cfg(unix)]
pub(crate) async fn ready_stub_process(
    dir: &std::path::Path,
    tag: &str,
    port: u16,
) -> camel_bridge::process::BridgeProcess {
    use std::os::unix::fs::PermissionsExt;

    let _ = rustls::crypto::ring::default_provider().install_default();
    let pidfile = dir.join(format!("{tag}.pid"));
    let script = format!(
        "#!/bin/sh\necho $$ > '{}'\nprintf '%s\\n' '{{\"status\":\"ready\",\"port\":{port}}}'\nexec sleep 30\n",
        pidfile.display()
    );
    let path = dir.join(format!("{tag}.sh"));
    std::fs::write(&path, script).expect("write stub bridge");
    let mut perms = std::fs::metadata(&path)
        .expect("stat stub bridge")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod stub bridge");

    let config = camel_bridge::process::BridgeProcessConfig::jms(
        path,
        "tcp://localhost:61616".to_string(),
        camel_bridge::process::BrokerType::ActiveMq,
        None,
        None,
        2_000,
        30_000,
        30_000,
        Some("/var/lib/state/rust-camel/jms-bridge".to_string()),
        Some("0123456789abcdef0123456789abcdef".to_string()),
        None,
    );
    camel_bridge::process::BridgeProcess::start(&config)
        .await
        .expect("stub must reach the ready line")
}

/// Task 2.5 test helper: takes and stops the slot's installed child under a
/// bounded timeout.
pub(crate) async fn stop_slot_process(slot: &BridgeSlot) {
    let process = tokio::time::timeout(Duration::from_secs(2), slot.process.lock())
        .await
        .expect("process lock within 2s")
        .take();
    if let Some(process) = process {
        let _ = tokio::time::timeout(Duration::from_secs(2), process.stop()).await;
    }
}

/// Task 2.4 test helper: broker-status transitions observed so far.
pub(crate) fn broker_transitions(
    pool: &JmsBridgePool,
) -> Vec<crate::component::BrokerStatusTransition> {
    pool.broker_status_transitions
        .lock()
        .expect("broker_status_transitions mutex poisoned")
        .clone()
}
