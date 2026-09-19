//! Metadata-only anchor for the `mcp` scheme (ADR-0041 single-source-of-truth
//! invariant).
//!
//! [`McpMetadataDescriptor`] mirrors the component's documented configuration
//! surface: the URI parameters recognized by [`crate::endpoint::McpEndpointUri::parse`]
//! (`server`, `tool`, `uri`, `schema`) plus the config-carried keys surfaced as
//! URI options by the previous hand-written list (`bind`, `security_policy`,
//! `transport`). The `#[uri_param]` annotations below MUST stay synchronized
//! with that surface — the parity test pins the recognized set.

use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "mcp"]
#[uri_config(
    skip_impl,
    descriptor,
    metadata(
        scheme = "mcp",
        description = "MCP (Model Context Protocol) tools + resources server and client",
        producer,
        consumer,
        streaming
    ),
    crate = "camel_component_api"
)]
pub(super) struct McpMetadataDescriptor {
    #[uri_param(
        name = "server",
        required,
        desc = "Named MCP server (Consumer bind target) or remote (Producer client target)"
    )]
    pub _server: String,

    #[uri_param(name = "tool", desc = "Tool name to invoke on the named server")]
    pub _tool: Option<String>,

    #[uri_param(
        name = "uri",
        desc = "MCP resource URI to read (producer) or the declared resource URI (consumer)"
    )]
    pub _uri: Option<String>,

    #[uri_param(
        name = "schema",
        desc = "URL-encoded tool input JSON Schema carried on the tool consumer URI — the DSL lowering channel"
    )]
    pub _schema: Option<String>,

    #[uri_param(
        name = "bind",
        desc = "Streamable-HTTP listen address for the shared server listener"
    )]
    pub _bind: Option<String>,

    #[uri_param(
        name = "security_policy",
        desc = "Route-level authorization policy required for a server bind"
    )]
    pub _security_policy: Option<String>,

    #[uri_param(name = "transport", desc = "MCP transport (Streamable HTTP only)")]
    pub _transport: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn mcp_metadata_uri_options_parity() {
        let meta: ComponentMetadata = McpMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();

        let mut expected = vec![
            "bind",
            "schema",
            "security_policy",
            "server",
            "tool",
            "transport",
            "uri",
        ];
        expected.sort_unstable();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match the component's documented configuration surface"
        );

        // Verify required flags
        let server = meta
            .uri_options
            .iter()
            .find(|o| o.name == "server")
            .unwrap();
        assert!(server.required, "server must be required");

        // Verify capabilities survive the delegation round-trip
        assert!(meta.capabilities.supports_consumer);
        assert!(meta.capabilities.supports_producer);
        assert!(meta.capabilities.supports_streaming);
        assert!(!meta.capabilities.supports_polling_consumer);
    }
}
