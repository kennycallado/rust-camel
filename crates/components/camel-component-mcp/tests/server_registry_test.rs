//! Shared listener registry (Task 2.2): one listener per bind, reuse on the
//! same bind, rejection of conflicting configs. Tool/resource registry units
//! (Task 2.3): registration, readiness, caps, and lookup.

use std::sync::atomic::Ordering;
use std::time::Duration;

use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::config::McpServerConfig;
use camel_component_mcp::error::McpError;
use camel_component_mcp::registry::{McpResourceRegistry, McpToolRegistry};
use camel_component_mcp::types::{McpResourceRead, McpToolInvocation};

fn tool_sender() -> tokio::sync::mpsc::Sender<McpToolInvocation> {
    tokio::sync::mpsc::channel::<McpToolInvocation>(1).0
}

fn resource_sender() -> tokio::sync::mpsc::Sender<McpResourceRead> {
    tokio::sync::mpsc::channel::<McpResourceRead>(1).0
}

fn cfg(bind: &str, tls: Option<serde_json::Value>) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({
        "bind": bind,
        "tls": tls,
        "security_policy": {"require": "auth"},
    }))
    .expect("valid server config")
}

#[tokio::test]
async fn first_consumer_spawns_listener() {
    let bind = "127.0.0.1:0";
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg(bind, None))
        .await
        .unwrap();

    let result = tokio::net::TcpStream::connect(handle.local_addr).await;
    assert!(
        result.is_ok(),
        "first consumer's listener must accept connections"
    );
}

#[tokio::test]
async fn second_consumer_reuses_listener() {
    let bind = "127.0.0.2:0";
    let cfg = cfg(bind, None);

    let first = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .unwrap();
    let second = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .unwrap();

    assert_eq!(first.local_addr, second.local_addr);
    assert_eq!(first.spawn_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn conflicting_bind_rejected() {
    let bind = "127.0.0.3:0";
    let with_tls = cfg(bind, Some(serde_json::json!({"cert": "x", "key": "y"})));
    let without_tls = cfg(bind, None);

    McpServerRegistry::global()
        .get_or_spawn(bind, &with_tls)
        .await
        .unwrap();

    let err = McpServerRegistry::global()
        .get_or_spawn(bind, &without_tls)
        .await
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message) if message.contains("tls")),
        "expected Endpoint error naming tls, got {err}"
    );
}

#[tokio::test]
async fn dead_listener_is_respawned() {
    let bind = "127.0.0.4:0";
    let cfg = cfg(bind, None);

    let first = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .unwrap();
    assert_eq!(first.spawn_count.load(Ordering::SeqCst), 1);

    // Kill the serve loop via the exposed JoinHandle seam, then wait for the
    // task to finish so `get_or_spawn` observes the dead server.
    first.monitor_task.abort();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !first.monitor_task.is_finished() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "aborted serve loop did not finish in time"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Same bind → the dead entry is evicted and a fresh listener is spawned.
    let second = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .unwrap();

    assert_eq!(second.spawn_count.load(Ordering::SeqCst), 2);
    assert!(
        tokio::net::TcpStream::connect(second.local_addr)
            .await
            .is_ok(),
        "respawned listener must accept connections"
    );
}

#[tokio::test]
async fn conflicting_bind_max_tools_rejected() {
    let bind = "127.0.0.5:0";
    let base = cfg(bind, None);

    McpServerRegistry::global()
        .get_or_spawn(bind, &base)
        .await
        .unwrap();

    let mut tools_diff = base.clone();
    tools_diff.max_tools = 1;

    let err = McpServerRegistry::global()
        .get_or_spawn(bind, &tools_diff)
        .await
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message) if message.contains("max_tools")),
        "expected Endpoint error naming max_tools, got {err}"
    );
}

#[tokio::test]
async fn conflicting_bind_max_resources_rejected() {
    let bind = "127.0.0.6:0";
    let base = cfg(bind, None);

    McpServerRegistry::global()
        .get_or_spawn(bind, &base)
        .await
        .unwrap();

    let mut resources_diff = base.clone();
    resources_diff.max_resources = 1;

    let err = McpServerRegistry::global()
        .get_or_spawn(bind, &resources_diff)
        .await
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message) if message.contains("max_resources")),
        "expected Endpoint error naming max_resources, got {err}"
    );
}

#[tokio::test]
async fn conflicting_allowed_hosts_rejected() {
    let bind = "127.0.0.7:0";
    let base = serde_json::from_value(serde_json::json!({
        "bind": bind,
        "security_policy": {"require": "auth"},
        "allowed_hosts": ["host-a.example"],
    }))
    .expect("valid server config with allowed_hosts");

    McpServerRegistry::global()
        .get_or_spawn(bind, &base)
        .await
        .unwrap();

    // Same bind, same tls/caps — only a stricter allowed_hosts list differs.
    let mut narrowed = base.clone();
    narrowed.allowed_hosts = Some(vec!["host-b.example".to_string()]);

    let err = McpServerRegistry::global()
        .get_or_spawn(bind, &narrowed)
        .await
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message) if message.contains("allowed_hosts")),
        "expected Endpoint error naming allowed_hosts, got {err}"
    );
}

#[test]
fn duplicate_tool_registration_rejected() {
    let registry = McpToolRegistry::new(128);
    registry
        .register("lookup".to_string(), tool_sender(), serde_json::json!({}))
        .unwrap();

    let err = registry
        .register("lookup".to_string(), tool_sender(), serde_json::json!({}))
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message)
            if message.contains("lookup") && message.contains("already registered")),
        "expected an Endpoint error naming the duplicate tool, got {err}"
    );
}

#[test]
fn duplicate_resource_registration_rejected() {
    let registry = McpResourceRegistry::new(128);
    registry
        .register("crm://customers".to_string(), resource_sender())
        .unwrap();

    let err = registry
        .register("crm://customers".to_string(), resource_sender())
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message)
            if message.contains("crm://customers") && message.contains("already registered")),
        "expected an Endpoint error naming the duplicate resource, got {err}"
    );
}

#[test]
fn register_129th_tool_rejected() {
    let registry = McpToolRegistry::new(128);

    for i in 0..128 {
        registry
            .register(format!("tool_{i}"), tool_sender(), serde_json::json!({}))
            .unwrap();
    }

    let err = registry
        .register("tool_128".to_string(), tool_sender(), serde_json::json!({}))
        .unwrap_err();

    assert!(
        matches!(err, McpError::CapExceeded { ref kind, max } if kind == "tools" && max == 128),
        "expected CapExceeded for tools at max 128, got {err}"
    );
}

#[test]
fn raised_cap_allows_150() {
    let registry = McpToolRegistry::new(200);

    for i in 0..150 {
        registry
            .register(format!("tool_{i}"), tool_sender(), serde_json::json!({}))
            .unwrap();
    }
}

#[test]
fn not_ready_tool_hidden_from_list() {
    let registry = McpToolRegistry::new(128);
    registry
        .register("hidden".to_string(), tool_sender(), serde_json::json!({}))
        .unwrap();

    assert!(
        registry.list_ready().is_empty(),
        "a not-ready tool must be hidden from listing"
    );

    registry.mark_ready("hidden");

    let ready = registry.list_ready();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].0, "hidden");
}

#[test]
fn stopped_tool_unregistered() {
    let registry = McpToolRegistry::new(128);
    registry
        .register("lookup".to_string(), tool_sender(), serde_json::json!({}))
        .unwrap();
    registry.mark_ready("lookup");

    registry.unregister("lookup");

    assert!(
        registry.resolve("lookup").is_none(),
        "an unregistered tool must not resolve"
    );
}

#[test]
fn unknown_resource_uri_unresolved() {
    let registry = McpResourceRegistry::new(128);

    assert!(
        registry.resolve("crm://unknown").is_none(),
        "an unknown resource URI must not resolve"
    );
}

#[test]
fn resource_cap_enforced() {
    let registry = McpResourceRegistry::new(2);

    registry
        .register("crm://a".to_string(), resource_sender())
        .unwrap();
    registry
        .register("crm://b".to_string(), resource_sender())
        .unwrap();

    let err = registry
        .register("crm://c".to_string(), resource_sender())
        .unwrap_err();

    assert!(
        matches!(err, McpError::CapExceeded { ref kind, max } if kind == "resources" && max == 2),
        "expected CapExceeded for resources at max 2, got {err}"
    );
}
