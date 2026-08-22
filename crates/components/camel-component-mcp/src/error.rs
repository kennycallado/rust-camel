//! Error taxonomy for MCP component operations.

use camel_api::CamelError;

/// Error taxonomy for MCP component operations.
///
/// Adapter-agnostic — no rmcp-specific variants; the rmcp SDK stays confined
/// to `src/adapter/` (ADR-0020 pattern). Use the `Endpoint(String)` catch-all
/// for endpoint/transport-specific errors.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum McpError {
    /// A remote MCP server (Producer role) does not speak protocol `2026-07-28`.
    #[error("incompatible remote '{server}': unsupported protocol version '{version}'")]
    IncompatibleRemote { server: String, version: String },

    /// Catalog cardinality cap exceeded (tools or resources).
    #[error("cap exceeded: too many {kind} (max {max})")]
    CapExceeded { kind: String, max: usize },

    /// The DSL `mcp:` block and TOML `mcp.servers.<name>` declare the same
    /// key with different values (hard conflict, spec: MCP listener
    /// ownership). The message names both sources and both values so the
    /// operator knows exactly which declaration to align.
    #[error(
        "conflicting declarations for server '{server}': {key} is declared by both the DSL mcp: block and TOML mcp.servers.{server} with different values — dsl: {dsl}, toml: {toml}"
    )]
    ConfigConflict {
        server: String,
        key: &'static str,
        dsl: String,
        toml: String,
    },

    /// Configuration deserialization/validation failure.
    #[error("config error: {0}")]
    Config(#[from] serde_json::Error),

    /// Endpoint/URI or transport-specific error (catch-all).
    #[error("endpoint error: {0}")]
    Endpoint(String),
}

impl From<McpError> for CamelError {
    fn from(e: McpError) -> Self {
        match &e {
            // Startup config checks (ADR-0038 family). The typed
            // `CamelError::ConfigValidation` carries fixed camel-api variants with
            // no MCP-shaped arm, so the stringly `Config` member of the same
            // "config" classification family is used.
            McpError::CapExceeded { .. } | McpError::ConfigConflict { .. } => {
                CamelError::Config(e.to_string())
            }
            // Config deserialization failure — same "config" family.
            McpError::Config(_) => CamelError::Config(e.to_string()),
            // Runtime requirement failure against a remote (discover fail-fast):
            // distinct "validation" classification, not endpoint creation.
            McpError::IncompatibleRemote { .. } => CamelError::ValidationError(e.to_string()),
            // Endpoint/transport errors — the only arm that is an Endpoint(String).
            McpError::Endpoint(_) => CamelError::EndpointCreationFailed(e.to_string()),
        }
    }
}
