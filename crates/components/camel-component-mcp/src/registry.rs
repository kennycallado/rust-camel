//! Process-global registry mapping a bind address to its single shared MCP
//! Streamable-HTTP listener (spec: MCP server shared listener registry).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::net::TcpListener;
use tokio::sync::OnceCell;

use crate::config::McpServerConfig;
use crate::error::McpError;
use crate::types::{McpResourceRead, McpToolInvocation};

/// Process-global registry mapping a bind string to the one shared listener
/// for that bind. Mirrors `ServerRegistry` in `camel-component-http`
/// (ADR-0060): the first consumer on a bind spawns the listener; later
/// consumers reuse the handle.
pub struct McpServerRegistry {
    inner: Mutex<HashMap<String, Arc<BindSlot>>>,
}

/// One bind's registry slot: a spawn counter that persists across dead-server
/// eviction plus the exactly-once-initialized handle cell.
struct BindSlot {
    spawn_count: Arc<AtomicUsize>,
    cell: OnceCell<Arc<McpListenerHandle>>,
}

impl BindSlot {
    fn with_counter(carried: Option<Arc<AtomicUsize>>) -> Self {
        Self {
            spawn_count: carried.unwrap_or_else(|| Arc::new(AtomicUsize::new(0))),
            cell: OnceCell::new(),
        }
    }
}

impl McpServerRegistry {
    /// Returns the process-global singleton.
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<McpServerRegistry> = OnceLock::new();
        INSTANCE.get_or_init(|| McpServerRegistry {
            inner: Mutex::new(HashMap::new()),
        })
    }

    /// Returns the shared listener handle for `bind`, spawning it on first use.
    ///
    /// A consumer whose config conflicts with an already-spawned bind (TLS
    /// mode, or tool/resource caps) is rejected with `McpError::Endpoint`.
    pub async fn get_or_spawn(
        &self,
        bind: &str,
        cfg: &McpServerConfig,
    ) -> Result<Arc<McpListenerHandle>, McpError> {
        let slot = {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| McpError::Endpoint("McpServerRegistry lock poisoned".to_string()))?;
            // Evict a dead server so a fresh one can spawn (matches camel-http
            // ServerRegistry dead-server eviction). The serve-loop JoinHandle's
            // `is_finished()` is a reliable proxy for the listener being gone
            // (crashed or aborted). The spawn counter is carried forward so a
            // respawn continues the count instead of resetting it.
            let mut carried: Option<Arc<AtomicUsize>> = None;
            if let Some(slot) = guard.get(bind)
                && let Some(handle) = slot.cell.get()
                && handle.monitor_task.is_finished()
            {
                carried = Some(slot.spawn_count.clone());
                guard.remove(bind);
            }
            guard
                .entry(bind.to_string())
                .or_insert_with(|| Arc::new(BindSlot::with_counter(carried)))
                .clone()
        };

        if let Some(existing) = slot.cell.get() {
            Self::check_conflict(bind, existing, cfg)?;
            return Ok(existing.clone());
        }

        let handle = slot
            .cell
            .get_or_try_init(|| {
                Self::spawn(bind.to_string(), cfg.clone(), slot.spawn_count.clone())
            })
            .await?;

        // Re-check after init: a concurrent first-spawner with a different
        // config may have won the race, leaving this caller with a mismatched
        // handle.
        Self::check_conflict(bind, handle, cfg)?;
        Ok(handle.clone())
    }

    /// Reject `cfg` when it conflicts with an already-spawned listener.
    fn check_conflict(
        bind: &str,
        existing: &McpListenerHandle,
        cfg: &McpServerConfig,
    ) -> Result<(), McpError> {
        if existing.cfg.tls != cfg.tls {
            return Err(McpError::Endpoint(format!(
                "conflicting bind '{bind}': tls config differs from the existing shared listener"
            )));
        }
        if existing.cfg.max_tools != cfg.max_tools {
            return Err(McpError::Endpoint(format!(
                "conflicting bind '{bind}': max_tools differs from the existing shared listener"
            )));
        }
        if existing.cfg.max_resources != cfg.max_resources {
            return Err(McpError::Endpoint(format!(
                "conflicting bind '{bind}': max_resources differs from the existing shared listener"
            )));
        }
        if existing.cfg.allowed_hosts != cfg.allowed_hosts {
            return Err(McpError::Endpoint(format!(
                "conflicting bind '{bind}': allowed_hosts differs from the existing shared listener"
            )));
        }
        Ok(())
    }

    /// Bind the address, mount the rmcp server adapter service, and spawn
    /// the serve loop. Process-lifetime: the listener is only torn down when
    /// the serve loop dies (then evicted and respawned by `get_or_spawn`).
    async fn spawn(
        bind: String,
        cfg: McpServerConfig,
        spawn_count: Arc<AtomicUsize>,
    ) -> Result<Arc<McpListenerHandle>, McpError> {
        let listener = TcpListener::bind(&bind)
            .await
            .map_err(|e| McpError::Endpoint(format!("failed to bind {bind}: {e}")))?;
        let local_addr = listener.local_addr().map_err(|e| {
            McpError::Endpoint(format!("failed to read local address for {bind}: {e}"))
        })?;

        // The registries are `Arc`ed because both the handle (route
        // registration/unregistration) and the rmcp server adapter
        // (`server/discover`, dispatch) share one registry per listener.
        let tool_registry = Arc::new(McpToolRegistry::new(cfg.max_tools));
        let resource_registry = Arc::new(McpResourceRegistry::new(cfg.max_resources));
        let app = crate::adapter::server::mcp_router(
            tool_registry.clone(),
            resource_registry.clone(),
            &bind,
            local_addr,
            cfg.allowed_hosts.clone(),
        );
        // The serve loop IS the monitored task (no separate wrapper): storing
        // its JoinHandle directly gives `get_or_spawn` a dead-server signal via
        // `is_finished()` and gives tests a kill seam via `abort()`.
        // `into_make_service_with_connect_info` feeds the rejection warn
        // layer's peer field (spec: rejection warn names the peer).
        let monitor_task = tokio::spawn(async move {
            if let Err(e) = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            {
                tracing::warn!(error = %e, "MCP server listener exited");
            }
        });

        spawn_count.fetch_add(1, Ordering::SeqCst);

        Ok(Arc::new(McpListenerHandle {
            local_addr,
            tool_registry,
            resource_registry,
            cfg,
            spawn_count,
            monitor_task,
        }))
    }
}

/// Shared handle for one MCP server listener.
pub struct McpListenerHandle {
    /// The actual bound address (ephemeral port resolved from a `:0` bind).
    pub local_addr: SocketAddr,
    /// Per-listener tool registry (name → route sender + schema + readiness).
    /// `Arc`-shared with the rmcp server adapter mounted on this listener.
    pub tool_registry: Arc<McpToolRegistry>,
    /// Per-listener resource registry (URI → route sender + readiness).
    /// `Arc`-shared with the rmcp server adapter mounted on this listener.
    pub resource_registry: Arc<McpResourceRegistry>,
    /// The server config this listener was spawned with (conflict detection).
    pub cfg: McpServerConfig,
    /// Spawn counter — total times this bind has been spawned across
    /// dead-server respawns (shared with the registry slot so eviction doesn't
    /// reset it).
    pub spawn_count: Arc<AtomicUsize>,
    /// JoinHandle for the spawned serve loop. `is_finished()` is the
    /// dead-server eviction signal in `get_or_spawn`, and `abort()` is the kill
    /// seam used by tests. No separate monitor wrapper: the serve loop is the
    /// monitored task directly, so aborting it stops the listener.
    pub monitor_task: tokio::task::JoinHandle<()>,
}

/// Manual `Debug` — `cfg.tls` may embed certificate/key material, so it is
/// redacted (camel-http credential-redaction pattern).
impl std::fmt::Debug for McpListenerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpListenerHandle")
            .field("local_addr", &self.local_addr)
            .field("spawn_count", &self.spawn_count.load(Ordering::SeqCst))
            .field("tool_registry", &self.tool_registry)
            .field("resource_registry", &self.resource_registry)
            .field("tls", &self.cfg.tls.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Per-listener tool registry: tool name → route sender, input schema, and
/// readiness flag.
///
/// Registration is bounded by `max`; the (N+1)th distinct name is rejected
/// with [`McpError::CapExceeded`] rather than silently truncated. Lookups
/// never await: the interior `std::sync::Mutex` is held only for the map
/// access and recovered on poison (see [`McpServerRegistry`]).
#[derive(Debug)]
pub struct McpToolRegistry {
    max: usize,
    entries: Mutex<HashMap<String, ToolEntry>>,
}

/// One registered tool route.
#[derive(Debug)]
pub struct ToolEntry {
    /// Sender that delivers an [`McpToolInvocation`] to the tool route.
    pub sender: tokio::sync::mpsc::Sender<McpToolInvocation>,
    /// Declared JSON Schema for the tool's arguments.
    pub input_schema: serde_json::Value,
    /// Readiness flag — tools not yet ready are hidden from `tools/list`.
    pub ready: AtomicBool,
}

/// Snapshot clone for `resolve`: the `AtomicBool` is copied by value (a fresh
/// flag at the entry's current state), while the sender (an `mpsc` handle)
/// and schema are cheap clones.
impl Clone for ToolEntry {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            input_schema: self.input_schema.clone(),
            ready: AtomicBool::new(self.ready.load(Ordering::SeqCst)),
        }
    }
}

impl McpToolRegistry {
    /// Creates an empty tool registry with capacity `max`.
    pub fn new(max: usize) -> Self {
        Self {
            max,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Registers `name` → (`sender`, `input_schema`).
    ///
    /// A duplicate `name` is rejected with an [`McpError::Endpoint`] — closing
    /// the check-then-register race in the consumer, where two concurrent
    /// same-name starts would otherwise silently replace the first
    /// registration (stranding the first route's channel). The (N+1)th
    /// distinct name is rejected with [`McpError::CapExceeded`].
    pub fn register(
        &self,
        name: String,
        sender: tokio::sync::mpsc::Sender<McpToolInvocation>,
        input_schema: serde_json::Value,
    ) -> Result<(), McpError> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if entries.contains_key(&name) {
            return Err(McpError::Endpoint(format!(
                "tool '{name}' is already registered"
            )));
        }
        if entries.len() >= self.max {
            return Err(McpError::CapExceeded {
                kind: "tools".to_string(),
                max: self.max,
            });
        }
        entries.insert(
            name,
            ToolEntry {
                sender,
                input_schema,
                ready: AtomicBool::new(false),
            },
        );
        Ok(())
    }

    /// Marks `name` ready so it appears in `list_ready`. This does NOT gate
    /// `resolve`, which snapshots any registered entry; call-time gating
    /// reads the snapshot's `ready` flag (dispatch, Tasks 2.6/2.7). A no-op
    /// when `name` is not registered.
    pub fn mark_ready(&self, name: &str) {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = entries.get(name) {
            entry.ready.store(true, Ordering::SeqCst);
        }
    }

    /// Removes `name` from the registry. A no-op when `name` is not
    /// registered.
    pub fn unregister(&self, name: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(name);
    }

    /// Ready tools as `(name, input_schema)` pairs; not-ready tools are
    /// hidden. Order is unspecified.
    pub fn list_ready(&self) -> Vec<(String, serde_json::Value)> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries
            .iter()
            .filter(|(_, entry)| entry.ready.load(Ordering::SeqCst))
            .map(|(name, entry)| (name.clone(), entry.input_schema.clone()))
            .collect()
    }

    /// A snapshot of the entry for `name` (cloned schema + cloned sender), or
    /// `None` when `name` is unknown or unregistered. The dispatch layer maps
    /// `None` to a clean MCP method error; no dead channel is ever awaited.
    pub fn resolve(&self, name: &str) -> Option<ToolEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .cloned()
    }
}

/// Per-listener resource registry: resource URI → route sender and readiness
/// flag.
///
/// Registration is bounded by `max`; the (N+1)th distinct URI is rejected
/// with [`McpError::CapExceeded`]. Lookups never await (see
/// [`McpToolRegistry`]).
#[derive(Debug)]
pub struct McpResourceRegistry {
    max: usize,
    entries: Mutex<HashMap<String, ResourceEntry>>,
}

/// One registered resource route.
#[derive(Debug)]
pub struct ResourceEntry {
    /// Sender that delivers an [`McpResourceRead`] to the resource route.
    pub sender: tokio::sync::mpsc::Sender<McpResourceRead>,
    /// Readiness flag — resources not yet ready are hidden from
    /// `resources/list`.
    pub ready: AtomicBool,
}

/// Snapshot clone for `resolve`: the `AtomicBool` is copied by value (a fresh
/// flag at the entry's current state), while the sender (an `mpsc` handle) is
/// a cheap clone.
impl Clone for ResourceEntry {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            ready: AtomicBool::new(self.ready.load(Ordering::SeqCst)),
        }
    }
}

impl McpResourceRegistry {
    /// Creates an empty resource registry with capacity `max`.
    pub fn new(max: usize) -> Self {
        Self {
            max,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Registers `uri` → `sender`.
    ///
    /// A duplicate `uri` is rejected with an [`McpError::Endpoint`] — closing
    /// the check-then-register race in the consumer. The (N+1)th distinct URI
    /// is rejected with [`McpError::CapExceeded`].
    pub fn register(
        &self,
        uri: String,
        sender: tokio::sync::mpsc::Sender<McpResourceRead>,
    ) -> Result<(), McpError> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if entries.contains_key(&uri) {
            return Err(McpError::Endpoint(format!(
                "resource '{uri}' is already registered"
            )));
        }
        if entries.len() >= self.max {
            return Err(McpError::CapExceeded {
                kind: "resources".to_string(),
                max: self.max,
            });
        }
        entries.insert(
            uri,
            ResourceEntry {
                sender,
                ready: AtomicBool::new(false),
            },
        );
        Ok(())
    }

    /// Marks `uri` ready so it appears in `list_ready`. This does NOT gate
    /// `resolve`, which snapshots any registered entry; call-time gating
    /// reads the snapshot's `ready` flag (dispatch, Tasks 2.6/2.7). A no-op
    /// when `uri` is not registered.
    pub fn mark_ready(&self, uri: &str) {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = entries.get(uri) {
            entry.ready.store(true, Ordering::SeqCst);
        }
    }

    /// Removes `uri` from the registry. A no-op when `uri` is not registered.
    pub fn unregister(&self, uri: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(uri);
    }

    /// Ready resource URIs; not-ready resources are hidden. Order is
    /// unspecified.
    pub fn list_ready(&self) -> Vec<String> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries
            .iter()
            .filter(|(_, entry)| entry.ready.load(Ordering::SeqCst))
            .map(|(uri, _)| uri.clone())
            .collect()
    }

    /// A snapshot of the entry for `uri` (cloned sender), or `None` when
    /// `uri` is unknown or unregistered. The dispatch layer maps `None` to a
    /// clean MCP method error; no dead channel is ever awaited.
    pub fn resolve(&self, uri: &str) -> Option<ResourceEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(uri)
            .cloned()
    }
}
