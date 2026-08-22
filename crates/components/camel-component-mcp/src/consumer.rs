//! MCP server-role Consumer — bridges registry-dispatched tool invocations
//! and resource reads into the route's Pipeline (Task 2.4; kernel
//! migration Task 2.6).
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
//! Security (Task 2.6): the route controller wires the compiled
//! [`RouteSecurityPlan`] and provider snapshot through
//! [`Consumer::set_security_context`] before `start()`; start registers
//! them with the bind's dispatch-security book (keyed by route id) and runs
//! the ADR-0061 per-bind exposure gate. The rmcp adapter then authenticates
//! each invocation at the request seam (`kernel_authenticate`) and the
//! bridge installs the minted principal as the Exchange's typed carrier
//! before the pipeline runs.
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

use camel_api::security_policy::{AccessMode, RouteSecurityPlan, TransportId};
use camel_api::{Body, CamelError, Exchange, Message};
use camel_auth::install_carrier;
use camel_component_api::{
    ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode, SecurityContext,
};

use crate::config::{McpDeclaredServer, McpServerConfig, validate_server_policy};
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

/// Merge the DSL-declared listener values into the TOML server config
/// (spec: MCP listener ownership).
///
/// The DSL `mcp:` block owns `bind`/`tls`/`max_tools`/`max_resources` when
/// it declares them — they ARE the runtime values for listener
/// construction/lookup. TOML `mcp.servers.<name>` remains the source for
/// keys with no DSL counterpart (`allowed_hosts`, `security_policy`) and
/// those still apply untouched.
///
/// Conflict rule: a key DECLARED by both sides with different values fails
/// start with [`McpError::ConfigConflict`] naming both sources and both
/// values. Caps and TLS are presence-based (`Option` on both sides): a
/// value declared by exactly one side is that side's runtime value —
/// silence on the other side is not a disagreement — and a cap declared by
/// neither keeps the 128 default (`config::DEFAULT_CAP`), applied after
/// this merge at listener materialization so the default can never
/// conflict with or overwrite a declared value. `bind` is always declared
/// by both sides and compared directly.
fn merge_declared_server(
    server: &str,
    mut toml: McpServerConfig,
    dsl: &McpDeclaredServer,
) -> Result<McpServerConfig, McpError> {
    if toml.bind != dsl.bind {
        return Err(McpError::ConfigConflict {
            server: server.to_string(),
            key: "bind",
            dsl: dsl.bind.clone(),
            toml: toml.bind.clone(),
        });
    }
    match (&dsl.tls, &toml.tls) {
        // Both sides declared TLS with different paths — hard conflict.
        (Some(dsl_tls), Some(toml_tls)) if dsl_tls != toml_tls => {
            return Err(McpError::ConfigConflict {
                server: server.to_string(),
                key: "tls",
                dsl: format!(
                    "tls(cert_path={}, key_path={})",
                    dsl_tls.cert_path, dsl_tls.key_path
                ),
                toml: format!(
                    "tls(cert_path={}, key_path={})",
                    toml_tls.cert_path, toml_tls.key_path
                ),
            });
        }
        _ => {}
    }
    // DSL declares TLS (TOML silent or equal) — the DSL value is runtime;
    // DSL-silent keeps TOML's TLS untouched.
    if let Some(dsl_tls) = &dsl.tls {
        toml.tls = Some(dsl_tls.clone());
    }
    // Caps, presence-based: both declared and different → hard conflict
    // naming both sources and both values.
    for (key, dsl_cap, toml_cap) in [
        ("max_tools", dsl.max_tools, toml.max_tools),
        ("max_resources", dsl.max_resources, toml.max_resources),
    ] {
        if let (Some(dsl_value), Some(toml_value)) = (dsl_cap, toml_cap)
            && dsl_value != toml_value
        {
            return Err(McpError::ConfigConflict {
                server: server.to_string(),
                key,
                dsl: dsl_value.to_string(),
                toml: toml_value.to_string(),
            });
        }
    }
    // One side declared → that side wins; neither → `None` stands (the
    // 128 default is applied after this merge, at listener
    // materialization — never here, where it could shadow a decision).
    toml.max_tools = dsl.max_tools.or(toml.max_tools);
    toml.max_resources = dsl.max_resources.or(toml.max_resources);
    Ok(toml)
}

/// What a started consumer registered — drives `stop` cleanup.
enum Registration {
    Tool { name: String },
    Resource { uri: String },
}

/// A started consumer's resources, released by `stop`.
struct Running {
    registration: Registration,
    handle: Arc<McpListenerHandle>,
    /// Route id this consumer registered its plan under (Task 2.6) —
    /// unregistered from the bind's security book on stop.
    route_id: String,
    /// The bridge task (detached by `start`); taken by
    /// [`Consumer::background_task_handle`] for runtime monitoring.
    bridge: Option<JoinHandle<Result<(), CamelError>>>,
}

/// Server-role (Consumer) endpoint for `mcp:<server>/tool/<name>` and
/// `mcp:<server>/resource/<name>` URIs.
pub struct McpConsumer {
    operation: McpEndpointUri,
    server_configs: Arc<HashMap<String, McpServerConfig>>,
    /// Security context wired by the route controller before `start()`
    /// (plan + provider snapshot, Task 1.8/2.6). `None` for routes without
    /// route-level security — the consumer then registers a default
    /// `Public` plan (ADR-0061 Rule 4: public by default, gated per bind).
    security_ctx: Option<SecurityContext>,
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
            security_ctx: None,
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
        let toml_cfg = self.server_configs.get(&server).cloned().ok_or_else(|| {
            McpError::Endpoint(format!("MCP server '{server}' not found in config"))
        })?;

        // (a2) DSL listener ownership: on a route lowered from an `mcp:` DSL
        // block (the endpoint URI carries `mcp.declared.*` parameters), the
        // DSL `bind`/`tls` and any DECLARED caps ARE the runtime values —
        // merged into the TOML entry, which keeps supplying the keys with
        // no DSL counterpart (`allowed_hosts`, `security_policy`). A key
        // declared by both sides with different values fails start naming
        // both sources (spec scenario 'TOML/DSL conflict fails startup');
        // a route without declared parameters is TOML-only and takes the
        // config verbatim.
        let cfg = match self.operation.declared_server() {
            Some(declared) => merge_declared_server(&server, toml_cfg, declared)?,
            None => toml_cfg,
        };

        // (b) Bind policy: fail-closed config checks (IP-literal bind,
        // non-zero caps) propagate; the loopback classification feeds the
        // exposure gate below. The former `security_policy` presence gate was
        // removed in Task 2.9 — public exposure is the kernel gate's call.
        let is_loopback = validate_server_policy(&server, &cfg)?.is_none();

        // (b2) Task 2.6 kernel migration: register this route's compiled
        // plan with the per-bind dispatch-security book, then run the
        // ADR-0061 per-bind exposure gate over every plan on the bind
        // (this route's included). This REPLACES the former warn-only
        // non-loopback advisory: non-loopback binds exposing Public routes
        // refuse to start unless the operator acknowledged the bind
        // (refuse-without-ack), and an acknowledged bind warns permanently
        // (inside the gate). Runs before any socket is bound — a refused
        // route never spawns a listener.
        let registry = McpServerRegistry::global();
        let route_id = ctx.route_id().to_owned();
        let security = registry.bind_security(&cfg.bind);
        let (plan, providers) = match self.security_ctx.as_ref() {
            // A route the controller wires with a security context carries a
            // compiled plan (Task 1.8 every-server-route invariant). A
            // missing plan here means un-wired compilation — fail closed
            // rather than downgrade declared security to Public
            // (ADR-0061 Rule 5).
            Some(sec_ctx) => match sec_ctx.plan.clone() {
                Some(plan) => (plan, sec_ctx.providers.clone()),
                None => {
                    return Err(CamelError::RouteError(format!(
                        "route '{route_id}' declares security but carries no compiled plan"
                    )));
                }
            },
            // No security context: the route declared no route-level
            // security — Public by default, still gated (ADR-0061 Rule 4).
            None => (default_public_plan(), None),
        };
        let owned = security.plans_snapshot();
        let mut plan_refs: Vec<(&str, &RouteSecurityPlan)> =
            owned.iter().map(|(id, plan)| (id.as_str(), plan)).collect();
        plan_refs.push((route_id.as_str(), &plan));
        camel_auth::enforce_bind_exposure_gate(
            &cfg.bind,
            is_loopback,
            &plan_refs,
            registry.acknowledged(&cfg.bind),
        )?;
        security.register_plan(&route_id, plan, providers);

        // (c) The shared listener for this bind (the first consumer spawns
        // it; later consumers reuse the handle).
        let handle = match registry.get_or_spawn(&cfg.bind, &cfg).await {
            Ok(handle) => handle,
            Err(e) => {
                security.unregister_plan(&route_id);
                return Err(e.into());
            }
        };

        // (d) Duplicate guard + register. The registry rejects duplicates
        // atomically (see `register`), so the pre-check here is only a
        // friendly-error fast path that lets the second consumer fail before
        // spawning a bridge: the registry's atomic rejection is what actually
        // prevents two concurrent same-name starts from silently overwriting
        // the first registration (stranding the first route's channel).
        // Any failure here unregisters the plan registered in (b2) — a
        // refused start must leave no stale plan on the bind's gate.
        let (registration, bridge) = match self.operation.clone() {
            McpEndpointUri::Tool {
                name, input_schema, ..
            } => {
                if handle.tool_registry.resolve(&name).is_some() {
                    security.unregister_plan(&route_id);
                    return Err(McpError::Endpoint(format!(
                        "tool '{name}' is already registered on MCP server '{server}' — a \
                         second consumer for the same tool name is refused"
                    ))
                    .into());
                }
                let (tx, rx) = mpsc::channel(INVOCATION_BUFFER);
                if let Err(e) =
                    handle
                        .tool_registry
                        .register(name.clone(), route_id.clone(), tx, input_schema)
                {
                    security.unregister_plan(&route_id);
                    return Err(e.into());
                }
                (
                    Registration::Tool { name },
                    spawn_tool_bridge(rx, ctx.clone()),
                )
            }
            McpEndpointUri::Resource { resource_uri, .. } => {
                if handle.resource_registry.resolve(&resource_uri).is_some() {
                    security.unregister_plan(&route_id);
                    return Err(McpError::Endpoint(format!(
                        "resource '{resource_uri}' is already registered on MCP server \
                         '{server}' — a second consumer for the same resource URI is refused"
                    ))
                    .into());
                }
                let (tx, rx) = mpsc::channel(INVOCATION_BUFFER);
                if let Err(e) =
                    handle
                        .resource_registry
                        .register(resource_uri.clone(), route_id.clone(), tx)
                {
                    security.unregister_plan(&route_id);
                    return Err(e.into());
                }
                (
                    Registration::Resource { uri: resource_uri },
                    spawn_resource_bridge(rx, ctx.clone()),
                )
            }
            operation => {
                security.unregister_plan(&route_id);
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
            route_id,
            bridge: Some(bridge),
        });
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        let Some(running) = self.running.take() else {
            return Ok(());
        };

        // Unregister FIRST: subsequent dispatch resolves `None` and maps to a
        // clean MCP method error instead of awaiting a dead channel. The
        // bind-security plan goes with it (stop releases everything the
        // consumer registered on the bind).
        match &running.registration {
            Registration::Tool { name } => running.handle.tool_registry.unregister(name),
            Registration::Resource { uri } => running.handle.resource_registry.unregister(uri),
        }
        running.handle.security.unregister_plan(&running.route_id);

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

    fn set_security_context(&mut self, ctx: SecurityContext) {
        self.security_ctx = Some(ctx);
    }
}

/// The default plan for a route that declared no route-level security
/// (ADR-0061 Rule 4): Public, no providers, no credential sources. The
/// per-bind exposure gate still sees it — a bare route on a non-loopback
/// bind needs operator acknowledgement exactly like a declared-Public one.
fn default_public_plan() -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Public,
        provider_ref: None,
        transport: TransportId::Mcp,
        credential_sources: vec![],
        audience_binding: None,
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
                            let mut exchange = Exchange::new(msg);
                            // Kernel-minted carrier (Task 2.6): the adapter
                            // authenticated this invocation before dispatch;
                            // the typed principal rides the Exchange so the
                            // pipeline's policy layer (and Task 2.9's
                            // dispatch check) sees a verified identity.
                            if let Some(principal) = &invocation.principal {
                                install_carrier(&mut exchange, principal);
                            }
                            ctx.send_and_wait(exchange).await
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
                            // `SecurityPolicy` (see the tool bridge); the
                            // adapter-minted carrier rides the Exchange the
                            // same way (Task 2.6).
                            let mut msg = Message::new(Body::Text(read.uri.clone()));
                            for (name, value) in &read.headers {
                                msg.set_header(
                                    name.clone(),
                                    serde_json::Value::String(value.clone()),
                                );
                            }
                            let mut exchange = Exchange::new(msg);
                            if let Some(principal) = &read.principal {
                                install_carrier(&mut exchange, principal);
                            }
                            ctx.send_and_wait(exchange).await
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
