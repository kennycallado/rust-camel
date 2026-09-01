//! Task 1.3 — rmcp client adapter: discover lifecycle, fail-fast, headers.
//! Task 1.4 — producer endpoints (`mcp:call` / `mcp:read`).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::{Body, CamelError, Exchange, Message, StepShutdownReason};
use camel_component_api::{
    Component, NoOpComponentContext, NoopRuntimeObservability, ProducerContext,
    RuntimeObservability,
};
use camel_component_mcp::adapter::RmcpClient;
use camel_component_mcp::client::McpClient;
use camel_component_mcp::component::McpComponent;
use camel_component_mcp::config::{McpGlobalConfig, McpRemoteConfig, McpTransport};
use camel_component_mcp::error::McpError;
use camel_component_mcp::headers::CAMEL_MCP_RESULT;
use common::{CANNED_RESOURCE_TEXT, CANNED_TOOL_SUFFIX, MockOptions, warn_capture};
use rmcp::model::ProtocolVersion;
use tower::Service;

fn remote_config(url: String) -> McpRemoteConfig {
    McpRemoteConfig {
        url,
        transport: McpTransport::StreamableHttp,
        allow_internal: true, // tests bind loopback
    }
}

fn rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoopRuntimeObservability)
}

/// Build an `McpComponent` with a single remote entry.
fn component_with_remote(name: &str, url: String) -> McpComponent {
    let mut remotes = HashMap::new();
    remotes.insert(name.to_string(), remote_config(url));
    McpComponent::new(McpGlobalConfig {
        servers: HashMap::new(),
        remotes,
    })
}

#[tokio::test]
async fn discover_accepts_2026_07_28_remote() {
    let mock =
        common::spawn_mock(MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])).await;
    let client = RmcpClient::connect("lookup-srv", &remote_config(mock.url))
        .await
        .expect("discover against a 2026-07-28 remote must succeed");
    let result = client
        .call_tool("lookup", serde_json::json!({"id": "42"}))
        .await
        .expect("call_tool must return the canned result");
    assert_eq!(
        result.content[0]["text"],
        format!("lookup:{CANNED_TOOL_SUFFIX}"),
        "canned McpToolResult must round-trip, got {:?}",
        result.content
    );
}

#[tokio::test]
async fn legacy_remote_fails_connect() {
    let mock =
        common::spawn_mock(MockOptions::advertises(vec![ProtocolVersion::V_2025_11_25])).await;
    let captures = warn_capture();
    let error = RmcpClient::connect("legacy-srv", &remote_config(mock.url))
        .await
        .expect_err("a 2025-11-25-only remote must fail connect");
    assert!(
        matches!(error, McpError::IncompatibleRemote { .. }),
        "expected IncompatibleRemote, got {error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("legacy-srv"),
        "Display must name the server: {message}"
    );
    assert!(
        message.contains("2025-11-25"),
        "Display must name the detected version: {message}"
    );
    assert!(
        captures.has_warn_containing("legacy-srv"),
        "connect must warn naming the server"
    );
}

#[tokio::test]
async fn no_discover_fails_connect() {
    let mock = common::spawn_mock(MockOptions::no_discover()).await;
    let error = RmcpClient::connect("no-discover-srv", &remote_config(mock.url))
        .await
        .expect_err("a remote without server/discover must fail connect");
    assert!(
        matches!(error, McpError::IncompatibleRemote { .. }),
        "expected IncompatibleRemote, got {error:?}"
    );
}

#[tokio::test]
async fn client_emits_standard_headers_no_session() {
    let mock =
        common::spawn_mock(MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])).await;
    let client = RmcpClient::connect("hdr-srv", &remote_config(mock.url.clone()))
        .await
        .expect("discover must succeed");
    client
        .call_tool("lookup", serde_json::json!({"id": "42"}))
        .await
        .expect("call_tool must succeed");

    let recorded: Vec<_> = mock
        .recorded()
        .into_iter()
        .filter(|request| request.method == "POST")
        .collect();
    assert!(
        recorded.len() >= 2,
        "expected the discover POST and the tools/call POST, got {} requests",
        recorded.len()
    );
    for request in &recorded {
        assert!(
            request.headers.contains_key("mcp-method"),
            "every POST must carry the Mcp-Method header: {:?}",
            request.headers
        );
        assert!(
            !request.headers.contains_key("mcp-session-id"),
            "client must never send Mcp-Session-Id: {:?}",
            request.headers
        );
    }
    let tool_calls: Vec<_> = recorded
        .iter()
        .filter(|request| request.body["method"] == "tools/call")
        .collect();
    assert_eq!(
        tool_calls.len(),
        1,
        "expected exactly one tools/call POST, got {tool_calls:?}"
    );
    for request in tool_calls {
        assert_eq!(
            request.headers.get("mcp-name").map(String::as_str),
            Some("lookup"),
            "tools/call must carry Mcp-Name = tool name: {:?}",
            request.headers
        );
        assert_eq!(
            request.body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            "2026-07-28",
            "tools/call _meta must carry the per-request protocol version: {}",
            request.body
        );
    }
}

// ── Task 1.4: producer endpoints ────────────────────────────────────────────

#[test]
fn producer_resolves_named_server() {
    let component = component_with_remote("crm", "http://127.0.0.1:1/mcp".to_string());
    let endpoint = component
        .create_endpoint("mcp:call?server=crm&tool=lookup", &NoOpComponentContext)
        .expect("endpoint construction from a named server must succeed");
    assert_eq!(endpoint.uri(), "mcp:call?server=crm&tool=lookup");
}

#[tokio::test]
async fn mcp_call_returns_result() {
    let mock =
        common::spawn_mock(MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])).await;
    let component = component_with_remote("crm", mock.url.clone());
    let endpoint = component
        .create_endpoint("mcp:call?server=crm&tool=lookup", &NoOpComponentContext)
        .expect("endpoint");
    endpoint
        .lifecycle()
        .expect("producer endpoint must have a lifecycle")
        .start()
        .await
        .expect("start must connect");
    let mut producer = endpoint
        .create_producer(rt(), &ProducerContext::default())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"id": "42"}))));
    let result = producer.call(exchange).await.expect("call must succeed");
    match &result.input.body {
        Body::Json(content) => assert_eq!(
            content[0]["text"],
            format!("lookup:{CANNED_TOOL_SUFFIX}"),
            "output body must be the canned tool result, got {content:?}"
        ),
        other => panic!("expected JSON output body, got {other:?}"),
    }
    let header = result
        .input
        .headers
        .get(CAMEL_MCP_RESULT)
        .expect("CamelMcpResult header must be set on success");
    assert_eq!(
        header["is_error"], false,
        "success path must carry is_error=false, got {header}"
    );
    assert_eq!(
        header["content"][0]["text"],
        format!("lookup:{CANNED_TOOL_SUFFIX}"),
        "header content must stay intact on success, got {header}"
    );
}

#[tokio::test]
async fn remote_error_flag_reaches_header() {
    // ADR-0060: the producer carries the remote's `is_error` flag and the
    // content into the `CamelMcpResult` header. A remote tool-level error
    // (`isError: true`) must surface in the header, not be dropped.
    let mock = common::spawn_mock(MockOptions {
        bind: "127.0.0.16".to_owned(),
        tool_error: true,
        ..MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])
    })
    .await;
    let component = component_with_remote("crm", mock.url.clone());
    let endpoint = component
        .create_endpoint("mcp:call?server=crm&tool=lookup", &NoOpComponentContext)
        .expect("endpoint");
    endpoint
        .lifecycle()
        .expect("producer endpoint must have a lifecycle")
        .start()
        .await
        .expect("start must connect");
    let mut producer = endpoint
        .create_producer(rt(), &ProducerContext::default())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"id": "42"}))));
    let result = producer.call(exchange).await.expect("call must succeed");
    let header = result
        .input
        .headers
        .get(CAMEL_MCP_RESULT)
        .expect("CamelMcpResult header must be set");
    assert_eq!(
        header["is_error"], true,
        "header must carry the remote is_error=true, got {header}"
    );
    assert_eq!(
        header["content"][0]["text"],
        format!("lookup:{CANNED_TOOL_SUFFIX}"),
        "header content must stay intact alongside is_error, got {header}"
    );
}

#[tokio::test]
async fn mcp_read_returns_content() {
    let mock =
        common::spawn_mock(MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])).await;
    let component = component_with_remote("docs", mock.url.clone());
    let endpoint = component
        .create_endpoint("mcp:read?server=docs&uri=file:///a", &NoOpComponentContext)
        .expect("endpoint");
    endpoint
        .lifecycle()
        .expect("producer endpoint must have a lifecycle")
        .start()
        .await
        .expect("start must connect");
    let mut producer = endpoint
        .create_producer(rt(), &ProducerContext::default())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Empty));
    let result = producer.call(exchange).await.expect("read must succeed");
    match &result.input.body {
        Body::Bytes(content) => assert_eq!(
            content.as_ref(),
            CANNED_RESOURCE_TEXT.as_bytes(),
            "output body must be the canned resource content"
        ),
        other => panic!("expected bytes output body, got {other:?}"),
    }
}

#[tokio::test]
async fn per_request_meta_three_exchanges_no_initialize() {
    let mock =
        common::spawn_mock(MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])).await;
    let component = component_with_remote("crm", mock.url.clone());
    let endpoint = component
        .create_endpoint("mcp:call?server=crm&tool=lookup", &NoOpComponentContext)
        .expect("endpoint");
    endpoint
        .lifecycle()
        .expect("producer endpoint must have a lifecycle")
        .start()
        .await
        .expect("start must connect");
    let mut producer = endpoint
        .create_producer(rt(), &ProducerContext::default())
        .expect("producer");

    for _ in 0..3 {
        let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"id": "42"}))));
        producer.call(exchange).await.expect("call must succeed");
    }

    let recorded = mock.recorded();
    let tool_calls: Vec<_> = recorded
        .iter()
        .filter(|request| request.body["method"] == "tools/call")
        .collect();
    assert_eq!(
        tool_calls.len(),
        3,
        "expected exactly three tools/call requests, got {recorded:?}"
    );
    for request in &tool_calls {
        assert_eq!(
            request.body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            "2026-07-28",
            "each tools/call must carry the per-request protocol version: {}",
            request.body
        );
    }
    let initializes = recorded
        .iter()
        .filter(|request| request.body["method"] == "initialize")
        .count();
    assert_eq!(initializes, 0, "no initialize request must ever be sent");
    for request in &recorded {
        assert!(
            !request.headers.contains_key("mcp-session-id"),
            "must never send Mcp-Session-Id: {:?}",
            request.headers
        );
    }
}

#[tokio::test]
async fn producer_start_fails_on_legacy_remote() {
    let mock =
        common::spawn_mock(MockOptions::advertises(vec![ProtocolVersion::V_2025_11_25])).await;
    let captures = warn_capture();
    let component = component_with_remote("legacy", mock.url.clone());
    let endpoint = component
        .create_endpoint("mcp:call?server=legacy&tool=x", &NoOpComponentContext)
        .expect("endpoint");
    let lifecycle = endpoint
        .lifecycle()
        .expect("producer endpoint must have a lifecycle");
    let error = lifecycle
        .start()
        .await
        .expect_err("a 2025-11-25-only remote must fail producer start");
    let message = error.to_string();
    assert!(
        message.contains("legacy"),
        "Display must name the server: {message}"
    );
    assert!(
        message.contains("2025-11-25"),
        "Display must name the detected version: {message}"
    );
    assert!(
        captures.has_warn_containing("remote 'legacy'"),
        "start must warn naming the server"
    );
}

#[tokio::test]
async fn producer_no_auto_loop() {
    let mock =
        common::spawn_mock(MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])).await;
    let component = component_with_remote("crm", mock.url.clone());
    let endpoint = component
        .create_endpoint("mcp:call?server=crm&tool=lookup", &NoOpComponentContext)
        .expect("endpoint");
    endpoint
        .lifecycle()
        .expect("producer endpoint must have a lifecycle")
        .start()
        .await
        .expect("start must connect");
    let mut producer = endpoint
        .create_producer(rt(), &ProducerContext::default())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"id": "42"}))));
    producer.call(exchange).await.expect("call must succeed");

    // Exactly one JSON-RPC call for the single Exchange (no auto-loop). The
    // discover handshake at start adds one `server/discover` request; the
    // producer itself must issue exactly one `tools/call`.
    let recorded = mock.recorded();
    let tool_calls = recorded
        .iter()
        .filter(|request| request.body["method"] == "tools/call")
        .count();
    assert_eq!(
        tool_calls, 1,
        "expected exactly one tools/call request, got {recorded:?}"
    );

    // The producer crate must not depend on camel-component-llm (spec:
    // Route-owned tool dispatch — the MCP producer never calls an LLM).
    let cargo_toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("read crate Cargo.toml");
    assert!(
        !cargo_toml.contains("camel-component-llm"),
        "producer crate must not depend on camel-component-llm"
    );
}

#[tokio::test]
async fn sibling_producers_survive_shutdown() {
    let mock =
        common::spawn_mock(MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])).await;
    let component = component_with_remote("crm", mock.url.clone());

    // Two routes targeting the same remote share one server-map entry. Each
    // route has its own lifecycle: both `start()` the same `crm` name.
    let endpoint_a = component
        .create_endpoint("mcp:call?server=crm&tool=a", &NoOpComponentContext)
        .expect("endpoint A");
    let endpoint_b = component
        .create_endpoint("mcp:call?server=crm&tool=b", &NoOpComponentContext)
        .expect("endpoint B");
    let lifecycle_a = endpoint_a
        .lifecycle()
        .expect("producer endpoint A must have a lifecycle");
    let lifecycle_b = endpoint_b
        .lifecycle()
        .expect("producer endpoint B must have a lifecycle");

    lifecycle_a.start().await.expect("start A must connect");
    lifecycle_b.start().await.expect("start B must connect");

    // Stopping route A must decrement the refcount, not remove the shared
    // entry — route B's producer must not stall afterward.
    lifecycle_a
        .shutdown(StepShutdownReason::RouteStop)
        .await
        .expect("shutdown A must succeed");

    let mut producer_b = endpoint_b
        .create_producer(rt(), &ProducerContext::default())
        .expect("producer B");

    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"id": "42"}))));
    let result = producer_b
        .call(exchange)
        .await
        .expect("producer B must still process after sibling A shutdown");
    match &result.input.body {
        Body::Json(content) => assert_eq!(
            content[0]["text"],
            format!("b:{CANNED_TOOL_SUFFIX}"),
            "producer B output body must be its canned tool result, got {content:?}"
        ),
        other => panic!("expected JSON output body, got {other:?}"),
    }
}

#[tokio::test]
async fn runtime_failure_maps_to_processor_error() {
    // A producer whose remote was never started hits the "not connected"
    // runtime path in `call`. That must surface as the processor kind, not
    // endpoint-creation, so route error policies route it correctly.
    let component = component_with_remote("crm", "http://127.0.0.1:1/mcp".to_string());
    let endpoint = component
        .create_endpoint("mcp:call?server=crm&tool=lookup", &NoOpComponentContext)
        .expect("endpoint");
    let mut producer = endpoint
        .create_producer(rt(), &ProducerContext::default())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"id": "42"}))));
    let error = producer
        .call(exchange)
        .await
        .expect_err("call without a started remote must fail");

    assert_eq!(
        error.classify(),
        "processor",
        "runtime failure must classify as processor, got {error:?}"
    );
    assert!(
        matches!(error, CamelError::ProcessorError(_)),
        "runtime failure must be ProcessorError, not EndpointCreationFailed: {error:?}"
    );
}
