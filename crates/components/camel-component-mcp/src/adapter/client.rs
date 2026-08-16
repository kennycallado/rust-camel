//! rmcp-backed [`McpClient`] (Producer role): discover lifecycle, `2026-07-28` only.
//!
//! Connect runs the stateless discover lifecycle —
//! `ClientLifecycleMode::Discover { preferred_versions: [V_2026_07_28] }` — never
//! the legacy `initialize`/session handshake. A remote that does not speak
//! `2026-07-28` (its advertised versions do not include it, or it answers
//! `server/discover` with `METHOD_NOT_FOUND`) fails fast as
//! [`McpError::IncompatibleRemote`] after one `warn!` naming the server.
//!
//! After a successful discover, rmcp's peer injects the negotiated protocol
//! version, client info, and capabilities into every request's `_meta`
//! (SEP-2575), and its streamable-HTTP worker emits the SEP-2243 `Mcp-Method` /
//! `Mcp-Name` standard headers on each POST. No session is ever established, so
//! `Mcp-Session-Id` is never sent.

use rmcp::ClientLifecycleMode;
use rmcp::model::{
    CallToolRequestParams, ErrorCode, ProtocolVersion, ReadResourceRequestParams, ResourceContents,
};
use rmcp::service::{ClientInitializeError, ClientServiceExt, RoleClient, RunningService};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};

use crate::client::McpClient;
use crate::config::McpRemoteConfig;
use crate::error::McpError;
use crate::types::{McpResource, McpToolResult};

/// The only protocol version this component speaks (spec: 2026-07-28 baseline).
const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2026_07_28;

/// A connected rmcp client for one remote MCP server.
pub struct RmcpClient {
    service: RunningService<RoleClient, ()>,
}

impl std::fmt::Debug for RmcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RmcpClient").finish_non_exhaustive()
    }
}

impl RmcpClient {
    /// Connect to `config.url` through the discover lifecycle.
    ///
    /// Fail-fast: any remote that cannot be reached on protocol `2026-07-28`
    /// returns [`McpError::IncompatibleRemote`] (after a `warn!`) instead of
    /// falling back to the legacy `initialize` handshake.
    pub async fn connect(name: &str, config: &McpRemoteConfig) -> Result<Self, McpError> {
        // rmcp's built-in reqwest backend (hardened defaults: no connection
        // pooling, redirects disabled so custom headers are never replayed).
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(config.url.clone()),
        );
        let service = ()
            .serve_with_lifecycle(
                transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![PROTOCOL_VERSION],
                },
            )
            .await
            .map_err(|error| map_connect_error(name, error))?;
        Ok(Self { service })
    }
}

/// Map discover-lifecycle failures onto the fail-fast taxonomy.
fn map_connect_error(server: &str, error: ClientInitializeError) -> McpError {
    match error {
        ClientInitializeError::NoCompatibleProtocolVersion {
            server_supported, ..
        } => {
            let version = versions_to_string(&server_supported);
            tracing::warn!(
                server = %server,
                detected_versions = %version,
                "MCP remote '{server}' does not speak protocol 2026-07-28 \
                 (server/discover reported: {version}); failing fast"
            );
            McpError::IncompatibleRemote {
                server: server.to_owned(),
                version,
            }
        }
        ClientInitializeError::JsonRpcError(data) if data.code == ErrorCode::METHOD_NOT_FOUND => {
            tracing::warn!(
                server = %server,
                detected_versions = "none",
                "MCP remote '{server}' does not implement server/discover \
                 (METHOD_NOT_FOUND); failing fast"
            );
            McpError::IncompatibleRemote {
                server: server.to_owned(),
                version: "none".to_owned(),
            }
        }
        other => McpError::Endpoint(other.to_string()),
    }
}

/// Render the versions a remote advertised (empty → "none").
fn versions_to_string(versions: &[ProtocolVersion]) -> String {
    if versions.is_empty() {
        "none".to_owned()
    } else {
        versions
            .iter()
            .map(ProtocolVersion::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[async_trait::async_trait]
impl McpClient for RmcpClient {
    async fn call_tool(
        &self,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, McpError> {
        let arguments = arguments
            .as_object()
            .cloned()
            .ok_or_else(|| McpError::Endpoint("tool arguments must be a JSON object".to_owned()))?;
        let mut params = CallToolRequestParams::new(tool.to_owned());
        params.arguments = Some(arguments);
        let result = self
            .service
            .call_tool(params)
            .await
            .map_err(|error| McpError::Endpoint(error.to_string()))?;
        let content = serde_json::to_value(&result.content)
            .map_err(|error| McpError::Endpoint(error.to_string()))?;
        // Carry the remote's protocol-level `isError` signal through
        // verbatim (absent on the wire means success). Never sniffed from
        // `content` — a remote's successful error-shaped payload must not be
        // mislabeled a failure.
        Ok(McpToolResult {
            content,
            is_error: result.is_error.unwrap_or(false),
        })
    }

    async fn read_resource(&self, uri: &str) -> Result<McpResource, McpError> {
        let params = ReadResourceRequestParams::new(uri.to_owned());
        let response = self
            .service
            .read_resource_once(params)
            .await
            .map_err(|error| McpError::Endpoint(error.to_string()))?;
        let result = match response {
            rmcp::model::ReadResourceResponse::Complete(result) => result,
            rmcp::model::ReadResourceResponse::InputRequired(_) => {
                return Err(McpError::Endpoint(
                    "remote resource read requires MRTR input rounds; not supported".to_owned(),
                ));
            }
            _ => {
                return Err(McpError::Endpoint(
                    "unexpected resources/read response shape".to_owned(),
                ));
            }
        };
        let mut resource = McpResource {
            uri: uri.to_owned(),
            content: Vec::new(),
            mime_type: "text/plain".to_owned(),
        };
        for contents in result.contents {
            match contents {
                ResourceContents::TextResourceContents {
                    uri: _,
                    mime_type,
                    text,
                    ..
                } => {
                    if let Some(mime) = mime_type {
                        resource.mime_type = mime;
                    }
                    resource.content.extend_from_slice(text.as_bytes());
                }
                ResourceContents::BlobResourceContents { .. } => {
                    return Err(McpError::Endpoint(
                        "binary resource contents are not supported yet".to_owned(),
                    ));
                }
                _ => {
                    return Err(McpError::Endpoint(
                        "unknown resource contents shape".to_owned(),
                    ));
                }
            }
        }
        Ok(resource)
    }
}
