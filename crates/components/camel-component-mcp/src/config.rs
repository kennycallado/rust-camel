//! MCP configuration types (per-item config channel, ADR-0038).

use std::collections::HashMap;
use std::net::SocketAddr;

use serde::Deserialize;

use crate::error::McpError;

/// Default catalog cardinality cap for tools and resources.
fn default_cap() -> usize {
    128
}

/// The MCP transport. Streamable HTTP is the only supported transport; every
/// other transport string is rejected at deserialization (spec: v1 protocol
/// surface — Streamable HTTP only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    /// Streamable HTTP (stateless, per-request `_meta`, no sessions).
    StreamableHttp,
}

impl<'de> Deserialize<'de> for McpTransport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct TransportVisitor;

        impl serde::de::Visitor<'_> for TransportVisitor {
            type Value = McpTransport;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("the transport string \"streamable-http\"")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "streamable-http" => Ok(McpTransport::StreamableHttp),
                    other => Err(E::custom(format!(
                        "unsupported MCP transport '{other}': only \"streamable-http\" is supported"
                    ))),
                }
            }
        }

        deserializer.deserialize_str(TransportVisitor)
    }
}

/// Server-role (Consumer) configuration for one named MCP server.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    /// Streamable-HTTP listen address for the shared server listener.
    pub bind: String,

    /// Optional TLS configuration (shape is policy-defined; opaque here).
    #[serde(default)]
    pub tls: Option<serde_json::Value>,

    /// Route-level authorization policy required for a server bind.
    #[serde(default)]
    pub security_policy: Option<serde_json::Value>,

    /// Maximum number of tools this server may register.
    #[serde(default = "default_cap")]
    pub max_tools: usize,

    /// Maximum number of resources this server may register.
    #[serde(default = "default_cap")]
    pub max_resources: usize,

    /// Operator allowlist of extra `Host` authorities (LAN IPs, DNS names, or
    /// `host:port`) accepted by rmcp's DNS-rebinding guard, on top of its
    /// loopback defaults (`localhost`, `127.0.0.1`, `::1`). `None` (default)
    /// additionally accepts only the bind host itself; a non-loopback bind
    /// must widen this list explicitly (ADR-0033).
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
}

/// Bind-policy concern the operator should be warned about once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindPolicyWarning {
    /// The bind address is not loopback, so the listener is reachable on a
    /// network interface (potentially the public internet).
    NonLoopback,
}

/// Validate a server-role (Consumer) config against bind policy, fail-closed.
///
/// Checks, in order:
/// 1. `security_policy` must be set — a missing policy refuses the bind
///    (`MissingSecurityPolicy`); a server without authentication never starts.
/// 2. The `bind` string must be an IP literal `SocketAddr` (`127.0.0.1:9100`,
///    `[::1]:9100`). A hostname such as `localhost:9100` does not parse as a
///    `SocketAddr` and is rejected with `Endpoint` — operators must use IP
///    literals so the loopback classification is unambiguous.
/// 3. Zero catalog caps are invalid (`max_tools` / `max_resources` must be
///    >= 1); the offending field is named in the `Endpoint` error.
///
/// Returns `Some(BindPolicyWarning::NonLoopback)` when the bind address is a
/// non-loopback IP, so the caller can `tracing::warn!` once; a loopback bind
/// returns `Ok(None)`.
pub fn validate_server_policy(
    name: &str,
    cfg: &McpServerConfig,
) -> Result<Option<BindPolicyWarning>, McpError> {
    if cfg.security_policy.is_none() {
        return Err(McpError::MissingSecurityPolicy {
            server: name.to_string(),
        });
    }

    let addr: SocketAddr = cfg.bind.parse().map_err(|_| {
        McpError::Endpoint(format!(
            "bind '{}' is not an IP:port literal (hostnames are not allowed)",
            cfg.bind
        ))
    })?;

    if cfg.max_tools == 0 {
        return Err(McpError::Endpoint(format!(
            "max_tools must be at least 1 (got 0) for server '{name}'"
        )));
    }
    if cfg.max_resources == 0 {
        return Err(McpError::Endpoint(format!(
            "max_resources must be at least 1 (got 0) for server '{name}'"
        )));
    }

    if addr.ip().is_loopback() {
        Ok(None)
    } else {
        Ok(Some(BindPolicyWarning::NonLoopback))
    }
}

/// Client-role (Producer) configuration for one named remote MCP server.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpRemoteConfig {
    /// Base URL of the remote MCP server (Streamable HTTP endpoint).
    pub url: String,

    /// Transport to use (Streamable HTTP only).
    pub transport: McpTransport,
}

/// Global MCP configuration, deserialized from the `mcp` config key.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpGlobalConfig {
    /// Named server-role (Consumer) servers.
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,

    /// Named client-role (Producer) remotes.
    #[serde(default)]
    pub remotes: HashMap<String, McpRemoteConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_stdio_rejected() {
        let json = r#"{"url": "http://127.0.0.1:0", "transport": "stdio"}"#;
        let err = serde_json::from_str::<McpRemoteConfig>(json).unwrap_err();
        assert!(err.to_string().contains("stdio"));
    }

    #[test]
    fn transport_legacy_sse_rejected() {
        let json = r#"{"url": "http://127.0.0.1:0", "transport": "http+sse"}"#;
        let err = serde_json::from_str::<McpRemoteConfig>(json).unwrap_err();
        assert!(err.to_string().contains("http+sse"));
    }

    #[test]
    fn server_config_defaults_caps_to_128() {
        let cfg: McpServerConfig = serde_json::from_str(r#"{"bind": "127.0.0.1:0"}"#).unwrap();
        assert_eq!(cfg.max_tools, 128);
        assert_eq!(cfg.max_resources, 128);
    }

    #[test]
    fn server_config_unknown_field_rejected() {
        let result =
            serde_json::from_str::<McpServerConfig>(r#"{"bind": "127.0.0.1:0", "session": true}"#);
        assert!(result.is_err());
    }

    #[test]
    fn remote_config_unknown_field_rejected() {
        let result = serde_json::from_str::<McpRemoteConfig>(
            r#"{"url": "http://127.0.0.1:0", "transport": "streamable-http", "session": true}"#,
        );
        assert!(result.is_err());
    }
}
