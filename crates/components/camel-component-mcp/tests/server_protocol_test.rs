//! Task 2.5 — server adapter protocol baseline + discover enforcement.
//!
//! The server is the real thing: `McpServerRegistry::get_or_spawn` mounts the
//! rmcp server adapter (no mock). Each test binds its own loopback IP (repo
//! convention for the process-global registry). Rejection paths are driven
//! with raw HTTP JSON-RPC POSTs (a minimal `Connection: close` client over
//! `tokio::net::TcpStream` — assertions live in the JSON-RPC body, and
//! rmcp's own HTTP status mapping is deliberately not asserted: the JSON-RPC
//! error is the single enforcement channel). Happy paths are driven with
//! rmcp's discover-lifecycle client, which never sends `Mcp-Session-Id`.

mod common;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::adapter::RmcpClient;
use camel_component_mcp::client::McpClient;
use camel_component_mcp::config::{McpRemoteConfig, McpServerConfig, McpTransport};
use camel_component_mcp::error::McpError;
use camel_component_mcp::types::{McpResourceRead, McpToolInvocation};
use rmcp::ClientLifecycleMode;
use rmcp::model::{ClientCapabilities, ProtocolVersion, RequestMetaObject};
use rmcp::service::ClientServiceExt;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};

/// One parsed HTTP reply, plus the client-side address the connection came
/// from (the "peer" the server's rejection warn names).
struct RawHttpResponse {
    /// HTTP status code (`0` when the status line is unparseable).
    status: u16,
    /// Header names lowercased → values.
    headers: HashMap<String, String>,
    body: String,
    peer: SocketAddr,
}

fn server_cfg(bind: &str) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({
        "bind": bind,
        "security_policy": {"require": "auth"},
    }))
    .expect("valid server config")
}

/// Spawn the real listener for `bind` and return its bound address.
async fn spawn_server(bind: &str) -> SocketAddr {
    let cfg = server_cfg(bind);
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("listener must spawn");
    handle.local_addr
}

/// POST one JSON-RPC message to the server's `/mcp` route. `Connection:
/// close` makes the reply EOF-terminated, so no chunked/length parsing is
/// needed.
async fn raw_json_rpc_post(
    addr: SocketAddr,
    body: &serde_json::Value,
    extra_headers: &[(&str, &str)],
) -> RawHttpResponse {
    raw_json_rpc_post_with_host(addr, &addr.to_string(), body, extra_headers).await
}

/// POST one JSON-RPC message with an explicit `Host` header. rmcp's
/// DNS-rebinding guard validates `Host`, so the allowlist tests drive it
/// directly rather than deriving it from the dialed address.
async fn raw_json_rpc_post_with_host(
    addr: SocketAddr,
    host: &str,
    body: &serde_json::Value,
    extra_headers: &[(&str, &str)],
) -> RawHttpResponse {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let payload = body.to_string();
    // Streamable HTTP requires Accept to name both JSON and SSE.
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nConnection: close\r\n\
         Content-Length: {}\r\n",
        payload.len()
    );
    for (name, value) in extra_headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&payload);

    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect to listener");
    // Linux picks the loopback prefsrc (127.0.0.1) as source even when
    // dialing 127.0.0.3, so capture the real peer address, not the dialed IP.
    let peer = stream
        .local_addr()
        .expect("client local address after connect");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write JSON-RPC request");
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .expect("read reply to EOF");
    let (status, headers, body) = parse_http_response(&raw);
    RawHttpResponse {
        status,
        headers,
        body,
        peer,
    }
}

/// Split one HTTP reply into (status code, lowercased header map, body). The
/// reply is read to EOF (`Connection: close`), so no length/chunked framing
/// is parsed.
fn parse_http_response(raw: &[u8]) -> (u16, HashMap<String, String>, String) {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .expect("HTTP reply must separate head and body");
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    (status, headers, body.to_owned())
}

/// A `tools/call` request whose `_meta` names `version` (the per-request
/// protocol-version channel).
fn tools_call_with_meta_version(version: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "lookup",
            "arguments": {},
            "_meta": {"io.modelcontextprotocol/protocolVersion": version}
        }
    })
}

#[tokio::test]
async fn discover_advertises_only_2026_07_28() {
    let addr = spawn_server("127.0.0.2:0").await;
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp")),
    );
    let client = ()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover-lifecycle connect against the component server");

    let mut meta = RequestMetaObject::new();
    meta.set_protocol_version(ProtocolVersion::V_2026_07_28);
    meta.set_client_capabilities(ClientCapabilities::default());
    let result = client
        .peer()
        .discover(meta)
        .await
        .expect("server/discover must succeed");

    assert_eq!(
        result.supported_versions,
        vec![ProtocolVersion::V_2026_07_28],
        "discover must advertise exactly 2026-07-28, got {:?}",
        result.supported_versions
    );
    let identity = result.server_info().expect("identity must be present");
    assert!(
        !identity.name.is_empty(),
        "identity name must be non-empty, got {:?}",
        identity.name
    );
    assert!(
        result.capabilities.tools.is_some(),
        "tools capability must be advertised"
    );
    assert!(
        result.capabilities.resources.is_some(),
        "resources capability must be advertised"
    );
}

#[tokio::test]
async fn pre_2026_07_28_meta_rejected_with_32022() {
    let captures = common::warn_capture();
    let addr = spawn_server("127.0.0.3:0").await;

    let response = raw_json_rpc_post(
        addr,
        &tools_call_with_meta_version("2025-11-25"),
        &[("MCP-Protocol-Version", "2025-11-25")],
    )
    .await;
    let value: serde_json::Value =
        serde_json::from_str(&response.body).expect("reply must be JSON");
    assert_eq!(
        value["error"]["code"],
        serde_json::json!(-32022),
        "rejection must be JSON-RPC -32022, got: {value}"
    );
    assert_eq!(
        value["error"]["data"]["supported"],
        serde_json::json!(["2026-07-28"]),
        "error data must list the supported versions: {value}"
    );

    // Peer-scoped: the peer address is unique to this test, so the exactly-one
    // warn assertions are immune to sibling tests' -32022 traffic sharing the
    // process-wide sink.
    let peer = response.peer.to_string();
    let warns = captures.warn_messages_containing(&peer);
    assert_eq!(
        warns.len(),
        1,
        "exactly one rejection warn naming the peer {peer}, got {warns:?}"
    );
    assert!(
        warns[0].contains("unsupported protocol version"),
        "the warn must describe the rejection, got {:?}",
        warns[0]
    );
    assert!(
        warns[0].contains("2025-11-25"),
        "the warn must name the rejected version, got {:?}",
        warns[0]
    );
}

#[tokio::test]
async fn legacy_initialize_does_not_open_session() {
    let addr = spawn_server("127.0.0.4:0").await;

    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "legacy-client", "version": "0.0.1"}
        }
    });
    let init_response = raw_json_rpc_post(addr, &initialize, &[]).await;
    assert!(
        !init_response.headers.contains_key("mcp-session-id"),
        "legacy initialize must not open a session (headers: {:?})",
        init_response.headers
    );

    let follow_up = raw_json_rpc_post(
        addr,
        &tools_call_with_meta_version("2025-11-25"),
        &[("MCP-Protocol-Version", "2025-11-25")],
    )
    .await;
    let value: serde_json::Value =
        serde_json::from_str(&follow_up.body).expect("reply must be JSON");
    assert_eq!(
        value["error"]["code"],
        serde_json::json!(-32022),
        "follow-up must be rejected -32022, got: {value}"
    );
}

#[tokio::test]
async fn legacy_initialize_rejected_fail_closed() {
    let addr = spawn_server("127.0.0.9:0").await;
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "test-client", "version": "0.0.1"}
        }
    });

    let response = raw_json_rpc_post(addr, &initialize, &[]).await;
    let value: serde_json::Value =
        serde_json::from_str(&response.body).expect("reply must be JSON");
    assert_eq!(value["error"]["code"], serde_json::json!(-32022));
    assert_eq!(
        value["error"]["data"]["supported"],
        serde_json::json!(["2026-07-28"])
    );
    assert_eq!(
        value["error"]["data"]["requested"],
        serde_json::json!("2025-11-25")
    );
    assert!(
        !value
            .as_object()
            .expect("reply must be an object")
            .contains_key("result")
    );
}

#[tokio::test]
async fn initialize_with_baseline_version_succeeds() {
    let addr = spawn_server("127.0.0.10:0").await;
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2026-07-28",
            "capabilities": {},
            "clientInfo": {"name": "test-client", "version": "0.0.1"}
        }
    });

    let response = raw_json_rpc_post(addr, &initialize, &[]).await;
    let value: serde_json::Value =
        serde_json::from_str(&response.body).expect("reply must be JSON");
    assert_eq!(
        value["result"]["protocolVersion"],
        serde_json::json!("2026-07-28")
    );
}

#[tokio::test]
async fn session_header_not_required() {
    let addr = spawn_server("127.0.0.5:0").await;
    // RmcpClient's discover lifecycle never sends `Mcp-Session-Id`: the
    // request below is authorized by its `_meta` alone.
    let client = RmcpClient::connect(
        "session-less",
        &McpRemoteConfig {
            url: format!("http://{addr}/mcp"),
            transport: McpTransport::StreamableHttp,
            allow_internal: true, // tests bind loopback
        },
    )
    .await
    .expect("discover-mode connect needs no session");

    let error = client
        .call_tool("lookup", serde_json::json!({"id": "42"}))
        .await
        .expect_err("the staged stub must answer with a method error");
    let message = match &error {
        McpError::Endpoint(message) => message.clone(),
        other => panic!("expected the stub method error, got {other:?}"),
    };
    assert!(
        message.contains("-32601"),
        "request must be dispatched on _meta strength alone (stub method error \
         expected, not a session rejection), got: {message}"
    );
}

#[tokio::test]
async fn tools_list_carries_cache_metadata() {
    let bind = "127.0.0.6:0";
    let addr = spawn_server(bind).await;
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &server_cfg(bind))
        .await
        .expect("listener must spawn");
    let (tx, _rx) = tokio::sync::mpsc::channel::<McpToolInvocation>(8);
    let owner = Arc::new(());
    handle
        .tool_registry
        .register(
            "lookup".to_string(),
            "cache-metadata-route".to_string(),
            tx,
            serde_json::json!({
                "type": "object",
                "required": ["id"],
                "properties": {"id": {"type": "string"}}
            }),
            Arc::downgrade(&owner),
        )
        .expect("tool must register");
    handle.tool_registry.mark_ready("lookup");

    let (request, headers) = list_request("tools/list");
    let response = raw_json_rpc_post(addr, &request, &headers).await;
    let value: serde_json::Value =
        serde_json::from_str(&response.body).expect("reply must be JSON");
    assert_eq!(value["result"]["ttlMs"], serde_json::json!(0));
    assert_eq!(value["result"]["cacheScope"], serde_json::json!("private"));
    assert!(
        value["result"]["tools"]
            .as_array()
            .expect("tools must be an array")
            .iter()
            .any(|tool| tool["name"] == "lookup")
    );
}

#[tokio::test]
async fn resources_list_carries_cache_metadata() {
    let bind = "127.0.0.7:0";
    let addr = spawn_server(bind).await;
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &server_cfg(bind))
        .await
        .expect("listener must spawn");
    let (rtx, _rrx) = tokio::sync::mpsc::channel::<McpResourceRead>(8);
    let owner = Arc::new(());
    handle
        .resource_registry
        .register(
            "crm://customers".to_string(),
            "cache-metadata-route".to_string(),
            rtx,
            Arc::downgrade(&owner),
        )
        .expect("resource must register");
    handle.resource_registry.mark_ready("crm://customers");

    let (request, headers) = list_request("resources/list");
    let response = raw_json_rpc_post(addr, &request, &headers).await;
    let value: serde_json::Value =
        serde_json::from_str(&response.body).expect("reply must be JSON");
    assert_eq!(value["result"]["ttlMs"], serde_json::json!(0));
    assert_eq!(value["result"]["cacheScope"], serde_json::json!("private"));
    assert!(
        value["result"]["resources"]
            .as_array()
            .expect("resources must be an array")
            .iter()
            .any(|resource| resource["uri"] == "crm://customers")
    );
}

#[tokio::test]
async fn tools_list_empty_catalog_still_carries_cache_metadata() {
    let addr = spawn_server("127.0.0.8:0").await;
    let (request, headers) = list_request("tools/list");
    let response = raw_json_rpc_post(addr, &request, &headers).await;
    let value: serde_json::Value =
        serde_json::from_str(&response.body).expect("reply must be JSON");
    assert_eq!(value["result"]["tools"], serde_json::json!([]));
    assert_eq!(value["result"]["ttlMs"], serde_json::json!(0));
    assert_eq!(value["result"]["cacheScope"], serde_json::json!("private"));
}

/// A `server/discover` JSON-RPC request (no `_meta` — discover is the
/// stateless opener, so its routing does not require per-request metadata).
fn discover_request() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover"
    })
}

/// The body carries both `_meta` keys (protocol version and client
/// capabilities), and the headers pair `MCP-Protocol-Version` with
/// `Mcp-Method` (SEP-2243 is required at version >= `2026-07-28`); stripping
/// either silently changes what these tests exercise.
fn list_request(method: &'static str) -> (serde_json::Value, [(&'static str, &'static str); 2]) {
    (
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": method,
            "params": {"_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }}
        }),
        [
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", method),
        ],
    )
}

#[tokio::test]
async fn oversize_reply_passes_through_unchanged() {
    use camel_component_mcp::adapter::server::{
        MAX_INSPECTED_BODY_BYTES, warn_protocol_rejections,
    };

    // A JSON reply larger than the warn layer's inspect cap. The layer must
    // pass it through byte-complete — never truncate it to an empty body
    // while the original framing is kept.
    async fn oversized_reply() -> axum::response::Response {
        let padding = "x".repeat(MAX_INSPECTED_BODY_BYTES + 1);
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "padding": padding }
        });
        let bytes = body.to_string();
        axum::response::Response::builder()
            .header("content-type", "application/json")
            .body(axum::body::Body::from(bytes))
            .expect("stub reply must build")
    }

    let app = axum::Router::new()
        .route("/mcp", axum::routing::post(oversized_reply))
        .layer(axum::middleware::from_fn(warn_protocol_rejections));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral stub port");
    let addr = listener.local_addr().expect("stub local address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let response = raw_json_rpc_post(addr, &discover_request(), &[]).await;
    let value: serde_json::Value =
        serde_json::from_str(&response.body).expect("stub reply must be JSON");
    assert_eq!(
        value["result"]["padding"].as_str().map(str::len),
        Some(MAX_INSPECTED_BODY_BYTES + 1),
        "the oversized reply must arrive byte-complete, not truncated"
    );
    assert_eq!(
        response
            .headers
            .get("content-length")
            .and_then(|v| v.parse::<usize>().ok()),
        Some(response.body.len()),
        "Content-Length must match the shipped body"
    );
}

#[tokio::test]
async fn wildcard_bind_serves_with_explicit_hosts() {
    let cfg = serde_json::from_value(serde_json::json!({
        "bind": "0.0.0.0:0",
        "security_policy": {"require": "auth"},
        "allowed_hosts": ["127.0.0.1"],
    }))
    .expect("valid server config with allowed_hosts");

    let handle = McpServerRegistry::global()
        .get_or_spawn("0.0.0.0:0", &cfg)
        .await
        .expect("wildcard listener must spawn");
    let port = handle.local_addr.port();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    // An allowed Host dialing the wildcard bind is served, not 403.
    let served =
        raw_json_rpc_post_with_host(addr, &format!("127.0.0.1:{port}"), &discover_request(), &[])
            .await;
    assert!(
        (200..300).contains(&served.status),
        "allowed Host must be served (200-range), got status {} body {}",
        served.status,
        served.body
    );

    // A Host outside the allowlist is refused: the guard stays active.
    let refused =
        raw_json_rpc_post_with_host(addr, "evil.example.com", &discover_request(), &[]).await;
    assert_eq!(
        refused.status, 403,
        "disallowed Host must be refused, got status {} body {}",
        refused.status, refused.body
    );
}
