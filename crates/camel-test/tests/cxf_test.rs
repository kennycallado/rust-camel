#![cfg(feature = "integration-tests")]

mod support;

use camel_api::{Body, Exchange, Message};
use camel_component_cxf::proto::{HealthRequest, cxf_bridge_client::CxfBridgeClient};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{Router, routing::post};
use camel_component_cxf::{CxfBridgePool, CxfComponent, CxfPoolConfig};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use reqwest::StatusCode;
use support::cxf::require_cxf_bridge_binary;
use support::send_to_direct;
use support::wait::wait_until;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

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

fn shared_cxf_component() -> CxfComponent {
    let pool = Arc::new(CxfBridgePool::from_config(CxfPoolConfig::default()).unwrap());
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

#[tokio::test]
async fn cxf_producer_invokes_mock_soap_service() {
    init_tracing();
    let _binary = require_cxf_bridge_binary();
    let addr = start_mock_soap_service().await;
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component())
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-producer-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello\"\n      - to: \"mock:done\"\n"
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

#[tokio::test]
async fn cxf_consumer_receives_request_and_returns_response() {
    init_tracing();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();
    let port = reserve_local_port();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component())
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-consumer-test\n    from: \"cxf://http://127.0.0.1:{port}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort\"\n    steps:\n      - set_body:\n          constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
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
            let url = format!("http://127.0.0.1:{port}/service");
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
        .post(format!("http://127.0.0.1:{port}/service"))
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

#[tokio::test]
async fn cxf_native_health_check_responds_within_5s() {
    init_tracing();
    let binary = require_cxf_bridge_binary();

    let mut child = Command::new(binary)
        .env("CXF_WSDL_PATH", cxf_wsdl_path())
        .env("CXF_SERVICE_NAME", "{http://example.com/hello}HelloService")
        .env("CXF_PORT_NAME", "{http://example.com/hello}HelloPort")
        .env("QUARKUS_HTTP_PORT", "0")
        .env("QUARKUS_GRPC_SERVER_PORT", "0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();

    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let grpc_port = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(line) = lines.next_line().await.unwrap() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                && v.get("status").and_then(|s| s.as_str()) == Some("ready")
            {
                return v.get("port").and_then(|p| p.as_u64()).map(|p| p as u16);
            }
        }
        None
    })
    .await
    .unwrap()
    .unwrap();

    wait_until(
        "cxf native health check",
        Duration::from_secs(5),
        Duration::from_millis(100),
        || async {
            let mut client = CxfBridgeClient::connect(format!("http://127.0.0.1:{grpc_port}"))
                .await
                .map_err(|e| e.to_string())?;
            let health = client
                .health(HealthRequest {})
                .await
                .map_err(|e| e.to_string())?
                .into_inner();
            Ok(health.healthy)
        },
    )
    .await
    .unwrap();

    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[tokio::test]
async fn cxf_producer_handles_soap_fault_response() {
    init_tracing();
    let _binary = require_cxf_bridge_binary();
    let addr = start_mock_soap_fault_service().await;
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component())
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-fault-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello\"\n      - to: \"mock:done\"\n"
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

#[tokio::test]
async fn cxf_producer_multiple_sequential_invocations() {
    init_tracing();
    let _binary = require_cxf_bridge_binary();
    let addr = start_mock_soap_service().await;
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component())
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-multi-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello\"\n      - to: \"mock:done\"\n"
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

#[tokio::test]
async fn cxf_consumer_returns_health_check_on_get() {
    init_tracing();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();
    let port = reserve_local_port();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component())
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-health-test\n    from: \"cxf://http://127.0.0.1:{port}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort\"\n    steps:\n      - set_body:\n          constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
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
            let url = format!("http://127.0.0.1:{port}/service");
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
        .get(format!("http://127.0.0.1:{port}/service"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    h.stop().await;
}

#[tokio::test]
async fn cxf_consumer_handles_malformed_soap() {
    init_tracing();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();
    let port = reserve_local_port();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component())
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-malformed-test\n    from: \"cxf://http://127.0.0.1:{port}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort\"\n    steps:\n      - set_body:\n          constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
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
            let url = format!("http://127.0.0.1:{port}/service");
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
        .post(format!("http://127.0.0.1:{port}/service"))
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

#[tokio::test]
async fn cxf_consumer_concurrent_requests() {
    init_tracing();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();
    let port = reserve_local_port();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component())
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-concurrent-test\n    from: \"cxf://http://127.0.0.1:{port}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort\"\n    steps:\n      - set_body:\n          constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
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
            let url = format!("http://127.0.0.1:{port}/service");
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
        let url = format!("http://127.0.0.1:{port}/service");
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
