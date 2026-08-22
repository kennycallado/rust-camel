//! MCP configuration types (per-item config channel, ADR-0038).

use std::collections::HashMap;
use std::net::SocketAddr;

use serde::Deserialize;

use crate::error::McpError;

fn non_empty_path<'de, D>(deserializer: D, field: &'static str) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let path = String::deserialize(deserializer)?;
    let path = path.trim();
    if path.is_empty() {
        return Err(serde::de::Error::custom(format!(
            "{field} must not be empty"
        )));
    }
    Ok(path.to_owned())
}

fn deserialize_cert_path<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    non_empty_path(deserializer, "cert_path")
}

fn deserialize_key_path<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    non_empty_path(deserializer, "key_path")
}

/// TLS certificate and private-key paths for an MCP server listener.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpTlsConfig {
    /// PEM-encoded server certificate chain.
    #[serde(deserialize_with = "deserialize_cert_path")]
    pub cert_path: String,
    /// PEM-encoded server private key.
    #[serde(deserialize_with = "deserialize_key_path")]
    pub key_path: String,
}

/// DSL-declared listener values for one named MCP server, threaded from an
/// `mcp:` DSL block through route lowering onto the consumer endpoint URI as
/// `mcp.declared.*` parameters (spec: MCP listener ownership — the DSL block
/// owns its listener configuration the way `rest:` does).
///
/// Present only on routes lowered from a DSL block; a TOML-only route carries
/// no `mcp.declared.*` parameters and [`McpDeclaredServer::from_endpoint_params`]
/// returns `None` for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDeclaredServer {
    /// Streamable-HTTP listen address declared by the DSL block.
    pub bind: String,
    /// TLS configuration declared by the DSL block (`None` when the block
    /// declares no `tls:` section).
    pub tls: Option<McpTlsConfig>,
    /// Tool catalog cap declared by the DSL block (`None` when the block
    /// declares no cap — silence is not a value and never conflicts with
    /// or overwrites a TOML-declared cap).
    pub max_tools: Option<usize>,
    /// Resource catalog cap declared by the DSL block (`None` when the
    /// block declares no cap).
    pub max_resources: Option<usize>,
}

/// Parameter names carried on lowered consumer endpoint URIs.
const DECLARED_BIND: &str = "mcp.declared.bind";
const DECLARED_MAX_TOOLS: &str = "mcp.declared.max_tools";
const DECLARED_MAX_RESOURCES: &str = "mcp.declared.max_resources";
const DECLARED_TLS_CERT: &str = "mcp.declared.tls.cert_path";
const DECLARED_TLS_KEY: &str = "mcp.declared.tls.key_path";

impl McpDeclaredServer {
    /// Extract the DSL-declared server values from parsed endpoint-URI
    /// parameters.
    ///
    /// `Ok(None)` when no `mcp.declared.*` parameter is present — a TOML-only
    /// route. `bind` is mandatory whenever any declared parameter is present.
    /// Caps and TLS are presence-based: an absent cap parameter means "not
    /// declared by the DSL" (`None`), while a present-but-invalid value (a
    /// non-numeric cap, an empty TLS path, or a TLS path without its twin)
    /// is rejected with [`McpError::Endpoint`] naming the offending
    /// parameter — fail-closed on a hand-written or corrupted URI.
    pub fn from_endpoint_params(
        params: &HashMap<String, String>,
    ) -> Result<Option<Self>, McpError> {
        let has_declared = params.keys().any(|key| key.starts_with("mcp.declared."));
        if !has_declared {
            return Ok(None);
        }

        let bind = params.get(DECLARED_BIND).ok_or_else(|| {
            McpError::Endpoint(format!(
                "endpoint URI carries mcp.declared.* parameters but is missing \
                 '{DECLARED_BIND}' (the DSL lowering always emits it)"
            ))
        })?;
        if bind.trim().is_empty() {
            return Err(McpError::Endpoint(format!(
                "endpoint parameter '{DECLARED_BIND}' must not be empty"
            )));
        }

        // Absent → not declared (`None`); present-but-invalid → error.
        let parse_cap = |name: &str| -> Result<Option<usize>, McpError> {
            params
                .get(name)
                .map(|value| {
                    value.parse::<usize>().map_err(|_| {
                        McpError::Endpoint(format!(
                            "endpoint parameter '{name}' must be a non-negative integer"
                        ))
                    })
                })
                .transpose()
        };
        let max_tools = parse_cap(DECLARED_MAX_TOOLS)?;
        let max_resources = parse_cap(DECLARED_MAX_RESOURCES)?;

        let tls = match (params.get(DECLARED_TLS_CERT), params.get(DECLARED_TLS_KEY)) {
            (Some(cert_path), Some(key_path)) => {
                if cert_path.trim().is_empty() || key_path.trim().is_empty() {
                    return Err(McpError::Endpoint(format!(
                        "endpoint TLS parameters '{DECLARED_TLS_CERT}' and \
                         '{DECLARED_TLS_KEY}' must not be empty"
                    )));
                }
                Some(McpTlsConfig {
                    cert_path: cert_path.clone(),
                    key_path: key_path.clone(),
                })
            }
            (None, None) => None,
            (Some(_), None) | (None, Some(_)) => {
                return Err(McpError::Endpoint(format!(
                    "endpoint TLS parameters must be declared as a pair: \
                     '{DECLARED_TLS_CERT}' and '{DECLARED_TLS_KEY}'"
                )));
            }
        };

        Ok(Some(Self {
            bind: bind.clone(),
            tls,
            max_tools,
            max_resources,
        }))
    }
}

/// Catalog cardinality cap applied when neither TOML nor the DSL declares
/// one. This is the EFFECTIVE-value default only — it never participates in
/// TOML/DSL conflict checks (only declared values can conflict) and is
/// applied after the merge, at listener materialization.
pub const DEFAULT_CAP: usize = 128;

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

    /// Optional TLS configuration.
    #[serde(default)]
    pub tls: Option<McpTlsConfig>,

    /// Route-level authorization policy required for a server bind.
    #[serde(default)]
    pub security_policy: Option<serde_json::Value>,

    /// Maximum number of tools this server may register (`None` when the
    /// TOML entry declares no cap — the effective value is then the DSL
    /// declaration when present, else [`DEFAULT_CAP`]).
    #[serde(default)]
    pub max_tools: Option<usize>,

    /// Maximum number of resources this server may register (`None` when
    /// the TOML entry declares no cap).
    #[serde(default)]
    pub max_resources: Option<usize>,

    /// Operator allowlist of extra `Host` authorities (LAN IPs, DNS names, or
    /// `host:port`) accepted by rmcp's DNS-rebinding guard, on top of its
    /// loopback defaults (`localhost`, `127.0.0.1`, `::1`). `None` (default)
    /// additionally accepts only the bind host itself; a non-loopback bind
    /// must widen this list explicitly (ADR-0033).
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
}

impl McpServerConfig {
    /// Effective tool cap: the declared value or [`DEFAULT_CAP`].
    pub fn effective_max_tools(&self) -> usize {
        self.max_tools.unwrap_or(DEFAULT_CAP)
    }

    /// Effective resource cap: the declared value or [`DEFAULT_CAP`].
    pub fn effective_max_resources(&self) -> usize {
        self.max_resources.unwrap_or(DEFAULT_CAP)
    }
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
/// 1. The `bind` string must be an IP literal `SocketAddr` (`127.0.0.1:9100`,
///    `[::1]:9100`). A hostname such as `localhost:9100` does not parse as a
///    `SocketAddr` and is rejected with `Endpoint` — operators must use IP
///    literals so the loopback classification is unambiguous.
/// 2. Zero catalog caps are invalid (an explicitly declared `max_tools` /
///    `max_resources` of 0 is rejected; an undeclared cap defaults to
///    [`DEFAULT_CAP`], never 0); the offending field is named in the
///    `Endpoint` error.
///
/// The ADR-0060 Rule 8 `security_policy` presence gate was removed in
/// `unify-transport-auth` Task 2.9 (ADR-0061 Rule 9): public exposure is the
/// kernel's per-bind exposure gate decision, with uniform semantics across
/// all four transports. A server without a `security_policy` now classifies
/// `Public` and is gated at consumer start by `enforce_bind_exposure_gate`.
///
/// Returns `Some(BindPolicyWarning::NonLoopback)` when the bind address is a
/// non-loopback IP, so the caller can `tracing::warn!` once; a loopback bind
/// returns `Ok(None)`.
pub fn validate_server_policy(
    name: &str,
    cfg: &McpServerConfig,
) -> Result<Option<BindPolicyWarning>, McpError> {
    let addr: SocketAddr = cfg.bind.parse().map_err(|_| {
        McpError::Endpoint(format!(
            "bind '{}' is not an IP:port literal (hostnames are not allowed)",
            cfg.bind
        ))
    })?;

    if cfg.effective_max_tools() == 0 {
        return Err(McpError::Endpoint(format!(
            "max_tools must be at least 1 (got 0) for server '{name}'"
        )));
    }
    if cfg.effective_max_resources() == 0 {
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
    fn server_config_caps_default_to_none() {
        // Absent caps deserialize to `None` (the 128 default is an
        // effective-value default applied after the TOML/DSL merge, never a
        // fabricated declared value).
        let cfg: McpServerConfig = serde_json::from_str(r#"{"bind": "127.0.0.1:0"}"#).unwrap();
        assert_eq!(cfg.max_tools, None);
        assert_eq!(cfg.max_resources, None);
        assert_eq!(cfg.effective_max_tools(), 128);
        assert_eq!(cfg.effective_max_resources(), 128);
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
