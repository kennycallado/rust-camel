//! End-to-end decode-limit coverage for bridge→Rust `ValidateResponse`s.
//!
//! Spawns an in-process tonic server implementing the bridge `ValidateWith`
//! RPC and connects through the production-path client constructor. A ~5 MiB
//! validation-error payload sits below the 17 MiB bridge decode limit but
//! exceeds tonic's 4 MiB default decode limit, so intact delivery is only
//! possible when the bridge decode limit is applied. Real XSD responses are
//! small; a >4 MiB reply means a broken or hostile bridge emitting giant
//! error text, which must surface as a domain error rather than an opaque
//! decode failure.

use std::time::Duration;

use tonic::{Request, Response, Status};

use crate::proto::{
    RegisterSchemaRequest, RegisterSchemaResponse, UnregisterSchemaRequest,
    UnregisterSchemaResponse, ValidateRequest, ValidateResponse, ValidateWithRequest,
    ValidationError,
    xsd_validator_client::XsdValidatorClient,
    xsd_validator_server::{XsdValidator, XsdValidatorServer},
};
use crate::xsd_bridge::xsd_bridge_client;

const NEAR_CAP_ERROR_SIZE: usize = 5 * 1024 * 1024;

/// Deterministic non-uniform pattern so the assertion compares real bytes
/// instead of an all-zero or all-same payload.
fn near_cap_error_text() -> String {
    (0..NEAR_CAP_ERROR_SIZE)
        .map(|i| char::from(b'a' + (i % 25) as u8))
        .collect()
}

#[derive(Clone)]
struct MockXsdBridge {
    validation_error: Option<String>,
}

#[tonic::async_trait]
impl XsdValidator for MockXsdBridge {
    async fn register_schema(
        &self,
        _request: Request<RegisterSchemaRequest>,
    ) -> Result<Response<RegisterSchemaResponse>, Status> {
        Err(Status::unimplemented("register_schema not exercised here"))
    }

    async fn validate_with(
        &self,
        _request: Request<ValidateWithRequest>,
    ) -> Result<Response<ValidateResponse>, Status> {
        let message = self
            .validation_error
            .clone()
            .ok_or_else(|| Status::internal("no prepared validation error"))?;
        Ok(Response::new(ValidateResponse {
            valid: false,
            errors: vec![ValidationError {
                message,
                line: 1,
                column: 1,
                severity: String::new(),
            }],
            error: None,
        }))
    }

    async fn unregister_schema(
        &self,
        _request: Request<UnregisterSchemaRequest>,
    ) -> Result<Response<UnregisterSchemaResponse>, Status> {
        Err(Status::unimplemented(
            "unregister_schema not exercised here",
        ))
    }

    async fn validate(
        &self,
        _request: Request<ValidateRequest>,
    ) -> Result<Response<ValidateResponse>, Status> {
        Err(Status::unimplemented("validate not exercised here"))
    }
}

async fn spawn_mock_bridge(error_text: String) -> std::io::Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(XsdValidatorServer::new(MockXsdBridge {
                validation_error: Some(error_text),
            }))
            .serve_with_incoming(incoming)
            .await;
    });

    // Give the spawned accept loop a moment before the client dials.
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(port)
}

#[tokio::test]
async fn near_cap_validation_error_decodes_end_to_end() {
    let error_text = near_cap_error_text();
    let port = spawn_mock_bridge(error_text.clone())
        .await
        .expect("spawn mock bridge");

    let endpoint = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("valid endpoint uri");

    // Red baseline: tonic's 4 MiB default decode limit must reject this
    // payload, proving the mock genuinely exercises the decode limit.
    let raw_channel = endpoint.connect().await.expect("connect to mock bridge");
    let mut raw_client = XsdValidatorClient::new(raw_channel);
    let _raw_err = raw_client
        .validate_with(Request::new(ValidateWithRequest {
            schema_id: "xsd-near-cap".to_string(),
            document: b"<doc/>".to_vec(),
        }))
        .await
        .expect_err("raw client must hit the 4 MiB default decode limit");

    // Production path: the helper's 17 MiB decode limit delivers byte-intact.
    let channel = endpoint.connect().await.expect("connect to mock bridge");
    let mut client = xsd_bridge_client(channel);
    let response = client
        .validate_with(Request::new(ValidateWithRequest {
            schema_id: "xsd-near-cap".to_string(),
            document: b"<doc/>".to_vec(),
        }))
        .await
        .expect("validate_with decodes through the production helper");

    let delivered = response.into_inner();
    assert_eq!(delivered.errors.len(), 1);
    assert_eq!(delivered.errors[0].message.len(), NEAR_CAP_ERROR_SIZE);
    assert_eq!(
        delivered.errors[0].message, error_text,
        "validation error text must arrive byte-intact below the 17 MiB decode limit"
    );
}

#[test]
fn xsd_bridge_decode_limit_above_cap() {
    // Lockstep contract: decode limit must exceed the xml bridge 16 MiB
    // inbound cap + 1 MiB headroom (see bridges/xml application.yml).
    assert!(
        crate::xsd_bridge::xsd_bridge_decode_limit() >= 17 * 1024 * 1024,
        "xsd decode limit must stay at or above the 16 MiB bridge cap + 1 MiB headroom"
    );
}
