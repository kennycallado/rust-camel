//! Task 1.4 — post-connect request deadline (`request_timeout_secs`).
//!
//! A stalled remote (handler parks on `tools/call` / `resources/read`) must
//! trip the configured deadline with a named error, while a fast remote and
//! the parsed default (120 s) remain untouched. Each stalled call is wrapped
//! in an outer 10 s watchdog: an elapsed assertion after an unbounded await
//! could never detect a hang, whereas the watchdog turns a broken deadline
//! into a loud failure instead of a stuck suite.

mod common;

use std::time::Duration;

use camel_component_mcp::adapter::RmcpClient;
use camel_component_mcp::client::McpClient;
use camel_component_mcp::config::{McpRemoteConfig, McpTransport};
use common::{CANNED_TOOL_SUFFIX, MockOptions};
use rmcp::model::ProtocolVersion;

/// Deadline the stalled tests configure; a healthy implementation fires at
/// roughly this after the request starts.
const STALLED_DEADLINE_SECS: u64 = 1;

/// Config for a loopback mock with the given request deadline.
fn remote_config(url: String, request_timeout_secs: u64) -> McpRemoteConfig {
    McpRemoteConfig {
        url,
        transport: McpTransport::StreamableHttp,
        allow_internal: true, // tests bind loopback
        allow_cleartext: false,
        request_timeout_secs,
    }
}

/// A mock whose `tools/call` and `resources/read` handlers never answer.
fn stalled_options() -> MockOptions {
    MockOptions {
        stall_requests: true,
        ..MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])
    }
}

#[tokio::test]
async fn stalled_tool_call_trips_deadline_with_named_error() {
    let mock = common::spawn_mock(MockOptions {
        bind: "127.0.0.51".to_owned(),
        ..stalled_options()
    })
    .await;
    // `server/discover` still answers on a stalled mock, so connect succeeds.
    let client = RmcpClient::connect("slow-srv", &remote_config(mock.url, STALLED_DEADLINE_SECS))
        .await
        .expect("connect must succeed: server/discover is not stalled");

    let outer = tokio::time::timeout(
        Duration::from_secs(10),
        client.call_tool("slow", serde_json::json!({})),
    );
    let result = outer
        .await
        .expect("watchdog: the 1s request deadline must fire first");
    let error = result.expect_err("a stalled tools/call must not return Ok");
    let message = error.to_string();
    assert!(
        message.contains("tool call 'slow' exceeded the 1s request deadline"),
        "expected the named request-deadline error, got {message:?}"
    );
}

#[tokio::test]
async fn stalled_resource_read_trips_deadline_with_named_error() {
    let mock = common::spawn_mock(MockOptions {
        bind: "127.0.0.51".to_owned(),
        ..stalled_options()
    })
    .await;
    let client = RmcpClient::connect(
        "slow-res-srv",
        &remote_config(mock.url, STALLED_DEADLINE_SECS),
    )
    .await
    .expect("connect must succeed: server/discover is not stalled");

    let outer = tokio::time::timeout(
        Duration::from_secs(10),
        client.read_resource("crm://stalled"),
    );
    let result = outer
        .await
        .expect("watchdog: the 1s request deadline must fire first");
    let error = result.expect_err("a stalled resources/read must not return Ok");
    let message = error.to_string();
    assert!(
        message.contains("resource read 'crm://stalled' exceeded the 1s request deadline"),
        "expected the named request-deadline error, got {message:?}"
    );
}

#[tokio::test]
async fn fast_call_succeeds_with_explicit_deadline() {
    let mock = common::spawn_mock(MockOptions {
        bind: "127.0.0.51".to_owned(),
        ..MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])
    })
    .await;
    let client = RmcpClient::connect("fast-srv", &remote_config(mock.url, 30))
        .await
        .expect("connect must succeed against a normal mock");

    let result = client
        .call_tool("lookup", serde_json::json!({"id": "42"}))
        .await
        .expect("a fast tools/call must not be disturbed by the deadline wrapper");
    assert_eq!(
        result.content[0]["text"],
        format!("lookup:{CANNED_TOOL_SUFFIX}"),
        "canned McpToolResult must round-trip, got {:?}",
        result.content
    );
}

#[tokio::test]
async fn default_deadline_reaches_adapter_when_option_absent() {
    let mock = common::spawn_mock(MockOptions {
        bind: "127.0.0.51".to_owned(),
        ..MockOptions::advertises(vec![ProtocolVersion::V_2026_07_28])
    })
    .await;
    // Parse instead of constructing: proves the serde default, not a literal.
    let cfg: McpRemoteConfig = serde_json::from_str(&format!(
        r#"{{"url": "{}", "transport": "streamable-http", "allow_internal": true}}"#,
        mock.url
    ))
    .expect("config without request_timeout_secs must parse via its serde default");
    assert_eq!(
        cfg.request_timeout_secs, 120,
        "the absent option must default to 120 s"
    );

    let client = RmcpClient::connect("dflt", &cfg)
        .await
        .expect("connect must succeed against a normal mock");
    assert_eq!(
        client.request_deadline(),
        Duration::from_secs(120),
        "the parsed default must reach the connected adapter, not just the config struct"
    );
}
