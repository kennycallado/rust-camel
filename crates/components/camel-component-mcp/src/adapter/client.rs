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

use std::time::Duration;

use rmcp::ClientLifecycleMode;
use rmcp::model::{
    CallToolRequestParams, ErrorCode, ProtocolVersion, ReadResourceRequestParams, ResourceContents,
};
use rmcp::service::{ClientInitializeError, ClientServiceExt, RoleClient, RunningService};
use rmcp::transport::DynamicTransportError;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig, StreamableHttpError,
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
    /// Post-connect request deadline, set from
    /// [`McpRemoteConfig::request_timeout_secs`] at connect.
    request_deadline: Duration,
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
        // DNS pinning (audit 2026-08-31 R4 / rc-juqrd): resolve the remote
        // once, validate every resolved IP against the SSRF blocklist, and
        // hand rmcp a reqwest client pinned to those addresses
        // (`resolve_to_addrs`) — parity with the camel-http component. The
        // builder keeps rmcp's hardened defaults (no connection pooling,
        // redirects disabled so custom headers are never replayed) and adds
        // `.no_proxy()` (environment proxies bypass `resolve_to_addrs`).
        let http_client = super::dns_pin::build_pinned_http_client(
            &config.url,
            config.allow_internal,
            config.allow_cleartext,
        )
        .await
        .map_err(|e| {
            tracing::warn!(
                server = %name,
                error = %e,
                "MCP remote '{name}' failed DNS pinning validation"
            );
            e
        })?;
        let transport = StreamableHttpClientTransport::with_client(
            http_client,
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
        let request_deadline = Duration::from_secs(config.request_timeout_secs);
        Ok(Self {
            service,
            request_deadline,
        })
    }

    /// The effective post-connect deadline for tool calls and resource reads.
    pub fn request_deadline(&self) -> Duration {
        self.request_deadline
    }
}

/// Map discover-lifecycle failures onto the fail-fast taxonomy.
fn map_connect_error(server: &str, error: ClientInitializeError) -> McpError {
    // Connect-phase timeouts first: name the 5 s budget. Everything else
    // falls through to the taxonomy below with its full Display intact
    // (context included).
    if let ClientInitializeError::TransportError {
        error: transport_error,
        ..
    } = &error
        && let Some(timeout_error) = connect_timeout_error(server, transport_error)
    {
        return timeout_error;
    }
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

/// Message for a TCP connect-phase timeout — names the 5 s budget
/// (`RESOLVE_TIMEOUT`, dns_pin.rs) the connection was given.
fn tcp_connect_timeout_message(server: &str, cause: &reqwest::Error) -> McpError {
    McpError::Endpoint(format!(
        "connect to remote '{server}' exceeded the 5s TCP connect limit ({cause})"
    ))
}

/// Detect a TCP connect-phase timeout inside rmcp's transport error.
///
/// The boxed error is `StreamableHttpError<reqwest::Error>` (rmcp:
/// StreamableHttpClientTransport = WorkerTransport<StreamableHttpClientWorker>,
/// worker Error = StreamableHttpError<C::Error>, reqwest errors enter as
/// StreamableHttpError::Client) — downcast to that wrapper, not to a bare
/// reqwest::Error. Returns None for anything else (passthrough; not every
/// startup error is a timeout).
fn connect_timeout_error(server: &str, error: &DynamicTransportError) -> Option<McpError> {
    let wrapper = error
        .error
        .downcast_ref::<StreamableHttpError<reqwest::Error>>()?;
    let StreamableHttpError::Client(cause) = wrapper else {
        return None;
    };
    (cause.is_connect() && cause.is_timeout()).then(|| tcp_connect_timeout_message(server, cause))
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
        let result = tokio::time::timeout(self.request_deadline, self.service.call_tool(params))
            .await
            .map_err(|_| {
                McpError::Endpoint(format!(
                    "tool call '{tool}' exceeded the {}s request deadline",
                    self.request_deadline.as_secs()
                ))
            })?
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
        let response = tokio::time::timeout(
            self.request_deadline,
            self.service.read_resource_once(params),
        )
        .await
        .map_err(|_| {
            McpError::Endpoint(format!(
                "resource read '{uri}' exceeded the {}s request deadline",
                self.request_deadline.as_secs()
            ))
        })?
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

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::transport::DynamicTransportError;
    use rmcp::transport::streamable_http_client::StreamableHttpError;

    /// Instant, deterministic connect-phase failure (nothing listens on port 1).
    async fn connection_refused_error() -> reqwest::Error {
        reqwest::Client::builder()
            .build()
            .unwrap()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .unwrap_err()
    }

    /// Box a transport error the way rmcp does for the streamable HTTP client.
    fn transport_error_from(error: StreamableHttpError<reqwest::Error>) -> DynamicTransportError {
        DynamicTransportError::new::<StreamableHttpClientTransport<reqwest::Client>, RoleClient>(
            error,
        )
    }

    #[tokio::test]
    async fn tcp_connect_timeout_message_names_limit_and_cause() {
        let error = connection_refused_error().await;
        let message = tcp_connect_timeout_message("srv", &error).to_string();
        assert!(message.contains("srv"), "server name missing: {message}");
        assert!(
            message.contains("5s TCP connect limit"),
            "budget missing: {message}"
        );
        assert!(
            message.contains(&error.to_string()),
            "cause text missing: {message}"
        );
    }

    #[tokio::test]
    async fn connect_timeout_error_passes_through_non_timeout_errors() {
        let dte = transport_error_from(StreamableHttpError::Client(
            connection_refused_error().await,
        ));
        assert!(
            connect_timeout_error("srv", &dte).is_none(),
            "a connect-phase non-timeout must not be mislabeled as a timeout"
        );

        // Rebuild the error: `DynamicTransportError` is not `Clone`.
        let original = ClientInitializeError::TransportError {
            error: transport_error_from(StreamableHttpError::Client(
                connection_refused_error().await,
            )),
            context: "startup".into(),
        };
        let expected = McpError::Endpoint(original.to_string()).to_string();
        let mapped = map_connect_error("srv", original);
        assert_eq!(
            mapped.to_string(),
            expected,
            "the fall-through must keep the full original Display (context included)"
        );
    }

    /// Never resolves, so the connect phase can only end through the timeout.
    struct PendingResolver;

    impl reqwest::dns::Resolve for PendingResolver {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(std::future::pending())
        }
    }

    /// A connect-phase timeout: the pending resolver forces the connect
    /// budget to fire; `start_paused` auto-advances tokio's clock, so no
    /// real time passes and no network route is needed.
    async fn connect_phase_timeout_error() -> reqwest::Error {
        let client = reqwest::Client::builder()
            .dns_resolver(PendingResolver)
            .connect_timeout(Duration::from_millis(300))
            .build()
            .unwrap();
        client
            .get("http://deadline-fixture.invalid/")
            .send()
            .await
            .unwrap_err()
    }

    #[tokio::test(start_paused = true)]
    async fn connect_timeout_error_names_tcp_limit_on_timeout() {
        let timeout_error = connect_phase_timeout_error().await;
        assert!(
            timeout_error.is_connect() && timeout_error.is_timeout(),
            "expected a connect-phase timeout, got: {timeout_error:?}"
        );

        let dte = transport_error_from(StreamableHttpError::Client(timeout_error));
        let message = connect_timeout_error("srv", &dte)
            .expect("connect-phase timeout must map to an endpoint error")
            .to_string();
        assert!(message.contains("srv"), "server name missing: {message}");
        assert!(
            message.contains("5s TCP connect limit"),
            "budget missing: {message}"
        );

        let second = connect_phase_timeout_error().await;
        let expected = tcp_connect_timeout_message("srv", &second).to_string();
        let original = ClientInitializeError::TransportError {
            error: transport_error_from(StreamableHttpError::Client(second)),
            context: "startup".into(),
        };
        assert_eq!(
            map_connect_error("srv", original).to_string(),
            expected,
            "the timeout arm must win over the generic taxonomy"
        );
    }
}
