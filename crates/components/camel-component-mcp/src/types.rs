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
#[derive(Debug)]
pub struct McpToolInvocation {
    /// Tool name (route key).
    pub name: String,
    /// Tool arguments as a JSON value (validated against the input schema).
    pub arguments: serde_json::Value,
    /// Inbound HTTP request headers, set as Exchange input headers by the
    /// bridge before the pipeline (and its `SecurityPolicy`) runs.
    pub headers: McpRequestHeaders,
    /// One-shot reply channel carrying the route's [`McpToolResult`].
    pub reply: oneshot::Sender<McpToolResult>,
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
#[derive(Debug)]
pub struct McpResourceRead {
    /// The declared MCP resource URI to read.
    pub uri: String,
    /// Inbound HTTP request headers, set as Exchange input headers by the
    /// bridge before the pipeline (and its `SecurityPolicy`) runs.
    pub headers: McpRequestHeaders,
    /// One-shot reply channel carrying the route's [`McpResource`].
    pub reply: oneshot::Sender<McpResource>,
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
