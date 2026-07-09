#![cfg(feature = "integration-tests")]

mod support;

use camel_api::{Body, Exchange, Message};
use camel_bridge::process::{BridgeProcess, BridgeProcessConfig, CxfProfileEnvVars};
use camel_component_cxf::proto::{HealthRequest, cxf_bridge_client::CxfBridgeClient};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{Router, routing::post};
use camel_component_cxf::{CxfBridgePool, CxfComponent, CxfPoolConfig, CxfProfileConfig};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use reqwest::StatusCode;
use support::bridge_bg_rt;
use support::cxf::require_cxf_bridge_binary;
use support::send_to_direct;
use support::wait::wait_until;
use tokio::sync::OnceCell;

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
            reconnect: Default::default(),
        })
        .unwrap(),
    );
    CxfComponent::new(pool)
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

fn reserve_local_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_invokes_mock_soap_service() {
    init_tracing();
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
        "routes:\n  - id: cxf-consumer-test\n    from: \"cxf://http://127.0.0.1:{port}/cxf/test_profile?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=test_profile\"\n    steps:\n      - set_body:\n          constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
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
                match reqwest::Client::new().get(&url).send().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    let client = reqwest::Client::new();
    let res = client
        .post(format!("http://127.0.0.1:{port}/cxf/test_profile"))
        .header("content-type", "text/xml; charset=utf-8")
        .header("soapaction", "sayHello")
        .body(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:hel="http://example.com/hello">
  <soapenv:Header/>
  <soapenv:Body>
    <hel:sayHello>
      <name>hello</name>
    </hel:sayHello>
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
async fn cxf_native_health_check_responds_within_5s() {
    init_tracing();
    let binary = require_cxf_bridge_binary();

    let wsdl_path = cxf_wsdl_path();

    let profiles = vec![CxfProfileEnvVars {
        name: "test".to_string(),
        wsdl_path: wsdl_path.to_string_lossy().to_string(),
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
        "routes:\n  - id: cxf-health-test\n    from: \"cxf://http://127.0.0.1:{port}/cxf/test_profile?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=test_profile\"\n    steps:\n      - set_body:\n          constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
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
                match reqwest::Client::new().get(&url).send().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    // Send GET request — should return health check (200 OK)
    let client = reqwest::Client::new();
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
        "routes:\n  - id: cxf-malformed-test\n    from: \"cxf://http://127.0.0.1:{port}/cxf/test_profile?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=test_profile\"\n    steps:\n      - set_body:\n          constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
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
                match reqwest::Client::new().get(&url).send().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    // Send POST with malformed XML body — bridge should handle gracefully, not crash
    let client = reqwest::Client::new();
    let res = client
        .post(format!("http://127.0.0.1:{port}/cxf/test_profile"))
        .header("content-type", "text/xml; charset=utf-8")
        .body("this is not valid XML at all <><><>")
        .send()
        .await
        .expect("request should not panic");

    // Response should be either server error (5xx) or 200 with error info — key is no crash
    let status = res.status();
    assert!(
        status.is_server_error() || status.is_success(),
        "Expected 2xx or 5xx for malformed SOAP, got {status}"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cxf_consumer_concurrent_requests() {
    init_tracing();
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
        "routes:\n  - id: cxf-concurrent-test\n    from: \"cxf://http://127.0.0.1:{port}/cxf/test_profile?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=test_profile\"\n    steps:\n      - set_body:\n          constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
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
                match reqwest::Client::new().get(&url).send().await {
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
    <hel:sayHello>
      <name>concurrent-user</name>
    </hel:sayHello>
  </soapenv:Body>
</soapenv:Envelope>"#;

    // Spawn 10 concurrent requests
    let client = reqwest::Client::new();
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
                    wsdl_path: wsdl_path,
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
