#![cfg(feature = "integration-tests")]

mod support;

// Nested module: declared by `#[path]` so it stays part of the `cxf_test`
// target (a plain `mod wire;` would resolve to `tests/wire.rs`, and a
// top-level `tests/wire.rs` would become its own integration target, breaking
// the `--test cxf_test` filter).
#[path = "cxf_test/wire.rs"]
mod wire;

// Nested module: real-native producer transport TLS integration (Task 1.7).
// Declared by `#[path]` for the same reason as `wire`.
#[path = "cxf_test/tls.rs"]
mod tls;

// Nested module: native signer-descriptor boundary (Task 2.5).
// Declared by `#[path]` for the same reason as `wire`.
#[path = "cxf_test/signer.rs"]
mod signer;

// Nested module: native consumer listener TLS + startup announcements (Task 2.7).
// Declared by `#[path]` for the same reason as `wire`.
#[path = "cxf_test/listener.rs"]
mod listener;

// Nested module: native end-to-end WS-Security signed call (SAAJ native
// registration regression). Declared by `#[path]` for the same reason as `wire`.
#[path = "cxf_test/wss_e2e.rs"]
mod wss_e2e;

use camel_api::{Body, Exchange, Message};
use camel_bridge::process::{
    BridgeError, BridgeProcess, BridgeProcessConfig, CxfProfileEnvVars, CxfTlsEnvVars, Redacted,
};
use camel_component_cxf::proto::{
    ConsumerResponse, HealthRequest, cxf_bridge_client::CxfBridgeClient,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{Router, routing::post};
use camel_component_cxf::config::CxfSecurityFields;
use camel_component_cxf::{CxfBridgePool, CxfComponent, CxfPoolConfig, CxfProfileConfig};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use reqwest::StatusCode;
use support::bridge_bg_rt;
use support::cxf::{SOAP11_ENVELOPE_NS, SOAP12_ENVELOPE_NS, require_cxf_bridge_binary};
use support::install_crypto_provider;
use support::send_to_direct;
use support::wait::wait_until;
use tokio::sync::OnceCell;

/// HTTP client with a hard timeout so consumer tests fail fast instead of
/// hanging indefinitely when the bridge accepts the TCP connection but never
/// responds (e.g. still initializing under CI load).
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client with timeout")
}

/// Shared bridge pool for producer-only tests.
///
/// Avoids N parallel JVM/native-binary spawns when producer tests run concurrently.
/// Consumer tests still create their own pool because each needs a unique `bind_address`.
/// Multi-profile tests create their own pool because they use a different profile set.
///
/// This mirrors the `ARTEMIS` / `ARTEMIS_AUTH` pattern in `support/artemis.rs`.
static SHARED_CXF_POOL: OnceCell<Arc<CxfBridgePool>> = OnceCell::const_new();

async fn get_shared_cxf_pool() -> Arc<CxfBridgePool> {
    SHARED_CXF_POOL
        .get_or_init(|| async {
            install_crypto_provider();
            let wsdl_path = cxf_wsdl_path();
            let pool = Arc::new(
                CxfBridgePool::from_config(CxfPoolConfig {
                    profiles: vec![CxfProfileConfig {
                        name: "test_profile".to_string(),
                        address: Some("http://localhost:8080/service".to_string()),
                        wsdl_path,
                        service_name: "{http://example.com/hello}HelloService".to_string(),
                        port_name: "{http://example.com/hello}HelloPort".to_string(),
                        security: Default::default(),
                    }],
                    max_bridges: 4,
                    bridge_start_timeout_ms: 30_000,
                    health_check_interval_ms: 5_000,
                    bridge_cache_dir: None,
                    version: camel_component_cxf::BRIDGE_VERSION.to_string(),
                    bind_address: None,
                    listener_tls: None,
                    reconnect: Default::default(),
                })
                .unwrap(),
            );
            // Eagerly create the bridge slot on the permanent background runtime.
            // This ensures the tonic Channel dispatch task and health monitor are
            // bound to a runtime that outlives all individual test runtimes, so
            // subsequent tests that reuse this pool do not get DispatchGone.
            let pool_init = pool.clone();
            let _ = bridge_bg_rt()
                .spawn(async move {
                    pool_init
                        .get_or_create_slot(&CxfBridgePool::slot_key())
                        .await
                })
                .await;
            pool
        })
        .await
        .clone()
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,camel=info")),
        )
        .with_test_writer()
        .try_init();
}

async fn start_mock_soap_service() -> SocketAddr {
    let app = Router::new().route(
        "/service",
        post(|_body: String| async move {
            (
                [("content-type", "text/xml; charset=utf-8")],
                r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:hel="http://example.com/hello">
  <soapenv:Header/>
  <soapenv:Body>
    <hel:sayHelloResponse>
      <return>pong</return>
    </hel:sayHelloResponse>
  </soapenv:Body>
</soapenv:Envelope>"#,
            )
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn start_mock_soap_fault_service() -> SocketAddr {
    let app = Router::new().route(
        "/service",
        post(|| async {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("content-type", "text/xml; charset=utf-8")],
                r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
  <soapenv:Body>
    <soapenv:Fault>
      <faultcode>soapenv:Server</faultcode>
      <faultstring>Internal service error</faultstring>
      <detail>
        <error>Something went wrong</error>
      </detail>
    </soapenv:Fault>
  </soapenv:Body>
</soapenv:Envelope>"#,
            )
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn shared_cxf_component() -> CxfComponent {
    CxfComponent::new(get_shared_cxf_pool().await)
}

/// Shared bridge pool for the SOAP 1.2 producer test.
///
/// Uses a distinct profile name (`soap12_profile`) and the SOAP 1.2 WSDL so the
/// bridge selects a SOAP 1.2 HTTP binding; reusing `test_profile` would let the
/// cached SOAP 1.1 dispatch answer a `urn:custom`-free request for the same
/// `sayHello` operation and mask a version regression.
static SHARED_CXF_SOAP12_POOL: OnceCell<Arc<CxfBridgePool>> = OnceCell::const_new();

async fn get_shared_cxf_soap12_pool() -> Arc<CxfBridgePool> {
    SHARED_CXF_SOAP12_POOL
        .get_or_init(|| async {
            install_crypto_provider();
            let wsdl_path = cxf_wsdl_path_soap12();
            let pool = Arc::new(
                CxfBridgePool::from_config(CxfPoolConfig {
                    profiles: vec![CxfProfileConfig {
                        name: "soap12_profile".to_string(),
                        address: Some("http://localhost:8080/service".to_string()),
                        wsdl_path,
                        service_name: "{http://example.com/hello}HelloService".to_string(),
                        port_name: "{http://example.com/hello}HelloPort".to_string(),
                        security: Default::default(),
                    }],
                    max_bridges: 4,
                    bridge_start_timeout_ms: 30_000,
                    health_check_interval_ms: 5_000,
                    bridge_cache_dir: None,
                    version: camel_component_cxf::BRIDGE_VERSION.to_string(),
                    bind_address: None,
                    listener_tls: None,
                    reconnect: Default::default(),
                })
                .unwrap(),
            );
            let pool_init = pool.clone();
            let _ = bridge_bg_rt()
                .spawn(async move {
                    pool_init
                        .get_or_create_slot(&CxfBridgePool::slot_key())
                        .await
                })
                .await;
            pool
        })
        .await
        .clone()
}

async fn shared_cxf_soap12_component() -> CxfComponent {
    CxfComponent::new(get_shared_cxf_soap12_pool().await)
}

fn cxf_component_with_bind(bind_port: Option<u16>) -> CxfComponent {
    let wsdl_path = cxf_wsdl_path();
    let pool = Arc::new(
        CxfBridgePool::from_config(CxfPoolConfig {
            profiles: vec![CxfProfileConfig {
                name: "test_profile".to_string(),
                address: Some("http://localhost:8080/service".to_string()),
                wsdl_path,
                service_name: "{http://example.com/hello}HelloService".to_string(),
                port_name: "{http://example.com/hello}HelloPort".to_string(),
                security: Default::default(),
            }],
            max_bridges: 4,
            bridge_start_timeout_ms: 30_000,
            health_check_interval_ms: 5_000,
            bridge_cache_dir: None,
            version: camel_component_cxf::BRIDGE_VERSION.to_string(),
            bind_address: bind_port.map(|p| format!("http://127.0.0.1:{p}/cxf")),
            listener_tls: None,
            reconnect: Default::default(),
        })
        .unwrap(),
    );
    CxfComponent::new(pool)
}

fn two_profile_consumer_component(bind_port: u16) -> CxfComponent {
    let wsdl_path = cxf_wsdl_path();
    let profile = |name: &str| CxfProfileConfig {
        name: name.to_string(),
        address: Some("http://localhost:8080/service".to_string()),
        wsdl_path: wsdl_path.clone(),
        service_name: "{http://example.com/hello}HelloService".to_string(),
        port_name: "{http://example.com/hello}HelloPort".to_string(),
        security: Default::default(),
    };
    let pool = Arc::new(
        CxfBridgePool::from_config(CxfPoolConfig {
            profiles: vec![profile("v11"), profile("v12")],
            max_bridges: 4,
            bridge_start_timeout_ms: 30_000,
            health_check_interval_ms: 5_000,
            bridge_cache_dir: None,
            version: camel_component_cxf::BRIDGE_VERSION.to_string(),
            bind_address: Some(format!("http://127.0.0.1:{bind_port}/cxf")),
            listener_tls: None,
            reconnect: Default::default(),
        })
        .unwrap(),
    );
    CxfComponent::new(pool)
}

async fn post_say_hello(port: u16, profile: &str) -> Result<reqwest::Response, reqwest::Error> {
    http_client()
        .post(format!("http://127.0.0.1:{port}/cxf/{profile}"))
        .header("content-type", "text/xml; charset=utf-8")
        .header("soapaction", "sayHello")
        .body(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:hel="http://example.com/hello">
  <soapenv:Header/>
  <soapenv:Body>
    <hel:sayHelloRequest>
      <name>hello</name>
    </hel:sayHelloRequest>
  </soapenv:Body>
</soapenv:Envelope>"#,
        )
        .send()
        .await
}

fn profile_route_yaml(port: u16, profile: &str) -> String {
    let wsdl_path = cxf_wsdl_path();
    format!(
        "routes:\n  - id: cxf-consumer-{profile}\n    from: \"cxf://http://127.0.0.1:{port}/cxf/{profile}?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile={profile}\"\n    steps:\n      - set_body:\n          value:\n            constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><hel:message>from-{profile}</hel:message></hel:sayHelloResponse>\"\n      - to: \"mock:{profile}\"\n"
    )
}

async fn profile_answers_200(port: u16, profile: &str) -> bool {
    matches!(post_say_hello(port, profile).await, Ok(r) if r.status() == StatusCode::OK)
}

fn cxf_wsdl_path() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    root.join("examples/cxf-example/wsdl/hello.wsdl")
        .to_string_lossy()
        .to_string()
}

/// SOAP 1.2 copy of `hello.wsdl` whose binding namespace makes CXF report the
/// `SOAPBinding.SOAP12HTTP_BINDING` (`.../soap/bindings/HTTP/`) for the port.
fn cxf_wsdl_path_soap12() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    root.join("examples/cxf-example/wsdl/hello-soap12.wsdl")
        .to_string_lossy()
        .to_string()
}

fn reserve_local_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// A real PKCS12 store (password `changeit`) shipped with the test suite. The
/// wrong-password test deliberately pairs it with a wrong password so the only
/// failing variable is the credential, not a missing or malformed store.
fn cxf_tls_keystore_fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cxf-tls-test-keystore.p12")
        .to_string_lossy()
        .to_string()
}

/// A real JKS keystore (`changeit`, alias `alice`) used as both keystore and
/// truststore for WSS signature verification. WSS4J's Merlin crypto loads JKS
/// only (the producer transport `.p12` fixture cannot be reused here), so the
/// consumer WSS fault is a genuine signature-verification rejection, never a
/// store-load false positive.
fn cxf_wss_keystore_fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cxf-wss-test-keystore.jks")
        .to_string_lossy()
        .to_string()
}

/// A signed SOAP 1.2 request (Body signature, no `wsu:Timestamp`, so no expiry)
/// generated by `SignedRequestFixtureGeneratorTest` with the JKS fixture. The
/// consumer verifies it against the same JKS.
fn cxf_wss_signed_request_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cxf-wss-signed-request.xml")
}

/// Security fields for a WSS profile: same JKS as keystore and truststore, with
/// an optional `fault_status` override.
fn wss_security(fault_status: Option<u16>) -> CxfSecurityFields {
    CxfSecurityFields {
        keystore_path: Some(cxf_wss_keystore_fixture()),
        keystore_password: Some("changeit".to_string()),
        truststore_path: Some(cxf_wss_keystore_fixture()),
        truststore_password: Some("changeit".to_string()),
        sig_username: Some("alice".to_string()),
        fault_status,
        ..Default::default()
    }
}

/// One WSS consumer profile in its own pool/bridge. The bridge keeps a single
/// consumer stream (`CxfServerManager.consumerRequestObserver`), so each consumer
/// route needs its own bridge to be reached.
fn wss_single_profile_component(
    name: &str,
    fault_status: Option<u16>,
    port: u16,
    wsdl_path: &str,
) -> CxfComponent {
    let pool = Arc::new(
        CxfBridgePool::from_config(CxfPoolConfig {
            profiles: vec![CxfProfileConfig {
                name: name.to_string(),
                address: None,
                wsdl_path: wsdl_path.to_string(),
                service_name: "{http://example.com/hello}HelloService".to_string(),
                port_name: "{http://example.com/hello}HelloPort".to_string(),
                security: wss_security(fault_status),
            }],
            max_bridges: 4,
            bridge_start_timeout_ms: 30_000,
            health_check_interval_ms: 5_000,
            bridge_cache_dir: None,
            version: camel_component_cxf::BRIDGE_VERSION.to_string(),
            bind_address: Some(format!("http://127.0.0.1:{port}/cxf")),
            listener_tls: None,
            reconnect: Default::default(),
        })
        .unwrap(),
    );
    CxfComponent::new(pool)
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_wrong_tls_password_never_reaches_ready() {
    init_tracing();
    install_crypto_provider();
    let binary = require_cxf_bridge_binary();

    let profiles = vec![CxfProfileEnvVars {
        name: "tls_wrong_password".to_string(),
        wsdl_path: cxf_wsdl_path(),
        service_name: "{http://example.com/hello}HelloService".to_string(),
        port_name: "{http://example.com/hello}HelloPort".to_string(),
        address: Some("https://127.0.0.1:1/service".to_string()),
        keystore_path: None,
        keystore_password: None,
        truststore_path: None,
        truststore_password: None,
        sig_username: None,
        sig_password: None,
        enc_username: None,
        security_actions_out: None,
        security_actions_in: None,
        signature_algorithm: None,
        signature_digest_algorithm: None,
        signature_c14n_algorithm: None,
        signature_parts: None,
        allowed_signature_algorithms: None,
        allowed_digest_algorithms: None,
        allow_expired_pinned_certificate: false,
        fault_status: None,
        tls: Some(CxfTlsEnvVars {
            keystore_path: Some(cxf_tls_keystore_fixture()),
            keystore_password: Some(Redacted::new("not-the-store-password".to_string())),
            keystore_type: Some("PKCS12".to_string()),
            truststore_path: None,
            truststore_password: None,
            truststore_type: None,
        }),
    }];

    // The bridge must never announce ready: eager store validation aborts Quarkus
    // startup, so stdout closes before any ready line. A Timeout would instead mean
    // the process stayed alive and healthy, which is exactly the regression this
    // test guards against.
    let config = BridgeProcessConfig::cxf_profiles(binary, &profiles, 30_000);
    match BridgeProcess::start(&config).await {
        Err(BridgeError::StdoutClosed) => {}
        Err(other) => panic!("expected stdout EOF before ready, got: {other:?}"),
        Ok(process) => {
            let _ = process.stop().await;
            panic!("bridge announced ready despite a wrong TLS store password");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_invokes_mock_soap_service() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let addr = start_mock_soap_service().await;
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component().await)
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-producer-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=test_profile\"\n      - to: \"mock:done\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Text(
        r#"<hel:sayHello xmlns:hel="http://example.com/hello"><name>ping</name></hel:sayHello>"#
            .to_string(),
    )));
    let _ = send_to_direct(&h, "direct:start", exchange).await.unwrap();

    let endpoint = h.mock().get_endpoint("done").unwrap();
    wait_until(
        "cxf producer delivery",
        Duration::from_secs(5),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges.len(), 1);

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_receives_request_and_returns_response() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();
    let port = reserve_local_port();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(cxf_component_with_bind(Some(port)))
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-consumer-test\n    from: \"cxf://http://127.0.0.1:{port}/cxf/test_profile?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=test_profile\"\n    steps:\n      - set_body:\n          value:\n            constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    wait_until(
        "cxf consumer endpoint ready",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let url = format!("http://127.0.0.1:{port}/cxf/test_profile");
            async move {
                match http_client().get(&url).send().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    let client = http_client();
    let res = client
        .post(format!("http://127.0.0.1:{port}/cxf/test_profile"))
        .header("content-type", "text/xml; charset=utf-8")
        .header("soapaction", "sayHello")
        .body(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:hel="http://example.com/hello">
  <soapenv:Header/>
  <soapenv:Body>
    <hel:sayHelloRequest>
      <name>hello</name>
    </hel:sayHelloRequest>
  </soapenv:Body>
</soapenv:Envelope>"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.text().await.unwrap();
    assert!(body.contains("sayHelloResponse"));
    assert!(body.contains("ok"));

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "cxf consumer received request",
        Duration::from_secs(5),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_two_profiles_dispatch_to_own_route() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let port = reserve_local_port();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(two_profile_consumer_component(port))
        .build()
        .await;

    for profile in ["v11", "v12"] {
        for route in parse_yaml(&profile_route_yaml(port, profile)).unwrap() {
            h.add_route(route).await.unwrap();
        }
    }
    h.start().await;

    wait_until(
        "v11 and v12 streams connected",
        Duration::from_secs(30),
        Duration::from_millis(200),
        || async move {
            Ok(profile_answers_200(port, "v11").await && profile_answers_200(port, "v12").await)
        },
    )
    .await
    .unwrap();

    for (profile, other) in [("v11", "v12"), ("v12", "v11")] {
        for _ in 0..3 {
            let res = post_say_hello(port, profile).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let body = res.text().await.unwrap();
            assert!(body.contains(&format!("from-{profile}")), "body: {body}");
            assert!(!body.contains(&format!("from-{other}")), "body: {body}");
        }
    }

    for profile in ["v11", "v12"] {
        let endpoint = h.mock().get_endpoint(profile).unwrap();
        let received = endpoint.get_received_exchanges().await;
        assert!(
            received.len() >= 3,
            "{profile} must receive the 3 posted exchanges, got {}",
            received.len()
        );
        for ex in received {
            assert_eq!(
                ex.input
                    .header("CxfSecurityProfile")
                    .and_then(|v| v.as_str()),
                Some(profile)
            );
        }
    }

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_profile_without_stream_returns_503() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let port = reserve_local_port();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(two_profile_consumer_component(port))
        .build()
        .await;

    for route in parse_yaml(&profile_route_yaml(port, "v12")).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    wait_until(
        "v12 stream connected",
        Duration::from_secs(30),
        Duration::from_millis(200),
        || async move { Ok(profile_answers_200(port, "v12").await) },
    )
    .await
    .unwrap();

    let res = post_say_hello(port, "v11").await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(res.headers().contains_key("retry-after"));
    let body = res.text().await.unwrap();
    assert!(!body.contains("Envelope"), "body: {body}");

    let endpoint = h.mock().get_endpoint("v12").unwrap();
    let received = endpoint.get_received_exchanges().await;
    assert!(!received.is_empty(), "v12 must have received exchanges");
    for ex in received {
        assert_eq!(
            ex.input
                .header("CxfSecurityProfile")
                .and_then(|v| v.as_str()),
            Some("v12")
        );
    }

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_native_health_check_responds_within_5s() {
    init_tracing();
    install_crypto_provider();
    let binary = require_cxf_bridge_binary();

    let wsdl_path = cxf_wsdl_path();

    let profiles = vec![CxfProfileEnvVars {
        name: "test".to_string(),
        wsdl_path: wsdl_path.clone(),
        service_name: "{http://example.com/hello}HelloService".to_string(),
        port_name: "{http://example.com/hello}HelloPort".to_string(),
        address: None,
        keystore_path: None,
        keystore_password: None,
        truststore_path: None,
        truststore_password: None,
        sig_username: None,
        sig_password: None,
        enc_username: None,
        security_actions_out: None,
        security_actions_in: None,
        signature_algorithm: None,
        signature_digest_algorithm: None,
        signature_c14n_algorithm: None,
        signature_parts: None,
        allowed_signature_algorithms: None,
        allowed_digest_algorithms: None,
        allow_expired_pinned_certificate: false,
        fault_status: None,
        tls: None,
    }];

    let config = BridgeProcessConfig::cxf_profiles(binary, &profiles, 5_000);
    let (process, channel) = BridgeProcess::start_and_connect(&config).await.unwrap();

    wait_until(
        "cxf native health check",
        Duration::from_secs(5),
        Duration::from_millis(100),
        || {
            let channel = channel.clone();
            async move {
                let mut client = CxfBridgeClient::new(channel);
                let health = client
                    .health(HealthRequest {})
                    .await
                    .map_err(|e| e.to_string())?
                    .into_inner();
                Ok(health.healthy)
            }
        },
    )
    .await
    .unwrap();

    process.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_handles_soap_fault_response() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let addr = start_mock_soap_fault_service().await;
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component().await)
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-fault-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=test_profile\"\n      - to: \"mock:done\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Text(
        r#"<hel:sayHello xmlns:hel="http://example.com/hello"><name>fault-test</name></hel:sayHello>"#
            .to_string(),
    )));
    let result = send_to_direct(&h, "direct:start", exchange).await;

    // SOAP fault may either propagate as error or pass through as response body
    match result {
        Ok(exchange) => {
            // Fault passed through — verify body contains fault content
            let body = exchange.body_as::<String>().unwrap_or_default();
            assert!(
                body.contains("Fault") || body.contains("fault"),
                "Expected SOAP Fault in response body, got: {body}"
            );
            // Still verify it reached mock endpoint
            let endpoint = h.mock().get_endpoint("done").unwrap();
            wait_until(
                "cxf fault delivery to mock",
                Duration::from_secs(5),
                Duration::from_millis(200),
                || {
                    let endpoint = endpoint.clone();
                    async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
                },
            )
            .await
            .unwrap();
        }
        Err(err) => {
            // Fault propagated as error — verify error mentions fault
            let err_msg = err.to_string();
            assert!(
                err_msg.contains("Fault")
                    || err_msg.contains("fault")
                    || err_msg.contains("500")
                    || err_msg.contains("error"),
                "Expected fault-related error, got: {err_msg}"
            );
        }
    }

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_multiple_sequential_invocations() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let addr = start_mock_soap_service().await;
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component().await)
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-multi-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=test_profile\"\n      - to: \"mock:done\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let endpoint = h.mock().get_endpoint("done").unwrap();

    // Send 3 sequential requests with unique payloads
    for i in 1..=3 {
        let exchange = Exchange::new(Message::new(Body::Text(format!(
            r#"<hel:sayHello xmlns:hel="http://example.com/hello"><name>user-{i}</name></hel:sayHello>"#
        ))));
        let _ = send_to_direct(&h, "direct:start", exchange).await.unwrap();
    }

    // Wait for all 3 exchanges at mock endpoint
    wait_until(
        "cxf multiple invocations delivery",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 3) }
        },
    )
    .await
    .unwrap();

    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        exchanges.len() >= 3,
        "Expected at least 3 exchanges, got {}",
        exchanges.len()
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_returns_health_check_on_get() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();
    let port = reserve_local_port();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(cxf_component_with_bind(Some(port)))
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-health-test\n    from: \"cxf://http://127.0.0.1:{port}/cxf/test_profile?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=test_profile\"\n    steps:\n      - set_body:\n          value:\n            constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    // Wait for endpoint to be ready
    wait_until(
        "cxf consumer endpoint ready",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let url = format!("http://127.0.0.1:{port}/cxf/test_profile");
            async move {
                match http_client().get(&url).send().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    // Send GET request — should return health check (200 OK)
    let client = http_client();
    let res = client
        .get(format!("http://127.0.0.1:{port}/cxf/test_profile"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_handles_malformed_soap() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();
    let port = reserve_local_port();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(cxf_component_with_bind(Some(port)))
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-malformed-test\n    from: \"cxf://http://127.0.0.1:{port}/cxf/test_profile?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=test_profile\"\n    steps:\n      - set_body:\n          value:\n            constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    // Wait for endpoint to be ready
    wait_until(
        "cxf consumer endpoint ready",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let url = format!("http://127.0.0.1:{port}/cxf/test_profile");
            async move {
                match http_client().get(&url).send().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    // Send POST with malformed XML body — bridge should handle gracefully, not crash
    let client = http_client();
    let res = client
        .post(format!("http://127.0.0.1:{port}/cxf/test_profile"))
        .header("content-type", "application/soap+xml; charset=utf-8")
        .body("this is not valid XML at all <><><>")
        .send()
        .await
        .expect("request should not panic");

    // Malformed envelopes on SOAP 1.2 are rejected with HTTP 400 (Sender fault,
    // hardened extraction, nothing forwarded); the key guarantee is a structured
    // rejection, no crash.
    let status = res.status();
    assert_eq!(
        status, 400,
        "Expected 400 Bad Request for malformed SOAP, got {status}"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_override_applies_on_soap11() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();
    let port = reserve_local_port();

    let pool = Arc::new(
        CxfBridgePool::from_config(CxfPoolConfig {
            profiles: vec![CxfProfileConfig {
                name: "esap".to_string(),
                address: None,
                wsdl_path: wsdl_path.clone(),
                service_name: "{http://example.com/hello}HelloService".to_string(),
                port_name: "{http://example.com/hello}HelloPort".to_string(),
                security: wss_security(Some(400)),
            }],
            max_bridges: 4,
            bridge_start_timeout_ms: 30_000,
            health_check_interval_ms: 5_000,
            bridge_cache_dir: None,
            version: camel_component_cxf::BRIDGE_VERSION.to_string(),
            bind_address: Some(format!("http://127.0.0.1:{port}/cxf")),
            listener_tls: None,
            reconnect: Default::default(),
        })
        .unwrap(),
    );

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(CxfComponent::new(pool))
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: soap11-override\n    from: \"cxf://http://127.0.0.1:{port}/cxf/esap?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=esap\"\n    steps:\n      - set_body:\n          value:\n            constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    // An unsigned SOAP 1.1 request cannot pass the required Signature action; the
    // profile's fault_status = 400 override applies on SOAP 1.1 (the default is 500).
    let url = format!("http://127.0.0.1:{port}/cxf/esap");
    let unsigned = soap_envelope(
        SOAP11_ENVELOPE_NS,
        "<hel:sayHello xmlns:hel=\"http://example.com/hello\"><name>x</name></hel:sayHello>",
    );
    wait_until(
        "soap11 WSS override status is ready",
        Duration::from_secs(15),
        Duration::from_millis(300),
        || {
            let url = url.clone();
            let unsigned = unsigned.clone();
            async move {
                match try_post_soap(&url, "text/xml; charset=utf-8", "sayHello", unsigned).await {
                    Some(res) => Ok(res.status() == StatusCode::BAD_REQUEST),
                    None => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    let res = post_soap(&url, "text/xml; charset=utf-8", "sayHello", unsigned).await;
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "SOAP 1.1 WSS fault must honour fault_status = 400 (not the default 500)"
    );
    let body = res.text().await.unwrap();
    assert!(
        body.contains("Fault") || body.contains("fault"),
        "expected a SOAP fault body, got: {body}"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_override_403_applies_to_wss_only() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();

    // --- Bridge A: esap, WSS with fault_status = 403 and a failing route ---
    let port_esap = reserve_local_port();
    let h_esap = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(wss_single_profile_component(
            "esap",
            Some(403),
            port_esap,
            &wsdl_path,
        ))
        .build()
        .await;
    // esap's route always fails (validate false) once WSS verification passes, so a
    // signed request proves the Receiver path is a route error, not a WSS fault.
    let yaml_esap = format!(
        "routes:\n  - id: esap-consumer\n    from: \"cxf://http://127.0.0.1:{port_esap}/cxf/esap?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=esap\"\n    steps:\n      - to: \"mock:esap-hit\"\n      - validate: \"${{body.size()}} > 100000000\"\n"
    );
    for route in parse_yaml(&yaml_esap).unwrap() {
        h_esap.add_route(route).await.unwrap();
    }
    h_esap.start().await;

    // --- Bridge B: cyl, WSS defaults and a success route (proves the signed path
    // verifies WSS and completes end-to-end) ---
    let port_cyl = reserve_local_port();
    let h_cyl = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(wss_single_profile_component(
            "cyl", None, port_cyl, &wsdl_path,
        ))
        .build()
        .await;
    let yaml_cyl = format!(
        "routes:\n  - id: cyl-consumer\n    from: \"cxf://http://127.0.0.1:{port_cyl}/cxf/cyl?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=cyl\"\n    steps:\n      - set_body:\n          value:\n            constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n"
    );
    for route in parse_yaml(&yaml_cyl).unwrap() {
        h_cyl.add_route(route).await.unwrap();
    }
    h_cyl.start().await;

    let esap_url = format!("http://127.0.0.1:{port_esap}/cxf/esap");
    let cyl_url = format!("http://127.0.0.1:{port_cyl}/cxf/cyl");

    let unsigned12 = soap_envelope(
        SOAP12_ENVELOPE_NS,
        "<hel:sayHello xmlns:hel=\"http://example.com/hello\"><name>x</name></hel:sayHello>",
    );

    // WSS fault on esap → 403 (override applies to WSS faults only).
    wait_until(
        "esap WSS override status is ready",
        Duration::from_secs(15),
        Duration::from_millis(300),
        || {
            let url = esap_url.clone();
            let body = unsigned12.clone();
            async move {
                match try_post_soap(&url, "application/soap+xml", "sayHello", body).await {
                    Some(res) => Ok(res.status() == StatusCode::FORBIDDEN),
                    None => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();
    let res = post_soap(
        &esap_url,
        "application/soap+xml",
        "sayHello",
        unsigned12.clone(),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "esap WSS fault must honour fault_status = 403"
    );
    let body = res.text().await.unwrap();
    assert!(
        body.contains("Fault") || body.contains("fault"),
        "esap WSS response must be a SOAP fault, got: {body}"
    );

    // Equivalent WSS fault on cyl → default Sender 400.
    let res = post_soap(&cyl_url, "application/soap+xml", "sayHello", unsigned12).await;
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "cyl WSS fault must keep the default 400"
    );

    // Malformed envelope on esap is a non-WSS Sender fault → 400, override untouched.
    let res = post_soap(
        &esap_url,
        "application/soap+xml",
        "sayHello",
        "this is not valid XML <><>".to_string(),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "malformed Sender fault must stay 400 (override must not apply)"
    );

    // A missing SOAP Body on esap is also a non-WSS Sender fault → 400 (detected
    // before WSS, so the override cannot rewrite it).
    let no_body = "<soapenv:Envelope xmlns:soapenv=\"http://www.w3.org/2003/05/soap-envelope\">\
                   <soapenv:Header/></soapenv:Envelope>";
    let res = post_soap(
        &esap_url,
        "application/soap+xml",
        "sayHello",
        no_body.to_string(),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "missing-Body Sender fault must stay 400 (override must not apply)"
    );

    // A genuinely signed SOAP 1.2 request: WSS verification passes, so the request
    // reaches the route. cyl's success route returns 200 (proves verification and the
    // live consumer stream); esap's route records the hit, then fails → Receiver 500
    // despite the WSS-only 403 override.
    let signed_body = std::fs::read_to_string(cxf_wss_signed_request_fixture())
        .expect("signed request fixture must be readable");
    assert!(
        signed_body.contains("Signature") && signed_body.contains("Security"),
        "signed request fixture must carry a WS-Security signature"
    );

    wait_until(
        "signed request passes WSS on cyl",
        Duration::from_secs(20),
        Duration::from_millis(300),
        || {
            let url = cyl_url.clone();
            let signed = signed_body.clone();
            async move {
                match try_post_soap(&url, "application/soap+xml", "sayHello", signed).await {
                    Some(res) => Ok(res.status() == StatusCode::OK),
                    None => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    let esap_hit = h_esap.mock().get_endpoint("esap-hit").unwrap();
    let res = post_soap(&esap_url, "application/soap+xml", "sayHello", signed_body).await;
    assert_eq!(
        res.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "signed request reaching the failing route must be a Receiver 500 (override must not apply)"
    );
    // Observe the actual route hit: the 500 is a route error, not a WSS/malformed mask.
    wait_until(
        "esap route observed the signed request",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let esap_hit = esap_hit.clone();
            async move { Ok(!esap_hit.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    // The signed fixture carries a misleading SOAP 1.1 Body nested in the Header and
    // an unreferenced by the signature. The route must receive the direct SOAP 1.2
    // Body (sayHelloRequest, the WSDL input element), never the nested Header payload.
    let hits = esap_hit.get_received_exchanges().await;
    let hit_body = hits[0].body_as::<String>().unwrap_or_default();
    assert!(
        hit_body.contains("sayHelloRequest"),
        "the direct SOAP 1.2 Body operation must reach the route, got: {hit_body}"
    );
    assert!(
        !hit_body.contains("header-body"),
        "the nested Header SOAP 1.1 Body must not reach the route, got: {hit_body}"
    );

    // Task 2.5 native positive metadata chain: the same verified signer is
    // stored as the four dedicated `camel.cxf.signer.*` properties with the
    // known fixture certificate values, and never as an auth principal.
    signer::assert_alice_signer_properties(&hits[0]);

    h_cyl.stop().await;
    h_esap.stop().await;
}

/// A native consumer-fault matrix row profile (no WSS: the request must reach the
/// IPC so the crafted `ConsumerResponse` drives the HTTP status).
fn matrix_profile(name: &str, wsdl_path: &str) -> CxfProfileEnvVars {
    CxfProfileEnvVars {
        name: name.to_string(),
        wsdl_path: wsdl_path.to_string(),
        service_name: "{http://example.com/hello}HelloService".to_string(),
        port_name: "{http://example.com/hello}HelloPort".to_string(),
        address: None,
        keystore_path: None,
        keystore_password: None,
        truststore_path: None,
        truststore_password: None,
        sig_username: None,
        sig_password: None,
        enc_username: None,
        security_actions_out: None,
        security_actions_in: None,
        signature_algorithm: None,
        signature_digest_algorithm: None,
        signature_c14n_algorithm: None,
        signature_parts: None,
        allowed_signature_algorithms: None,
        allowed_digest_algorithms: None,
        allow_expired_pinned_certificate: false,
        fault_status: None,
        tls: None,
    }
}

/// Asserts a SOAP 1.2 fault envelope's `Code/Value` is a prefixed QName whose
/// prefix is bound to the SOAP 1.2 namespace and whose local part is `expected_local`.
fn assert_soap12_fault_code(xml: &str, expected_local: &str) {
    use quick_xml::NsReader;
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;

    let mut reader = NsReader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_value = false;
    let mut value_ns: Option<String> = None;
    let mut value_prefix: Option<String> = None;
    let mut value_text = String::new();
    loop {
        match reader.read_resolved_event_into(&mut buf) {
            Ok((resolve, Event::Start(e))) => {
                if e.local_name().as_ref() == "Value" {
                    in_value = true;
                    if let ResolveResult::Bound(ns) = resolve {
                        value_ns = Some(ns.0.to_string());
                    }
                    let raw = e.name().as_ref().to_string();
                    value_prefix = raw.split_once(':').map(|(prefix, _)| prefix.to_string());
                }
            }
            Ok((_, Event::Text(t))) if in_value => {
                value_text.push_str(t.as_ref());
            }
            Ok((_, Event::End(e))) => {
                if e.local_name().as_ref() == "Value" {
                    break;
                }
            }
            Ok((_, Event::Eof)) => break,
            Err(err) => panic!("fault envelope is not well-formed XML: {err}"),
            _ => {}
        }
        buf.clear();
    }
    assert_eq!(
        value_ns.as_deref(),
        Some(SOAP12_ENVELOPE_NS),
        "Code/Value must be in the SOAP 1.2 namespace: {xml}"
    );
    let (prefix, local) = value_text
        .trim()
        .split_once(':')
        .unwrap_or_else(|| panic!("Code/Value must be a prefixed QName: {value_text}"));
    assert_eq!(
        Some(prefix),
        value_prefix.as_deref(),
        "QName prefix must match the bound Value prefix: {value_text}"
    );
    assert_eq!(local, expected_local, "unexpected fault code: {value_text}");
}

/// Task 2.2 Step 5 matrix on the real native consumer IPC: typed `ConsumerResponse`
/// fault codes and a withheld reply (timeout) drive the actual HTTP status and the
/// rendered, namespace-correct SOAP 1.2 fault code.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_fault_code_and_timeout_matrix() {
    init_tracing();
    install_crypto_provider();
    let binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();
    let port = reserve_local_port();

    let profiles: Vec<CxfProfileEnvVars> = [
        "m_client",
        "m_server",
        "m_version",
        "m_must",
        "m_data",
        "m_timeout",
    ]
    .iter()
    .map(|name| matrix_profile(name, &wsdl_path))
    .collect();

    // Per-child env (never global): a 150 ms consumer timeout makes the withheld
    // reply an actual production timeout.
    let mut config = BridgeProcessConfig::cxf_profiles(binary, &profiles, 30_000);
    config.env_vars.push((
        "CXF_ADDRESS".to_string(),
        format!("http://127.0.0.1:{port}/cxf"),
    ));
    config
        .env_vars
        .push(("CXF_CONSUMER_TIMEOUT_MS".to_string(), "150".to_string()));

    let (process, channel) = BridgeProcess::start_and_connect(&config).await.unwrap();

    let arrived: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // The bridge registers one consumer stream per profile and rejects an open
    // without `x-cxf-profile`. `open_consumer_stream` only resolves after the
    // bridge flushes response headers, which it defers until the first inbound
    // SOAP request for that profile is dispatched. Spawn one task per profile so
    // the test can POST and drive that first request.
    for profile_name in profiles.iter().map(|p| p.name.clone()) {
        let channel = channel.clone();
        let arrived_task = Arc::clone(&arrived);
        let (tx, rx) = futures::channel::mpsc::unbounded::<ConsumerResponse>();
        tokio::spawn(async move {
            let mut client = CxfBridgeClient::new(channel);
            let mut open_request = tonic::Request::new(rx);
            open_request.metadata_mut().insert(
                "x-cxf-profile",
                profile_name.parse().expect("profile metadata value"),
            );
            let response = client
                .open_consumer_stream(open_request)
                .await
                .expect("open consumer stream");
            let mut requests = response.into_inner();
            while let Ok(Some(req)) = requests.message().await {
                let profile = req.security_profile.clone();
                arrived_task
                    .lock()
                    .expect("arrived lock")
                    .push(profile.clone());
                if profile == "m_timeout" {
                    // Withhold the reply: the publisher's consumer timeout must fire.
                    continue;
                }
                let code = match profile.as_str() {
                    "m_client" => "soap:Client",
                    "m_server" => "soap:Server",
                    "m_version" => "soap:VersionMismatch",
                    "m_must" => "soap:MustUnderstand",
                    "m_data" => "soap:DataEncodingUnknown",
                    _ => "soap:Server",
                };
                let _ = tx.unbounded_send(ConsumerResponse {
                    request_id: req.request_id.clone(),
                    payload: Vec::new(),
                    fault: true,
                    fault_code: code.to_string(),
                    fault_string: "typed fault".to_string(),
                    security_profile: profile,
                });
            }
        });
    }

    let base = format!("http://127.0.0.1:{port}/cxf");
    wait_until(
        "matrix consumer endpoint ready",
        Duration::from_secs(15),
        Duration::from_millis(200),
        || {
            let url = format!("{base}/m_server");
            async move {
                match http_client().get(&url).send().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    let soap12 = soap_envelope(
        SOAP12_ENVELOPE_NS,
        "<hel:sayHelloRequest xmlns:hel=\"http://example.com/hello\"><name>x</name></hel:sayHelloRequest>",
    );
    let soap11 = soap_envelope(
        SOAP11_ENVELOPE_NS,
        "<hel:sayHelloRequest xmlns:hel=\"http://example.com/hello\"><name>x</name></hel:sayHelloRequest>",
    );

    for (profile, expected_local, expected_status) in [
        ("m_client", "Sender", 400u16),
        ("m_server", "Receiver", 500),
        ("m_version", "VersionMismatch", 500),
        ("m_must", "MustUnderstand", 500),
        ("m_data", "DataEncodingUnknown", 500),
    ] {
        let (res, _) = post_soap_when_registered(
            &format!("{base}/{profile}"),
            "application/soap+xml",
            "sayHello",
            &soap12,
        )
        .await;
        assert_eq!(res.status().as_u16(), expected_status, "{profile} status");
        let ctype = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ctype.contains("application/soap+xml"),
            "{profile} content-type must be SOAP 1.2, got: {ctype}"
        );
        let text = res.text().await.unwrap();
        assert_soap12_fault_code(&text, expected_local);
    }

    // Timeout: real production timeout, bounded by CXF_CONSUMER_TIMEOUT_MS.
    let (res, started) = post_soap_when_registered(
        &format!("{base}/m_timeout"),
        "application/soap+xml",
        "sayHello",
        &soap12,
    )
    .await;
    assert_eq!(res.status().as_u16(), 500, "timeout must be a Receiver 500");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "timeout must be bounded by the consumer timeout, took {:?}",
        started.elapsed()
    );
    let text = res.text().await.unwrap();
    assert_soap12_fault_code(&text, "Receiver");

    // SOAP 1.1: any fault class → 500.
    for profile in ["m_client", "m_server"] {
        let res = post_soap(
            &format!("{base}/{profile}"),
            "text/xml; charset=utf-8",
            "sayHello",
            soap11.clone(),
        )
        .await;
        assert_eq!(
            res.status().as_u16(),
            500,
            "SOAP 1.1 {profile} must be 500 regardless of fault class"
        );
    }

    // Every profile must have observed an actual ConsumerRequest on the stream.
    let seen = arrived.lock().expect("arrived lock").clone();
    for profile in [
        "m_client",
        "m_server",
        "m_version",
        "m_must",
        "m_data",
        "m_timeout",
    ] {
        assert!(
            seen.iter().any(|s| s == profile),
            "{profile} must arrive on the consumer stream, saw {seen:?}"
        );
    }

    process.stop().await.unwrap();
}

/// Formats a SOAP envelope in the given namespace around `body`.
fn soap_envelope(ns: &str, body: &str) -> String {
    format!(
        "<soapenv:Envelope xmlns:soapenv=\"{ns}\"><soapenv:Body>{body}</soapenv:Body></soapenv:Envelope>"
    )
}

/// POSTs a SOAP body with a hard client timeout; never panics on a protocol error.
async fn post_soap(
    url: &str,
    content_type: &str,
    soapaction: &str,
    body: String,
) -> reqwest::Response {
    http_client()
        .post(url)
        .header("content-type", content_type)
        .header("soapaction", soapaction)
        .body(body)
        .send()
        .await
        .expect("SOAP POST must not panic")
}

/// POSTs a SOAP body, retrying while the publisher answers 503 (the profile's
/// consumer stream is not registered yet; a 503 is sent before dispatch, so
/// retrying has no side effects). Returns the first non-503 response and the
/// instant that attempt started.
async fn post_soap_when_registered(
    url: &str,
    content_type: &str,
    soapaction: &str,
    body: &str,
) -> (reqwest::Response, Instant) {
    let slot: Arc<Mutex<Option<(reqwest::Response, Instant)>>> = Arc::new(Mutex::new(None));
    let slot_poll = Arc::clone(&slot);
    wait_until(
        "profile stream registered (non-503 SOAP response)",
        Duration::from_secs(15),
        Duration::from_millis(100),
        || {
            let slot = Arc::clone(&slot_poll);
            let body = body.to_string();
            async move {
                let started = Instant::now();
                let Some(res) = try_post_soap(url, content_type, soapaction, body).await else {
                    return Ok(false);
                };
                if res.status() == StatusCode::SERVICE_UNAVAILABLE {
                    return Ok(false);
                }
                *slot.lock().expect("slot lock") = Some((res, started));
                Ok(true)
            }
        },
    )
    .await
    .unwrap();
    let taken = slot.lock().expect("slot lock").take();
    taken.expect("response stored on success")
}

/// POSTs a SOAP body, returning `None` when the endpoint is not accepting
/// connections yet (used by readiness waits before the bridge has bound).
async fn try_post_soap(
    url: &str,
    content_type: &str,
    soapaction: &str,
    body: String,
) -> Option<reqwest::Response> {
    http_client()
        .post(url)
        .header("content-type", content_type)
        .header("soapaction", soapaction)
        .body(body)
        .send()
        .await
        .ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_concurrent_requests() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();
    let port = reserve_local_port();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(cxf_component_with_bind(Some(port)))
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-concurrent-test\n    from: \"cxf://http://127.0.0.1:{port}/cxf/test_profile?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=test_profile\"\n    steps:\n      - set_body:\n          value:\n            constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    // Wait for endpoint to be ready
    wait_until(
        "cxf consumer endpoint ready",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let url = format!("http://127.0.0.1:{port}/cxf/test_profile");
            async move {
                match http_client().get(&url).send().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    let soap_request = r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:hel="http://example.com/hello">
  <soapenv:Header/>
  <soapenv:Body>
    <hel:sayHelloRequest>
      <name>concurrent-user</name>
    </hel:sayHelloRequest>
  </soapenv:Body>
</soapenv:Envelope>"#;

    // Spawn 10 concurrent requests
    let client = http_client();
    let mut handles = Vec::new();
    for _ in 0..10 {
        let client = client.clone();
        let url = format!("http://127.0.0.1:{port}/cxf/test_profile");
        let body = soap_request.to_string();
        let handle = tokio::spawn(async move {
            let res = client
                .post(&url)
                .header("content-type", "text/xml; charset=utf-8")
                .header("soapaction", "sayHello")
                .body(body)
                .send()
                .await
                .unwrap();
            let status = res.status();
            let text = res.text().await.unwrap();
            (status, text)
        });
        handles.push(handle);
    }

    // Collect all results
    let results: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|h| h.unwrap())
        .collect();

    // Verify all 10 got 200 OK
    for (i, (status, _text)) in results.iter().enumerate() {
        assert_eq!(
            *status,
            StatusCode::OK,
            "Request {i} failed with status {status}"
        );
    }

    // Verify all 10 responses contain sayHelloResponse
    for (i, (_status, text)) in results.iter().enumerate() {
        assert!(
            text.contains("sayHelloResponse"),
            "Request {i} response missing sayHelloResponse"
        );
    }

    // Wait for mock endpoint to have received all 10 exchanges
    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "cxf concurrent requests all delivered",
        Duration::from_secs(15),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 10) }
        },
    )
    .await
    .unwrap();

    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        exchanges.len() >= 10,
        "Expected at least 10 exchanges at mock endpoint, got {}",
        exchanges.len()
    );

    h.stop().await;
}

// ── Multi-Profile Tests ──────────────────────────────────────────────────────

/// Helper that creates a CxfComponent with two profiles: "community_a" and "community_b".
/// Each profile has a distinct address (different ports).
fn multi_profile_cxf_component(port_a: u16, port_b: u16) -> CxfComponent {
    let wsdl_path = cxf_wsdl_path();
    let pool = Arc::new(
        CxfBridgePool::from_config(CxfPoolConfig {
            profiles: vec![
                CxfProfileConfig {
                    name: "community_a".to_string(),
                    address: Some(format!("http://127.0.0.1:{port_a}/service")),
                    wsdl_path: wsdl_path.clone(),
                    service_name: "{http://example.com/hello}HelloService".to_string(),
                    port_name: "{http://example.com/hello}HelloPort".to_string(),
                    security: Default::default(),
                },
                CxfProfileConfig {
                    name: "community_b".to_string(),
                    address: Some(format!("http://127.0.0.1:{port_b}/service")),
                    wsdl_path,
                    service_name: "{http://example.com/hello}HelloService".to_string(),
                    port_name: "{http://example.com/hello}HelloPort".to_string(),
                    security: Default::default(),
                },
            ],
            max_bridges: 4,
            bridge_start_timeout_ms: 30_000,
            health_check_interval_ms: 5_000,
            bridge_cache_dir: None,
            version: camel_component_cxf::BRIDGE_VERSION.to_string(),
            bind_address: None,
            listener_tls: None,
            reconnect: Default::default(),
        })
        .unwrap(),
    );
    CxfComponent::new(pool)
}

/// Starts a mock SOAP service that returns a custom response body inside a SOAP envelope.
async fn start_mock_soap_service_with_response(response_body: &'static str) -> SocketAddr {
    let app = Router::new().route(
        "/service",
        post(move |_body: String| async move {
            (
                [("content-type", "text/xml; charset=utf-8")],
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:hel="http://example.com/hello">
  <soapenv:Header/>
  <soapenv:Body>
    {response_body}
  </soapenv:Body>
</soapenv:Envelope>"#,
                ),
            )
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Verifies that two profiles coexist in one pool and that each profile's producer
/// routes to the correct backend (profile A → mock A, profile B → mock B).
#[tokio::test(flavor = "multi_thread")]
async fn cxf_multi_profile_producer_routes_to_correct_backend() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();

    // Start two separate mock SOAP services with distinct responses
    let addr_a = start_mock_soap_service_with_response(
        "<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>community_a_pong</return></hel:sayHelloResponse>",
    )
    .await;
    let addr_b = start_mock_soap_service_with_response(
        "<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>community_b_pong</return></hel:sayHelloResponse>",
    )
    .await;

    let component = multi_profile_cxf_component(addr_a.port(), addr_b.port());
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(component)
        .build()
        .await;

    // Route for profile community_a → sends to mock on addr_a
    let yaml_a = format!(
        "routes:\n  - id: profile-a-route\n    from: direct:start_a\n    steps:\n      - to: \"cxf://http://{addr_a}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=community_a\"\n      - to: \"mock:done_a\"\n"
    );
    for route in parse_yaml(&yaml_a).unwrap() {
        h.add_route(route).await.unwrap();
    }

    // Route for profile community_b → sends to mock on addr_b
    let yaml_b = format!(
        "routes:\n  - id: profile-b-route\n    from: direct:start_b\n    steps:\n      - to: \"cxf://http://{addr_b}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=community_b\"\n      - to: \"mock:done_b\"\n"
    );
    for route in parse_yaml(&yaml_b).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    // Send request via profile A
    let exchange_a = Exchange::new(Message::new(Body::Text(
        r#"<hel:sayHello xmlns:hel="http://example.com/hello"><name>a</name></hel:sayHello>"#
            .to_string(),
    )));
    let _ = send_to_direct(&h, "direct:start_a", exchange_a)
        .await
        .unwrap();

    // Send request via profile B
    let exchange_b = Exchange::new(Message::new(Body::Text(
        r#"<hel:sayHello xmlns:hel="http://example.com/hello"><name>b</name></hel:sayHello>"#
            .to_string(),
    )));
    let _ = send_to_direct(&h, "direct:start_b", exchange_b)
        .await
        .unwrap();

    // Verify profile A response arrived
    let endpoint_a = h.mock().get_endpoint("done_a").unwrap();
    wait_until(
        "profile A delivery",
        Duration::from_secs(5),
        Duration::from_millis(200),
        || {
            let endpoint_a = endpoint_a.clone();
            async move { Ok(!endpoint_a.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    // Verify profile B response arrived
    let endpoint_b = h.mock().get_endpoint("done_b").unwrap();
    wait_until(
        "profile B delivery",
        Duration::from_secs(5),
        Duration::from_millis(200),
        || {
            let endpoint_b = endpoint_b.clone();
            async move { Ok(!endpoint_b.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    // Verify profile A response contains community_a_pong
    let exchanges_a = endpoint_a.get_received_exchanges().await;
    assert_eq!(exchanges_a.len(), 1);
    let body_a = exchanges_a[0].body_as::<String>().unwrap_or_default();
    assert!(
        body_a.contains("community_a_pong"),
        "Profile A response should contain 'community_a_pong', got: {body_a}"
    );

    // Verify profile B response contains community_b_pong
    let exchanges_b = endpoint_b.get_received_exchanges().await;
    assert_eq!(exchanges_b.len(), 1);
    let body_b = exchanges_b[0].body_as::<String>().unwrap_or_default();
    assert!(
        body_b.contains("community_b_pong"),
        "Profile B response should contain 'community_b_pong', got: {body_b}"
    );

    h.stop().await;
}

/// Verifies that a URI without `profile=` parameter is rejected at endpoint creation.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_multi_profile_rejects_uri_without_profile() {
    init_tracing();
    install_crypto_provider();

    let component = multi_profile_cxf_component(9090, 9091);
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(component)
        .build()
        .await;

    // Route without profile= parameter — should fail during route building
    let wsdl_path = cxf_wsdl_path();
    let yaml = format!(
        "routes:\n  - id: no-profile-route\n    from: direct:start\n    steps:\n      - to: \"cxf://http://127.0.0.1:9090/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello\"\n      - to: \"mock:done\"\n"
    );

    let routes = parse_yaml(&yaml).unwrap();
    let result = h.add_route(routes.into_iter().next().unwrap()).await;

    assert!(
        result.is_err(),
        "Route without profile= should be rejected, but it succeeded"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("profile"),
        "Error should mention 'profile', got: {err_msg}"
    );
}

/// Verifies that a URI referencing an unknown profile name is rejected.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_multi_profile_rejects_unknown_profile() {
    init_tracing();
    install_crypto_provider();

    let component = multi_profile_cxf_component(9090, 9091);
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(component)
        .build()
        .await;

    let wsdl_path = cxf_wsdl_path();
    let yaml = format!(
        "routes:\n  - id: unknown-profile-route\n    from: direct:start\n    steps:\n      - to: \"cxf://http://127.0.0.1:9090/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=nonexistent\"\n      - to: \"mock:done\"\n"
    );

    let routes = parse_yaml(&yaml).unwrap();
    let result = h.add_route(routes.into_iter().next().unwrap()).await;

    assert!(
        result.is_err(),
        "Route with unknown profile should be rejected, but it succeeded"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("unknown profile"),
        "Error should mention 'unknown profile', got: {err_msg}"
    );
}
