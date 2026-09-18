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

    /// Allow this remote to point at internal/private addresses (SSRF
    /// policy, audit 2026-08-31 F2-4, aligned with camel-http by
    /// ADR-0079). Default `false`: IP-literal URLs in
    /// private/loopback/link-local ranges are rejected at config load.
    /// With this flag the remote may target internal-network literals;
    /// it says nothing about cleartext — public-cleartext policy is
    /// [`McpRemoteConfig::allow_cleartext`]'s job (ADR-0081).
    /// Hostname-based URLs are resolution-independent at this layer —
    /// their DNS resolution is validated and pinned at connect time
    /// (audit 2026-08-31 R4 / rc-juqrd, `adapter::dns_pin`): resolve
    /// once, validate every IP, connect to the validated addresses only.
    #[serde(default)]
    pub allow_internal: bool,

    /// Default `false`: under the uniform public-cleartext rule
    /// (ADR-0081), cleartext `http://` to a PUBLIC target is rejected
    /// unless this flag is `true` for this remote — the only escape
    /// hatch for the rule. Independent of
    /// [`McpRemoteConfig::allow_internal`].
    #[serde(default)]
    pub allow_cleartext: bool,
}

impl McpRemoteConfig {
    /// Validate the remote URL against scheme and SSRF policy. Called at
    /// component/endpoint creation (config load) — fails closed.
    pub fn validate_url(&self, name: &str) -> Result<(), McpError> {
        let lower = self.url.to_ascii_lowercase();
        let scheme_ok = lower.starts_with("http://") || lower.starts_with("https://");
        if !scheme_ok {
            return Err(McpError::Endpoint(format!(
                "remote '{name}' url must use http:// or https:// (got scheme of '{}')",
                self.url.split(':').next().unwrap_or("")
            )));
        }

        // Extract host for IP-literal checks. String-based: this crate has no
        // url dependency; the authority is between "://" and the next '/'.
        let after_scheme = &self.url[self.url.find("://").unwrap() + 3..]; // allow-unwrap: starts_with checked above
        let authority = after_scheme
            .split('/')
            .next()
            .unwrap_or(after_scheme)
            // strip userinfo if present
            .rsplit('@')
            .next()
            .unwrap_or("");
        // Strip port (and IPv6 brackets).
        let host = if let Some(rest) = authority.strip_prefix('[') {
            rest.split(']').next().unwrap_or(rest)
        } else {
            authority.split(':').next().unwrap_or(authority)
        };

        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            let blocked = camel_api::is_ssrf_blocked_ip(&ip);
            if blocked && !self.allow_internal {
                return Err(McpError::Endpoint(format!(
                    "remote '{name}' url points at a blocked/internal address ({ip}); \
                     set allow_internal=true to override"
                )));
            }
            // Uniform public-cleartext rule (ADR-0081): cleartext
            // http:// to a public address is rejected unless this
            // remote opts out via allow_cleartext — independent of
            // allow_internal (exact parity with camel-http and
            // adapter::dns_pin's hostname rule).
            if !blocked && lower.starts_with("http://") && !self.allow_cleartext {
                return Err(McpError::Endpoint(format!(
                    "remote '{name}' uses cleartext http:// to a public address; \
                     set allow_cleartext=true or use https://"
                )));
            }
        }
        Ok(())
    }
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

    // -----------------------------------------------------------------------
    // Audit 2026-08-31, F2-4: remote URL SSRF/scheme policy
    // -----------------------------------------------------------------------

    fn remote(url: &str, allow_internal: bool, allow_cleartext: bool) -> McpRemoteConfig {
        McpRemoteConfig {
            url: url.to_string(),
            transport: McpTransport::StreamableHttp,
            allow_internal,
            allow_cleartext,
        }
    }

    #[test]
    fn remote_url_rejects_non_http_schemes() {
        for bad in ["file:///etc/passwd", "ftp://host/", "gopher://x"] {
            let err = remote(bad, true, false).validate_url("r").unwrap_err();
            assert!(
                err.to_string().contains("http"),
                "{bad} must be rejected: {err}"
            );
        }
    }

    #[test]
    fn remote_url_blocks_private_ip_literals_by_default() {
        for ip in [
            "http://127.0.0.1:8000/mcp",
            "http://10.0.0.5/mcp",
            "http://192.168.1.1/mcp",
            "http://169.254.169.254/latest/meta-data", // cloud metadata
            "http://[::1]/mcp",
        ] {
            let err = remote(ip, false, false).validate_url("r").unwrap_err();
            assert!(
                err.to_string().contains("blocked/internal"),
                "{ip} must be blocked: {err}"
            );
        }
        // Explicit opt-in allows them (test/local deployments).
        assert!(
            remote("http://127.0.0.1:8000/mcp", true, false)
                .validate_url("r")
                .is_ok()
        );
    }

    // -----------------------------------------------------------------------
    // ADR-0081 uniform public-cleartext rule (literal branch). Cleartext
    // http:// to a public IP literal is rejected unless the remote opts
    // out via allow_cleartext — independent of allow_internal. This
    // inverts the formerly permissive default cell (ADR-0079, rc-lztp2);
    // hostname cleartext policy remains connect-time work
    // (adapter::dns_pin).
    // -----------------------------------------------------------------------

    #[test]
    fn remote_url_rejects_public_cleartext_literal_by_default() {
        let err = remote("http://93.184.216.34/mcp", false, false)
            .validate_url("r")
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cleartext"), "{msg}");
        assert!(msg.contains("allow_cleartext"), "{msg}");
    }

    #[test]
    fn remote_url_rejects_public_cleartext_literal_when_allow_internal() {
        // ADR-0079 cell kept as a regression pin: under allow_internal=true
        // (allow_cleartext=false) cleartext to a public IP literal is
        // still rejected at config load — the verdict a hostname
        // resolving to that IP gets at connect time
        // (adapter::dns_pin::validate_resolution).
        let err = remote("http://93.184.216.34/mcp", true, false)
            .validate_url("r")
            .unwrap_err();
        assert!(err.to_string().contains("cleartext"), "{err}");
    }

    #[test]
    fn remote_url_permits_public_cleartext_literal_with_allow_cleartext() {
        // The allow_cleartext escape hatch is independent of
        // allow_internal: both internal postures permit cleartext to a
        // public literal when the remote opts in (ADR-0081).
        assert!(
            remote("http://93.184.216.34/mcp", false, true)
                .validate_url("r")
                .is_ok()
        );
        assert!(
            remote("http://93.184.216.34/mcp", true, true)
                .validate_url("r")
                .is_ok()
        );
    }

    #[test]
    fn remote_url_cleartext_rejection_message_masks_userinfo() {
        // ADR-0051: rejection messages never echo credentials — the
        // cleartext message names only the remote and the escape hatch.
        let err = remote("http://user:pass@93.184.216.34/mcp", false, false)
            .validate_url("r")
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cleartext"), "{msg}");
        assert!(!msg.contains("user"), "{msg}");
        assert!(!msg.contains("pass"), "{msg}");
    }

    #[test]
    fn remote_url_absent_allow_cleartext_defaults_false() {
        let cfg: McpRemoteConfig = serde_json::from_str(
            r#"{"url": "https://mcp.example.com/mcp", "transport": "streamable-http"}"#,
        )
        .unwrap();
        assert!(!cfg.allow_cleartext);

        let opted: McpRemoteConfig = serde_json::from_str(
            r#"{"url": "https://mcp.example.com/mcp", "transport": "streamable-http", "allow_cleartext": true}"#,
        )
        .unwrap();
        assert!(opted.allow_cleartext);

        // https regression: a public literal over https:// stays fine.
        assert!(
            remote("https://93.184.216.34/mcp", false, false)
                .validate_url("r")
                .is_ok()
        );
    }

    #[test]
    fn remote_url_hostname_cleartext_passes_config_layer() {
        // Hostnames skip the literal branch entirely: cleartext policy for
        // them is decided at connect time by adapter::dns_pin, not here.
        assert!(
            remote("http://mcp.example.com/mcp", true, false)
                .validate_url("r")
                .is_ok(),
            "hostname remotes pass config load; dns_pin owns their \
             cleartext policy at connect time"
        );
    }

    #[test]
    fn remote_url_masks_userinfo_before_ip_check() {
        // Credentials in userinfo must not fool the host extraction.
        let err = remote("http://user:pass@127.0.0.1:8000/mcp", false, false)
            .validate_url("r")
            .unwrap_err();
        assert!(err.to_string().contains("blocked/internal"), "{err}");
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
