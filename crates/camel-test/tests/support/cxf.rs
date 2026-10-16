#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::http::HeaderMap;
use axum::routing::post;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

pub const ENV_CAMEL: &str = "CAMEL_CXF_BRIDGE_BINARY_PATH";
pub const ENV_ALIAS: &str = "CXF_BRIDGE_PATH";

/// SOAP envelope namespace for SOAP 1.1.
pub const SOAP11_ENVELOPE_NS: &str = "http://schemas.xmlsoap.org/soap/envelope/";
/// SOAP envelope namespace for SOAP 1.2.
pub const SOAP12_ENVELOPE_NS: &str = "http://www.w3.org/2003/05/soap-envelope";
/// Response payload element namespace declared by `examples/cxf-example/wsdl/hello.wsdl`.
pub const HELLO_NS: &str = "http://example.com/hello";

/// Wire-level SOAP version a capturing mock service replies with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoapVersion {
    Soap11,
    Soap12,
}

/// A request observed by [`start_capturing_mock_soap_service`].
#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Starts an Axum service bound to `127.0.0.1:0` that records every POST to
/// `/service` and replies with a version-matched `sayHelloResponse` fixture.
///
/// The reply uses the real WSDL fixture namespace
/// (`{http://example.com/hello}sayHelloResponse` with a `message` child), not
/// the legacy `return` element. A SOAP 1.1 request is never answered with a
/// SOAP 1.2 response and vice versa.
pub async fn start_capturing_mock_soap_service(
    version: SoapVersion,
) -> (SocketAddr, Arc<Mutex<Vec<CapturedRequest>>>) {
    let captured: Arc<Mutex<Vec<CapturedRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();

    let app = Router::new().route(
        "/service",
        post(move |headers: HeaderMap, body: String| {
            let sink = sink.clone();
            async move {
                let recorded_headers = headers
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.as_str().to_string(),
                            value.to_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect::<Vec<_>>();
                sink.lock()
                    .expect("captured request lock")
                    .push(CapturedRequest {
                        headers: recorded_headers,
                        body,
                    });

                let (envelope_ns, content_type) = match version {
                    SoapVersion::Soap11 => (SOAP11_ENVELOPE_NS, "text/xml; charset=utf-8"),
                    SoapVersion::Soap12 => (
                        SOAP12_ENVELOPE_NS,
                        "application/soap+xml; charset=utf-8; action=\"sayHello\"",
                    ),
                };
                let response = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                     <soapenv:Envelope xmlns:soapenv=\"{envelope_ns}\" xmlns:hel=\"{HELLO_NS}\">\n\
                     <soapenv:Header/>\n\
                     <soapenv:Body>\n\
                     <hel:sayHelloResponse><hel:message>Hello World</hel:message></hel:sayHelloResponse>\n\
                     </soapenv:Body>\n\
                     </soapenv:Envelope>"
                );
                ([("content-type", content_type)], response)
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, captured)
}

pub fn resolve_cxf_bridge_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var(ENV_CAMEL) {
        let path = PathBuf::from(&p);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Ok(p) = std::env::var(ENV_ALIAS) {
        let path = PathBuf::from(&p);
        if path.is_file() {
            unsafe { std::env::set_var(ENV_CAMEL, &path) };
            return Some(path);
        }
    }

    if let Some(root) = find_workspace_root() {
        let candidate = root.join("bridges/cxf/build/native/cxf-bridge");
        if candidate.is_file() {
            unsafe { std::env::set_var(ENV_CAMEL, &candidate) };
            return Some(candidate);
        }
    }

    None
}

pub fn require_cxf_bridge_binary() -> PathBuf {
    resolve_cxf_bridge_binary().unwrap_or_else(|| {
        panic!(
            "cxf-bridge binary not found.\n\
             Build it with:\n  cd bridges/cxf && ./build-native.sh\n\
             Or set {} or {} to the binary path.\n\
             Example: {} bridges/cxf/build/native/cxf-bridge cargo test ...",
            ENV_CAMEL, ENV_ALIAS, ENV_ALIAS,
        )
    })
}

// ── Real native mutual-TLS integration fixtures ─────────────────────────────

/// SOAP 1.1 response the TLS harness returns for one `sayHello` invocation. The
/// element and namespace match the real `examples/cxf-example/wsdl/hello.wsdl`
/// WSDL so the bridge parses it as the operation response.
pub const SOAP11_SAY_HELLO_RESPONSE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:hel="http://example.com/hello">
<soapenv:Header/>
<soapenv:Body>
<hel:sayHelloResponse><hel:message>Hello World</hel:message></hel:sayHelloResponse>
</soapenv:Body>
</soapenv:Envelope>"#;

/// Ephemeral test CA with retained signing material, so both the server and the
/// client leaf can be signed from the same root. The private key never leaves
/// the test process.
pub struct TestTlsCa {
    params: rcgen::CertificateParams,
    key: rcgen::KeyPair,
    der: Vec<u8>,
}

impl TestTlsCa {
    pub fn generate(common_name: &str) -> Self {
        let key = rcgen::KeyPair::generate().expect("test CA keypair");
        let mut params = rcgen::CertificateParams::default();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, common_name);
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.key_usages.push(rcgen::KeyUsagePurpose::KeyCertSign);
        params.key_usages.push(rcgen::KeyUsagePurpose::CrlSign);
        let cert = params.self_signed(&key).expect("test CA self-sign");
        Self {
            params,
            key,
            der: cert.der().to_vec(),
        }
    }

    pub fn der(&self) -> Vec<u8> {
        self.der.clone()
    }

    pub fn sign_server(&self, sans: Vec<rcgen::SanType>) -> TestTlsLeaf {
        let mut params = rcgen::CertificateParams::default();
        params.subject_alt_names = sans;
        params
            .extended_key_usages
            .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
        self.sign(params)
    }

    pub fn sign_client(&self, common_name: &str) -> TestTlsLeaf {
        let mut params = rcgen::CertificateParams::default();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, common_name);
        params
            .extended_key_usages
            .push(rcgen::ExtendedKeyUsagePurpose::ClientAuth);
        self.sign(params)
    }

    fn sign(&self, params: rcgen::CertificateParams) -> TestTlsLeaf {
        let key = rcgen::KeyPair::generate().expect("leaf keypair");
        let issuer = rcgen::Issuer::from_params(&self.params, &self.key);
        let cert = params.signed_by(&key, &issuer).expect("leaf sign");
        TestTlsLeaf {
            cert_der: cert.der().to_vec(),
            key_der: key.serialize_der(),
        }
    }
}

/// A signed leaf certificate with its PKCS#8 private key (DER), for the
/// in-process rustls server and for PKCS12 encoding.
pub struct TestTlsLeaf {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

/// Tempdir-backed PKCS12 store factory. Store files live in an isolated
/// `TempDir` and are dropped with the test; nothing is written under the
/// workspace root. Stores are written by `p12-keystore` (pure Rust) so the
/// trusted-cert and private-key-chain entries carry the friendly names and
/// trusted attribute Java's `PKCS12KeyStore` expects.
pub struct TestTlsStores {
    dir: tempfile::TempDir,
    pub password: String,
}

impl TestTlsStores {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tls store tempdir"),
            password: "changeit".to_string(),
        }
    }

    /// Write a certificate-only PKCS12 truststore holding the CA as a trusted
    /// certificate entry.
    pub fn truststore(&self, ca: &TestTlsCa, file: &str) -> PathBuf {
        let cert = p12_keystore::Certificate::from_der(&ca.der()).expect("truststore CA DER");
        let mut store = p12_keystore::KeyStore::new();
        store.add_entry("cxf-trust", p12_keystore::KeyStoreEntry::Certificate(cert));
        self.write_store(store, file)
    }

    /// Write a PKCS12 keystore carrying the leaf private key, its certificate,
    /// and the signing CA as the chain.
    pub fn keystore(&self, leaf: &TestTlsLeaf, ca: &TestTlsCa, file: &str) -> PathBuf {
        let leaf_cert =
            p12_keystore::Certificate::from_der(&leaf.cert_der).expect("keystore leaf DER");
        let ca_cert = p12_keystore::Certificate::from_der(&ca.der()).expect("keystore CA DER");
        let chain =
            p12_keystore::PrivateKeyChain::new(&leaf.key_der, b"cxf-client", [leaf_cert, ca_cert]);
        let mut store = p12_keystore::KeyStore::new();
        store.add_entry(
            "cxf-client",
            p12_keystore::KeyStoreEntry::PrivateKeyChain(chain),
        );
        self.write_store(store, file)
    }

    fn write_store(&self, store: p12_keystore::KeyStore, file: &str) -> PathBuf {
        let bytes = store
            .writer(&self.password)
            .write()
            .expect("PKCS12 store encodes");
        let out = self.dir.path().join(file);
        std::fs::write(&out, bytes).expect("PKCS12 store writes");
        out
    }
}

impl Default for TestTlsStores {
    fn default() -> Self {
        Self::new()
    }
}

/// Client-authentication policy for [`spawn_soap_tls_server`].
pub enum TestClientAuth {
    /// Do not request a client certificate.
    None,
    /// Request a client certificate, but accept a connection without one.
    Optional { roots_der: Vec<u8> },
    /// Require a client certificate chaining to `roots_der`.
    Required { roots_der: Vec<u8> },
}

/// Server-side observability captured by [`spawn_soap_tls_server`]: how many
/// handshakes completed, the TLS error strings of failed handshakes, the
/// presented client certificate (if any), and the number of requests served.
#[derive(Default)]
pub struct TestTlsServerState {
    pub completed: AtomicUsize,
    pub failures: Mutex<Vec<String>>,
    pub client_cert_der: Mutex<Option<Vec<u8>>>,
    pub requests: AtomicUsize,
}

const TEST_TLS_CONNECTION_BUDGET: usize = 8;
const TEST_TLS_ACCEPT_TIMEOUT: Duration = Duration::from_secs(45);
const TEST_TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const TEST_TLS_IO_TIMEOUT: Duration = Duration::from_secs(10);
const TEST_TLS_READ_CAP: usize = 64 * 1024;

/// Loopback HTTPS server that replies to every request with a SOAP 1.1
/// `sayHelloResponse`. Every accept/handshake/read/write is bounded, so a
/// client that cannot complete the handshake releases the connection slot
/// quickly instead of hanging the test. Cleanup is the caller's `handle.abort()`.
pub async fn spawn_soap_tls_server(
    server_cert_der: &[u8],
    server_key_der: &[u8],
    auth: TestClientAuth,
) -> (SocketAddr, Arc<TestTlsServerState>, JoinHandle<()>) {
    let chain = vec![CertificateDer::from(server_cert_der.to_vec())];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key_der.to_vec()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let versions = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring provider supports the default protocol versions");
    let config = match auth {
        TestClientAuth::None => versions.with_no_client_auth().with_single_cert(chain, key),
        TestClientAuth::Optional { roots_der } => versions
            .with_client_cert_verifier(client_verifier(roots_der, true))
            .with_single_cert(chain, key),
        TestClientAuth::Required { roots_der } => versions
            .with_client_cert_verifier(client_verifier(roots_der, false))
            .with_single_cert(chain, key),
    }
    .expect("test TLS server config builds");

    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test TLS listener binds");
    let addr = listener
        .local_addr()
        .expect("test TLS listener yields a local addr");
    let state = Arc::new(TestTlsServerState::default());
    let task_state = Arc::clone(&state);

    let handle = tokio::spawn(async move {
        for _ in 0..TEST_TLS_CONNECTION_BUDGET {
            let tcp = match tokio::time::timeout(TEST_TLS_ACCEPT_TIMEOUT, listener.accept()).await {
                Ok(Ok((stream, _))) => stream,
                _ => break,
            };
            let mut tls = match tokio::time::timeout(
                TEST_TLS_HANDSHAKE_TIMEOUT,
                acceptor.accept(tcp),
            )
            .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => {
                    task_state
                        .failures
                        .lock()
                        .expect("tls failure lock")
                        .push(e.to_string());
                    continue;
                }
                Err(_) => {
                    task_state
                        .failures
                        .lock()
                        .expect("tls failure lock")
                        .push("handshake timed out".to_string());
                    continue;
                }
            };
            task_state.completed.fetch_add(1, Ordering::SeqCst);
            if let Some(first) = tls
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|certs| certs.first())
            {
                *task_state.client_cert_der.lock().expect("client cert lock") =
                    Some(first.as_ref().to_vec());
            }

            let mut buf = vec![0u8; TEST_TLS_READ_CAP];
            let mut seen = 0usize;
            while seen < buf.len() && !has_header_end(&buf[..seen]) {
                match tokio::time::timeout(TEST_TLS_IO_TIMEOUT, tls.read(&mut buf[seen..])).await {
                    Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                    Ok(Ok(n)) => seen += n,
                }
            }
            task_state.requests.fetch_add(1, Ordering::SeqCst);

            let body = SOAP11_SAY_HELLO_RESPONSE;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ =
                tokio::time::timeout(TEST_TLS_IO_TIMEOUT, tls.write_all(reply.as_bytes())).await;
            let _ = tokio::time::timeout(TEST_TLS_IO_TIMEOUT, tls.shutdown()).await;
        }
    });

    (addr, state, handle)
}

/// True when `buf` already contains the HTTP header terminator `\r\n\r\n`.
fn has_header_end(buf: &[u8]) -> bool {
    buf.windows(4).any(|window| window == b"\r\n\r\n")
}

fn client_verifier(
    roots_der: Vec<u8>,
    optional: bool,
) -> Arc<dyn rustls::server::danger::ClientCertVerifier> {
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(vec![CertificateDer::from(roots_der)]);
    let builder = WebPkiClientVerifier::builder(Arc::new(roots));
    let builder = if optional {
        builder.allow_unauthenticated()
    } else {
        builder
    };
    builder.build().expect("test client cert verifier builds")
}

fn find_workspace_root() -> Option<PathBuf> {
    let start = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut current = start;
    for _ in 0..10 {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists()
            && std::fs::read_to_string(&cargo_toml)
                .map(|c| c.contains("[workspace]"))
                .unwrap_or(false)
            && current.join("bridges").exists()
        {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}
