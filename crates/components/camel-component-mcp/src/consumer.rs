//! MCP server-role Consumer — bridges registry-dispatched tool invocations
//! and resource reads into the route's Pipeline (Task 2.4).
//!
//! Follows the repo's request-reply bridge consumers (`MqttConsumer` in
//! `camel-mqtt`, `KafkaConsumer` in `camel-kafka`, `SqlConsumer` in
//! `camel-sql`, `SurrealDbConsumer` in `camel-component-surrealdb`): the
//! shared Streamable-HTTP listener comes from the process-global
//! [`McpServerRegistry`] (one per bind, ADR-0060), this consumer registers
//! its route channel in the listener's tool or resource registry, and a
//! spawned bridge task turns each [`McpToolInvocation`] /
//! [`McpResourceRead`] into an Exchange that is handed to the route through
//! [`ConsumerContext::send_and_wait`] — the same request-reply handoff those
//! consumers use, with the route's answer sent back on the invocation's
//! oneshot `reply`.
//!
//! Lifecycle: `start()` resolves the named server's config (fail-fast on an
//! unknown server or a bind-policy violation), spawns the shared listener if
//! needed, registers, spawns the bridge, and signals readiness (Explicit
//! startup mode). `stop()` unregisters and releases the handle; subsequent
//! dispatch resolves `None` and maps to a clean MCP method error instead of
//! awaiting a dead channel.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use camel_api::{Body, CamelError, Exchange, Message};
use camel_component_api::{ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode};

use crate::config::{BindPolicyWarning, McpServerConfig, validate_server_policy};
use crate::endpoint::McpEndpointUri;
use crate::error::McpError;
use crate::registry::{McpListenerHandle, McpServerRegistry};
use crate::types::{McpResource, McpResourceRead, McpToolInvocation, McpToolResult};

/// Buffer for registry → bridge invocations (feeds the request-reply
/// `send_and_wait` bridge that `MqttConsumer` / `KafkaConsumer` /
/// `SqlConsumer` / `SurrealDbConsumer` also run).
const INVOCATION_BUFFER: usize = 64;

/// Upper bound when materializing a streamed route output body.
const MAX_STREAM_BODY_BYTES: usize = 10 << 20;

/// What a started consumer registered — drives `stop` cleanup.
enum Registration {
    Tool { name: String },
    Resource { uri: String },
}

/// A started consumer's resources, released by `stop`.
struct Running {
    registration: Registration,
    handle: Arc<McpListenerHandle>,
    /// The bridge task (detached by `start`); taken by
    /// [`Consumer::background_task_handle`] for runtime monitoring.
    bridge: Option<JoinHandle<Result<(), CamelError>>>,
}

/// Server-role (Consumer) endpoint for `mcp:<server>/tool/<name>` and
/// `mcp:<server>/resource/<name>` URIs.
pub struct McpConsumer {
    operation: McpEndpointUri,
    server_configs: Arc<HashMap<String, McpServerConfig>>,
    running: Option<Running>,
}

impl McpConsumer {
    pub fn new(
        operation: McpEndpointUri,
        server_configs: Arc<HashMap<String, McpServerConfig>>,
    ) -> Self {
        Self {
            operation,
            server_configs,
            running: None,
        }
    }
}

#[async_trait]
impl Consumer for McpConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // (a) Resolve the named server's config. Consumer START fails fast on
        // an unknown server (the producer resolves its remote at creation
        // instead) so bind-policy failures surface through the consumer.
        let server = self.operation.server().to_owned();
        let cfg = self.server_configs.get(&server).cloned().ok_or_else(|| {
            McpError::Endpoint(format!("MCP server '{server}' not found in config"))
        })?;

        // (b) Bind policy: fail-closed errors propagate; a non-loopback bind
        // warns exactly once per start, naming server and bind in the message
        // (operator-visible, ADR-0012 advisory shape).
        if let Some(BindPolicyWarning::NonLoopback) = validate_server_policy(&server, &cfg)? {
            tracing::warn!(
                "MCP server '{server}' binds non-loopback address '{}' — the listener is \
                 reachable from the network",
                cfg.bind
            );
        }

        // (c) The shared listener for this bind (the first consumer spawns
        // it; later consumers reuse the handle).
        let handle = McpServerRegistry::global()
            .get_or_spawn(&cfg.bind, &cfg)
            .await?;

        // (d) Duplicate guard + register. The registry rejects duplicates
        // atomically (see `register`), so the pre-check here is only a
        // friendly-error fast path that lets the second consumer fail before
        // spawning a bridge: the registry's atomic rejection is what actually
        // prevents two concurrent same-name starts from silently overwriting
        // the first registration (stranding the first route's channel).
        let (registration, bridge) = match self.operation.clone() {
            McpEndpointUri::Tool {
                name, input_schema, ..
            } => {
                if handle.tool_registry.resolve(&name).is_some() {
                    return Err(McpError::Endpoint(format!(
                        "tool '{name}' is already registered on MCP server '{server}' — a \
                         second consumer for the same tool name is refused"
                    ))
                    .into());
                }
                let (tx, rx) = mpsc::channel(INVOCATION_BUFFER);
                handle
                    .tool_registry
                    .register(name.clone(), tx, input_schema)?;
                (
                    Registration::Tool { name },
                    spawn_tool_bridge(rx, ctx.clone()),
                )
            }
            McpEndpointUri::Resource { resource_uri, .. } => {
                if handle.resource_registry.resolve(&resource_uri).is_some() {
                    return Err(McpError::Endpoint(format!(
                        "resource '{resource_uri}' is already registered on MCP server \
                         '{server}' — a second consumer for the same resource URI is refused"
                    ))
                    .into());
                }
                let (tx, rx) = mpsc::channel(INVOCATION_BUFFER);
                handle
                    .resource_registry
                    .register(resource_uri.clone(), tx)?;
                (
                    Registration::Resource { uri: resource_uri },
                    spawn_resource_bridge(rx, ctx.clone()),
                )
            }
            operation => {
                return Err(CamelError::EndpointCreationFailed(format!(
                    "MCP consumer cannot start for producer operation {operation:?}"
                )));
            }
        };

        // (f) Readiness, in order: registry first (the tool/resource appears
        // in `tools/list` / `resources/list`), then the runtime handshake —
        // bind, register, and bridge are all done at this point.
        match &registration {
            Registration::Tool { name } => handle.tool_registry.mark_ready(name),
            Registration::Resource { uri } => handle.resource_registry.mark_ready(uri),
        }
        ctx.mark_ready();

        self.running = Some(Running {
            registration,
            handle,
            bridge: Some(bridge),
        });
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        let Some(running) = self.running.take() else {
            return Ok(());
        };

        // Unregister FIRST: subsequent dispatch resolves `None` and maps to a
        // clean MCP method error instead of awaiting a dead channel.
        match &running.registration {
            Registration::Tool { name } => running.handle.tool_registry.unregister(name),
            Registration::Resource { uri } => running.handle.resource_registry.unregister(uri),
        }

        // Cancel the bridge if we still own it (it may be parked on `recv`
        // while external sender clones keep the channel open), then release
        // the shared-listener handle.
        if let Some(ref bridge) = running.bridge {
            bridge.abort();
        }
        drop(running);
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        // Inbound server consumer — each invocation is answered by its own
        // spawned bridge task, so invocations run concurrently.
        ConcurrencyModel::Concurrent { max: None }
    }

    // The consumer binds a listener (get_or_spawn) and registers inside
    // start(); Explicit startup makes the runtime await that before treating
    // the route as started (bind failures fail the route, rc-w1u9 shape).
    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.running.as_mut()?.bridge.take()
    }
}

/// Route-facing bridge for one tool consumer: turns registry-delivered
/// [`McpToolInvocation`]s into request-reply Exchanges.
///
/// Each invocation is handled in its own spawned task so the bridge keeps
/// accepting while earlier invocations are still in the pipeline —
/// concurrent `tools/call` requests are served concurrently.
fn spawn_tool_bridge(
    mut rx: mpsc::Receiver<McpToolInvocation>,
    ctx: ConsumerContext,
) -> JoinHandle<Result<(), CamelError>> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = ctx.cancelled() => break,
                invocation = rx.recv() => {
                    let Some(invocation) = invocation else { break };
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        // Do not inject into a shutting-down pipeline
                        // (camel-http 503-on-cancel precedent).
                        let result = if ctx.is_cancelled() {
                            Err(CamelError::ConsumerStopping)
                        } else {
                            // Carry the wire headers onto the Exchange input so
                            // the route-level `SecurityPolicy` evaluates
                            // credentials (e.g. `Authorization`) exactly as
                            // camel-http routes do.
                            let mut msg = Message::new(Body::Json(invocation.arguments));
                            for (name, value) in &invocation.headers {
                                msg.set_header(
                                    name.clone(),
                                    serde_json::Value::String(value.clone()),
                                );
                            }
                            ctx.send_and_wait(Exchange::new(msg)).await
                        };
                        // The structured `is_error` flag is set ONLY on a
                        // genuine failure: a failed exchange, or a body
                        // materialization failure. A successful route that
                        // happens to produce an `{"error": ...}` payload stays
                        // `is_error: false` — hosts must not sniff content.
                        let (content, is_error) = match result {
                            Ok(out) => body_to_json(out.input.body).await,
                            Err(e) => (serde_json::json!({ "error": e.to_string() }), true),
                        };
                        // A dropped reply means the dispatcher gave up; the
                        // invocation is answered or discarded on that side.
                        let _ = invocation
                            .reply
                            .send(McpToolResult { content, is_error });
                    });
                }
            }
        }
        Ok(())
    })
}

/// Route-facing bridge for one resource consumer: turns registry-delivered
/// [`McpResourceRead`]s into request-reply Exchanges (same shape as
/// [`spawn_tool_bridge`]).
fn spawn_resource_bridge(
    mut rx: mpsc::Receiver<McpResourceRead>,
    ctx: ConsumerContext,
) -> JoinHandle<Result<(), CamelError>> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = ctx.cancelled() => break,
                read = rx.recv() => {
                    let Some(read) = read else { break };
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result = if ctx.is_cancelled() {
                            Err(CamelError::ConsumerStopping)
                        } else {
                            // Body carries the requested resource URI — the
                            // route's representation of the read. Wire headers
                            // ride the Exchange input for the route-level
                            // `SecurityPolicy` (see the tool bridge).
                            let mut msg = Message::new(Body::Text(read.uri.clone()));
                            for (name, value) in &read.headers {
                                msg.set_header(
                                    name.clone(),
                                    serde_json::Value::String(value.clone()),
                                );
                            }
                            ctx.send_and_wait(Exchange::new(msg)).await
                        };
                        let resource = match result {
                            Ok(out) => body_to_resource(out, &read.uri).await,
                            Err(e) => McpResource {
                                uri: read.uri.clone(),
                                content: e.to_string().into_bytes(),
                                mime_type: "text/plain; charset=utf-8".to_owned(),
                            },
                        };
                        let _ = read.reply.send(resource);
                    });
                }
            }
        }
        Ok(())
    })
}

/// Serialize a route output body into tool-result content.
///
/// Returns `(content, is_error)`: `is_error` is true only when the body
/// materialization itself failed (oversize or unreadable stream), never based
/// on what the content contains.
///
/// `Json` bodies pass through as JSON; textual bodies become JSON strings;
/// bytes are best-effort UTF-8; empty bodies become `null`; streams are
/// materialized (bounded) and treated as bytes.
async fn body_to_json(body: Body) -> (serde_json::Value, bool) {
    match body {
        Body::Json(value) => (value, false),
        Body::Text(s) => (serde_json::Value::String(s), false),
        Body::Xml(s) => (serde_json::Value::String(s), false),
        other => match other.into_bytes(MAX_STREAM_BODY_BYTES).await {
            Ok(bytes) if bytes.is_empty() => (serde_json::Value::Null, false),
            Ok(bytes) => (
                serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned()),
                false,
            ),
            Err(e) => (serde_json::json!({ "error": e.to_string() }), true),
        },
    }
}

/// Materialize a route output body into resource content.
///
/// MIME type precedence: the output message's `Content-Type` header, then a
/// streamed body's metadata content type, then the
/// `application/octet-stream` default.
///
/// Read failures are never silent: a failed or oversize stream body yields
/// the error string as `text/plain` content — the same shape the resource
/// bridge uses for a route failure (and [`body_to_json`] mirrors for tools
/// via `{"error": ...}`) — so a failed read is distinguishable from an
/// empty resource.
async fn body_to_resource(out: Exchange, uri: &str) -> McpResource {
    let header_mime = out
        .input
        .header("Content-Type")
        .and_then(|value| value.as_str())
        .map(str::to_owned);

    let materialized: Result<(Vec<u8>, Option<String>), CamelError> = match out.input.body {
        Body::Json(value) => Ok((value.to_string().into_bytes(), None)),
        Body::Text(s) => Ok((s.into_bytes(), None)),
        Body::Xml(s) => Ok((s.into_bytes(), None)),
        Body::Stream(stream) => {
            let mime = stream.metadata.content_type.clone();
            Body::Stream(stream)
                .into_bytes(MAX_STREAM_BODY_BYTES)
                .await
                .map(|bytes| (bytes.to_vec(), mime))
        }
        other => other
            .into_bytes(MAX_STREAM_BODY_BYTES)
            .await
            .map(|bytes| (bytes.to_vec(), None)),
    };

    let (content, mime_type) = match materialized {
        Ok((bytes, stream_mime)) => (
            bytes,
            header_mime
                .or(stream_mime)
                .unwrap_or_else(|| "application/octet-stream".to_owned()),
        ),
        Err(e) => (
            e.to_string().into_bytes(),
            "text/plain; charset=utf-8".to_owned(),
        ),
    };

    McpResource {
        uri: uri.to_owned(),
        content,
        mime_type,
    }
}
