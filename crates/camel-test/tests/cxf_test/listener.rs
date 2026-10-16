//! Native consumer listener TLS and startup announcements (Task 2.7/2.8).
//!
//! Raw-spawn helper: boots the real native cxf-bridge binary with valid IPC mTLS
//! credentials (the fixed `QUARKUS_TLS_BRIDGE_*` env keys, ephemeral rcgen material)
//! and reads the startup announcements — the gRPC `ready` line and the SOAP
//! `soap-ready` line — directly from stdout. `BridgeProcess` (Task 2.8) parses
//! both announcement lines itself; the native tests below drive that production
//! path (`BridgeProcess::start` / the `CxfBridgePool`) against the real binary.
//! Nested module of the `cxf_test` target (declared with `#[path]`), so the
//! `--test cxf_test` filter keeps working.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use camel_bridge::process::{BridgeProcess, BridgeProcessConfig};
use camel_component_cxf::config::CxfListenerTls;
use camel_component_cxf::{CxfBridgePool, CxfComponent, CxfPoolConfig, CxfProfileConfig};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;

use crate::cxf_wsdl_path;
use crate::init_tracing;
use crate::support::cxf::{TestTlsCa, TestTlsLeaf, require_cxf_bridge_binary};
use crate::support::wait::wait_until;
use crate::support::{bridge_bg_rt, install_crypto_provider};
use tokio::io::{AsyncBufReadExt, BufReader};

/// How long the raw-spawned bridge may take to emit BOTH announcement lines.
const ANNOUNCE_BUDGET: Duration = Duration::from_secs(90);
/// Bound for a loopback TCP connect probe to an ANNOUNCED listener port. The
/// listener is already up (its port was announced), so the connect resolves at
/// once; the bound turns a wedged accept path into a fast failure.
const CONNECT_PROBE_BUDGET: Duration = Duration::from_secs(10);
/// Bound for the eagerly-spawned slot-initialisation join. The pool's own
/// `bridge_start_timeout_ms` is 30 s, so the join must outlast that startup.
const BRIDGE_INIT_BUDGET: Duration = Duration::from_secs(60);
const MAX_LINE_BYTES: usize = 64 * 1024;

/// Ephemeral PEM listener-TLS material: one server identity plus the mutual-TLS
/// client CA. Benign test-only keys, generated per run into a tempdir.
pub struct ListenerTlsMaterial {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
    pub client_ca_pem: PathBuf,
    pub ca_pem_bytes: Vec<u8>,
    /// Client identity (cert + key concatenated PEM) signed by the client CA —
    /// the good mutual-TLS client for handshake-acceptance tests.
    pub client_identity_pem: Vec<u8>,
    _dir: tempfile::TempDir,
}

pub fn listener_tls_material() -> ListenerTlsMaterial {
    let dir = tempfile::tempdir().expect("listener tls tempdir");
    let ca = TestTlsCa::generate("cxf listener test ca");
    let server = ca.sign_server(vec![rcgen::SanType::IpAddress(IpAddr::V4(
        Ipv4Addr::LOCALHOST,
    ))]);
    let client = ca.sign_client("cxf listener test client");
    let cert_pem = dir.path().join("server.pem");
    let key_pem = dir.path().join("server-key.pem");
    let client_ca_pem = dir.path().join("client-ca.pem");
    std::fs::write(&cert_pem, pem("CERTIFICATE", &server.cert_der)).expect("server cert pem");
    std::fs::write(&key_pem, pem("PRIVATE KEY", &server.key_der)).expect("server key pem");
    std::fs::write(&client_ca_pem, pem("CERTIFICATE", &ca.der())).expect("client ca pem");
    let mut client_identity_pem = pem("CERTIFICATE", &client.cert_der).into_bytes();
    client_identity_pem.extend_from_slice(pem("PRIVATE KEY", &client.key_der).as_bytes());
    ListenerTlsMaterial {
        cert_pem,
        key_pem,
        client_ca_pem,
        ca_pem_bytes: pem("CERTIFICATE", &ca.der()).into_bytes(),
        client_identity_pem,
        _dir: dir,
    }
}

/// A raw-spawned native bridge: announced ports plus the child (killed on drop).
pub struct ListenerBridge {
    pub child: tokio::process::Child,
    pub grpc_port: u16,
    pub soap_port: u16,
    _ipc: tempfile::TempDir,
}

impl ListenerBridge {
    /// Bounded shutdown: SIGKILL + wait, never leaving a child or a held port.
    pub async fn stop(mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await;
    }
}

impl Drop for ListenerBridge {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Spawns the native bridge with valid IPC mTLS credentials and the given SOAP
/// listener bind/TLS material, then reads `ready` + `soap-ready` from stdout.
/// No reserve-bind-drop probe anywhere: the SOAP port is the ANNOUNCED port.
pub async fn spawn_listener_bridge(
    binary: &Path,
    bind_address: &str,
    material: &ListenerTlsMaterial,
) -> ListenerBridge {
    let ipc_dir = tempfile::tempdir().expect("ipc tls tempdir");
    let ipc = write_ipc_material(ipc_dir.path());

    let mut cmd = tokio::process::Command::new(binary);
    cmd.env("QUARKUS_HTTP_SSL_PORT", "0")
        .env(
            "QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_CERT",
            ipc.server_cert.as_os_str(),
        )
        .env(
            "QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_KEY",
            ipc.server_key.as_os_str(),
        )
        .env(
            "QUARKUS_TLS_BRIDGE_TRUST_STORE_PEM_CERTS",
            ipc.ca_pem.as_os_str(),
        )
        .env("CXF_PROFILES", "test")
        .env("CXF_PROFILE_TEST_WSDL_PATH", cxf_wsdl_path())
        .env(
            "CXF_PROFILE_TEST_SERVICE_NAME",
            "{http://example.com/hello}HelloService",
        )
        .env(
            "CXF_PROFILE_TEST_PORT_NAME",
            "{http://example.com/hello}HelloPort",
        )
        .env("CXF_ADDRESS", bind_address)
        .env("CXF_SOAP_TLS_CERT_PATH", material.cert_pem.as_os_str())
        .env("CXF_SOAP_TLS_KEY_PATH", material.key_pem.as_os_str())
        .env(
            "CXF_SOAP_TLS_CLIENT_CA_PATH",
            material.client_ca_pem.as_os_str(),
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());

    let mut child = cmd.spawn().expect("spawn native cxf-bridge");
    let stdout = child.stdout.take().expect("child stdout");
    let mut reader = BufReader::new(stdout);

    let mut grpc_port: Option<u16> = None;
    let mut soap_port: Option<u16> = None;
    let deadline = Instant::now() + ANNOUNCE_BUDGET;
    let mut line = Vec::new();
    while (grpc_port.is_none() || soap_port.is_none()) && Instant::now() < deadline {
        line.clear();
        let read = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            read_line_bounded(&mut reader, &mut line),
        )
        .await;
        let n = match read {
            Ok(Ok(n)) => n,
            Ok(Err(_)) | Err(_) => break,
        };
        if n == 0 {
            break; // EOF before both announcements: fail below with what we saw
        }
        let text = String::from_utf8_lossy(&line);
        if let Some(port) = parse_announcement(text.trim(), "ready") {
            grpc_port = Some(port);
        }
        if let Some(port) = parse_announcement(text.trim(), "soap-ready") {
            soap_port = Some(port);
        }
    }

    let grpc_port = grpc_port.unwrap_or_else(|| {
        let _ = child.start_kill();
        panic!("native bridge never announced the gRPC ready line before the budget");
    });
    let soap_port = soap_port.unwrap_or_else(|| {
        let _ = child.start_kill();
        panic!("native bridge never announced soap-ready before the budget");
    });

    ListenerBridge {
        child,
        grpc_port,
        soap_port,
        _ipc: ipc_dir,
    }
}

/// `{"status":"<status>","port":N}` → `Some(N)` for the exact status.
fn parse_announcement(line: &str, status: &str) -> Option<u16> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("status")?.as_str()? != status {
        return None;
    }
    value.get("port")?.as_u64().map(|p| p as u16)
}

/// Reads one newline-terminated line into `buf` (bounded); `Ok(0)` = EOF.
async fn read_line_bounded<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    buf: &mut Vec<u8>,
) -> std::io::Result<usize> {
    let mut total = 0usize;
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(total);
        }
        let take = chunk
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(chunk.len());
        let capped = take.min(MAX_LINE_BYTES.saturating_sub(buf.len()));
        buf.extend_from_slice(&chunk[..capped]);
        let line_complete = take > capped || chunk[..take].contains(&b'\n');
        reader.consume(take);
        total += take;
        if line_complete {
            return Ok(total);
        }
    }
}

struct IpcMaterial {
    server_cert: PathBuf,
    server_key: PathBuf,
    ca_pem: PathBuf,
}

/// IPC mTLS material for the gRPC channel — the same shape `BridgeTlsMaterial`
/// writes per spawn, so the bridge boots with valid credentials exactly as in
/// production (fixed env keys, ephemeral PEMs, nothing persisted).
fn write_ipc_material(dir: &Path) -> IpcMaterial {
    let ca = TestTlsCa::generate("camel-bridge CA");
    let server: TestTlsLeaf = ca.sign_server(vec![
        rcgen::SanType::DnsName(rcgen::string::Ia5String::try_from("localhost").expect("ia5")),
        rcgen::SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        rcgen::SanType::IpAddress(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
    ]);
    let server_cert = dir.join("ipc-server.pem");
    let server_key = dir.join("ipc-server-key.pem");
    let ca_pem = dir.join("ipc-ca.pem");
    std::fs::write(&server_cert, pem("CERTIFICATE", &server.cert_der)).expect("ipc cert pem");
    std::fs::write(&server_key, pem("PRIVATE KEY", &server.key_der)).expect("ipc key pem");
    std::fs::write(&ca_pem, pem("CERTIFICATE", &ca.der())).expect("ipc ca pem");
    IpcMaterial {
        server_cert,
        server_key,
        ca_pem,
    }
}

fn pem(kind: &str, der: &[u8]) -> String {
    use base64::Engine as _;
    let body = base64::engine::general_purpose::STANDARD.encode(der);
    let mut wrapped = String::new();
    for (i, ch) in body.chars().enumerate() {
        if i > 0 && i % 64 == 0 {
            wrapped.push('\n');
        }
        wrapped.push(ch);
    }
    format!("-----BEGIN {kind}-----\n{wrapped}\n-----END {kind}-----\n")
}

/// Walks an error's source chain into one lowercased string for classification.
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut out = format!("{err}");
    let mut current = err.source();
    let mut depth = 0;
    while let Some(source) = current {
        out.push_str(" -> ");
        out.push_str(&source.to_string());
        current = source.source();
        depth += 1;
        if depth > 8 {
            break;
        }
    }
    out.to_lowercase()
}

/// Assert a SPECIFIC client-auth TLS alert rejection: the normalized error
/// chain (case-insensitive, punctuation/format-insensitive) must carry
/// `expected_alert` verbatim as a word — `CertificateRequired` for a missing
/// client certificate, `CertificateUnknown` for a foreign-CA identity — and
/// must not carry any wrong-failure mode. No generic "any TLS error" pass
/// exists: the two codes are distinct and each case pins its own.
fn assert_client_auth_alert(label: &str, err: &reqwest::Error, expected_alert: &str) {
    assert!(
        err.status().is_none(),
        "{label}: no HTTP response may be produced: {err:?}"
    );
    let chain = error_chain(err);
    eprintln!("{label}: handshake rejection chain: {chain}");
    let normalized: String = chain
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let expected: String = expected_alert
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    for forbidden in [
        "unknownissuer",
        "notvalidforname",
        "hostname",
        "timedout",
        "timeout",
        "connectionrefused",
        "connectrefused",
    ] {
        assert!(
            !normalized.contains(forbidden),
            "{label}: unexpected failure mode '{forbidden}' — this must be a client-auth \
             alert rejection ({expected_alert}): {chain}"
        );
    }
    assert!(
        normalized.contains(&expected),
        "{label}: expected the exact client-auth alert {expected_alert} — chain: {chain}"
    );
}

/// Task 2.7, native consumer: a mutual-TLS SOAP listener (client CA required)
/// rejects a client WITHOUT a client certificate at the TLS handshake — before
/// any SOAP processing. The client trusts the listener CA and connects to the
/// ANNOUNCED `soap-ready` port (no reserve probe): if TLS were not enforcing
/// client auth, the POST would reach the pre-stream availability gate and the
/// returned `503` body would fail this test.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_mutual_tls_with_no_client_cert_is_rejected() {
    init_tracing();
    install_crypto_provider();
    let binary = require_cxf_bridge_binary();
    let material = listener_tls_material();

    // Port 0: the JVM picks and HOLDS the port; the announced number is the real one.
    let bridge = spawn_listener_bridge(&binary, "https://127.0.0.1:0/cxf", &material).await;
    let soap_port = bridge.soap_port;
    assert!(bridge.grpc_port > 0, "ready must announce a real gRPC port");
    assert!(soap_port > 0, "soap-ready must announce a real port");

    // The announced port is live (connects to it — no bind/drop probe of our own).
    let addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), soap_port));
    tokio::time::timeout(CONNECT_PROBE_BUDGET, tokio::net::TcpStream::connect(addr))
        .await
        .unwrap_or_else(|_elapsed| {
            panic!(
                "announced SOAP port {soap_port} connect did not resolve within {CONNECT_PROBE_BUDGET:?}"
            )
        })
        .unwrap_or_else(|e| panic!("announced SOAP port {soap_port} must be connectable: {e}"));

    // Trusts the listener CA, presents NO client identity.
    let ca = reqwest::Certificate::from_pem(&material.ca_pem_bytes).expect("listener CA parses");
    let client = reqwest::Client::builder()
        .add_root_certificate(ca)
        .timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client");

    let response = client
        .post(format!("https://127.0.0.1:{soap_port}/cxf/test"))
        .header("content-type", "text/xml; charset=utf-8")
        .body(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
  <soapenv:Body><test/></soapenv:Body>
</soapenv:Envelope>"#,
        )
        .send()
        .await;

    match response {
        Ok(res) => {
            // Any HTTP response (including the pre-stream 503 body) proves the
            // handshake did NOT enforce a client certificate — the exact failure
            // this test exists to catch.
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            panic!(
                "client without a certificate must be rejected at the TLS handshake, \
                 but the request reached SOAP processing: status={status} body={body:?}"
            );
        }
        Err(err) => {
            assert!(
                err.status().is_none(),
                "no HTTP response may be produced without a client certificate: {err:?}"
            );
            let chain = error_chain(&err);
            assert!(
                !chain.contains("unknownissuer") && !chain.contains("notvalidforname"),
                "the client must trust the listener CA — this must be a client-cert \
                 rejection, not a server-trust/hostname failure: {chain}"
            );
            assert!(
                !chain.contains("timed out") && !chain.contains("timeout"),
                "the rejection must be a handshake rejection, not a timeout: {chain}"
            );
            assert!(
                chain.contains("alert")
                    || chain.contains("handshake")
                    || chain.contains("certificate_required")
                    || chain.contains("certificaterequired"),
                "expected a specific TLS client-certificate handshake rejection: {chain}"
            );
        }
    }

    bridge.stop().await;
}

// ── Task 2.8: dual ready-line startup + announced-port access ─────────────────

/// Bare `sayHello` SOAP 1.1 envelope posted to the consumer listener.
const LISTENER_SAY_HELLO_SOAP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:hel="http://example.com/hello">
  <soapenv:Header/>
  <soapenv:Body>
    <hel:sayHello>
      <name>hello</name>
    </hel:sayHello>
  </soapenv:Body>
</soapenv:Envelope>"#;

/// Mutual-TLS HTTPS client: trusts the listener CA and presents the good
/// client identity from the same test CA.
fn mutual_tls_client(material: &ListenerTlsMaterial) -> reqwest::Client {
    let ca = reqwest::Certificate::from_pem(&material.ca_pem_bytes).expect("listener CA parses");
    let identity =
        reqwest::Identity::from_pem(&material.client_identity_pem).expect("client identity parses");
    reqwest::Client::builder()
        .add_root_certificate(ca)
        .identity(identity)
        .timeout(Duration::from_secs(15))
        .build()
        .expect("mutual tls reqwest client")
}

/// HTTPS client trusting the listener CA but presenting NO client identity.
fn no_cert_client(material: &ListenerTlsMaterial) -> reqwest::Client {
    let ca = reqwest::Certificate::from_pem(&material.ca_pem_bytes).expect("listener CA parses");
    reqwest::Client::builder()
        .add_root_certificate(ca)
        .timeout(Duration::from_secs(15))
        .build()
        .expect("no-cert reqwest client")
}

/// HTTPS client that trusts the REAL listener server CA (hostname verification
/// kept on) but presents a client identity from an UNRELATED CA — the handshake
/// must fail on the SERVER's client-certificate check, never on server trust.
fn wrong_ca_client(material: &ListenerTlsMaterial) -> reqwest::Client {
    let ca = reqwest::Certificate::from_pem(&material.ca_pem_bytes).expect("listener CA parses");
    let wrong_ca = TestTlsCa::generate("cxf listener wrong ca");
    let wrong_client = wrong_ca.sign_client("cxf listener wrong client");
    let mut identity_pem = pem("CERTIFICATE", &wrong_client.cert_der).into_bytes();
    identity_pem.extend_from_slice(pem("PRIVATE KEY", &wrong_client.key_der).as_bytes());
    let identity = reqwest::Identity::from_pem(&identity_pem).expect("wrong identity parses");
    reqwest::Client::builder()
        .add_root_certificate(ca)
        .identity(identity)
        .timeout(Duration::from_secs(15))
        .build()
        .expect("wrong-ca reqwest client")
}

/// Real `CxfBridgePool` with a mutual-TLS SOAP listener on `https://127.0.0.1:0`
/// (OS-assigned, JVM-held port announced through `soap-ready`) and the single
/// `test_profile`. Pool-level listener TLS only — never per-profile material.
fn mutual_tls_listener_pool(
    material: &ListenerTlsMaterial,
    health_check_interval_ms: u64,
) -> Arc<CxfBridgePool> {
    let mut listener_tls = CxfListenerTls::default();
    listener_tls.cert_path = Some(material.cert_pem.to_string_lossy().to_string());
    listener_tls.key_path = Some(material.key_pem.to_string_lossy().to_string());
    listener_tls.client_ca_path = Some(material.client_ca_pem.to_string_lossy().to_string());
    Arc::new(
        CxfBridgePool::from_config(CxfPoolConfig {
            profiles: vec![CxfProfileConfig {
                name: "test_profile".to_string(),
                address: Some("http://localhost:8080/service".to_string()),
                wsdl_path: cxf_wsdl_path(),
                service_name: "{http://example.com/hello}HelloService".to_string(),
                port_name: "{http://example.com/hello}HelloPort".to_string(),
                security: Default::default(),
            }],
            max_bridges: 1,
            bridge_start_timeout_ms: 30_000,
            health_check_interval_ms,
            bridge_cache_dir: None,
            version: camel_component_cxf::BRIDGE_VERSION.to_string(),
            bind_address: Some("https://127.0.0.1:0/cxf".to_string()),
            listener_tls: Some(listener_tls),
            reconnect: Default::default(),
        })
        .expect("valid cxf pool config"),
    )
}

/// Consumer route against the ANNOUNCED SOAP port (no reserve probe anywhere).
fn listener_consumer_route(soap_port: u16) -> String {
    let wsdl = cxf_wsdl_path();
    format!(
        "routes:\n  - id: cxf-listener-route\n    from: \"cxf://https://127.0.0.1:{soap_port}/cxf/test_profile?wsdl={wsdl}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=test_profile\"\n    steps:\n      - set_body:\n          value:\n            constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
    )
}

/// Bounded wait until the mutual-TLS GET readiness probe answers 200 (the
/// health-check response the listener serves once a consumer stream is open).
async fn wait_mutual_get_ready(client: &reqwest::Client, soap_port: u16) {
    wait_until(
        "cxf mutual-tls consumer endpoint ready",
        Duration::from_secs(30),
        Duration::from_millis(200),
        || {
            let client = client.clone();
            let url = format!("https://127.0.0.1:{soap_port}/cxf/test_profile");
            async move {
                match client.get(&url).send().await {
                    Ok(res) => Ok(res.status() == reqwest::StatusCode::OK),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .expect("consumer endpoint ready on the announced port");
}

/// Task 2.8, native consumer: with a SOAP bind (HTTP, port 0) and
/// `expect_soap_ready = true`, `BridgeProcess::start` ALONE — no mTLS connect,
/// no `OpenConsumerStream` RPC — returns the ANNOUNCED `soap-ready` port, and
/// that port answers the pre-stream availability gate (HTTP 503 + `Retry-After`
/// + plaintext) promptly. Proves the Task 2.7 startup deadlock is gone: no
/// consumer RPC is required before the SOAP listener is announced.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_expect_soap_ready_starts_without_consumer_rpc() {
    init_tracing();
    install_crypto_provider();
    let binary = require_cxf_bridge_binary();

    let profiles = vec![crate::matrix_profile("test", &cxf_wsdl_path())];
    let mut config = BridgeProcessConfig::cxf_profiles(binary, &profiles, 60_000);
    config.env_vars.push((
        "CXF_ADDRESS".to_string(),
        "http://127.0.0.1:0/cxf".to_string(),
    ));
    config.expect_soap_ready = true;

    // ONLY start — no connect(), no consumer RPC of any kind.
    let process = BridgeProcess::start(&config)
        .await
        .expect("start must return on the dual ready lines without any consumer RPC");
    let soap_port = process
        .soap_port()
        .expect("soap-ready must announce the OS-held port");
    assert!(soap_port > 0, "announced soap port must be a real port");
    assert!(
        process.grpc_port() > 0,
        "ready must announce a real gRPC port"
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("reqwest client");
    let response = tokio::time::timeout(
        Duration::from_secs(15),
        client
            .post(format!("http://127.0.0.1:{soap_port}/cxf/test"))
            .header("content-type", "text/xml; charset=utf-8")
            .body(LISTENER_SAY_HELLO_SOAP)
            .send(),
    )
    .await
    .expect("the pre-stream gate must answer promptly, never hang")
    .expect("request must reach the announced port");

    assert_eq!(
        response.status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "no consumer stream → the pre-stream availability gate answers 503"
    );
    assert!(
        response.headers().get("retry-after").is_some(),
        "the 503 must carry Retry-After"
    );
    let body = response.text().await.expect("503 body");
    assert!(
        !body.contains('<'),
        "the pre-stream body must be plaintext, never a SOAP fault: {body:?}"
    );

    process.stop().await.expect("stop bridge");
}

/// Task 2.8, native consumer: a REAL `CxfBridgePool` with a mutual-TLS SOAP
/// listener on bind port 0 (announced through `soap-ready`) accepts a good
/// client identity over HTTPS on the ANNOUNCED port, and the request is
/// processed (200 + route hit). Every port read is announce metadata — no
/// reserve/bind-drop probe is used anywhere.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_mutual_tls_uses_announced_port() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let material = listener_tls_material();
    let pool = mutual_tls_listener_pool(&material, 2_000);

    // Eagerly create the slot on the permanent background runtime so the native
    // bridge is up before the route starts.
    let pool_init = Arc::clone(&pool);
    let mut pool_init_task = bridge_bg_rt().spawn(async move {
        pool_init
            .get_or_create_slot(&CxfBridgePool::slot_key())
            .await
    });
    if tokio::time::timeout(BRIDGE_INIT_BUDGET, &mut pool_init_task)
        .await
        .is_err()
    {
        pool_init_task.abort();
        panic!("bridge slot init must complete within {BRIDGE_INIT_BUDGET:?}");
    }

    // The ONLY port source: the announced metadata of the current process.
    wait_until(
        "announced soap port",
        Duration::from_secs(60),
        Duration::from_millis(200),
        || {
            let pool = Arc::clone(&pool);
            async move {
                Ok(pool
                    .listener_info()
                    .await
                    .is_some_and(|info| info.soap_port.is_some()))
            }
        },
    )
    .await
    .expect("bridge must announce its soap port");
    let info = pool
        .listener_info()
        .await
        .expect("a bridge process must be running");
    let soap_port = info.soap_port.expect("soap-ready announced port");
    assert!(soap_port > 0, "announced soap port must be a real port");

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(CxfComponent::new(Arc::clone(&pool)))
        .build()
        .await;
    for route in parse_yaml(&listener_consumer_route(soap_port)).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let client = mutual_tls_client(&material);
    wait_mutual_get_ready(&client, soap_port).await;

    let response = tokio::time::timeout(
        Duration::from_secs(15),
        client
            .post(format!("https://127.0.0.1:{soap_port}/cxf/test_profile"))
            .header("content-type", "text/xml; charset=utf-8")
            .header("soapaction", "sayHello")
            .body(LISTENER_SAY_HELLO_SOAP)
            .send(),
    )
    .await
    .expect("mutual-tls request must answer promptly, never hang")
    .expect("good client identity must complete the handshake on the announced port");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = response.text().await.expect("response body");
    assert!(
        body.contains("sayHelloResponse"),
        "route must answer: {body}"
    );

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "cxf consumer route hit",
        Duration::from_secs(5),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .expect("request must hit the route");

    h.stop().await;
    pool.shutdown().await.expect("pool shutdown");
}

/// Task 2.8, native consumer: a health-monitor restart of the SAME `CxfBridgePool`
/// re-emits the slot-persisted listener TLS + bind + `expect_soap_ready` opt-in
/// (one shared builder for startup and restart). After an owned bridge-process
/// failure (SIGKILL of the EXACT recorded PID), the pool restarts the slot on
/// its own (no manual second start), the CURRENT process publishes a fresh
/// `soap-ready` port (Some — never a stale cached value), the consumer stream
/// reconnects through the existing `Reconnect` policy, and the same CA client
/// succeeds again on the new announced port while no-cert and wrong-CA clients
/// still fail the handshake (required client auth preserved).
#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_mutual_tls_restart_preserves_ca_and_port() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let material = listener_tls_material();
    let pool = mutual_tls_listener_pool(&material, 500);

    let pool_init = Arc::clone(&pool);
    let mut pool_init_task = bridge_bg_rt().spawn(async move {
        pool_init
            .get_or_create_slot(&CxfBridgePool::slot_key())
            .await
    });
    if tokio::time::timeout(BRIDGE_INIT_BUDGET, &mut pool_init_task)
        .await
        .is_err()
    {
        pool_init_task.abort();
        panic!("bridge slot init must complete within {BRIDGE_INIT_BUDGET:?}");
    }

    wait_until(
        "announced soap port",
        Duration::from_secs(60),
        Duration::from_millis(200),
        || {
            let pool = Arc::clone(&pool);
            async move {
                Ok(pool
                    .listener_info()
                    .await
                    .is_some_and(|info| info.soap_port.is_some()))
            }
        },
    )
    .await
    .expect("bridge must announce its soap port");
    let info = pool
        .listener_info()
        .await
        .expect("a bridge process must be running");
    let soap_port = info.soap_port.expect("startup soap-ready announced port");
    let start_pid = info.pid.expect("owned child pid accessor");
    assert!(soap_port > 0);

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(CxfComponent::new(Arc::clone(&pool)))
        .build()
        .await;
    for route in parse_yaml(&listener_consumer_route(soap_port)).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let client = mutual_tls_client(&material);
    wait_mutual_get_ready(&client, soap_port).await;

    // Initial good request: 200 + route hit.
    let response = tokio::time::timeout(
        Duration::from_secs(15),
        client
            .post(format!("https://127.0.0.1:{soap_port}/cxf/test_profile"))
            .header("content-type", "text/xml; charset=utf-8")
            .header("soapaction", "sayHello")
            .body(LISTENER_SAY_HELLO_SOAP)
            .send(),
    )
    .await
    .expect("initial request must answer promptly")
    .expect("good identity must complete the handshake");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "cxf consumer route hit (initial)",
        Duration::from_secs(5),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .expect("initial request must hit the route");

    // Cause ONE owned bridge-process failure: SIGKILL the exact recorded PID
    // (no pkill pattern — other suites run bridge children concurrently).
    let kill_status = std::process::Command::new("kill")
        .arg("-9")
        .arg(start_pid.to_string())
        .status()
        .expect("kill command must run");
    assert!(
        kill_status.success(),
        "SIGKILL of the owned pid must succeed"
    );

    // Wait for the ACTUAL health-monitor restart (bounded): the CURRENT process
    // publishes a fresh announce. No deliberate different-port assertion — the
    // OS may reuse the port — only freshness of the metadata is asserted.
    wait_until(
        "health-monitor restart",
        Duration::from_secs(60),
        Duration::from_millis(250),
        || {
            let pool = Arc::clone(&pool);
            async move {
                Ok(match pool.listener_info().await {
                    Some(info) => info.pid.is_some_and(|pid| pid != start_pid),
                    None => false,
                })
            }
        },
    )
    .await
    .expect("the health monitor must restart the slot with a fresh child");
    let info = pool
        .listener_info()
        .await
        .expect("restarted bridge process must be running");
    assert_ne!(
        info.pid,
        Some(start_pid),
        "the current process must be the restarted child"
    );
    let new_soap_port = info
        .soap_port
        .expect("restarted bridge must announce a fresh soap-ready port");
    assert!(new_soap_port > 0, "restarted announce must be a real port");

    // The consumer stream reconnects (existing Reconnect policy) and the same
    // CA + client identity succeeds again on the NEW announced port.
    let client = mutual_tls_client(&material);
    wait_mutual_get_ready(&client, new_soap_port).await;
    let response = tokio::time::timeout(
        Duration::from_secs(15),
        client
            .post(format!(
                "https://127.0.0.1:{new_soap_port}/cxf/test_profile"
            ))
            .header("content-type", "text/xml; charset=utf-8")
            .header("soapaction", "sayHello")
            .body(LISTENER_SAY_HELLO_SOAP)
            .send(),
    )
    .await
    .expect("post-restart request must answer promptly")
    .expect("same CA + identity must complete the handshake after restart");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = response.text().await.expect("response body");
    assert!(
        body.contains("sayHelloResponse"),
        "route must answer: {body}"
    );
    wait_until(
        "cxf consumer route hit (after restart)",
        Duration::from_secs(5),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 2) }
        },
    )
    .await
    .expect("the post-restart request must hit the route");

    // Required client auth preserved: no cert and wrong-CA identities still
    // fail the TLS handshake on the new port with their OWN exact client-auth
    // alert (`CertificateRequired` vs `CertificateUnknown`) — never UnknownIssuer
    // (both clients trust the REAL listener CA), never a hostname failure,
    // never a timeout/connect failure, never another TLS internal error.
    for (label, client, expected_alert) in [
        (
            "no client certificate",
            no_cert_client(&material),
            "CertificateRequired",
        ),
        (
            "wrong-CA client certificate",
            wrong_ca_client(&material),
            "CertificateUnknown",
        ),
    ] {
        // The listener is alive at the KNOWN announced port: an ambiguous
        // error chain can never be excused as a dead port.
        let addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), new_soap_port));
        tokio::time::timeout(CONNECT_PROBE_BUDGET, tokio::net::TcpStream::connect(addr))
            .await
            .unwrap_or_else(|_elapsed| {
                panic!(
                    "{label}: announced SOAP port {new_soap_port} connect did not resolve within {CONNECT_PROBE_BUDGET:?}"
                )
            })
            .unwrap_or_else(|e| {
                panic!("{label}: announced SOAP port {new_soap_port} must stay connectable: {e}")
            });
        let response = tokio::time::timeout(
            Duration::from_secs(15),
            client
                .post(format!(
                    "https://127.0.0.1:{new_soap_port}/cxf/test_profile"
                ))
                .header("content-type", "text/xml; charset=utf-8")
                .body(LISTENER_SAY_HELLO_SOAP)
                .send(),
        )
        .await
        .unwrap_or_else(|_| panic!("{label}: handshake rejection must be prompt, never hang"));
        let err = match response {
            Err(err) => err,
            Ok(res) => panic!(
                "{label}: must be rejected at the TLS handshake, got {}",
                res.status()
            ),
        };
        assert_client_auth_alert(label, &err, expected_alert);
    }

    h.stop().await;
    pool.shutdown().await.expect("pool shutdown");
}
