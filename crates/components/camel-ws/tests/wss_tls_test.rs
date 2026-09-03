//! WSS producer integration test (Task 1.7).
//!
//! The producer path uses `connect_async` and trusts the process' native
//! root store only. The test therefore makes the committed fixture CA
//! visible through `SSL_CERT_FILE` (honored by rustls-native-certs on
//! Unix) and round-trips one exchange through a rustls TLS WebSocket
//! echo server built from the fixture cert/key.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_component_api::{Body as CamelBody, Exchange, Message as CamelMessage};
use camel_component_ws::{WsEndpointConfig, WsProducer};
use futures::{SinkExt, StreamExt};
use tokio::task::JoinHandle;
use tower::ServiceExt;

/// Serializes the process-global `SSL_CERT_FILE` mutation across tests in
/// this binary.
static TLS_ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard: sets `SSL_CERT_FILE` on construction and restores the
/// previous value on drop — even when an assertion panics mid-test.
struct SslCertFileGuard(Option<String>);

impl SslCertFileGuard {
    fn set(path: &Path) -> Self {
        let prev = std::env::var("SSL_CERT_FILE").ok();
        // `set_var` is unsafe under edition 2024. Sound here because
        // TLS_ENV_LOCK serializes every env access in this binary.
        unsafe { std::env::set_var("SSL_CERT_FILE", path) };
        Self(prev)
    }
}

impl Drop for SslCertFileGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(prev) => unsafe { std::env::set_var("SSL_CERT_FILE", prev) },
            None => unsafe { std::env::remove_var("SSL_CERT_FILE") },
        }
    }
}

/// Absolute path of a committed TLS test fixture (generated once with
/// openssl at development time, see `tests/fixtures/`).
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Server TLS config loaded from the committed fixture cert/key via
/// `rustls-pemfile`, on the ring provider explicitly.
fn fixture_server_config() -> rustls::ServerConfig {
    let cert_pem =
        std::fs::read(fixture("ws-test-server.crt")).expect("fixture server cert must be readable");
    let key_pem =
        std::fs::read(fixture("ws-test-server.key")).expect("fixture server key must be readable");
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

/// rustls TLS WebSocket echo server: upgrades ONE connection and echoes
/// every data frame back, then holds the connection until the peer goes
/// away.
async fn spawn_tls_echo_server(bind: &str) -> (SocketAddr, JoinHandle<()>) {
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
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_text() || msg.is_binary() {
                    if ws.send(msg).await.is_err() {
                        return;
                    }
                } else if msg.is_close() {
                    break;
                }
            }
        }
    });
    (addr, handle)
}

// The `SSL_CERT_FILE` mutation must stay in effect while the producer
// send awaits, so the lock (via the guard) is intentionally held across
// `.await` points; TLS_ENV_LOCK serializes that window against other
// tests in this binary.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn producer_wss_connect() {
    let _env_lock = TLS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env_guard = SslCertFileGuard::set(&fixture("ws-test-ca.crt"));
    // tokio-tungstenite builds its native-roots config with
    // `ClientConfig::builder()`, which needs a process-level crypto
    // provider when more than one provider feature is unified in this
    // workspace; install the ring default like the crate's other tests.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (addr, server) = spawn_tls_echo_server("127.0.0.1:0").await;
    let cfg = WsEndpointConfig {
        scheme: "wss".into(),
        host: "localhost".into(),
        port: addr.port(),
        path: "/feed".into(),
        connect_timeout: Duration::from_secs(2),
        response_timeout: Duration::from_secs(5),
        ..WsEndpointConfig::default()
    }
    .client_config();
    let producer = WsProducer::new(cfg);

    let exchange = Exchange::new(CamelMessage::new(CamelBody::Text(
        "wss-echo-payload".into(),
    )));
    let response = tokio::time::timeout(Duration::from_secs(10), producer.oneshot(exchange))
        .await
        .expect("producer send must finish within 10s")
        .expect("producer send must succeed");

    match &response.input.body {
        CamelBody::Text(s) => assert_eq!(
            s, "wss-echo-payload",
            "echoed body must round-trip through the TLS connection"
        ),
        other => panic!("expected Text body, got {other:?}"),
    }
    server.abort();
}
