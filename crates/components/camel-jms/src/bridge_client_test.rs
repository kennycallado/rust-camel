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
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("valid endpoint uri")
        .connect()
        .await
        .expect("connect to mock bridge");
    let mut client = bridge_service_client(channel);

    let resp = client
        .subscribe(SubscribeRequest {
            destination: "queue.decode.limit.test".to_string(),
            subscription_id: "near-cap-test".to_string(),
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

    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("valid endpoint uri")
        .connect()
        .await
        .expect("connect to mock bridge");
    let mut client = bridge_service_client(channel);

    let resp = client
        .subscribe(SubscribeRequest {
            destination: "queue.ct.test".to_string(),
            subscription_id: "ct-round-trip".to_string(),
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

    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("valid endpoint uri")
        .connect()
        .await
        .expect("connect to mock bridge");
    let mut first = bridge_service_client(channel.clone());
    let mut second = bridge_service_client(channel);

    first
        .subscribe(SubscribeRequest {
            destination: "queue.dup.test".to_string(),
            subscription_id: "dup-test".to_string(),
        })
        .await
        .expect("first subscribe accepted");

    let err = match second
        .subscribe(SubscribeRequest {
            destination: "queue.dup.test".to_string(),
            subscription_id: "dup-test".to_string(),
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

    let channel =
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", captured.port))
            .expect("valid endpoint uri")
            .connect()
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

    let channel =
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", captured.port))
            .expect("valid endpoint uri")
            .connect()
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
