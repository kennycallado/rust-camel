//! MCP producer — the client-role Tower `Service<Exchange>` for `mcp:call`
//! and `mcp:read`.
//!
//! [`McpProducer`] is a dispatch *target*, never a *decider* (spec: Route-owned
//! tool dispatch, no auto-loop). It issues exactly one JSON-RPC request per
//! Exchange — `tools/call` for [`McpEndpointUri::Call`], `resources/read` for
//! [`McpEndpointUri::Read`] — and returns the result as the Exchange output
//! body. It never calls an LLM component and never issues a second call.
//!
//! Startup is driven by [`McpProducerLifecycle`] (wired through
//! [`crate::endpoint::McpEndpoint::lifecycle`]): `start()` connects the remote
//! via `RmcpClient::connect` and caches the client in the shared live map.
//! Until that succeeds (or forever after it fails), [`McpProducer::poll_ready`]
//! stays `Pending`, so a producer whose remote is incompatible never becomes
//! ready.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use camel_api::{Body, CamelError, Exchange, StepLifecycle, StepShutdownReason};
use tower::Service;

use crate::adapter::RmcpClient;
use crate::client::McpServerMapHandle;
use crate::config::McpRemoteConfig;
use crate::endpoint::McpEndpointUri;
use crate::error::McpError;
use crate::headers::{CAMEL_MCP_RESULT, CAMEL_MCP_TOOL_CALL};

/// Client-role producer: dispatches one tool call or resource read per Exchange.
#[derive(Clone)]
pub struct McpProducer {
    operation: McpEndpointUri,
    server: String,
    servers: McpServerMapHandle,
}

impl McpProducer {
    pub fn new(operation: McpEndpointUri, servers: McpServerMapHandle) -> Self {
        let server = operation.server().to_owned();
        Self {
            operation,
            server,
            servers,
        }
    }

    /// The cached connected client, or an error if the producer is not started.
    async fn client(&self) -> Result<Arc<dyn crate::client::McpClient>, McpError> {
        self.servers.get(&self.server).await.ok_or_else(|| {
            McpError::Endpoint(format!(
                "MCP remote '{}' is not connected (producer not started)",
                self.server
            ))
        })
    }

    /// Run the operation for this Exchange.
    async fn process(&self, exchange: &mut Exchange) -> Result<(), McpError> {
        match &self.operation {
            McpEndpointUri::Call { tool, .. } => self.handle_call(exchange, tool).await,
            McpEndpointUri::Read { uri, .. } => self.handle_read(exchange, uri).await,
            // Consumer-shaped operations never reach a producer
            // (`create_producer` rejects them); defensive arm keeps the
            // match exhaustive as the enum grows.
            McpEndpointUri::Tool { .. } | McpEndpointUri::Resource { .. } => {
                Err(McpError::Endpoint(format!(
                    "MCP producer cannot dispatch consumer-shaped operation \
                     for remote '{}'",
                    self.server
                )))
            }
        }
    }

    /// `mcp:call` — issue one `tools/call` with the Exchange body as arguments.
    async fn handle_call(&self, exchange: &mut Exchange, tool: &str) -> Result<(), McpError> {
        let arguments = extract_json_arguments(exchange)?;
        exchange.input.headers.insert(
            CAMEL_MCP_TOOL_CALL.to_string(),
            serde_json::json!({ "server": self.server, "tool": tool }),
        );
        let result = self.client().await?.call_tool(tool, arguments).await?;
        // ADR-0060: the producer carries the remote's `is_error` flag and the
        // content into the CamelMcpResult header (and the content alone into
        // the body). It never acts on the flag — the route author decides.
        exchange.input.headers.insert(
            CAMEL_MCP_RESULT.to_string(),
            serde_json::json!({
                "is_error": result.is_error,
                "content": result.content.clone(),
            }),
        );
        exchange.input.body = Body::Json(result.content);
        Ok(())
    }

    /// `mcp:read` — issue one `resources/read` with no arguments body.
    async fn handle_read(&self, exchange: &mut Exchange, uri: &str) -> Result<(), McpError> {
        let resource = self.client().await?.read_resource(uri).await?;
        exchange.input.body = Body::Bytes(resource.content.into());
        Ok(())
    }
}

/// Extract the Exchange body as the tool-arguments JSON object.
///
/// `Body::Json` passes through; `Body::Empty` becomes `{}`; `Body::Text` is
/// parsed as JSON. Anything else is rejected.
fn extract_json_arguments(exchange: &Exchange) -> Result<serde_json::Value, McpError> {
    match &exchange.input.body {
        Body::Json(value) => Ok(value.clone()),
        Body::Empty => Ok(serde_json::json!({})),
        Body::Text(text) => serde_json::from_str(text).map_err(|error| {
            McpError::Endpoint(format!(
                "mcp:call arguments body is not a JSON object: {error}"
            ))
        }),
        _ => Err(McpError::Endpoint(
            "mcp:call arguments body must be a JSON object".to_owned(),
        )),
    }
}

impl Service<Exchange> for McpProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Ready only after the lifecycle `start()` has connected the remote.
        // A failed connect leaves the map empty, so this stays Pending forever
        // — the producer never becomes ready (fail-fast at start).
        if self.servers.try_contains(&self.server) {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let producer = self.clone();
        Box::pin(async move {
            match producer.process(&mut exchange).await {
                Ok(()) => Ok(exchange),
                Err(error) => {
                    tracing::warn!(
                        server = %producer.server,
                        error = %error,
                        "mcp producer error"
                    );
                    // Runtime dispatch failures (bad arguments body, mid-flight
                    // transport errors, "not connected") are processor errors, not
                    // endpoint-creation: route error policies classify off the
                    // CamelError kind. Construction/start-time paths keep the
                    // `From<McpError>` impl.
                    Err(CamelError::ProcessorError(error.to_string()))
                }
            }
        })
    }
}

/// `StepLifecycle` handle that connects the producer's remote at route start.
///
/// `start()` runs `RmcpClient::connect` (discover lifecycle, `2026-07-28`
/// only) and caches the connected client in the shared live map keyed by
/// server name. On failure it returns the mapped `CamelError` (fail-fast at
/// start, spec: Producer fail-fast on incompatible remote).
pub struct McpProducerLifecycle {
    server: String,
    remote: McpRemoteConfig,
    servers: McpServerMapHandle,
}

impl McpProducerLifecycle {
    pub(crate) fn new(
        server: String,
        remote: McpRemoteConfig,
        servers: McpServerMapHandle,
    ) -> Self {
        Self {
            server,
            remote,
            servers,
        }
    }
}

impl std::fmt::Debug for McpProducerLifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpProducerLifecycle")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl StepLifecycle for McpProducerLifecycle {
    fn name(&self) -> &'static str {
        "mcp-producer"
    }

    async fn start(&self) -> Result<(), CamelError> {
        let client = RmcpClient::connect(&self.server, &self.remote)
            .await
            .map_err(CamelError::from)?;
        self.servers
            .register(self.server.clone(), Arc::new(client))
            .await;
        Ok(())
    }

    async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
        // Decrement the per-name refcount; the cached client is dropped only
        // when no live producer remains on this server name (sibling routes
        // sharing the remote keep their client).
        self.servers.deregister(&self.server).await;
        Ok(())
    }
}
