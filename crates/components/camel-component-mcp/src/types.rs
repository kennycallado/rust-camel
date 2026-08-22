//! Component-local MCP boundary types (Camel-shaped, adapter-agnostic).
//!
//! These are the types that cross the adapter boundary (ADR-0020): the
//! dispatch layer (Tasks 2.6/2.7) receives a route's answer through the
//! oneshot `reply` channel, and the consumer bridge (Task 2.4) sends it back.

use std::collections::HashMap;

use tokio::sync::oneshot;

/// Inbound HTTP request headers (name → value), lowercased names.
///
/// Carried from the wire into the dispatch payload so the route-level
/// `SecurityPolicy` evaluates credentials against the Exchange exactly as
/// camel-http routes do (the `AuthorizationHeader` source reads
/// `Authorization`). The names are lowercased (hyper normalization).
pub type McpRequestHeaders = HashMap<String, String>;

/// A tool-call invocation dispatched to a tool route.
///
/// Not `Clone`/`Sync` — the `oneshot::Sender` reply channel is neither.
pub struct McpToolInvocation {
    /// Tool name (route key).
    pub name: String,
    /// Tool arguments as a JSON value (validated against the input schema).
    pub arguments: serde_json::Value,
    /// Inbound HTTP request headers, set as Exchange input headers by the
    /// bridge before the pipeline (and its `SecurityPolicy`) runs.
    pub headers: McpRequestHeaders,
    /// Kernel-minted principal (Task 2.6): the adapter authenticates per
    /// invocation (`kernel_authenticate`) BEFORE dispatch; the bridge
    /// installs it as the Exchange's typed carrier via `install_carrier`
    /// so the route pipeline (and Task 2.9's dispatch check) sees a
    /// verified identity. `None` for Public plans and unregistered routes
    /// (pass-through, no extraction).
    pub principal: Option<camel_auth::AuthenticatedPrincipal>,
    /// One-shot reply channel carrying the route's [`McpToolResult`].
    pub reply: oneshot::Sender<McpToolResult>,
}

/// Manual `Debug` — the kernel-minted principal is a sealed type without
/// `Debug` (its contents never render); the presence bit is enough.
impl std::fmt::Debug for McpToolInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpToolInvocation")
            .field("name", &self.name)
            .field("arguments", &self.arguments)
            .field("headers", &self.headers)
            .field(
                "principal",
                &self.principal.as_ref().map(|_| "<authenticated>"),
            )
            .field("reply", &"<oneshot>")
            .finish()
    }
}

/// The result of a tool invocation, returned by the tool route.
#[derive(Debug, Clone)]
pub struct McpToolResult {
    /// Tool output content as a JSON value.
    pub content: serde_json::Value,
    /// Structured failure signal: true when the tool route failed. Set by the
    /// bridge at the genuine failure sites only — never sniffed from
    /// `content` (a successful `{"error": null}` must stay success).
    pub is_error: bool,
}

/// A resource read dispatched to a resource route.
///
/// Not `Clone`/`Sync` — the `oneshot::Sender` reply channel is neither.
pub struct McpResourceRead {
    /// The declared MCP resource URI to read.
    pub uri: String,
    /// Inbound HTTP request headers, set as Exchange input headers by the
    /// bridge before the pipeline (and its `SecurityPolicy`) runs.
    pub headers: McpRequestHeaders,
    /// Kernel-minted principal (Task 2.6) — see
    /// [`McpToolInvocation::principal`].
    pub principal: Option<camel_auth::AuthenticatedPrincipal>,
    /// One-shot reply channel carrying the route's [`McpResource`].
    pub reply: oneshot::Sender<McpResource>,
}

/// Manual `Debug` — see [`McpToolInvocation`].
impl std::fmt::Debug for McpResourceRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpResourceRead")
            .field("uri", &self.uri)
            .field("headers", &self.headers)
            .field(
                "principal",
                &self.principal.as_ref().map(|_| "<authenticated>"),
            )
            .field("reply", &"<oneshot>")
            .finish()
    }
}

/// A resource returned by a resource route.
#[derive(Debug, Clone)]
pub struct McpResource {
    /// The resource URI this content belongs to.
    pub uri: String,
    /// Raw resource bytes.
    pub content: Vec<u8>,
    /// MIME type of the resource content.
    pub mime_type: String,
}
