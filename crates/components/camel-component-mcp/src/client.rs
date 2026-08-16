//! Client-role MCP client trait + server map.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::error::McpError;
use crate::types::{McpResource, McpToolResult};

/// Client-role (Producer) MCP client — the twin of llm's `LlmProvider`
/// (ADR-0020). Implementations live behind the adapter (`src/adapter/`), where
/// the SDK is confined; this trait seam carries only Camel-shaped types.
#[async_trait]
pub trait McpClient: Send + Sync {
    /// Invoke one MCP tool on the remote server.
    async fn call_tool(
        &self,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, McpError>;

    /// Read one MCP resource by URI from the remote server.
    async fn read_resource(&self, uri: &str) -> Result<McpResource, McpError>;
}

/// Live map of client-role remote server name to its connected client — the
/// client-role twin of llm's `ProviderMap` (ADR-0020).
///
/// Seeded empty at component construction (no network at construction); each
/// producer's lifecycle `start()` connects its remote and registers the client
/// keyed by server name (fail-fast at start — ADR-0060).
///
/// # Refcounting
///
/// Multiple producer routes may target the same remote server name
/// (`mcp:call?server=crm&tool=a` and `...&tool=b`). Every `start()` on a name
/// increments a per-name refcount and every `shutdown()` decrements it; the
/// entry is removed only when the count reaches zero. This keeps a still-live
/// sibling route's producer connected when its neighbour shuts down, instead
/// of a blanket `remove` stranding it (its `poll_ready` would stay `Pending`
/// forever with no reconnect).
///
/// **First-wins insert:** if a client for `name` is already registered, a
/// subsequent `start()` increments the refcount and drops its own freshly
/// connected client rather than replacing the live one. Both clients point at
/// the same remote config, so which one is kept is functionally equivalent;
/// first-wins simply avoids replacing a healthy connection mid-flight.
pub struct McpServerMap {
    // std RwLock, not tokio's: every critical section below is await-free
    // (pure map ops), so the sync `read()` in `try_contains` blocks briefly
    // instead of failing under write-lock contention and leaving `poll_ready`
    // Pending with no registered waker (permanent route stall).
    inner: RwLock<Inner>,
}

/// Interior state: connected clients plus the count of live producers per name.
struct Inner {
    clients: HashMap<String, Arc<dyn McpClient>>,
    refs: HashMap<String, usize>,
}

impl McpServerMap {
    /// An empty map — no remotes connected yet.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                clients: HashMap::new(),
                refs: HashMap::new(),
            }),
        }
    }

    /// Register a started producer's client under `name`.
    ///
    /// First-wins: when a client is already live for `name`, the refcount is
    /// incremented and `client` is dropped (see the struct doc comment).
    pub async fn register(&self, name: String, client: Arc<dyn McpClient>) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match inner.refs.get_mut(&name) {
            Some(count) => *count += 1,
            None => {
                inner.refs.insert(name.clone(), 1);
                inner.clients.insert(name, client);
            }
        }
    }

    /// Deregister a stopped producer from `name`.
    ///
    /// Decrements the refcount; the entry is removed only when no live
    /// producer remains on the name.
    pub async fn deregister(&self, name: &str) {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = inner.refs.get_mut(name) {
            *count -= 1;
            if *count == 0 {
                inner.refs.remove(name);
                inner.clients.remove(name);
            }
        }
    }

    /// The connected client for `name`, if any live producer has registered.
    pub async fn get(&self, name: &str) -> Option<Arc<dyn McpClient>> {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clients
            .get(name)
            .cloned()
    }

    /// Whether a client for `name` is currently registered, without awaiting
    /// (for `poll_ready`).
    pub fn try_contains(&self, name: &str) -> bool {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clients
            .contains_key(name)
    }
}

impl Default for McpServerMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared handle to the live client map.
pub type McpServerMapHandle = Arc<McpServerMap>;
