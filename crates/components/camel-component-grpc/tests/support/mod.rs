// Shared test harness for TLS integration tests (rc-1vb2)

use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// Per-call counter for unique temp filenames (avoids collision when tests run in parallel).
static PEM_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a self-signed CA + server cert signed by that CA.
/// Returns (ca_pem, server_cert_pem, server_key_pem).
/// Server cert SAN includes "localhost" and "127.0.0.1".
pub fn gen_server_cert() -> (String, String, String) {
    // CA cert
    let ca_key = KeyPair::generate().expect("ca keygen");
    let mut ca_params = CertificateParams::new(vec!["Test CA".to_string()]).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca self-sign");

    // Server cert signed by CA
    let server_key = KeyPair::generate().expect("server keygen");
    let server_params =
        CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("server params");
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .expect("server cert sign");

    (ca_cert.pem(), server_cert.pem(), server_key.serialize_pem())
}

/// Write PEM content to a unique temp file, return the path.
/// Each call produces a distinct filename (pid + atomic counter) so parallel
/// tests don't race on the same path.
pub fn write_pem_tmp(name: &str, pem: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("camel-grpc-tls-test");
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let counter = PEM_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("pem");
    let unique_name = format!("{stem}-{}-{counter}.{ext}", std::process::id());
    let path = dir.join(unique_name);
    std::fs::write(&path, pem).expect("write pem");
    path
}

/// Generate a full mTLS PKI: CA + server cert + client cert, all signed by the same CA.
/// Returns (ca_pem, server_cert_pem, server_key_pem, client_cert_pem, client_key_pem).
/// Server cert SAN includes "localhost" and "127.0.0.1".
/// Client cert SAN includes "test-client".
pub fn gen_mtls_certs() -> (String, String, String, String, String) {
    // CA cert
    let ca_key = KeyPair::generate().expect("ca keygen");
    let mut ca_params = CertificateParams::new(vec!["Test CA".to_string()]).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca self-sign");

    // Server cert signed by CA
    let server_key = KeyPair::generate().expect("server keygen");
    let server_params =
        CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("server params");
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .expect("server cert sign");

    // Client cert signed by same CA
    let client_key = KeyPair::generate().expect("client keygen");
    let client_params =
        CertificateParams::new(vec!["test-client".to_string()]).expect("client params");
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .expect("client cert sign");

    (
        ca_cert.pem(),
        server_cert.pem(),
        server_key.serialize_pem(),
        client_cert.pem(),
        client_key.serialize_pem(),
    )
}

/// Spawn a tonic TLS server on ephemeral port with the given cert+key.
/// Returns the bound port. Server runs GreeterService.
pub async fn spawn_tls_test_server(cert_pem: &str, key_pem: &str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let incoming = TcpListenerStream::new(listener);

    let cert_pem = cert_pem.to_string();
    let key_pem = key_pem.to_string();

    tokio::spawn(async move {
        let identity = tonic::transport::Identity::from_pem(cert_pem, key_pem);
        let tls_config = tonic::transport::ServerTlsConfig::new().identity(identity);

        tonic::transport::Server::builder()
            .tls_config(tls_config)
            .expect("tls config")
            .add_service(super::GreeterServer::new(super::GreeterImpl))
            .serve_with_incoming(incoming)
            .await
            .expect("serve tls");
    });

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    port
}
