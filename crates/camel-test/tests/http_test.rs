//! Integration tests for the HTTP component (camel-component-http).
//!
//! Exercises the full pipeline: CamelContext → HttpConsumer → Processors → Producer,
//! verifying that exchanges flow end-to-end correctly through the HTTP server/consumer.
//!
//! Requires `integration-tests` feature to compile and run.

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;
use support::stage_http_listener;

use std::sync::Arc;
use std::time::Duration;

use camel_api::security_policy::{CredentialSource, Principal, SecurityPolicyConfig};
use camel_api::{Exchange, Message, Value};
use camel_auth::native_auth::NativeCredentialStore;
use camel_auth::{
    NativeCredential, NativeCredentialSecret, RolePolicy, StaticTokenAuthenticator,
    TokenAuthenticator,
};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{CamelError, Component, ComponentBundle, NoOpComponentContext};
use camel_component_http::{HttpBundle, HttpComponent, HttpsComponent};
use camel_dsl::{SecurityCompileContext, parse_yaml_with_threshold_and_security};
use camel_test::CamelTestContext;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test 1: Consumer lifecycle — create endpoint, start, verify, stop, cleanup
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_consumer_lifecycle_start_stop_cleanup() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from(&format!("http://127.0.0.1:{port}/lifecycle"))
        .route_id("http-lifecycle")
        .set_body(Value::String("handled".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    // Give the server time to bind
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify the server is listening
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/lifecycle"))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(resp.status(), 200);

    h.stop().await;

    // After stop, the path is deregistered — request should get 404
    let resp = client
        .get(format!("http://127.0.0.1:{port}/lifecycle"))
        .send()
        .await
        .expect("request should still reach the server");
    assert_eq!(resp.status(), 404);

    h.mock()
        .get_endpoint("result")
        .unwrap()
        .assert_exchange_count(1)
        .await;
}

// ---------------------------------------------------------------------------
// Test 2: Config resolution — TOML defaults + URI override
// ---------------------------------------------------------------------------

#[test]
fn http_config_toml_defaults() {
    use camel_component_http::HttpConfig;

    let config = HttpConfig::default();
    assert_eq!(config.connect_timeout_ms, 5_000);
    assert_eq!(config.max_request_body, 2_097_152);
    assert_eq!(config.max_body_size, 10_485_760);
    assert!(!config.follow_redirects);
    assert!(config.blocked_hosts.is_empty());
}

#[test]
fn http_config_from_toml_custom_values() {
    use camel_component_http::HttpConfig;

    let toml_str = r#"
        connect_timeout_ms = 1000
        max_request_body = 5242880
        max_body_size = 20971520
    "#;
    let value: toml::Value = toml::from_str(toml_str).unwrap();
    let config: HttpConfig = value.try_into().unwrap();
    assert_eq!(config.connect_timeout_ms, 1000);
    assert_eq!(config.max_request_body, 5_242_880);
    assert_eq!(config.max_body_size, 20_971_520);
}

// ---------------------------------------------------------------------------
// Test 3: Multiple mounts — two HTTP consumers on different paths, same port
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_multiple_mounts_same_port_isolation() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route_a = RouteBuilder::from(&format!("http://127.0.0.1:{port}/api/a"))
        .route_id("http-mount-a")
        .set_body(Value::String("route-a".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to("mock:result-a")
        .build()
        .unwrap();

    let route_b = RouteBuilder::from(&format!("http://127.0.0.1:{port}/api/b"))
        .route_id("http-mount-b")
        .set_body(Value::String("route-b".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to("mock:result-b")
        .build()
        .unwrap();

    h.add_route(route_a).await.unwrap();
    h.add_route(route_b).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();

    // Request to /api/a hits route A
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/a"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("route-a"),
        "body should contain route-a, got: {body}"
    );

    // Request to /api/b hits route B
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/b"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("route-b"),
        "body should contain route-b, got: {body}"
    );

    // Request to /api/c hits neither — 404
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/c"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    h.mock()
        .get_endpoint("result-a")
        .unwrap()
        .assert_exchange_count(1)
        .await;
    h.mock()
        .get_endpoint("result-b")
        .unwrap()
        .assert_exchange_count(1)
        .await;
}

// ---------------------------------------------------------------------------
// Test 4: Bundle registration — HttpBundle registers http and https schemes
// ---------------------------------------------------------------------------

#[test]
fn http_bundle_registers_http_and_https_schemes() {
    struct TestRegistrar {
        schemes: Vec<String>,
    }
    impl camel_component_api::ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(
            &mut self,
            component: std::sync::Arc<dyn camel_component_api::Component>,
        ) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    let bundle = HttpBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap();
    let mut registrar = TestRegistrar { schemes: vec![] };
    bundle.register_all(&mut registrar);
    assert_eq!(registrar.schemes, vec!["http", "https"]);
}

// ---------------------------------------------------------------------------
// Test 5: Empty config — from_toml("") succeeds with defaults
// ---------------------------------------------------------------------------

#[test]
fn http_bundle_empty_config_uses_defaults() {
    let value: toml::Value = toml::from_str("").unwrap();
    let result = HttpBundle::from_toml(value);
    assert!(result.is_ok(), "empty toml must use defaults");
}

// ---------------------------------------------------------------------------
// Test 6: Invalid config — from_toml with garbage returns error
// ---------------------------------------------------------------------------

#[test]
fn http_bundle_invalid_config_returns_error() {
    let mut table = toml::map::Map::new();
    table.insert(
        "connect_timeout_ms".to_string(),
        toml::Value::String("not-a-number".to_string()),
    );
    let result = HttpBundle::from_toml(toml::Value::Table(table));
    match result {
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("Configuration error"),
                "expected CamelError::Config, got: {err_msg}"
            );
        }
        Ok(_) => panic!("expected Err on malformed config"),
    }
}

// ---------------------------------------------------------------------------
// Test 7: Health check — HttpHealthCheck works with a real listener
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_health_check_with_real_listener() {
    install_crypto_provider();
    use camel_api::{AsyncHealthCheck, HealthStatus};
    use camel_component_http::HttpHealthCheck;

    // Bind a listener so the health check can connect
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    // Drop the listener immediately — health check should fail
    drop(listener);

    let check = HttpHealthCheck::new("127.0.0.1".to_string(), port);
    let result = check.check().await;
    assert_eq!(result.name, "http");
    assert_eq!(result.status, HealthStatus::Unhealthy);
}

// ---------------------------------------------------------------------------
// Test 8: Bearer token extraction — auth module
// ---------------------------------------------------------------------------

#[test]
fn http_bearer_token_extraction() {
    use camel_auth::extract_bearer_token;

    // `camel_auth::extract_bearer_token` parses a header *value* string, so
    // pull the `Authorization` header out of the map first (the deleted
    // `camel_component_http::auth` module did this for us).
    fn extract(headers: &http::HeaderMap) -> Result<Option<String>, CamelError> {
        let value = headers
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        extract_bearer_token(value)
    }

    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        "Bearer my-token".parse().unwrap(),
    );
    let result = extract(&headers).unwrap();
    assert_eq!(result, Some("my-token".to_string()));

    // Missing header
    let headers = http::HeaderMap::new();
    let result = extract(&headers).unwrap();
    assert!(result.is_none());

    // Wrong scheme
    let mut headers = http::HeaderMap::new();
    headers.insert(http::header::AUTHORIZATION, "Basic abc".parse().unwrap());
    let result = extract(&headers);
    assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
}

// ---------------------------------------------------------------------------
// Test 9: Request/response flow — HTTP server receives request, routes process it
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_request_response_flow() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from(&format!("http://127.0.0.1:{port}/echo"))
        .route_id("http-echo")
        .set_header("CamelHttpResponseCode", Value::Number(201.into()))
        .set_header("X-Custom-Header", Value::String("custom-value".into()))
        .to("mock:echo")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/echo"))
        .body("hello world")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201);
    assert_eq!(
        resp.headers().get("x-custom-header").unwrap(),
        "custom-value"
    );

    h.mock()
        .get_endpoint("echo")
        .unwrap()
        .assert_exchange_count(1)
        .await;
}

// ---------------------------------------------------------------------------
// Test 11: Error handling — pipeline error returns 500
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_pipeline_error_returns_500() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from(&format!("http://127.0.0.1:{port}/fail"))
        .route_id("http-fail")
        .process(|_ex| async { Err(CamelError::ProcessorError("intentional failure".into())) })
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/fail"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
}

// ---------------------------------------------------------------------------
// Test 12: Concurrent requests — multiple simultaneous connections
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_concurrent_requests() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from(&format!("http://127.0.0.1:{port}/concurrent"))
        .route_id("http-concurrent")
        .set_body(Value::String("ok".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to("mock:concurrent")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    for i in 0..10 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            let resp = client
                .get(format!("http://127.0.0.1:{port}/concurrent?i={i}"))
                .send()
                .await
                .unwrap();
            resp.status().as_u16()
        }));
    }

    let results: Vec<u16> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|h| h.unwrap())
        .collect();

    assert!(
        results.iter().all(|&s| s == 200),
        "all requests should succeed: {results:?}"
    );

    h.mock()
        .get_endpoint("concurrent")
        .unwrap()
        .assert_exchange_count(10)
        .await;
}

// ---------------------------------------------------------------------------
// Test 13: Shutdown behavior — graceful cleanup on stop
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_shutdown_deregisters_path() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from(&format!("http://127.0.0.1:{port}/shutdown"))
        .route_id("http-shutdown")
        .set_body(Value::String("alive".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to("mock:shutdown")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/shutdown"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    h.stop().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Path should be deregistered after stop
    let resp = client
        .get(format!("http://127.0.0.1:{port}/shutdown"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Test 14: SSRF protection — private IP blocking
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_producer_blocks_localhost_by_default() {
    install_crypto_provider();
    let ctx = camel_component_api::ProducerContext::new();
    let component = HttpComponent::new();
    let endpoint_ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint("http://example.com/api", &endpoint_ctx)
        .unwrap();
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ctx)
        .unwrap();

    let mut exchange = Exchange::new(Message::default());
    exchange.input.set_header(
        "CamelHttpUri",
        Value::String("http://localhost:8080/internal".to_string()),
    );

    let result = producer.oneshot(exchange).await;
    assert!(result.is_err(), "Should block localhost by default");
    assert!(result.unwrap_err().to_string().contains("not allowed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn http_producer_allows_localhost_when_configured() {
    install_crypto_provider();
    // Start a simple echo server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let actual_port = listener.local_addr().unwrap().port();
    let _handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let body = r#"{"ok":true}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    });

    let ctx = camel_component_api::ProducerContext::new();
    let component = HttpComponent::new();
    let endpoint_ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint(
            &format!("http://127.0.0.1:{actual_port}/test?allowInternal=true"),
            &endpoint_ctx,
        )
        .unwrap();
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ctx)
        .unwrap();

    let exchange = Exchange::new(Message::default());
    let result = producer.oneshot(exchange).await.unwrap();
    let status = result
        .input
        .header("CamelHttpResponseCode")
        .and_then(|v| v.as_u64())
        .unwrap();
    assert_eq!(status, 200);
}

// ---------------------------------------------------------------------------
// Test 15: Status code range — custom okStatusCodeRange
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_custom_ok_status_code_range() {
    install_crypto_provider();
    // Start a server that returns 204
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let actual_port = listener.local_addr().unwrap().port();
    let _handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            if let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let _ = stream.read(&mut buf).await;
                    let resp = "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        }
    });

    let ctx = camel_component_api::ProducerContext::new();
    let component = HttpComponent::new();
    let endpoint_ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint(
            &format!(
                "http://127.0.0.1:{actual_port}/test?allowInternal=true&okStatusCodeRange=200-204"
            ),
            &endpoint_ctx,
        )
        .unwrap();
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ctx)
        .unwrap();

    let exchange = Exchange::new(Message::default());
    let result = producer.oneshot(exchange).await;
    assert!(result.is_ok(), "204 should be within 200-204 range");
}

// ---------------------------------------------------------------------------
// Test 16: Header propagation — HTTP headers forwarded to exchange
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_headers_forwarded_to_exchange() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from(&format!("http://127.0.0.1:{port}/headers"))
        .route_id("http-headers")
        .process(|mut ex| async move {
            // Verify HTTP headers are present
            assert!(
                ex.input.header("CamelHttpMethod").is_some(),
                "CamelHttpMethod should be set"
            );
            assert!(
                ex.input.header("CamelHttpPath").is_some(),
                "CamelHttpPath should be set"
            );
            ex.input
                .set_header("CamelHttpResponseCode", Value::Number(200.into()));
            Ok(ex)
        })
        .to("mock:headers")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/headers"))
        .header("X-Test-Header", "test-value")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// ---------------------------------------------------------------------------
// Test 17: HTTPS component scheme
// ---------------------------------------------------------------------------

#[test]
fn https_component_scheme_is_https() {
    let component = HttpsComponent::new();
    assert_eq!(component.scheme(), "https");
}

#[test]
fn http_component_scheme_is_http() {
    let component = HttpComponent::new();
    assert_eq!(component.scheme(), "http");
}

// ---------------------------------------------------------------------------
// Test 18: Endpoint creation from URI
// ---------------------------------------------------------------------------

#[test]
fn http_endpoint_created_from_uri() {
    let component = HttpComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint("http://0.0.0.0:19200/test", &ctx)
        .unwrap();
    assert_eq!(endpoint.uri(), "http://0.0.0.0:19200/test");
    assert!(
        endpoint
            .create_consumer(Arc::new(NoOpComponentContext))
            .is_ok()
    );
}

#[test]
fn https_endpoint_created_from_uri() {
    let component = HttpsComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint(
            "https://0.0.0.0:8443/test?tlsCert=/a/cert.pem&tlsKey=/a/key.pem",
            &ctx,
        )
        .unwrap();
    assert_eq!(
        endpoint.uri(),
        "https://0.0.0.0:8443/test?tlsCert=/a/cert.pem&tlsKey=/a/key.pem"
    );
    assert!(
        endpoint
            .create_consumer(Arc::new(NoOpComponentContext))
            .is_ok()
    );
}

// ---------------------------------------------------------------------------
// Test 19: HTTP server config parsing
// ---------------------------------------------------------------------------

#[test]
fn http_server_config_parses_host_port_path() {
    use camel_component_api::UriConfig;
    use camel_component_http::HttpServerConfig;

    let config = HttpServerConfig::from_uri("http://0.0.0.0:8080/api/v1").unwrap();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 8080);
    assert_eq!(config.path, "/api/v1");
}

#[test]
fn http_server_config_default_port_for_http() {
    use camel_component_api::UriConfig;
    use camel_component_http::HttpServerConfig;

    let config = HttpServerConfig::from_uri("http://example.com/").unwrap();
    assert_eq!(config.host, "example.com");
    assert_eq!(config.port, 80);
}

#[test]
fn http_server_config_default_port_for_https() {
    use camel_component_api::UriConfig;
    use camel_component_http::HttpServerConfig;

    let config = HttpServerConfig::from_uri("https://example.com/").unwrap();
    assert_eq!(config.host, "example.com");
    assert_eq!(config.port, 443);
}

#[test]
fn http_server_config_invalid_port() {
    use camel_component_api::UriConfig;
    use camel_component_http::HttpServerConfig;

    let result = HttpServerConfig::from_uri("http://example.com:abc/");
    assert!(result.is_err());
}

#[test]
fn http_server_config_wrong_scheme() {
    use camel_component_api::UriConfig;
    use camel_component_http::HttpServerConfig;

    let result = HttpServerConfig::from_uri("file:/tmp");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Test 20: HTTP endpoint config parsing
// ---------------------------------------------------------------------------

#[test]
fn http_endpoint_config_parses_base_url() {
    use camel_component_api::UriConfig;
    use camel_component_http::HttpEndpointConfig;

    let config = HttpEndpointConfig::from_uri("http://api.example.com/v1/users").unwrap();
    assert_eq!(config.base_url, "http://api.example.com/v1/users");
}

#[test]
fn http_endpoint_config_query_params() {
    use camel_component_api::UriConfig;
    use camel_component_http::HttpEndpointConfig;

    let config =
        HttpEndpointConfig::from_uri("http://example.com/api?apiKey=secret&httpMethod=GET")
            .unwrap();
    assert!(config.query_params.contains_key("apiKey"));
    assert!(!config.query_params.contains_key("httpMethod")); // Camel option, not forwarded
}

#[test]
fn http_endpoint_config_auth_basic() {
    use camel_component_api::UriConfig;
    use camel_component_http::{HttpAuth, HttpEndpointConfig};

    let config = HttpEndpointConfig::from_uri(
        "http://example.com/api?authMethod=Basic&authUsername=user&authPassword=pass",
    )
    .unwrap();
    assert!(matches!(
        config.auth,
        HttpAuth::Basic { username, password } if username == "user" && password == "pass"
    ));
}

#[test]
fn http_endpoint_config_auth_bearer() {
    use camel_component_api::UriConfig;
    use camel_component_http::{HttpAuth, HttpEndpointConfig};

    let config = HttpEndpointConfig::from_uri(
        "http://example.com/api?authMethod=Bearer&authBearerToken=token123",
    )
    .unwrap();
    assert!(matches!(
        config.auth,
        HttpAuth::Bearer { token } if token == "token123"
    ));
}

#[test]
fn http_endpoint_config_invalid_auth() {
    use camel_component_api::UriConfig;
    use camel_component_http::HttpEndpointConfig;

    let result =
        HttpEndpointConfig::from_uri("http://example.com/api?authMethod=Basic&authUsername=user");
    assert!(result.is_err()); // missing authPassword
}

#[test]
fn http_endpoint_config_cookie_handling() {
    use camel_component_api::UriConfig;
    use camel_component_http::HttpEndpointConfig;

    // cookieHandling was a silently-ignored runtime state (TODO HTTP-013). It is
    // now rejected by the URI parser so operators get an explicit error instead
    // of a no-op.
    let result = HttpEndpointConfig::from_uri("http://example.com/api?cookieHandling=InMemory");
    assert!(
        matches!(result, Err(camel_api::CamelError::InvalidUri(ref m)) if m.contains("cookieHandling is not supported"))
    );

    let result = HttpEndpointConfig::from_uri("http://example.com/api?cookieHandling=Disabled");
    assert!(
        matches!(result, Err(camel_api::CamelError::InvalidUri(ref m)) if m.contains("cookieHandling is not supported"))
    );
}

// ---------------------------------------------------------------------------
// Test 21: HTTP config validation
// ---------------------------------------------------------------------------

#[test]
fn http_config_validates_max_redirects() {
    use camel_component_http::HttpConfig;

    let cfg = HttpConfig {
        max_redirects: Some(21),
        ..HttpConfig::default()
    };
    assert!(cfg.validate().is_err());

    let cfg = HttpConfig {
        max_redirects: Some(10),
        ..HttpConfig::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn http_config_validates_proxy_url() {
    use camel_component_http::HttpConfig;

    // proxy_url is retained for serde backward-compat, but any non-None
    // value is rejected at validation time because it defeats SSRF DNS
    // pinning. Both garbage and well-formed URLs fail the same way.
    let cfg = HttpConfig {
        proxy_url: Some("::not-a-proxy::".into()),
        ..HttpConfig::default()
    };
    let err = cfg
        .validate()
        .expect_err("garbage proxy_url must be rejected");
    assert!(
        err.to_string()
            .contains("incompatible with SSRF DNS pinning"),
        "expected SSRF rejection message, got: {err}"
    );

    let cfg = HttpConfig {
        proxy_url: Some("http://proxy.example.com:8080".into()),
        ..HttpConfig::default()
    };
    let err = cfg
        .validate()
        .expect_err("valid proxy_url must also be rejected");
    assert!(
        err.to_string()
            .contains("incompatible with SSRF DNS pinning"),
        "expected SSRF rejection message, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Test 22: Route stop returns 204
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_route_stop_returns_204() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from(&format!("http://127.0.0.1:{port}/stop-test"))
        .route_id("http-stop-test")
        .set_body(Value::String("running".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to("mock:stop")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop the context — routes transition to Stopped
    h.stop().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // After stop, the path is deregistered
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/stop-test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Test 23: HTTP producer with bridgeEndpoint skips auth headers
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_producer_bridge_endpoint_skips_auth() {
    install_crypto_provider();
    // Capture the raw request to verify no auth header
    let captured: std::sync::Arc<std::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_clone = captured.clone();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let actual_port = listener.local_addr().unwrap().port();
    let _handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let auth = request
                .lines()
                .find(|l| l.to_lowercase().starts_with("authorization:"))
                .map(|l| l.to_string());
            *captured_clone.lock().unwrap() = auth;
            let body = r#"{"ok":true}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    });

    let ctx = camel_component_api::ProducerContext::new();
    let component = HttpComponent::new();
    let endpoint_ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint(
            &format!(
                "http://127.0.0.1:{actual_port}/test?allowInternal=true&bridgeEndpoint=true&authMethod=Basic&authUsername=u&authPassword=p"
            ),
            &endpoint_ctx,
        )
        .unwrap();
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ctx)
        .unwrap();

    let exchange = Exchange::new(Message::default());
    let result = producer.oneshot(exchange).await.unwrap();
    let status = result
        .input
        .header("CamelHttpResponseCode")
        .and_then(|v| v.as_u64())
        .unwrap();
    assert_eq!(status, 200);

    tokio::time::sleep(Duration::from_millis(50)).await;
    let auth = captured.lock().unwrap().take();
    assert!(
        auth.is_none(),
        "bridgeEndpoint should skip auth headers, got: {auth:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 24: HTTP producer with connectionClose header
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_producer_connection_close_header() {
    install_crypto_provider();
    let captured: std::sync::Arc<std::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_clone = captured.clone();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let actual_port = listener.local_addr().unwrap().port();
    let _handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let conn = request
                .lines()
                .find(|l| l.to_lowercase().starts_with("connection:"))
                .map(|l| l.to_string());
            *captured_clone.lock().unwrap() = conn;
            let body = r#"{"ok":true}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    });

    let ctx = camel_component_api::ProducerContext::new();
    let component = HttpComponent::new();
    let endpoint_ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint(
            &format!("http://127.0.0.1:{actual_port}/test?allowInternal=true&connectionClose=true"),
            &endpoint_ctx,
        )
        .unwrap();
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ctx)
        .unwrap();

    let exchange = Exchange::new(Message::default());
    let result = producer.oneshot(exchange).await.unwrap();
    assert_eq!(
        result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap(),
        200
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    let conn = captured.lock().unwrap().take();
    let conn_has_close = conn
        .as_ref()
        .is_some_and(|c| c.to_lowercase().contains("close"));
    assert!(
        conn_has_close,
        "connectionClose=true should send Connection: close header, got: {conn:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 25: HTTP producer with skip_request_headers
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn http_producer_skip_request_headers() {
    install_crypto_provider();
    let captured: std::sync::Arc<std::sync::Mutex<String>> =
        std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let captured_clone = captured.clone();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let actual_port = listener.local_addr().unwrap().port();
    let _handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            *captured_clone.lock().unwrap() = request;
            let body = r#"{"ok":true}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    });

    let ctx = camel_component_api::ProducerContext::new();
    let component = HttpComponent::new();
    let endpoint_ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint(
            &format!(
                "http://127.0.0.1:{actual_port}/test?allowInternal=true&skipRequestHeaders=X-Secret"
            ),
            &endpoint_ctx,
        )
        .unwrap();
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ctx)
        .unwrap();

    let mut exchange = Exchange::new(Message::default());
    exchange
        .input
        .set_header("X-Secret", Value::String("hidden".into()));
    exchange
        .input
        .set_header("X-Visible", Value::String("shown".into()));
    let result = producer.oneshot(exchange).await.unwrap();
    assert_eq!(
        result
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap(),
        200
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    let request = captured.lock().unwrap().clone();
    assert!(
        !request.to_lowercase().contains("x-secret"),
        "X-Secret should be skipped, request: {request}"
    );
    assert!(
        request.to_lowercase().contains("x-visible"),
        "X-Visible should be forwarded, request: {request}"
    );
}

// ---------------------------------------------------------------------------
// Test 26: Cookie credential source integration (credential-sources task 2.2)
// ---------------------------------------------------------------------------

const SENTINEL_CRED_1: &str = "SENTINEL_CRED_1";
const SENTINEL_WRONG_1: &str = "SENTINEL_WRONG_1";
const SENTINEL_COOKIE_NAME: &str = "session";
const SENTINEL_HDR_9: &str = "SENTINEL_HDR_9";
const SENTINEL_HEADER_NAME: &str = "X-API-Key";
const SENTINEL_QUERY_PARAM: &str = "cred";
const TILES_READER_ROLE: &str = "tiles-reader";

// Mirrors compile_security_policy (crates/camel-dsl/src/compile.rs); the YAML end-to-end path is covered by yaml_compiled_cookie_route_authenticates.
/// Build an HTTP route protected by a `RolePolicy` over a native store seeded
/// with `SENTINEL_CRED_1` (principal role `tiles-reader`). `sources` selects
/// the credential extraction sources; `None` means the fail-closed header-only
/// default (ADR-0033), matching an absent `credential_sources` DSL key.
async fn build_secure_route(
    path: &str,
    sources: Option<Vec<CredentialSource>>,
) -> (CamelTestContext, u16) {
    install_crypto_provider();
    let port = stage_http_listener("0.0.0.0").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let principal = Principal {
        subject: "tiles-user".into(),
        issuer: "native".into(),
        audience: vec![],
        scopes: vec![],
        roles: vec![TILES_READER_ROLE.to_string()],
        claims: serde_json::json!({}),
    };
    let store = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: SENTINEL_CRED_1.to_string().into(),
        },
        principal,
    }])
    .unwrap();
    let authenticator: Arc<dyn TokenAuthenticator> = Arc::new(StaticTokenAuthenticator::new(store));

    // The kernel compiles plans against the provider registry (sole-provider
    // rule); the bare authenticator alone is not a registered provider.
    let provider_registry = {
        let registry = camel_auth::ProviderRegistry::new();
        registry.register(
            "native",
            camel_auth::ProviderEntry {
                authenticator: Arc::clone(&authenticator),
                audience_binding: None,
            },
        );
        Arc::new(registry)
    };

    let sources = sources.unwrap_or_else(|| vec![CredentialSource::AuthorizationHeader]);
    let policy = RolePolicy::new(vec![TILES_READER_ROLE.to_string()], true);
    let config = SecurityPolicyConfig::new(policy).with_credential_sources(sources);

    let route = RouteBuilder::from(&format!("http://0.0.0.0:{port}/{path}"))
        .route_id(format!("secure-http-{path}"))
        .security_policy(config)
        .security_authenticator(authenticator)
        .provider_registry(provider_registry)
        .set_body(Value::String("img-ok".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to(format!("mock:{path}"))
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    (h, port)
}

#[tokio::test(flavor = "multi_thread")]
async fn cookie_source_authenticates_img_request() {
    let (_h, port) = build_secure_route(
        "img",
        Some(vec![CredentialSource::Cookie {
            name: SENTINEL_COOKIE_NAME.to_string(),
        }]),
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/img"))
        .header(
            "Cookie",
            format!("{SENTINEL_COOKIE_NAME}={SENTINEL_CRED_1}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn cookie_miss_maps_401_not_500() {
    let (_h, port) = build_secure_route(
        "miss",
        Some(vec![CredentialSource::Cookie {
            name: SENTINEL_COOKIE_NAME.to_string(),
        }]),
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/miss"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn absent_key_rejects_cookie_token() {
    let (_h, port) = build_secure_route("absent", None).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/absent"))
        .header(
            "Cookie",
            format!("{SENTINEL_COOKIE_NAME}={SENTINEL_CRED_1}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_source_first_match_wins() {
    let (_h, port) = build_secure_route(
        "multi",
        Some(vec![
            CredentialSource::AuthorizationHeader,
            CredentialSource::Cookie {
                name: SENTINEL_COOKIE_NAME.to_string(),
            },
        ]),
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/multi"))
        .header("Authorization", format!("Bearer {SENTINEL_CRED_1}"))
        .header(
            "Cookie",
            format!("{SENTINEL_COOKIE_NAME}={SENTINEL_WRONG_1}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn fallback_to_second_source() {
    let (_h, port) = build_secure_route(
        "fallback",
        Some(vec![
            CredentialSource::AuthorizationHeader,
            CredentialSource::Cookie {
                name: SENTINEL_COOKIE_NAME.to_string(),
            },
        ]),
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/fallback"))
        .header(
            "Cookie",
            format!("{SENTINEL_COOKIE_NAME}={SENTINEL_CRED_1}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn default_absent_key_bearer_identical() {
    let (_h, port) = build_secure_route("bearer-default", None).await;

    let client = reqwest::Client::new();
    let ok = client
        .get(format!("http://127.0.0.1:{port}/bearer-default"))
        .header("Authorization", format!("Bearer {SENTINEL_CRED_1}"))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    let denied = client
        .get(format!("http://127.0.0.1:{port}/bearer-default"))
        .header("Authorization", format!("Bearer {SENTINEL_WRONG_1}"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn all_sources_miss_maps_401() {
    let (_h, port) = build_secure_route(
        "allmiss",
        Some(vec![
            CredentialSource::AuthorizationHeader,
            CredentialSource::Cookie {
                name: SENTINEL_COOKIE_NAME.to_string(),
            },
        ]),
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/allmiss"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// ---------------------------------------------------------------------------
// Test 26b: Header credential source integration (credential-sources task 4.2)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn header_source_authenticates_api_key_http() {
    let (_h, port) = build_secure_route(
        "apikey",
        Some(vec![CredentialSource::Header {
            name: SENTINEL_HEADER_NAME.to_string(),
        }]),
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/apikey"))
        .header(SENTINEL_HEADER_NAME, SENTINEL_CRED_1)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn header_source_miss_maps_401_http() {
    let (_h, port) = build_secure_route(
        "apikey-miss",
        Some(vec![CredentialSource::Header {
            name: SENTINEL_HEADER_NAME.to_string(),
        }]),
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/apikey-miss"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn query_source_authenticates_http() {
    // Real HTTP consumer path: camel-http must carry the raw query string into
    // the internal `CamelHttpQuery` header so camel-auth's query extraction can
    // read it. No Authorization header — the URL query string is the only
    // credential presented.
    let (_h, port) = build_secure_route(
        "qsrc",
        Some(vec![CredentialSource::QueryParam {
            param: SENTINEL_QUERY_PARAM.to_string(),
        }]),
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "http://127.0.0.1:{port}/qsrc?{SENTINEL_QUERY_PARAM}={SENTINEL_CRED_1}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// Whether any tracing record captured into the shared buffer contains `needle`.
/// The capturing subscriber is installed by `error_context_redacts_custom_header_sentinel`
/// and writes process-wide (the request-handling task runs on a tokio worker thread).
fn captured_logs_contain(buf: &std::sync::Mutex<Vec<u8>>, needle: &str) -> bool {
    String::from_utf8_lossy(&buf.lock().unwrap()).contains(needle)
}

#[tokio::test(flavor = "multi_thread")]
async fn error_context_redacts_custom_header_sentinel() {
    // Install a global subscriber that writes every record into a shared buffer.
    // Mirrors camel-http's task 2.3 redaction capture (per-crate env filter,
    // shared buffer, positive control), without the tracing-test dependency.
    struct SharedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = std::sync::Arc::clone(&buf);
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "camel_component_http=trace",
        ))
        .with_writer(move || SharedWriter(std::sync::Arc::clone(&writer)))
        .try_init();

    let (_h, port) = build_secure_route(
        "redact-hdr",
        Some(vec![CredentialSource::Header {
            name: SENTINEL_HEADER_NAME.to_string(),
        }]),
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/redact-hdr"))
        .header(SENTINEL_HEADER_NAME, SENTINEL_HDR_9)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body = resp.text().await.unwrap();
    assert!(
        !body.contains(SENTINEL_HDR_9),
        "reply body must not contain the custom-header credential"
    );
    assert!(
        !captured_logs_contain(&buf, SENTINEL_HDR_9),
        "no tracing record during request handling may render the custom-header credential"
    );
    // Permanent positive control: the failed-auth warn! must be captured.
    assert!(
        captured_logs_contain(&buf, "Authentication failed"),
        "positive control: the failed-auth warn! must be captured by the test subscriber"
    );
}

// ---------------------------------------------------------------------------
// Test 27: YAML→compile→policy seam — the real path, not the hand-built fixture
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn yaml_compiled_cookie_route_authenticates() {
    install_crypto_provider();
    let port = stage_http_listener("0.0.0.0").await;

    let principal = Principal {
        subject: "tiles-user".into(),
        issuer: "native".into(),
        audience: vec![],
        scopes: vec![],
        roles: vec![TILES_READER_ROLE.to_string()],
        claims: serde_json::json!({}),
    };
    let store = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: SENTINEL_CRED_1.to_string().into(),
        },
        principal,
    }])
    .unwrap();
    let authenticator: Arc<dyn TokenAuthenticator> = Arc::new(StaticTokenAuthenticator::new(store));

    let yaml = format!(
        r#"
routes:
  - id: yaml-compiled-cookie
    from: "http://0.0.0.0:{port}/yaml-compiled"
    security_policy:
      roles: ["{TILES_READER_ROLE}"]
      credential_sources:
        - cookie: {{name: "{SENTINEL_COOKIE_NAME}"}}
    steps:
      - set_header:
          key: CamelHttpResponseCode
          value: 200
      - to: "mock:yaml-compiled"
"#
    );

    let ctx = SecurityCompileContext::new(Some(authenticator), None);
    let defs = parse_yaml_with_threshold_and_security(&yaml, 1024, ctx).unwrap();
    assert_eq!(defs.len(), 1);

    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;
    for def in defs {
        h.add_route(def).await.unwrap();
    }
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();

    // Cookie carries the credential → 200
    let ok = client
        .get(format!("http://127.0.0.1:{port}/yaml-compiled"))
        .header(
            "Cookie",
            format!("{SENTINEL_COOKIE_NAME}={SENTINEL_CRED_1}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    // No credential → 401
    let denied = client
        .get(format!("http://127.0.0.1:{port}/yaml-compiled"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
}
