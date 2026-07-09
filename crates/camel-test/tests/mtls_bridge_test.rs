#![cfg(feature = "integration-tests")]

mod support;

use camel_bridge::process::{BridgeProcess, BridgeProcessConfig};
use camel_xslt::proto::{HealthCheckRequest, health_client::HealthClient};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
use support::xml_bridge::require_xml_bridge_binary;
use tonic::transport::{Certificate, ClientTlsConfig, Endpoint, Identity};

/// Install rustls crypto provider once per process.
fn install_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Positive: start_and_connect establishes mTLS and the Health RPC works.
#[tokio::test]
async fn start_and_connect_mtls_handshake() {
    install_crypto();
    let binary = require_xml_bridge_binary();
    let config = BridgeProcessConfig::xml(binary, 30_000);
    let (process, channel) = BridgeProcess::start_and_connect(&config)
        .await
        .expect("start_and_connect succeeds");

    let mut client = HealthClient::new(channel);
    let resp = client
        .check(HealthCheckRequest {})
        .await
        .expect("health check RPC");
    assert_eq!(resp.into_inner().status, "SERVING");

    process.stop().await.expect("stop");
}

/// Negative: connecting with a different CA fails the TLS handshake.
///
/// Generates an independent CA + client cert (unrelated to the bridge's
/// ephemeral certs) and verifies that the mTLS handshake is rejected.
#[tokio::test]
async fn wrong_ca_handshake_rejected() {
    install_crypto();
    let binary = require_xml_bridge_binary();

    // Start bridge (it generates its own TLS material internally)
    let config = BridgeProcessConfig::xml(binary, 30_000);
    let process = BridgeProcess::start(&config).await.expect("start");
    let port = process.grpc_port();

    // Generate a completely independent CA + client cert
    let wrong_ca_key = KeyPair::generate().expect("wrong CA key");
    let mut wrong_ca_params = CertificateParams::default();
    wrong_ca_params
        .distinguished_name
        .push(DnType::CommonName, "wrong-test-CA");
    wrong_ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let wrong_ca_cert = wrong_ca_params
        .self_signed(&wrong_ca_key)
        .expect("wrong CA cert");

    let wrong_client_key = KeyPair::generate().expect("wrong client key");
    let mut wrong_client_params = CertificateParams::default();
    wrong_client_params
        .distinguished_name
        .push(DnType::CommonName, "wrong-test-client");
    let wrong_client_cert = wrong_client_params
        .signed_by(&wrong_client_key, &wrong_ca_cert, &wrong_ca_key)
        .expect("wrong client cert");

    let wrong_ca_pem = wrong_ca_cert.pem().into_bytes();
    let wrong_client_pem = wrong_client_cert.pem().into_bytes();
    let wrong_client_key_pem = wrong_client_key.serialize_pem().into_bytes();

    // Attempt TLS connection with wrong CA → must fail
    let uri = format!("https://127.0.0.1:{port}");
    let tls_config = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(&wrong_ca_pem))
        .identity(Identity::from_pem(&wrong_client_pem, &wrong_client_key_pem))
        .domain_name("127.0.0.1");

    let result = Endpoint::from_shared(uri)
        .expect("uri")
        .tls_config(tls_config)
        .expect("tls config")
        .connect()
        .await;

    assert!(
        result.is_err(),
        "connection with wrong CA must fail TLS handshake"
    );

    process.stop().await.expect("stop");
}
