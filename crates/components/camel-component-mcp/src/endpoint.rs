//! MCP endpoint — both `mcp:` roles on one [`Endpoint`] type.
//!
//! [`McpEndpointUri`] parses the four URI shapes:
//!
//! - `mcp:call?server=<name>&tool=<name>` → [`McpEndpointUri::Call`]
//! - `mcp:read?server=<name>&uri=<uri>` → [`McpEndpointUri::Read`]
//! - `mcp:<server>/tool/<name>?schema=<url-encoded JSON Schema>` →
//!   [`McpEndpointUri::Tool`]
//! - `mcp:<server>/resource/<name>?uri=<mcp-uri>` →
//!   [`McpEndpointUri::Resource`]
//!
//! Anything else is rejected as [`McpError::Endpoint`] naming the URI. Read
//! does NOT field-sniff tool-vs-resource — the two operations are distinct
//! URI shapes.
//!
//! [`McpEndpoint`] carries the parsed operation plus role-specific state:
//! the resolved [`McpRemoteConfig`] for producer URIs (resolved at creation,
//! fail-fast on an unknown remote), and the server-role configs for consumer
//! URIs (the named server is resolved at consumer START, so bind-policy
//! failures such as a missing security policy surface through the consumer).
//! The actual `RmcpClient::connect` happens at producer START through the
//! [`StepLifecycle`] handle returned by [`Endpoint::lifecycle`] — never at
//! construction (fail-fast-at-start, spec: Producer fail-fast on incompatible
//! remote). Consumer URIs have no producer lifecycle (`lifecycle` returns
//! `None` for them).

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::{BoxProcessor, CamelError, StepLifecycle};
use camel_component_api::{Consumer, Endpoint, ProducerContext, RuntimeObservability};

use crate::client::McpServerMapHandle;
use crate::config::{McpRemoteConfig, McpServerConfig};
use crate::consumer::McpConsumer;
use crate::error::McpError;
use crate::producer::{McpProducer, McpProducerLifecycle};

/// The parsed operation from an `mcp:` endpoint URI.
#[derive(Debug, Clone, PartialEq)]
pub enum McpEndpointUri {
    /// `mcp:call?server=<name>&tool=<name>` — producer: invoke one tool.
    Call { server: String, tool: String },
    /// `mcp:read?server=<name>&uri=<uri>` — producer: read one resource.
    Read { server: String, uri: String },
    /// `mcp:<server>/tool/<name>?schema=<json>` — consumer: serve one tool
    /// from the shared server listener.
    Tool {
        server: String,
        name: String,
        /// Declared JSON Schema for the tool's arguments (the DSL lowering
        /// channel — carried URL-encoded on the URI, decoded by parsing).
        input_schema: serde_json::Value,
    },
    /// `mcp:<server>/resource/<name>?uri=<mcp-uri>` — consumer: serve one
    /// resource from the shared server listener.
    Resource {
        server: String,
        name: String,
        /// The declared MCP resource URI (operator config; the registry key).
        resource_uri: String,
    },
}

impl McpEndpointUri {
    /// Parse an endpoint URI into an operation.
    ///
    /// The operation path is everything between the `mcp:` scheme and the
    /// first `?`. A single-segment path must be a producer operation
    /// (`call`/`read`); a three-segment path `<server>/<kind>/<name>` must be
    /// a consumer operation (`tool`/`resource`). Any other shape is rejected
    /// naming the URI.
    pub fn parse(uri: &str) -> Result<Self, McpError> {
        let components = camel_component_api::parse_uri(uri).map_err(|error| {
            McpError::Endpoint(format!("invalid MCP endpoint URI '{uri}': {error}"))
        })?;

        match components.path.as_str() {
            "call" => {
                let server = required_param(&components.params, "server", uri)?;
                let tool = required_param(&components.params, "tool", uri)?;
                Ok(McpEndpointUri::Call { server, tool })
            }
            "read" => {
                let server = required_param(&components.params, "server", uri)?;
                let resource_uri = required_param(&components.params, "uri", uri)?;
                Ok(McpEndpointUri::Read {
                    server,
                    uri: resource_uri,
                })
            }
            other => Self::parse_consumer(other, &components.params, uri),
        }
    }

    /// Parse a consumer-shaped path `<server>/<kind>/<name>`.
    ///
    /// Query values arrive percent-decoded from [`camel_component_api::parse_uri`],
    /// so an undecodable `schema` parameter surfaces here as a JSON parse
    /// failure, rejected naming the URI.
    fn parse_consumer(
        path: &str,
        params: &HashMap<String, String>,
        uri: &str,
    ) -> Result<Self, McpError> {
        let reject = || {
            McpError::Endpoint(format!(
                "unknown MCP endpoint path '{path}' in URI '{uri}' (expected producer \
                 'call'/'read' or consumer '<server>/tool/<name>' / \
                 '<server>/resource/<name>')"
            ))
        };

        let mut segments = path.split('/');
        let (server, kind, name) = match (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ) {
            (Some(server), Some(kind), Some(name), None) => (server, kind, name),
            _ => return Err(reject()),
        };
        if server.is_empty() || name.is_empty() {
            return Err(reject());
        }

        match kind {
            "tool" => {
                let raw_schema = params.get("schema").ok_or_else(|| {
                    McpError::Endpoint(format!(
                        "MCP endpoint URI '{uri}' is missing required parameter 'schema' \
                         (URL-encoded tool input JSON Schema)"
                    ))
                })?;
                let input_schema: serde_json::Value =
                    serde_json::from_str(raw_schema).map_err(|error| {
                        McpError::Endpoint(format!(
                            "MCP endpoint URI '{uri}' carries an undecodable 'schema' \
                             parameter: {error}"
                        ))
                    })?;
                if !input_schema.is_object() {
                    return Err(McpError::Endpoint(format!(
                        "MCP endpoint URI '{uri}' carries a 'schema' parameter that is \
                         not a JSON object"
                    )));
                }
                Ok(McpEndpointUri::Tool {
                    server: server.to_string(),
                    name: name.to_string(),
                    input_schema,
                })
            }
            "resource" => {
                let resource_uri = required_param(params, "uri", uri)?;
                Ok(McpEndpointUri::Resource {
                    server: server.to_string(),
                    name: name.to_string(),
                    resource_uri,
                })
            }
            _ => Err(reject()),
        }
    }

    /// The named server this operation targets (remote for producer shapes,
    /// server-role entry for consumer shapes).
    pub fn server(&self) -> &str {
        match self {
            McpEndpointUri::Call { server, .. }
            | McpEndpointUri::Read { server, .. }
            | McpEndpointUri::Tool { server, .. }
            | McpEndpointUri::Resource { server, .. } => server,
        }
    }
}

/// Extract a required query parameter, naming the URI on absence.
fn required_param(
    params: &HashMap<String, String>,
    key: &str,
    uri: &str,
) -> Result<String, McpError> {
    params.get(key).cloned().ok_or_else(|| {
        McpError::Endpoint(format!(
            "MCP endpoint URI '{uri}' is missing required parameter '{key}'"
        ))
    })
}

/// An `mcp:` endpoint — producer (client) and consumer (server) role.
pub struct McpEndpoint {
    uri: String,
    operation: McpEndpointUri,
    /// Resolved remote config — `Some` only for producer-shaped URIs.
    remote: Option<McpRemoteConfig>,
    /// Server-role configs for consumer-shaped URIs; the named server is
    /// resolved (fail-fast) at consumer START, not at creation.
    server_configs: Arc<HashMap<String, McpServerConfig>>,
    /// The shared live client map for producer dispatch (seeded empty at
    /// component construction; connected at producer start).
    live_servers: McpServerMapHandle,
}

impl McpEndpoint {
    pub fn new(
        uri: String,
        operation: McpEndpointUri,
        remote: Option<McpRemoteConfig>,
        server_configs: Arc<HashMap<String, McpServerConfig>>,
        live_servers: McpServerMapHandle,
    ) -> Self {
        Self {
            uri,
            operation,
            remote,
            server_configs,
            live_servers,
        }
    }
}

impl Endpoint for McpEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        match &self.operation {
            McpEndpointUri::Tool { .. } | McpEndpointUri::Resource { .. } => Ok(Box::new(
                McpConsumer::new(self.operation.clone(), Arc::clone(&self.server_configs)),
            )),
            McpEndpointUri::Call { .. } | McpEndpointUri::Read { .. } => {
                Err(CamelError::EndpointCreationFailed(format!(
                    "MCP endpoint URI '{}' is producer-shaped; only \
                     '<server>/tool/<name>' and '<server>/resource/<name>' create consumers",
                    self.uri
                )))
            }
        }
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        if self.remote.is_none() {
            return Err(CamelError::EndpointCreationFailed(format!(
                "MCP endpoint URI '{}' is consumer-shaped; only 'call' and 'read' create \
                 producers",
                self.uri
            )));
        }
        Ok(BoxProcessor::new(McpProducer::new(
            self.operation.clone(),
            Arc::clone(&self.live_servers),
        )))
    }

    /// The producer connects to its remote at route start (fail-fast); the
    /// returned handle performs `RmcpClient::connect` and caches the client in
    /// the shared live map on `start()`. Consumer-shaped URIs have no
    /// producer lifecycle.
    fn lifecycle(&self) -> Option<Arc<dyn StepLifecycle>> {
        let server = match &self.operation {
            McpEndpointUri::Call { server, .. } | McpEndpointUri::Read { server, .. } => server,
            McpEndpointUri::Tool { .. } | McpEndpointUri::Resource { .. } => return None,
        };
        // Always `Some` for Call/Read (set by `create_endpoint`); defensive
        // for any future construction path.
        let remote = self.remote.as_ref()?;
        Some(Arc::new(McpProducerLifecycle::new(
            server.to_owned(),
            remote.clone(),
            Arc::clone(&self.live_servers),
        )))
    }
}
