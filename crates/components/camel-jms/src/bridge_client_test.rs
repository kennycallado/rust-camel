//! End-to-end decode-limit coverage for bridge→Rust `JmsMessage` streams.
//!
//! Spawns an in-process tonic server implementing the bridge `Subscribe`
//! stream (the `spawn_mock_bridge` pattern from
//! `crates/components/camel-cxf/tests/support/mock_bridge.rs`) and connects
//! through the production-path client constructor. A ~15 MiB body sits below
//! the Java-side 16 MiB body cap but exceeds tonic's 4 MiB default decode
//! limit, so intact delivery is only possible when the bridge decode limit is
//! applied.

use std::pin::Pin;
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
}

#[async_trait::async_trait]
impl BridgeService for MockJmsBridge {
    async fn send(&self, _request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        Err(Status::unimplemented("send not exercised here"))
    }

    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<JmsMessage, Status>> + Send>>;

    async fn subscribe(
        &self,
        _request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
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
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse {
            healthy: true,
            broker_connected: true,
            message: "ok".to_string(),
        }))
    }
}

async fn spawn_mock_bridge(message: JmsMessage) -> std::io::Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(BridgeServiceServer::new(MockJmsBridge {
                subscribe_message: Some(message),
            }))
            .serve_with_incoming(incoming)
            .await;
    });

    // Give the spawned accept loop a moment before the client dials.
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(port)
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

    let port = spawn_mock_bridge(message).await.expect("spawn mock bridge");

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

#[test]
fn bridge_decode_limit_above_cap() {
    assert!(
        crate::component::bridge_decode_limit() > 19 * 1024 * 1024,
        "decode limit must exceed the 19 MiB JMS_MAX_BODY_BYTES ceiling"
    );
}
