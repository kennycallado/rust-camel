//! Task 3 — connect-phase deadline (`connect_timeout_secs`).
//!
//! A remote that never completes the `server/discover` handshake must trip the
//! configured connect deadline with a named error, while a healthy remote and
//! post-connect requests beyond that bound remain unaffected. The stalled
//! connect is wrapped in an outer 10 s watchdog: an elapsed assertion after an
//! unbounded await could never detect a hang, whereas the watchdog turns a
//! broken deadline into a loud failure instead of a stuck suite.

mod common;

use std::time::Duration;

use camel_component_mcp::adapter::RmcpClient;
use camel_component_mcp::client::McpClient;
use camel_component_mcp::config::{McpRemoteConfig, McpTransport};
use common::{CANNED_TOOL_SUFFIX, MockOptions};
use rmcp::model::ProtocolVersion;

/// Connect deadline the stalled test configures; a healthy implementation
/// fires at roughly this after connect starts.
const STALLED_CONNECT_DEADLINE_SECS: u64 = 1;

/// Config for a loopback mock with the given connect and request deadlines.
fn remote_config(
    url: String,
    connect_timeout_secs: u64,
    request_timeout_secs: u64,
) -> McpRemoteConfig {
    McpRemoteConfig {
        url,
        transport: McpTransport::StreamableHttp,
        allow_internal: true, // tests bind loopback
        allow_cleartext: false,
        request_timeout_secs,
        connect_timeout_secs,
    }
}

#[tokio::test]
async fn stalled_discover_trips_connect_deadline_with_named_error() {
    let mock = common::spawn_mock(MockOptions {
        bind: "127.0.0.52".to_owned(),
        stall_discover: true,
        ..MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])
    })
    .await;

    let cfg = remote_config(mock.url, STALLED_CONNECT_DEADLINE_SECS, 120);
    let outer = tokio::time::timeout(
        Duration::from_secs(10),
        RmcpClient::connect("stalled-discover", &cfg),
    );
    let result = outer
        .await
        .expect("watchdog: the 1s connect deadline must fire first");
    let error = result.expect_err("a stalled discover handshake must not connect");
    let message = error.to_string();
    assert!(
        message.contains(
            "connect to remote 'stalled-discover' did not complete the discover \
             handshake within 1s"
        ),
        "expected the named connect-deadline error, got {message:?}"
    );
    assert!(
        !message.contains("5s TCP connect limit") && !message.contains("after 5s"),
        "the mock accepts promptly, so no inner TCP/DNS budget may have fired: {message:?}"
    );
}

#[tokio::test]
async fn healthy_connect_succeeds_under_explicit_connect_bound() {
    let mock = common::spawn_mock(MockOptions {
        bind: "127.0.0.52".to_owned(),
        ..MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])
    })
    .await;
    let client = RmcpClient::connect("healthy", &remote_config(mock.url, 5, 120))
        .await
        .expect("the discover handshake of a healthy mock must fit the 5s bound");

    let result = client
        .call_tool("lookup", serde_json::json!({"id": "42"}))
        .await
        .expect("a healthy tools/call must not be disturbed by the connect wrapper");
    assert_eq!(
        result.content[0]["text"],
        format!("lookup:{CANNED_TOOL_SUFFIX}"),
        "canned McpToolResult must round-trip, got {:?}",
        result.content
    );
}

#[tokio::test]
async fn connect_bound_does_not_bound_post_connect_requests() {
    let mock = common::spawn_mock(MockOptions {
        bind: "127.0.0.52".to_owned(),
        tool_delay_ms: 1_500,
        ..MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])
    })
    .await;
    // The handshake itself is fast; only `tools/call` is delayed.
    let client = RmcpClient::connect("slow-tool", &remote_config(mock.url, 1, 30))
        .await
        .expect("the connect bound bounds only the handshake, not tool latency");

    let outer = tokio::time::timeout(
        Duration::from_secs(10),
        client.call_tool("slow-ok", serde_json::json!({})),
    );
    let result = outer
        .await
        .expect("watchdog: a 1.5s tool call must resolve inside the 30s request deadline");
    let result = result.expect("a 1.5s tool call must outlive the 1s connect bound");
    assert_eq!(
        result.content[0]["text"],
        format!("slow-ok:{CANNED_TOOL_SUFFIX}"),
        "a request past the connect bound must still answer, got {:?}",
        result.content
    );
}
