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
fn tls_typed_config_parses() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
bind = "127.0.0.1:9100"

[tls]
cert_path = "a.pem"
key_path = "b.pem"
"#,
    )
    .expect("valid TLS config");
    let tls = cfg.tls.expect("TLS config");
    assert_eq!(tls.cert_path, "a.pem");
    assert_eq!(tls.key_path, "b.pem");
}

#[test]
fn tls_empty_cert_path_rejected_at_load() {
    let err = toml::from_str::<McpServerConfig>(
        r#"
bind = "127.0.0.1:9100"

[tls]
cert_path = ""
key_path = "b.pem"
"#,
    )
    .expect_err("empty certificate path must fail");
    assert!(err.to_string().contains("cert_path"), "error: {err}");
}

#[test]
fn tls_empty_key_path_rejected_at_load() {
    let err = toml::from_str::<McpServerConfig>(
        r#"
bind = "127.0.0.1:9100"

[tls]
cert_path = "a.pem"
key_path = " "
"#,
    )
    .expect_err("whitespace-only key path must fail");
    assert!(err.to_string().contains("key_path"), "error: {err}");
}

#[test]
fn tls_unknown_field_rejected() {
    let err = toml::from_str::<McpServerConfig>(
        r#"
bind = "127.0.0.1:9100"

[tls]
cert_path = "a.pem"
key_path = "b.pem"
min_version = "1.3"
"#,
    )
    .expect_err("unknown TLS field must fail");
    assert!(err.to_string().contains("min_version"), "error: {err}");
}

#[test]
fn policy_less_config_no_longer_refused_at_validate() {
    // Task 2.9 removed the component-local `security_policy` presence gate
    // (ADR-0061 Rule 9): `validate_server_policy` only classifies the bind
    // and checks caps now — the kernel's per-bind exposure gate owns the
    // public-exposure decision at consumer start.
    let result = validate_server_policy("crm", &cfg_without_policy("127.0.0.1:9100"));
    assert!(
        matches!(result, Ok(None)),
        "loopback policy-less config validates, got {result:?}"
    );
    let result = validate_server_policy("crm", &cfg_without_policy("0.0.0.0:9100"));
    assert!(
        matches!(result, Ok(Some(BindPolicyWarning::NonLoopback))),
        "non-loopback policy-less config classifies NonLoopback, got {result:?}"
    );
}

#[tokio::test]
async fn mcp_old_bind_gate_removed() {
    use std::collections::HashMap;
    use std::sync::Arc;

    use camel_api::CamelError;
    use camel_component_api::{
        Component, ConsumerContext, NoOpComponentContext, NoopRuntimeObservability,
        RuntimeObservability,
    };
    use camel_component_mcp::McpServerRegistry;
    use camel_component_mcp::component::McpComponent;
    use camel_component_mcp::config::McpGlobalConfig;
    use tokio_util::sync::CancellationToken;

    fn rt() -> Arc<dyn RuntimeObservability> {
        Arc::new(NoopRuntimeObservability)
    }

    // Non-loopback + TOML-declared server WITHOUT security_policy + no ack:
    // the KERNEL per-bind exposure gate refuses naming the ack key. The old
    // component-local `McpError::MissingSecurityPolicy` consumer-start check
    // (ADR-0060 Rule 8) is gone — superseded by ADR-0061 Rule 9.
    let bind = "0.0.0.0:0";
    McpServerRegistry::global().set_bind_exposure_acks(HashMap::new()); // fail-closed: no acknowledgement

    let mut servers = HashMap::new();
    servers.insert("nosec".to_string(), cfg_without_policy(bind));
    let component = McpComponent::new(McpGlobalConfig {
        servers,
        remotes: HashMap::new(),
    });
    let schema = percent_encoding::utf8_percent_encode(
        r#"{"type":"object"}"#,
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();
    let endpoint = component
        .create_endpoint(
            &format!("mcp:nosec/tool/t?schema={schema}"),
            &NoOpComponentContext,
        )
        .expect("endpoint creation must succeed");
    let mut consumer = endpoint
        .create_consumer(rt())
        .expect("consumer creation must succeed");

    let (tx, _route_rx) = tokio::sync::mpsc::channel(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "test-route".to_string());
    let err = consumer
        .start(ctx)
        .await
        .expect_err("non-loopback Public server without ack must refuse to start");

    match &err {
        CamelError::RouteError(message) => {
            assert!(
                message.contains("allow_public_exposure"),
                "the KERNEL gate must name the ack key, got: {err}"
            );
            assert!(
                message.contains(bind),
                "the refusal must name the bind, got: {err}"
            );
        }
        other => panic!("expected RouteError from the kernel gate, got: {other}"),
    }
    assert!(
        !err.to_string()
            .to_lowercase()
            .contains("missing security policy"),
        "the removed component-local gate must never fire: {err}"
    );
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
