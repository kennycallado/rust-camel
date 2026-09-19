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

use futures::Stream;
use tonic::{Request, Response, Status};

use crate::component::bridge_service_client;
use crate::proto::{
    HealthRequest, HealthResponse, JmsMessage, SendRequest, SendResponse, SubscribeRequest,
    bridge_service_server::{BridgeService, BridgeServiceServer},
};

const NEAR_CAP_BODY_SIZE: usize = 15 * 1024 * 1024;

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
}

/// Metadata captured by the mock bridge per RPC shape, so tests can assert on
/// the exact gRPC metadata the client attached to each request.
#[cfg_attr(not(feature = "otel"), allow(dead_code))]
struct CapturedMetadata {
    port: u16,
    send: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>,
    subscribe: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>,
    health: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>,
}

async fn spawn_mock_bridge(message: JmsMessage) -> std::io::Result<CapturedMetadata> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let captured_send = Arc::new(Mutex::new(None));
    let captured_subscribe = Arc::new(Mutex::new(None));
    let captured_health = Arc::new(Mutex::new(None));

    let mock_send = captured_send.clone();
    let mock_subscribe = captured_subscribe.clone();
    let mock_health = captured_health.clone();

    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(BridgeServiceServer::new(MockJmsBridge {
                subscribe_message: Some(message),
                active_ids: Arc::new(Mutex::new(HashSet::new())),
                captured_send: mock_send,
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
