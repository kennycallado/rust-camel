//! Server-config bind-policy validation (Task 2.1: fail-closed auth, bind
//! policy warning, cap validation).

use camel_component_mcp::config::{BindPolicyWarning, McpServerConfig, validate_server_policy};
use camel_component_mcp::error::McpError;

fn cfg(bind: &str) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({
        "bind": bind,
        "security_policy": {"require": "auth"}
    }))
    .expect("valid server config")
}

fn cfg_without_policy(bind: &str) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({ "bind": bind })).expect("valid server config")
}

fn cfg_zero_cap(field: &str) -> McpServerConfig {
    let mut value = serde_json::json!({
        "bind": "127.0.0.1:9100",
        "security_policy": {"require": "auth"}
    });
    value[field] = serde_json::json!(0);
    serde_json::from_value(value).expect("valid server config")
}

#[test]
fn bind_refused_without_security_policy() {
    let err = validate_server_policy("crm", &cfg_without_policy("127.0.0.1:9100")).unwrap_err();
    assert!(matches!(err, McpError::MissingSecurityPolicy { ref server } if server == "crm"));
}

#[test]
fn loopback_bind_no_warning() {
    let result = validate_server_policy("crm", &cfg("127.0.0.1:9100"));
    assert!(matches!(result, Ok(None)));
}

#[test]
fn non_loopback_bind_warns() {
    let result = validate_server_policy("crm", &cfg("0.0.0.0:9100"));
    assert!(matches!(result, Ok(Some(BindPolicyWarning::NonLoopback))));
}

#[test]
fn zero_cap_rejected() {
    let err = validate_server_policy("crm", &cfg_zero_cap("max_tools")).unwrap_err();
    assert!(
        matches!(err, McpError::Endpoint(ref message) if message.contains("max_tools")),
        "expected Endpoint error naming the offending field, got {err}"
    );
}

#[test]
fn zero_cap_rejected_max_resources() {
    let err = validate_server_policy("crm", &cfg_zero_cap("max_resources")).unwrap_err();
    assert!(
        matches!(err, McpError::Endpoint(ref message) if message.contains("max_resources")),
        "expected Endpoint error naming the offending field, got {err}"
    );
}
