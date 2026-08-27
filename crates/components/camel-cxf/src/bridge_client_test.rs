//! End-to-end decode-limit coverage for bridge→Rust `ConsumerRequest` streams.
//!
//! Spawns an in-process tonic server implementing the bridge
//! `OpenConsumerStream` stream (the `spawn_mock_bridge` pattern from
//! `crates/components/camel-cxf/tests/support/mock_bridge.rs`) and connects
//! through the production-path client constructor. A ~15 MiB payload sits
//! below the Java-side 16 MiB body cap but exceeds tonic's 4 MiB default
//! decode limit, so intact delivery is only possible when the bridge decode
//! limit is applied.

use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use tonic::{Request, Response, Status};

use crate::pool::cxf_bridge_client;
use crate::proto::{
    ConsumerRequest, ConsumerResponse, HealthRequest, HealthResponse, SoapRequest, SoapResponse,
    cxf_bridge_server::{CxfBridge, CxfBridgeServer},
};

const NEAR_CAP_PAYLOAD_SIZE: usize = 15 * 1024 * 1024;

/// Deterministic non-uniform pattern so the assertion compares real bytes
/// instead of an all-zero or all-same payload.
fn near_cap_payload() -> Vec<u8> {
    (0..NEAR_CAP_PAYLOAD_SIZE)
        .map(|i| (i % 251) as u8)
        .collect()
}

#[derive(Clone)]
struct MockCxfBridge {
    consumer_request: Option<ConsumerRequest>,
}

#[tonic::async_trait]
impl CxfBridge for MockCxfBridge {
    async fn invoke(
        &self,
        _request: Request<SoapRequest>,
    ) -> Result<Response<SoapResponse>, Status> {
        Err(Status::unimplemented("invoke not exercised here"))
    }

    type OpenConsumerStreamStream =
        Pin<Box<dyn Stream<Item = Result<ConsumerRequest, Status>> + Send>>;

    async fn open_consumer_stream(
        &self,
        _request: Request<tonic::Streaming<ConsumerResponse>>,
    ) -> Result<Response<Self::OpenConsumerStreamStream>, Status> {
        let req = self
            .consumer_request
            .clone()
            .ok_or_else(|| Status::internal("no prepared consumer request"))?;
        Ok(Response::new(Box::pin(futures::stream::once(async move {
            Ok(req)
        }))))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse {
            healthy: true,
            message: "ok".to_string(),
        }))
    }
}

async fn spawn_mock_bridge(request: ConsumerRequest) -> std::io::Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(CxfBridgeServer::new(MockCxfBridge {
                consumer_request: Some(request),
            }))
            .serve_with_incoming(incoming)
            .await;
    });

    // Give the spawned accept loop a moment before the client dials.
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(port)
}

#[tokio::test]
async fn near_cap_payload_decodes_end_to_end() {
    let payload = near_cap_payload();
    let request = ConsumerRequest {
        request_id: "req-near-cap".to_string(),
        operation: "decode.limit.test".to_string(),
        payload,
        headers: std::collections::HashMap::new(),
        soap_action: String::new(),
        security_profile: String::new(),
    };

    let port = spawn_mock_bridge(request).await.expect("spawn mock bridge");

    // Same channel-construction shape the bridge pool produces, connected to
    // the in-process mock instead of the Java bridge process.
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("valid endpoint uri")
        .connect()
        .await
        .expect("connect to mock bridge");
    let mut client = cxf_bridge_client(channel);

    let resp = client
        .open_consumer_stream(futures::stream::empty::<ConsumerResponse>())
        .await
        .expect("open consumer stream accepted");

    let mut stream = resp.into_inner();
    let delivered = stream
        .message()
        .await
        .expect("consumer request decodes")
        .expect("stream yields one consumer request");
    assert_eq!(delivered.payload.len(), NEAR_CAP_PAYLOAD_SIZE);
    assert_eq!(
        delivered.payload,
        near_cap_payload(),
        "payload must arrive byte-intact below the 16 MiB cap"
    );
}

#[test]
fn cxf_bridge_decode_limit_above_cap() {
    assert!(
        crate::pool::cxf_bridge_decode_limit() > 17 * 1024 * 1024,
        "decode limit must leave at least 1 MiB headroom above the 16 MiB Java-side body cap"
    );
}
