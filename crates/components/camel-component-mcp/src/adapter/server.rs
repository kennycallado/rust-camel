//! rmcp server adapter (Consumer role): protocol baseline + discover + tool
//! and resource dispatch.
//!
//! [`McpServerAdapter`] implements rmcp's `ServerHandler` for one shared
//! listener: it advertises protocol `2026-07-28` as the sole supported
//! version and answers `server/discover` with the listener's identity and
//! tool/resource capabilities (spec: server advertises only 2026-07-28 via
//! discover). `tools/list` and `tools/call` dispatch through the tool
//! registry: ready tools only are listed, arguments are validated against
//! the registered input schema before any route sees them, and a validated
//! invocation travels to the tool route over its registry sender with a
//! one-shot reply. `resources/list` and `resources/read` dispatch the same
//! way through the resource registry; `prompts/list` and
//! `resources/subscribe` are declined as unsupported (spec: v1 protocol
//! surface — Prompts is deferred, subscriptions are legacy-only).
//!
//! # Version-rejection seam (documented deviation from design.md)
//!
//! Pre-`2026-07-28` rejection is rmcp 3.1.2's inline guard: the blanket
//! `Service<RoleServer> for H` impl validates each request's `_meta`
//! protocol version against `supported_protocol_versions()` BEFORE any
//! handler method runs, answering JSON-RPC `-32022` with
//! `data: {requested, supported: ["2026-07-28"]}` (single-channel: the
//! JSON-RPC error is the whole reply; no extra HTTP 400 of our own). That
//! path exposes no handler hook, so the one-`warn!`-per-rejection contract
//! (spec: pre-2026-07-28 request is rejected) is surfaced at the nearest
//! observable point: [`warn_protocol_rejections`], an axum layer that
//! inspects JSON replies for `-32022` and emits exactly one `warn!` naming
//! the peer (axum `ConnectInfo`) and the rejected version (`data.requested`).
//!
//! The legacy lifecycle never runs: the mount disables
//! `legacy_session_mode`, so every POST routes statelessly, no
//! `initialize`/session routing happens, and `Mcp-Session-Id` is never read
//! or required (`on_initialized` keeps its no-op default).

use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;

use axum::body::{Body, HttpBody as _};
use axum::extract::{ConnectInfo, Request};
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::ErrorData;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, DiscoverResult,
    ErrorCode, Implementation, ListPromptsResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Resource, ResourceContents, ResourcesCapability, ServerCapabilities,
    ServerInfo, SubscribeRequestParams, Tool, ToolsCapability,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::sync::{mpsc, oneshot};

use crate::registry::{McpResourceRegistry, McpToolRegistry};
use crate::types::{McpRequestHeaders, McpResource, McpResourceRead, McpToolInvocation};

/// The sole protocol version this server speaks (spec: baseline 2026-07-28).
const SUPPORTED_VERSIONS: &[ProtocolVersion] = &[ProtocolVersion::V_2026_07_28];

/// JSON-RPC code rmcp's inline guard answers unsupported versions with.
const UNSUPPORTED_PROTOCOL_VERSION_CODE: i64 = -32022;

/// Bound on one dispatch (`tools/call` or `resources/read`): sending the
/// invocation to the route and awaiting its one-shot reply. Matches the
/// repo's consumer-side await bound (`camel-mqtt`'s 10-second join timeout —
/// the request-reply bridges there use the same order of magnitude); a route
/// slower than this answers into a dropped reply channel, and the caller gets
/// a clean error instead of hanging.
const TOOL_DISPATCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on a reply body inspected by the rejection warn layer (matches the
/// test recorder's cap; JSON-RPC replies are small). A JSON reply at or above
/// this size is never a `-32022` rejection, so the warn layer passes it
/// through untouched instead of buffering it.
#[doc(hidden)]
pub const MAX_INSPECTED_BODY_BYTES: usize = 1 << 20;

/// rmcp's DNS-rebinding guard defaults: the loopback authorities every
/// Streamable-HTTP server accepts unless the operator widens the list
/// (ADR-0033: keep loopback functional, add non-loopback names explicitly).
const RMCP_DEFAULT_ALLOWED_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// rmcp `ServerHandler` for one shared listener: the handle's registries
/// plus the identity derived from the listener's config bind.
///
/// `tools/list` / `tools/call` dispatch through the tool registry;
/// `resources/list` / `resources/read` dispatch through the resource
/// registry; `prompts/list` and `resources/subscribe` are declined
/// (unsupported surface).
#[derive(Clone)]
pub struct McpServerAdapter {
    tools: Arc<McpToolRegistry>,
    resources: Arc<McpResourceRegistry>,
    identity_name: String,
}

impl rmcp::ServerHandler for McpServerAdapter {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(capabilities());
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info.server_info =
            Implementation::new(self.identity_name.clone(), env!("CARGO_PKG_VERSION"));
        info
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(SUPPORTED_VERSIONS)
    }

    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        Ok(DiscoverResult::from_server_info(
            SUPPORTED_VERSIONS.to_vec(),
            self.get_info(),
        ))
    }

    // ── Tool dispatch (Task 2.6) ──────────────────────────────────────────

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // Readiness gating: only `mark_ready`ed tools are listed, each with
        // its registered input schema (spec: not-ready tools are hidden).
        // (`Tool` is non-exhaustive, so it is built from its `Default`.)
        let tools = self
            .tools
            .list_ready()
            .into_iter()
            .map(|(name, input_schema)| {
                let mut tool = Tool::default();
                tool.name = Cow::Owned(name);
                tool.input_schema = Arc::new(input_schema_object(&input_schema));
                tool
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let name = request.name.to_string();
        // Unknown tool (never registered, or unregistered by a stopped
        // consumer): clean method error, no Exchange, no dead channel.
        let Some(entry) = self.tools.resolve(&name) else {
            return Err(tool_unavailable(&name));
        };
        // Readiness call-time gate: a not-ready tool is hidden AND
        // uncallable (spec scenario), answered like an unknown one.
        if !entry.ready.load(Ordering::SeqCst) {
            return Err(tool_unavailable(&name));
        }
        let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
        validate_arguments(&name, &entry.input_schema, &arguments)?;

        let (reply_tx, reply_rx) = oneshot::channel();
        let invocation = McpToolInvocation {
            name: name.clone(),
            arguments,
            headers: request_headers(&context),
            reply: reply_tx,
        };
        let label = format!("tool '{name}'");
        let result = dispatch_bound(
            &entry.sender,
            invocation,
            reply_rx,
            tool_unavailable(&name),
            &label,
        )
        .await?;
        // The bridge's structured flag (true only on a genuine route
        // failure) drives failure semantics: hosts see `isError` rather than
        // a successful result. The flag is never derived by sniffing the
        // content for an "error" key — a successful route reply shaped
        // `{"error": null}` or as a JSON-RPC envelope stays success.
        let blocks = vec![content_block(result.content)];
        let tool_result = if result.is_error {
            CallToolResult::error(blocks)
        } else {
            CallToolResult::success(blocks)
        };
        Ok(CallToolResponse::Complete(tool_result))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        // Readiness gating: only `mark_ready`ed resources are listed (spec:
        // not-ready resources are hidden). (`Resource` is non-exhaustive, so
        // it is built from its `new` constructor; the registry stores URIs
        // only, so the URI doubles as the programmatic name.)
        let resources = self
            .resources
            .list_ready()
            .into_iter()
            .map(|uri| Resource::new(uri.clone(), uri))
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri;
        // Unknown resource (never registered, or unregistered by a stopped
        // consumer): clean method error, no Exchange, no dead channel.
        let Some(entry) = self.resources.resolve(&uri) else {
            return Err(resource_unavailable(&uri));
        };
        // Readiness call-time gate: a not-ready resource is hidden AND
        // unreadable (spec scenario), answered like an unknown one.
        if !entry.ready.load(Ordering::SeqCst) {
            return Err(resource_unavailable(&uri));
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        let read = McpResourceRead {
            uri: uri.clone(),
            headers: request_headers(&context),
            reply: reply_tx,
        };
        let label = format!("resource '{uri}'");
        let resource = dispatch_bound(
            &entry.sender,
            read,
            reply_rx,
            resource_unavailable(&uri),
            &label,
        )
        .await?;
        Ok(ReadResourceResponse::Complete(ReadResourceResult::new(
            vec![resource_contents(resource)],
        )))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Err(ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            "prompts are not supported by this server",
            None,
        ))
    }

    // Legacy-only surface: implementing it does not enable the legacy
    // lifecycle (`legacy_session_mode` is disabled in the mount below).
    // `resources/subscribe` is declined explicitly (spec: v1 protocol
    // surface).
    async fn subscribe(
        &self,
        _request: SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        Err(ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            "resource subscriptions are not supported",
            None,
        ))
    }
}

/// Clean method-level error for a tool the listener will not dispatch:
/// unknown, stopped (unregistered), or not ready (hidden and uncallable).
fn tool_unavailable(name: &str) -> ErrorData {
    ErrorData::new(
        ErrorCode::METHOD_NOT_FOUND,
        format!("tool '{name}' is not available"),
        None,
    )
}

/// Clean method-level error for a resource the listener will not dispatch:
/// unknown, stopped (unregistered), or not ready (hidden and unreadable).
fn resource_unavailable(uri: &str) -> ErrorData {
    ErrorData::new(
        ErrorCode::METHOD_NOT_FOUND,
        format!("resource '{uri}' is not available"),
        None,
    )
}

/// Copy the inbound HTTP request headers from the request context into a
/// [`McpRequestHeaders`] map.
///
/// rmcp injects the raw [`http::request::Parts`] (headers included) into the
/// request's extensions, reachable through the handler's `RequestContext`.
/// A request whose parts are absent (e.g. a non-HTTP transport) yields an
/// empty map — the downstream `SecurityPolicy` then fails closed exactly as
/// it would for a headerless request.
fn request_headers(context: &RequestContext<RoleServer>) -> McpRequestHeaders {
    let mut headers = McpRequestHeaders::new();
    let Some(parts) = context.extensions.get::<http::request::Parts>() else {
        return headers;
    };
    for (name, value) in parts.headers.iter() {
        let Some(value) = value.to_str().ok() else {
            // Non-UTF-8 values cannot carry a credential; skipping keeps the
            // map honest (empty would fabricate a present-but-blank header).
            continue;
        };
        let key = name.as_str().to_ascii_lowercase();
        // HTTP treats repeated headers as comma-joined; preserve both so a
        // Cookie-source credential in the second header is not lost.
        headers
            .entry(key)
            .and_modify(|existing| {
                existing.push_str(", ");
                existing.push_str(value);
            })
            .or_insert_with(|| value.to_owned());
    }
    headers
}

/// One dispatch under the shared bound ([`TOOL_DISPATCH_TIMEOUT`]): send
/// `msg` to the route, await its one-shot reply, and map the three failure
/// modes to clean errors — a closed route channel (all receivers dropped) is
/// `unavailable`, a dropped reply names the route in `label`, and a route
/// slower than the bound answers into a dropped reply channel so the caller
/// gets a timeout error instead of hanging. A dead route is never awaited
/// unbounded, never panics. Callers keep only their success projection.
async fn dispatch_bound<T, R>(
    sender: &mpsc::Sender<T>,
    msg: T,
    reply_rx: oneshot::Receiver<R>,
    unavailable: ErrorData,
    label: &str,
) -> Result<R, ErrorData> {
    let outcome = tokio::time::timeout(TOOL_DISPATCH_TIMEOUT, async {
        if sender.send(msg).await.is_err() {
            return Err(unavailable);
        }
        reply_rx.await.map_err(|_| {
            ErrorData::internal_error(format!("{label} route dropped its reply"), None)
        })
    })
    .await;
    match outcome {
        Ok(Ok(reply)) => Ok(reply),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(ErrorData::internal_error(
            format!("{label} route did not reply within the dispatch timeout"),
            None,
        )),
    }
}

/// Validate `tools/call` arguments against the tool's registered input
/// schema (spec: Tool argument JSON Schema validation).
///
/// Compiles the schema per call (`jsonschema::validator_for`). Uses the same
/// crate as `camel-processor` (which caches its compiled validator at
/// construction); tool schemas are small and per-listener, so per-call
/// compilation is an accepted tradeoff here. An invalid schema itself is a
/// clean `invalid_params` error naming the tool; validation failures travel
/// in the error's `data.violations` array. No Exchange is created for a
/// rejected call.
fn validate_arguments(
    name: &str,
    input_schema: &serde_json::Value,
    arguments: &serde_json::Value,
) -> Result<(), ErrorData> {
    let validator = jsonschema::validator_for(input_schema).map_err(|error| {
        ErrorData::invalid_params(
            format!("tool '{name}' has an invalid input schema: {error}"),
            None,
        )
    })?;
    let violations: Vec<String> = validator
        .iter_errors(arguments)
        .map(|error| format!("{error} at {}", error.instance_path()))
        .collect();
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ErrorData::invalid_params(
            format!("tool '{name}' arguments failed input schema validation"),
            Some(serde_json::json!({ "violations": violations })),
        ))
    }
}

/// Project a registry schema (`serde_json::Value`) into rmcp's `JsonObject`
/// (`Arc<Map<String, Value>>`). A non-object schema cannot be expressed and
/// degrades to an empty object — the schema's invalidity surfaces at
/// `tools/call` time with a clean error instead.
fn input_schema_object(
    input_schema: &serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    input_schema.as_object().cloned().unwrap_or_default()
}

/// Convert a route's [`McpToolResult`] content into the `tools/call` result
/// content-block shape: a JSON string becomes the text verbatim, anything
/// else is serialized to its compact JSON form as the text.
fn content_block(content: serde_json::Value) -> ContentBlock {
    match content {
        serde_json::Value::String(text) => ContentBlock::text(text),
        other => ContentBlock::text(other.to_string()),
    }
}

/// Convert a route's [`McpResource`] into rmcp resource contents, carrying
/// the resource's declared MIME type through. Textual MIME types render as
/// `TextResourceContents` only when the bytes are valid UTF-8 (a lossy
/// conversion would silently corrupt a JSON/XML payload with U+FFFD at a
/// host trust boundary); non-textual MIME types — and textual ones whose
/// bytes are not valid UTF-8 — render as `BlobResourceContents`
/// (base64-encoded) so the raw bytes survive the JSON-RPC hop losslessly.
fn resource_contents(resource: McpResource) -> ResourceContents {
    let McpResource {
        uri,
        content,
        mime_type,
    } = resource;
    if !is_textual_mime(&mime_type) {
        return ResourceContents::blob(BASE64_STANDARD.encode(&content), uri)
            .with_mime_type(mime_type);
    }
    match String::from_utf8(content) {
        Ok(text) => ResourceContents::text(text, uri).with_mime_type(mime_type),
        // Invalid UTF-8 in a textual MIME: fall back to the blob branch so
        // the bytes survive intact instead of being mangled lossily.
        Err(err) => ResourceContents::blob(BASE64_STANDARD.encode(err.into_bytes()), uri)
            .with_mime_type(mime_type),
    }
}

/// True when `mime` denotes textual content (rendered as text rather than a
/// base64 blob): `text/*`, or a JSON/XML-flavoured application type. MIME
/// types are case-insensitive, so the base type is lowercased before
/// matching; any parameters (after `;`) are ignored.
fn is_textual_mime(mime: &str) -> bool {
    let base = mime.split(';').next().unwrap_or(mime).to_ascii_lowercase();
    base.starts_with("text/") || base.contains("json") || base.contains("xml")
}

/// Capabilities the listener advertises: tools + resources (spec: the server
/// hosts tool and resource consumer routes).
fn capabilities() -> ServerCapabilities {
    let mut capabilities = ServerCapabilities::default();
    capabilities.tools = Some(ToolsCapability::default());
    capabilities.resources = Some(ResourcesCapability::default());
    capabilities
}

/// Build the axum service mounted by `McpServerRegistry::get_or_spawn`:
/// the rmcp Streamable-HTTP service over [`McpServerAdapter`], stateless
/// (`NeverSessionManager`, `legacy_session_mode(false)` — no sessions, no
/// legacy `initialize` routing), plain-JSON replies, plus the rejection
/// warn layer.
pub(crate) fn mcp_router(
    tools: Arc<McpToolRegistry>,
    resources: Arc<McpResourceRegistry>,
    bind: &str,
    local_addr: SocketAddr,
    allowed_hosts: Option<Vec<String>>,
) -> axum::Router {
    let adapter = McpServerAdapter {
        tools,
        resources,
        // The servers-map name is not visible at the shared-listener layer
        // (one bind may host many named servers), so the identity comes from
        // the config field that identifies the listener: its resolved bind
        // address (the ephemeral port, not a literal `:0`).
        identity_name: format!("camel-mcp@{local_addr}"),
    };
    let service = StreamableHttpService::new(
        move || Ok(adapter.clone()),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_json_response(true)
            .with_legacy_session_mode(false)
            // rmcp's DNS-rebinding guard defaults to 127.0.0.1/localhost
            // only; add the operator's allowlist (or, by default, the bind
            // host) on top of those loopback defaults so a non-loopback bind
            // is reachable without silently dropping the guard.
            .with_allowed_hosts(resolved_allowed_hosts(allowed_hosts, bind)),
    );
    axum::Router::new()
        .route("/mcp", axum::routing::any_service(service))
        .layer(axum::middleware::from_fn(warn_protocol_rejections))
}

/// Resolve the `Host` authorities rmcp's DNS-rebinding guard accepts.
///
/// Always a superset of rmcp's loopback defaults (`localhost`, `127.0.0.1`,
/// `::1`) so loopback clients keep working. With no operator list, only the
/// bind host is added on top of the defaults (preserving the `127.0.0.x`
/// test convention); with an operator list, that list is added instead, so
/// LAN IPs / DNS names must be named explicitly (ADR-0033 explicit choice).
fn resolved_allowed_hosts(operator: Option<Vec<String>>, bind: &str) -> Vec<String> {
    let mut allowed: Vec<String> = RMCP_DEFAULT_ALLOWED_HOSTS
        .iter()
        .map(|host| (*host).to_owned())
        .collect();
    match operator {
        Some(hosts) => allowed.extend(hosts),
        None => allowed.push(bind_host(bind).to_owned()),
    }
    allowed
}

/// Axum layer: emit exactly one `warn!` per `-32022` rejection, naming the
/// peer (from `ConnectInfo`) and the rejected version (from the error's
/// `data.requested`). Non-JSON replies (SSE streams) pass through untouched;
/// the layer never alters status or body.
///
/// Replies known to exceed [`MAX_INSPECTED_BODY_BYTES`] are never buffered:
/// a `-32022` rejection is always a small JSON reply, so a large reply is
/// passed through byte-for-byte rather than truncated.
// `#[doc(hidden)]` — test-widened pub item (repo convention:
// `camel-api/src/datasource.rs`): not part of the component's public
// contract, only its protocol tests mount the layer directly.
#[doc(hidden)]
pub async fn warn_protocol_rejections(request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let response = next.run(request).await;
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if !is_json || reply_exceeds_inspect_cap(&response) {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_INSPECTED_BODY_BYTES).await {
        Ok(bytes) => bytes,
        // Never ship a truncated body: a collection failure (including a
        // size hint that lied about its upper bound) becomes a 500 rather
        // than a corrupted reply.
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to buffer MCP reply: {err}"),
            )
                .into_response();
        }
    };
    if let Some(value) = serde_json::from_slice::<serde_json::Value>(&bytes).ok()
        && is_unsupported_protocol_version(&value)
    {
        let requested = value["error"]["data"]["requested"]
            .as_str()
            .unwrap_or("unknown");
        tracing::warn!(
            "MCP request rejected: unsupported protocol version {requested} (peer {peer})"
        );
    }
    // The buffered body replaces the stream; keep `Content-Length` consistent
    // with the bytes actually shipped (absent for rmcp replies, where the
    // header is added by the transport anyway).
    parts
        .headers
        .insert(CONTENT_LENGTH, HeaderValue::from(bytes.len()));
    Response::from_parts(parts, Body::from(bytes))
}

/// True when a JSON reply is known to exceed the inspect cap — from an
/// explicit `Content-Length` header or the body's own size hint (rmcp bodies
/// are exact-sized, so the hint is authoritative). Such replies skip
/// inspection rather than being buffered.
fn reply_exceeds_inspect_cap(response: &Response) -> bool {
    let declared = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let hinted = response.body().size_hint().upper();
    declared.is_some_and(|length| length > MAX_INSPECTED_BODY_BYTES as u64)
        || hinted.is_some_and(|upper| upper > MAX_INSPECTED_BODY_BYTES as u64)
}

/// True when the JSON body is a JSON-RPC `-32022` error reply.
fn is_unsupported_protocol_version(body: &serde_json::Value) -> bool {
    body.get("error")
        .and_then(|error| error.get("code"))
        .and_then(serde_json::Value::as_i64)
        == Some(UNSUPPORTED_PROTOCOL_VERSION_CODE)
}

/// Host part of a `host:port` bind (IPv6 brackets handled); the bind itself
/// when no port separator is present.
fn bind_host(bind: &str) -> &str {
    if let Some(rest) = bind.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    bind.rsplit_once(':').map_or(bind, |(host, _)| host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::McpResource;
    use base64::prelude::BASE64_STANDARD;

    fn resource(uri: &str, content: &[u8], mime_type: &str) -> McpResource {
        McpResource {
            uri: uri.to_owned(),
            content: content.to_vec(),
            mime_type: mime_type.to_owned(),
        }
    }

    /// Extract the text payload, panicking when the contents are a blob.
    fn expect_text(contents: ResourceContents) -> String {
        match contents {
            ResourceContents::TextResourceContents { text, .. } => text,
            other => panic!("expected text resource contents, got {other:?}"),
        }
    }

    /// Extract the blob payload (base64-decoded), panicking when the contents
    /// are text.
    fn expect_blob(contents: ResourceContents) -> Vec<u8> {
        match contents {
            ResourceContents::BlobResourceContents { blob, .. } => {
                BASE64_STANDARD.decode(blob).unwrap()
            }
            other => panic!("expected blob resource contents, got {other:?}"),
        }
    }

    #[test]
    fn resource_contents_valid_utf8_text_is_text() {
        for (mime, text) in [("text/plain", "hello, mcp"), ("text/markdown", "# doc")] {
            let contents = resource_contents(resource("file:///doc.txt", text.as_bytes(), mime));
            assert_eq!(expect_text(contents), text);
        }
    }

    #[test]
    fn resource_contents_invalid_utf8_textual_falls_back_to_blob() {
        let original = b"\xff\xfe{garbage}";
        let contents =
            resource_contents(resource("file:///data.json", original, "application/json"));
        assert_eq!(expect_blob(contents), original.as_slice());
    }

    #[test]
    fn resource_contents_binary_mime_is_blob() {
        let original = b"\x89PNG\r\n\x1a\n";
        let contents = resource_contents(resource("file:///logo.png", original, "image/png"));
        assert_eq!(expect_blob(contents), original.as_slice());
    }

    #[test]
    fn resource_contents_mime_matching_is_case_insensitive() {
        for (mime, text) in [
            ("TEXT/PLAIN; charset=utf-8", "hello"),
            ("Application/JSON", r#"{"ok": true}"#),
            ("Text/Markdown; charset=utf-8", "# hi"),
        ] {
            let contents = resource_contents(resource("file:///case.txt", text.as_bytes(), mime));
            assert_eq!(expect_text(contents), text);
        }
    }
}
