//! Boundary-proof TLS harness: fully local, no broker, no `#[ignore]`.
//!
//! Each test binds a real loopback listener, drives the production
//! [`RabbitConnectionManager`] constructor path at it, and asserts on what the
//! SERVER observed. That keeps the assertions independent of lapin's client
//! error text after the TLS layer: the fixture server is not an AMQP broker,
//! so the client future is expected to err once TLS (or plaintext) is done.
//!
//! The manager is the public production seam
//! ([`RabbitConnectionManager::try_from_broker_config`]): it captures the
//! transport and derives `ConnectFn` from it, so calling `connect()` a second
//! time after a failed attempt re-invokes the SAME captured closure — the
//! reconnect-reuse property under test.
//!
//! Mutation sensitivity: removing the `with_tls_config` injection in
//! `src/connection.rs` makes the client fall back to system roots. The
//! self-signed fixture server is not trusted there, the handshake fails, and
//! `tls_closure_completes_handshake_with_captured_config` fails with a
//! server-side handshake error. The test therefore detects silent loss of TLS
//! injection.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use camel_component_api::NetworkRetryPolicy;
use camel_component_rabbitmq::config::RabbitBrokerConfig;
use camel_component_rabbitmq::connection::RabbitConnectionManager;
use camel_component_rabbitmq::tls::RabbitTlsConfig;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// Bound on every client wait and on the server task join.
const BOUND: Duration = Duration::from_secs(5);

/// AMQP 0-9-1 protocol header, the first bytes a plaintext client sends.
const AMQP_HEADER: &[u8; 8] = b"AMQP\x00\x00\x09\x01";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tls")
        .join(name)
}

fn server_config() -> Arc<ServerConfig> {
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut BufReader::new(
        File::open(fixture("server-cert.pem")).expect("fixture server cert is readable"),
    ))
    .collect::<Result<_, _>>()
    .expect("fixture server cert PEM parses");
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut BufReader::new(
        File::open(fixture("server-key.pem")).expect("fixture server key is readable"),
    ))
    .expect("fixture server key PEM parses")
    .expect("fixture server key contains a private key");

    let config = ServerConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("the ring provider supports the default protocol versions")
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .expect("fixture server certificate and key are a valid pair");

    Arc::new(config)
}

/// Bind an OS-assigned loopback port and serve `handshakes` TLS connections.
///
/// Returns the port and a handle resolving to one entry per accepted
/// connection: `Ok(Some(sni))` / `Ok(None)` on a completed handshake, `Err`
/// when the acceptor rejected the client. Each stream is dropped right after
/// the handshake, which is the AMQP-level EOF the client observes.
async fn spawn_tls_server(
    handshakes: usize,
) -> (u16, JoinHandle<Vec<Result<Option<String>, String>>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("an OS-assigned loopback port is bindable");
    let port = listener
        .local_addr()
        .expect("the bound listener has a local address")
        .port();
    let acceptor = TlsAcceptor::from(server_config());

    let handle = tokio::spawn(async move {
        let mut observed = Vec::with_capacity(handshakes);
        for _ in 0..handshakes {
            let (stream, _peer) = listener.accept().await.expect("listener accepts a client");
            match acceptor.accept(stream).await {
                Ok(tls) => {
                    let sni = tls.get_ref().1.server_name().map(str::to_string);
                    observed.push(Ok(sni));
                }
                Err(error) => observed.push(Err(error.to_string())),
            }
        }
        observed
    });

    (port, handle)
}

/// Bind an OS-assigned loopback port and capture one plaintext protocol header.
async fn spawn_plaintext_server() -> (u16, JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("an OS-assigned loopback port is bindable");
    let port = listener
        .local_addr()
        .expect("the bound listener has a local address")
        .port();

    let handle = tokio::spawn(async move {
        let (mut stream, _peer): (TcpStream, _) =
            listener.accept().await.expect("listener accepts");
        let mut header = [0u8; AMQP_HEADER.len()];
        stream
            .read_exact(&mut header)
            .await
            .expect("the client sends the AMQP protocol header first");
        header.to_vec()
    });

    (port, handle)
}

fn broker(url: String, ca_cert: Option<String>) -> RabbitBrokerConfig {
    RabbitBrokerConfig {
        url,
        username: None,
        password: None,
        vhost: None,
        tls: ca_cert.map(|ca_cert| RabbitTlsConfig {
            ca_cert: Some(ca_cert),
            server_name: None,
        }),
    }
}

/// One bounded attempt, immediate and non-repeating so a failed connect
/// settles the manager to `Disconnected` for the next `connect()` call.
fn single_attempt_retry() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        enabled: true,
        max_attempts: 1,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_millis(1),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    }
}

fn ca_path(name: &str) -> String {
    fixture(name).to_string_lossy().into_owned()
}

#[tokio::test]
async fn tls_closure_completes_handshake_with_captured_config() {
    let (port, server) = spawn_tls_server(2).await;
    let broker = broker(
        format!("amqps://localhost:{port}/"),
        Some(ca_path("test-ca.pem")),
    );
    let manager = Arc::new(
        RabbitConnectionManager::try_from_broker_config("harness", &broker, single_attempt_retry())
            .expect("the harness broker config is valid"),
    );

    for attempt in 0..2 {
        let settled = time::timeout(BOUND, manager.connect())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "connect attempt {} must settle within {BOUND:?}",
                    attempt + 1
                )
            });
        assert!(
            settled.is_err(),
            "attempt {}: the fixture server is not an AMQP broker, so the client must err",
            attempt + 1
        );
    }

    let observed = time::timeout(BOUND, server)
        .await
        .expect("both fixture handshakes complete within the bound")
        .expect("the fixture server task does not panic");
    assert_eq!(
        observed.len(),
        2,
        "the captured config must be invoked once per connect attempt"
    );
    for (index, handshake) in observed.iter().enumerate() {
        let sni = handshake.as_ref().unwrap_or_else(|error| {
            panic!(
                "handshake {} failed against the test-ca-signed fixture server: {error}",
                index + 1
            )
        });
        assert_eq!(
            sni.as_deref(),
            Some("localhost"),
            "handshake {} must carry SNI localhost derived from the URL host",
            index + 1
        );
    }
}

#[tokio::test]
async fn plain_adapter_sends_plaintext_only_for_amqp_urls() {
    let (port, server) = spawn_plaintext_server().await;
    let broker = broker(format!("amqp://localhost:{port}/"), None);
    let manager = Arc::new(
        RabbitConnectionManager::try_from_broker_config("harness", &broker, single_attempt_retry())
            .expect("the harness broker config is valid"),
    );

    let settled = time::timeout(BOUND, manager.connect())
        .await
        .expect("the plaintext connect settles within the bound");
    assert!(
        settled.is_err(),
        "the fixture server is not an AMQP broker, so the client must err"
    );

    let header = time::timeout(BOUND, server)
        .await
        .expect("the fixture server reads the header within the bound")
        .expect("the fixture server task does not panic");
    assert_eq!(
        header, AMQP_HEADER,
        "an amqp:// URL through the plain adapter must start with the plaintext \
         AMQP header, never a TLS ClientHello"
    );
}
