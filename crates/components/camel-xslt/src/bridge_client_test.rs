//! End-to-end decode-limit coverage for bridge→Rust `TransformResponse`s.
//!
//! Spawns an in-process tonic server implementing the bridge `Transform` RPC
//! and connects through the production-path client constructor. A ~5 MiB
//! result sits below the 17 MiB bridge decode limit but exceeds tonic's
//! 4 MiB default decode limit, so intact delivery is only possible when the
//! bridge decode limit is applied.

use std::time::Duration;

use tonic::{Request, Response, Status};

use crate::client::xslt_transformer_client;
use crate::proto::{
    CompileStylesheetRequest, CompileStylesheetResponse, ReleaseStylesheetRequest,
    ReleaseStylesheetResponse, TransformRequest, TransformResponse,
    xslt_transformer_client::XsltTransformerClient,
    xslt_transformer_server::{XsltTransformer, XsltTransformerServer},
};

const NEAR_CAP_RESULT_SIZE: usize = 5 * 1024 * 1024;

/// Deterministic non-uniform pattern so the assertion compares real bytes
/// instead of an all-zero or all-same payload.
fn near_cap_result() -> Vec<u8> {
    (0..NEAR_CAP_RESULT_SIZE).map(|i| (i % 251) as u8).collect()
}

#[derive(Clone)]
struct MockXsltBridge {
    transform_result: Option<Vec<u8>>,
}

#[tonic::async_trait]
impl XsltTransformer for MockXsltBridge {
    async fn compile_stylesheet(
        &self,
        _request: Request<CompileStylesheetRequest>,
    ) -> Result<Response<CompileStylesheetResponse>, Status> {
        Err(Status::unimplemented(
            "compile_stylesheet not exercised here",
        ))
    }

    async fn transform(
        &self,
        _request: Request<TransformRequest>,
    ) -> Result<Response<TransformResponse>, Status> {
        let result = self
            .transform_result
            .clone()
            .ok_or_else(|| Status::internal("no prepared transform result"))?;
        Ok(Response::new(TransformResponse {
            result,
            error: None,
        }))
    }

    async fn release_stylesheet(
        &self,
        _request: Request<ReleaseStylesheetRequest>,
    ) -> Result<Response<ReleaseStylesheetResponse>, Status> {
        Err(Status::unimplemented(
            "release_stylesheet not exercised here",
        ))
    }
}

async fn spawn_mock_bridge(result: Vec<u8>) -> std::io::Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(XsltTransformerServer::new(MockXsltBridge {
                transform_result: Some(result),
            }))
            .serve_with_incoming(incoming)
            .await;
    });

    // Give the spawned accept loop a moment before the client dials.
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(port)
}

#[tokio::test]
async fn near_cap_transform_result_decodes_end_to_end() {
    let result = near_cap_result();
    let port = spawn_mock_bridge(result.clone())
        .await
        .expect("spawn mock bridge");

    let endpoint = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("valid endpoint uri");

    // Red baseline: tonic's 4 MiB default decode limit must reject this
    // payload, proving the mock genuinely exercises the decode limit.
    let raw_channel = endpoint.connect().await.expect("connect to mock bridge");
    let mut raw_client = XsltTransformerClient::new(raw_channel);
    let _raw_err = raw_client
        .transform(Request::new(TransformRequest {
            stylesheet_id: "xslt-near-cap".to_string(),
            document: b"<doc/>".to_vec(),
            parameters: Default::default(),
            output_method: String::new(),
        }))
        .await
        .expect_err("raw client must hit the 4 MiB default decode limit");

    // Production path: the helper's 17 MiB decode limit delivers byte-intact.
    let channel = endpoint.connect().await.expect("connect to mock bridge");
    let mut client = xslt_transformer_client(channel);
    let response = client
        .transform(Request::new(TransformRequest {
            stylesheet_id: "xslt-near-cap".to_string(),
            document: b"<doc/>".to_vec(),
            parameters: Default::default(),
            output_method: String::new(),
        }))
        .await
        .expect("transform decodes through the production helper");

    let delivered = response.into_inner();
    assert_eq!(delivered.result.len(), NEAR_CAP_RESULT_SIZE);
    assert_eq!(
        delivered.result, result,
        "transform result must arrive byte-intact below the 17 MiB decode limit"
    );
}

#[test]
fn xslt_bridge_decode_limit_above_cap() {
    // Lockstep contract: decode limit must exceed the xml bridge 16 MiB
    // inbound cap + 1 MiB headroom (see bridges/xml application.yml).
    assert!(
        crate::client::xslt_bridge_decode_limit() >= 17 * 1024 * 1024,
        "xslt decode limit must stay at or above the 16 MiB bridge cap + 1 MiB headroom"
    );
}
