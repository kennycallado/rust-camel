//! MCP component — factory for `mcp:` endpoints (scheme `"mcp"`).

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::CamelError;
use camel_api::component_metadata::{
    ComponentCapabilities, ComponentMetadata, OptionKind, UriOption,
};
use camel_component_api::{Component, ComponentContext, Endpoint};

use crate::client::{McpServerMap, McpServerMapHandle};
use crate::config::McpGlobalConfig;
use crate::endpoint::{McpEndpoint, McpEndpointUri};
use crate::error::McpError;

/// The MCP component — scheme `"mcp"`.
///
/// Owns both MCP roles, disambiguated by consumer-vs-producer creation (the
/// same split `camel-http` uses for server/client):
///
/// - **Consumer (server)** role — a shared Streamable-HTTP listener serving
///   tool (`mcp:<server>/tool/<name>`) and resource
///   (`mcp:<server>/resource/<name>`) routes.
/// - **Producer (client)** role — `mcp:call?server=<name>&tool=<name>` and
///   `mcp:read?server=<name>&uri=<...>` dispatch to remote MCP servers.
///
/// The live [`crate::client::McpServerMap`] is seeded empty at construction
/// (no network I/O at construction — `RmcpClient::connect` is async); each
/// producer connects its remote at route start via the endpoint lifecycle
/// handle. `create_endpoint` resolves a producer URI's server name against
/// `config.remotes` and fails with [`McpError::Endpoint`] naming it when the
/// name is unknown.
pub struct McpComponent {
    config: Arc<McpGlobalConfig>,
    servers: McpServerMapHandle,
}

impl McpComponent {
    /// Build a component from global config.
    ///
    /// Infallible: stores the remote configs and an empty live map — no
    /// network at construction; the `RmcpClient::connect` happens at producer
    /// start.
    pub fn new(config: McpGlobalConfig) -> Self {
        Self {
            config: Arc::new(config),
            servers: Arc::new(McpServerMap::new()),
        }
    }
}

impl Default for McpComponent {
    /// Minimal no-config component (unit tests, metadata harvesting).
    fn default() -> Self {
        Self {
            config: Arc::new(McpGlobalConfig {
                servers: HashMap::new(),
                remotes: HashMap::new(),
            }),
            servers: Arc::new(McpServerMap::new()),
        }
    }
}

impl Component for McpComponent {
    fn scheme(&self) -> &str {
        "mcp"
    }

    fn metadata(&self) -> ComponentMetadata {
        let mut metadata = ComponentMetadata::minimal("mcp");
        metadata.version = env!("CARGO_PKG_VERSION").into();
        metadata.description =
            "MCP (Model Context Protocol) tools + resources server and client".into();
        metadata.uri_syntax = "mcp:<server>/tool/<name>?schema=<json> | mcp:<server>/resource/<name>?uri=<mcp-uri> | mcp:call?server=<name>&tool=<name> | mcp:read?server=<name>&uri=<uri>".into();
        metadata.capabilities = ComponentCapabilities {
            supports_consumer: true,
            supports_producer: true,
            supports_polling_consumer: false,
            supports_streaming: true,
        };
        metadata.uri_options = vec![
            UriOption::new(
                "server",
                "Named MCP server (Consumer bind target) or remote (Producer client target)",
                OptionKind::String,
            )
            .required(),
            UriOption::new(
                "tool",
                "Tool name to invoke on the named server",
                OptionKind::String,
            ),
            UriOption::new(
                "uri",
                "MCP resource URI to read (producer) or the declared resource URI (consumer)",
                OptionKind::String,
            ),
            UriOption::new(
                "schema",
                "URL-encoded tool input JSON Schema carried on the tool consumer URI — the DSL lowering channel",
                OptionKind::String,
            ),
            UriOption::new(
                "bind",
                "Streamable-HTTP listen address for the shared server listener",
                OptionKind::String,
            ),
            UriOption::new(
                "security_policy",
                "Route-level authorization policy required for a server bind",
                OptionKind::String,
            ),
            UriOption::new(
                "transport",
                "MCP transport (Streamable HTTP only)",
                OptionKind::String,
            ),
        ];
        metadata
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let operation = McpEndpointUri::parse(uri)?;

        match &operation {
            // Fail-fast: the named remote must exist in config (spec scenario:
            // client producer endpoint resolves a named server).
            McpEndpointUri::Call { server, .. } | McpEndpointUri::Read { server, .. } => {
                let remote = self.config.remotes.get(server).cloned().ok_or_else(|| {
                    McpError::Endpoint(format!(
                        "MCP remote '{server}' not found in config (URI '{uri}')"
                    ))
                })?;
                // Audit 2026-08-31, F2-4: SSRF/scheme policy on the remote URL
                // (fail-closed at endpoint creation, before any connection).
                remote.validate_url(server)?;
                Ok(self.endpoint(uri, operation, Some(remote)))
            }
            // Consumer shapes carry the server-role configs; the named server
            // is resolved at consumer START so bind-policy failures (missing
            // security policy, non-loopback warn) surface through the
            // consumer, matching the spec's bind-time scenarios.
            McpEndpointUri::Tool { .. } | McpEndpointUri::Resource { .. } => {
                Ok(self.endpoint(uri, operation, None))
            }
        }
    }
}

impl McpComponent {
    fn endpoint(
        &self,
        uri: &str,
        operation: McpEndpointUri,
        remote: Option<crate::config::McpRemoteConfig>,
    ) -> Box<dyn Endpoint> {
        Box::new(McpEndpoint::new(
            uri.to_string(),
            operation,
            remote,
            Arc::new(self.config.servers.clone()),
            Arc::clone(&self.servers),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_declares_both_roles() {
        let metadata = McpComponent::default().metadata();

        assert_eq!(metadata.scheme, "mcp");
        assert!(metadata.capabilities.supports_consumer);
        assert!(metadata.capabilities.supports_producer);
        assert!(metadata.capabilities.supports_streaming);

        let option_names: Vec<&str> = metadata
            .uri_options
            .iter()
            .map(|o| o.name.as_str())
            .collect();
        assert!(option_names.contains(&"server"));
        assert!(option_names.contains(&"schema"));
    }

    #[test]
    fn metadata_validates_against_scheme() {
        assert_eq!(
            McpComponent::default().metadata().validate_scheme("mcp"),
            Ok(())
        );
    }
}
