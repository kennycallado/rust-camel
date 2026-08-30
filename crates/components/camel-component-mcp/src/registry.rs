//! Process-global registry mapping a bind address to its single shared MCP
//! Streamable-HTTP listener (spec: MCP server shared listener registry).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::net::TcpListener;
use tokio::sync::OnceCell;

use camel_api::security_policy::RouteSecurityPlan;
use camel_auth::ProviderRegistry;

use crate::config::McpServerConfig;
use crate::error::McpError;
use crate::types::{McpResourceRead, McpToolInvocation};

/// Process-global registry mapping a bind string to the one shared listener
/// for that bind. Mirrors `ServerRegistry` in `camel-component-http`
/// (ADR-0060): the first consumer on a bind spawns the listener; later
/// consumers reuse the handle.
pub struct McpServerRegistry {
    inner: Mutex<HashMap<String, Arc<BindSlot>>>,
    /// Operator acknowledgements for public exposure per bind address
    /// (ADR-0061), threaded from the CLI's `[binds."<addr>"]`
    /// `allow_public_exposure` map. Fail-closed default: empty.
    bind_acks: Mutex<HashMap<String, bool>>,
}

/// One bind's registry slot: a spawn counter that persists across dead-server
/// eviction plus the exactly-once-initialized handle cell.
struct BindSlot {
    spawn_count: Arc<AtomicUsize>,
    cell: OnceCell<Arc<McpListenerHandle>>,
    /// Per-bind dispatch-security book (Task 2.6): compiled route plans +
    /// provider snapshot for this bind. Created with the slot so it survives
    /// dead-server eviction exactly like the spawn counter, and shared with
    /// the rmcp adapter mounted on the slot's listener.
    security: Arc<McpBindSecurity>,
}

impl BindSlot {
    fn with_counter(carried: Option<Arc<AtomicUsize>>) -> Self {
        Self {
            spawn_count: carried.unwrap_or_else(|| Arc::new(AtomicUsize::new(0))),
            cell: OnceCell::new(),
            security: Arc::new(McpBindSecurity::new()),
        }
    }
}

/// One route's compiled plan plus the registering consumer's owner
/// liveness token (ADR-0068): the consumer holds the strong `Arc<()>`,
/// the entry holds this `Weak`. When the token no longer upgrades the
/// plan is dead — replaced by the next registration and ignored by
/// lookups and the exposure gate.
struct OwnedPlan {
    plan: RouteSecurityPlan,
    owner: Weak<()>,
}

/// Per-bind dispatch-security book (Task 2.6): the compiled
/// [`RouteSecurityPlan`] of every route registered on this bind plus the
/// [`ProviderRegistry`] snapshot the route controller threaded through the
/// consumer's `SecurityContext`.
///
/// Two readers: the consumer start path (registers the plan, then runs the
/// per-bind exposure gate over the full snapshot) and the rmcp adapter's
/// request path (per tool/resource invocation: Public plan → pass-through;
/// otherwise extract per `credential_sources` → `kernel_authenticate`).
pub struct McpBindSecurity {
    /// Plans by route id (one mcp: route serves exactly one tool or one
    /// resource; the route id is the registration key). Each entry is
    /// scoped to its registering consumer's owner token (ADR-0068).
    plans: Mutex<HashMap<String, OwnedPlan>>,
    /// Provider registry snapshot (last registration wins; routes on one
    /// bind share the process-wide registry in practice).
    providers: Mutex<Option<Arc<ProviderRegistry>>>,
}

impl McpBindSecurity {
    fn new() -> Self {
        Self {
            plans: Mutex::new(HashMap::new()),
            providers: Mutex::new(None),
        }
    }

    /// Register (or replace) `route_id`'s plan under the liveness token
    /// `owner` and, when present, refresh the provider snapshot (last
    /// registration wins).
    ///
    /// Owner-scoped (ADR-0068): a plan for `route_id` held by a live
    /// owner is KEPT — this call returns without removing or overwriting
    /// it (the newcomer fails the duplicate guard later in `start()`);
    /// a dead owner's plan is replaced.
    pub fn register_plan(
        &self,
        route_id: &str,
        plan: RouteSecurityPlan,
        providers: Option<Arc<ProviderRegistry>>,
        owner: Weak<()>,
    ) {
        let mut plans = self
            .plans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = plans.get(route_id)
            && existing.owner.upgrade().is_some()
        {
            // ADR-0068: keep-incumbent — the live owner's plan must stay
            // intact; removing or overwriting it would open an
            // unauthenticated pass-through window for the incumbent route.
            return;
        }
        plans.insert(route_id.to_string(), OwnedPlan { plan, owner });
        drop(plans);
        if let Some(providers) = providers {
            *self
                .providers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(providers);
        }
    }

    /// Unconditionally install `route_id`'s plan under `owner`, replacing
    /// any incumbent. Called by a consumer that has just WON the tool or
    /// resource entry `register` (ADR-0068: winner re-assertion — entry
    /// ownership proves route identity): in the concurrent-start
    /// interleaving the keep-incumbent [`Self::register_plan`] may have
    /// kept another live consumer's plan while this consumer won the
    /// name/URI entry; without the re-assertion the loser's
    /// owner-conditional cleanup would strip the only plan behind the
    /// winner's live entry. Overwriting an incumbent owned by `owner`
    /// itself (`Weak::ptr_eq`) is a same-content re-register.
    pub fn register_plan_takeover(
        &self,
        route_id: &str,
        plan: RouteSecurityPlan,
        providers: Option<Arc<ProviderRegistry>>,
        owner: Weak<()>,
    ) {
        let mut plans = self
            .plans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plans.insert(route_id.to_string(), OwnedPlan { plan, owner });
        drop(plans);
        if let Some(providers) = providers {
            *self
                .providers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(providers);
        }
    }

    /// Remove `route_id`'s plan (consumer stop / refused start). A no-op
    /// when no plan is registered.
    pub fn unregister_plan(&self, route_id: &str) {
        self.plans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(route_id);
    }

    /// Removes `route_id`'s plan only when the registered entry carries
    /// exactly `owner` (`Weak::ptr_eq`). Returns whether a removal
    /// happened. A late stop by a dead owner must not delete a live
    /// replacement's plan, which would drop dispatch to unauthenticated
    /// pass-through (ADR-0068).
    pub fn unregister_plan_owned(&self, route_id: &str, owner: &Weak<()>) -> bool {
        let mut plans = self
            .plans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let owned = plans
            .get(route_id)
            .is_some_and(|entry| Weak::ptr_eq(&entry.owner, owner));
        if owned {
            plans.remove(route_id);
        }
        owned
    }

    /// The plan registered for `route_id` under a live owner, or `None`
    /// (pre-2.6 direct-drive routes: no plan, pass-through; or the plan's
    /// owner died — a dead plan must not authenticate dispatch,
    /// ADR-0068). Dead-owner entries are pruned lazily on read.
    pub fn plan_for(&self, route_id: &str) -> Option<RouteSecurityPlan> {
        let mut plans = self
            .plans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plans.retain(|_, entry| entry.owner.upgrade().is_some());
        plans.get(route_id).map(|entry| entry.plan.clone())
    }

    /// All live-owner plans as `(route_id, plan)` pairs (bind-gate input;
    /// order unspecified). Dead owners are pruned first, so a dead
    /// route's plan stops influencing the bind exposure gate (ADR-0068).
    pub fn plans_snapshot(&self) -> Vec<(String, RouteSecurityPlan)> {
        let mut plans = self
            .plans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plans.retain(|_, entry| entry.owner.upgrade().is_some());
        plans
            .iter()
            .map(|(id, entry)| (id.clone(), entry.plan.clone()))
            .collect()
    }

    /// The provider registry snapshot (fail-closed: `None` until a route
    /// with providers registers).
    pub fn providers(&self) -> Option<Arc<ProviderRegistry>> {
        self.providers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl McpServerRegistry {
    /// Returns the process-global singleton.
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<McpServerRegistry> = OnceLock::new();
        INSTANCE.get_or_init(|| McpServerRegistry {
            inner: Mutex::new(HashMap::new()),
            bind_acks: Mutex::new(HashMap::new()),
        })
    }

    /// Install per-bind public-exposure acknowledgements (ADR-0061).
    ///
    /// The CLI builds this from `CamelConfig.binds`
    /// (`allow_public_exposure`) — the same map it hands the route
    /// controller — so the per-bind gate enforced at consumer start
    /// (Task 2.6) fails closed on non-loopback binds until acknowledged.
    /// Replaces the whole map (tests reset it with an empty map).
    pub fn set_bind_exposure_acks(&self, acks: HashMap<String, bool>) {
        *self
            .bind_acks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = acks;
    }

    /// Whether the operator acknowledged public exposure for `bind`
    /// (bind address string as written, e.g. `"0.0.0.0:8080"`). Absent or
    /// not set → false (fail-closed).
    pub fn acknowledged(&self, bind: &str) -> bool {
        self.bind_acks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(bind)
            .copied()
            .unwrap_or(false)
    }

    /// The per-bind dispatch-security book for `bind`, creating the slot
    /// (without spawning a listener) when absent.
    ///
    /// The consumer start path uses this to register its route's plan and
    /// run the exposure gate BEFORE any socket is bound; the same `Arc`
    /// later rides `McpListenerHandle::security` into the rmcp adapter.
    pub fn bind_security(&self, bind: &str) -> Arc<McpBindSecurity> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(bind.to_string())
            .or_insert_with(|| Arc::new(BindSlot::with_counter(None)))
            .security
            .clone()
    }

    /// Returns the shared listener handle for `bind`, spawning it on first use.
    ///
    /// A consumer whose config conflicts with an already-spawned bind (TLS
    /// mode, or tool/resource caps) is rejected with `McpError::Endpoint`.
    ///
    /// Caps are materialized to their EFFECTIVE values here (declared value
    /// or the 128 default): this is the single funnel below the consumer's
    /// TOML/DSL merge, so an undeclared cap becomes the default only AFTER
    /// any conflict check, and every stored handle carries the runtime caps
    /// its listener was actually spawned with.
    pub async fn get_or_spawn(
        &self,
        bind: &str,
        cfg: &McpServerConfig,
    ) -> Result<Arc<McpListenerHandle>, McpError> {
        let mut effective = cfg.clone();
        effective.max_tools = Some(cfg.effective_max_tools());
        effective.max_resources = Some(cfg.effective_max_resources());
        let cfg = &effective;

        let slot = {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| McpError::Endpoint("McpServerRegistry lock poisoned".to_string()))?;
            // Evict a dead server so a fresh one can spawn (matches camel-http
            // ServerRegistry dead-server eviction). The serve-loop JoinHandle's
            // `is_finished()` is a reliable proxy for the listener being gone
            // (crashed or aborted). The spawn counter is carried forward so a
            // respawn continues the count instead of resetting it; the
            // security book dies with the slot — its plans belong to routes
            // on the dead listener, which re-register on restart.
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
                Self::spawn(
                    bind.to_string(),
                    cfg.clone(),
                    slot.spawn_count.clone(),
                    slot.security.clone(),
                )
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
    ///
    /// Plain binds use `TcpListener` + `axum::serve` (fail-fast on the bind,
    /// resolved address available before the router is built). TLS binds
    /// follow the camel-ws/camel-http precedent (`axum-server`
    /// `bind_rustls`): the TCP bind happens inside the spawned serve
    /// future, so bind success is awaited via `axum_server::Handle::listening()`
    /// and the adapter identity uses the declared bind address (identical
    /// for fixed ports; `:0` is a test convenience).
    async fn spawn(
        bind: String,
        cfg: McpServerConfig,
        spawn_count: Arc<AtomicUsize>,
        security: Arc<McpBindSecurity>,
    ) -> Result<Arc<McpListenerHandle>, McpError> {
        // The registries are `Arc`ed because both the handle (route
        // registration/unregistration) and the rmcp server adapter
        // (`server/discover`, dispatch) share one registry per listener; the
        // security book rides the same shape (Task 2.6 per-invocation
        // kernel authentication).
        let tool_registry = Arc::new(McpToolRegistry::new(cfg.effective_max_tools()));
        let resource_registry = Arc::new(McpResourceRegistry::new(cfg.effective_max_resources()));

        let (local_addr, monitor_task) = if let Some(tls) = &cfg.tls {
            let rustls_config = load_tls_config(&tls.cert_path, &tls.key_path)?;
            let addr: SocketAddr = bind.parse().map_err(|_| {
                McpError::Endpoint(format!(
                    "bind '{bind}' is not an IP:port literal (hostnames are not allowed)"
                ))
            })?;
            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(rustls_config));
            let listen_handle = axum_server::Handle::new();
            let task_handle = listen_handle.clone();
            let app = crate::adapter::server::mcp_router(
                tool_registry.clone(),
                resource_registry.clone(),
                security.clone(),
                &bind,
                addr,
                cfg.allowed_hosts.clone(),
            );
            // The serve loop IS the monitored task (no separate wrapper):
            // storing its JoinHandle directly gives `get_or_spawn` a
            // dead-server signal via `is_finished()` and gives tests a kill
            // seam via `abort()`. Its terminal error is also fed to
            // `bind_err_tx` so `spawn` can put the CAUSE in the startup
            // error when the listener never comes up (a bare "did not come
            // up" hides the reason); on the Ok path the sender is dropped
            // unsent and the receiver falls back to a no-cause message.
            let (bind_err_tx, bind_err_rx) = tokio::sync::oneshot::channel::<String>();
            let monitor_task = tokio::spawn(async move {
                if let Err(e) = axum_server::bind_rustls(addr, tls_config)
                    .handle(task_handle)
                    .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                    .await
                {
                    tracing::warn!(error = %e, "MCP server listener exited");
                    let _ = bind_err_tx.send(e.to_string());
                }
            });
            // `listening()` resolves `None` when the serve loop exited
            // before binding (address in use, permissions) — surface that
            // as the spawn failure WITH the serve-loop cause, the same way
            // the plain path carries the TcpListener bind error.
            let local_addr = match listen_handle.listening().await {
                Some(local_addr) => local_addr,
                None => {
                    let cause = bind_err_rx
                        .await
                        .unwrap_or_else(|_| "the serve loop exited without an error".to_string());
                    return Err(McpError::Endpoint(format!(
                        "failed to bind {bind}: the TLS listener did not come up: {cause}"
                    )));
                }
            };
            (local_addr, monitor_task)
        } else {
            let listener = TcpListener::bind(&bind)
                .await
                .map_err(|e| McpError::Endpoint(format!("failed to bind {bind}: {e}")))?;
            let local_addr = listener.local_addr().map_err(|e| {
                McpError::Endpoint(format!("failed to read local address for {bind}: {e}"))
            })?;
            let app = crate::adapter::server::mcp_router(
                tool_registry.clone(),
                resource_registry.clone(),
                security.clone(),
                &bind,
                local_addr,
                cfg.allowed_hosts.clone(),
            );
            // Same serve-loop-as-monitored-task shape as the TLS branch.
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
            (local_addr, monitor_task)
        };

        spawn_count.fetch_add(1, Ordering::SeqCst);

        Ok(Arc::new(McpListenerHandle {
            local_addr,
            tool_registry,
            resource_registry,
            security,
            cfg,
            spawn_count,
            monitor_task,
        }))
    }
}

/// Load PEM cert/key paths into a rustls server config (camel-ws/camel-http
/// precedent). Fail-fast on unreadable or unparseable material, naming the
/// offending path and cause.
fn load_tls_config(
    cert_path: &str,
    key_path: &str,
) -> Result<tokio_rustls::rustls::ServerConfig, McpError> {
    use std::fs::File;
    use std::io::BufReader;

    let cert_file = File::open(cert_path)
        .map_err(|e| McpError::Endpoint(format!("TLS cert file '{cert_path}' error: {e}")))?;
    let key_file = File::open(key_path)
        .map_err(|e| McpError::Endpoint(format!("TLS key file '{key_path}' error: {e}")))?;

    let certs = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| McpError::Endpoint(format!("TLS cert parse error ('{cert_path}'): {e}")))?;

    let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))
        .map_err(|e| McpError::Endpoint(format!("TLS key parse error ('{key_path}'): {e}")))?
        .ok_or_else(|| {
            McpError::Endpoint(format!("TLS key file '{key_path}' carries no private key"))
        })?;

    tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| McpError::Endpoint(format!("TLS config error: {e}")))
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
    /// Per-bind dispatch-security book (Task 2.6): route plans + provider
    /// snapshot. `Arc`-shared with the rmcp server adapter (per-invocation
    /// kernel authentication) and the consumer start path (plan
    /// registration + exposure gate). Same `Arc` as the registry slot's.
    pub security: Arc<McpBindSecurity>,
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

/// Manual `Debug` — `cfg.tls` holds certificate/key PATHS (not material);
/// summarized instead of printed to keep the output stable and small.
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
    /// The serving route's id — the key into the bind's
    /// [`McpBindSecurity`] plan map (dispatch-time authentication, Task 2.6).
    pub route_id: String,
    /// Readiness flag — tools not yet ready are hidden from `tools/list`.
    pub ready: AtomicBool,
    /// Owner liveness token: the registering consumer holds the strong
    /// `Arc<()>`, the entry holds this `Weak`. When the token no longer
    /// upgrades the entry is dead and is pruned lazily (ADR-0068).
    pub owner: Weak<()>,
}

/// Snapshot clone for `resolve`: the `AtomicBool` is copied by value (a fresh
/// flag at the entry's current state), while the sender (an `mpsc` handle)
/// and schema are cheap clones.
impl Clone for ToolEntry {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            input_schema: self.input_schema.clone(),
            route_id: self.route_id.clone(),
            ready: AtomicBool::new(self.ready.load(Ordering::SeqCst)),
            // Liveness is registry-internal (ADR-0068): the dispatch snapshot
            // carries an inert token, never the registering owner.
            owner: Weak::new(),
        }
    }
}

/// Removes every entry whose owner token no longer upgrades — the strong
/// `Arc<()>` was dropped without a stop, so the entry is dead. Called by
/// listings, resolves, and the cap check so dead entries stop being
/// advertised and release their cap slots without waiting for a restart
/// (ADR-0068: lazy prune).
fn prune_dead(entries: &mut HashMap<String, ToolEntry>) {
    entries.retain(|_, entry| entry.owner.upgrade().is_some());
}

impl McpToolRegistry {
    /// Creates an empty tool registry with capacity `max`.
    pub fn new(max: usize) -> Self {
        Self {
            max,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Registers `name` → (`sender`, `input_schema`) for `route_id` under
    /// the liveness token `owner`.
    ///
    /// A duplicate `name` whose entry is still owned by a live token is
    /// rejected with an [`McpError::Endpoint`] — closing the
    /// check-then-register race in the consumer, where two concurrent
    /// same-name starts would otherwise silently replace the first
    /// registration (stranding the first route's channel). A duplicate whose
    /// owner died (the consumer dropped without a stop) is REPLACED by the
    /// newcomer and the dead registration's route id is warned
    /// (ADR-0068: replace-dead-on-conflict) — a same-name replace does not
    /// grow the map, so it bypasses the cap check. Before the cap check,
    /// every dead-owner entry under any name is pruned, so a crashed
    /// consumer's slots are reclaimed without waiting for an unrelated
    /// listing; the (N+1)th distinct live name is rejected with
    /// [`McpError::CapExceeded`].
    /// # Security note
    ///
    /// Tools registered directly through this pub API carry no
    /// `RouteSecurityPlan` and are served WITHOUT authentication (public
    /// pass-through, invisible to the per-bind exposure gate).
    /// Kernel-secured registration flows through the consumer's plan
    /// registration instead.
    pub fn register(
        &self,
        name: String,
        route_id: String,
        sender: tokio::sync::mpsc::Sender<McpToolInvocation>,
        input_schema: serde_json::Value,
        owner: Weak<()>,
    ) -> Result<(), McpError> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = entries.get(&name) {
            if existing.owner.upgrade().is_some() {
                return Err(McpError::Endpoint(format!(
                    "tool '{name}' is already registered"
                )));
            }
            // ADR-0068: replace-dead-on-conflict
            tracing::warn!(
                name = %name,
                dead_route_id = %existing.route_id,
                "replacing a dead owner's tool registration"
            );
            entries.insert(
                name,
                ToolEntry {
                    sender,
                    input_schema,
                    route_id,
                    ready: AtomicBool::new(false),
                    owner,
                },
            );
            return Ok(());
        }
        // Reclaim the cap slots of dead owners under every name before the
        // cap check (ADR-0068: lazy prune). The replace branch above does not
        // grow the map, so only the fresh-name path enforces the cap.
        prune_dead(&mut entries);
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
                route_id,
                ready: AtomicBool::new(false),
                owner,
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

    /// Removes `name` only when the registered entry carries exactly `owner`
    /// (`Weak::ptr_eq`). Returns whether a removal happened. A late stop by
    /// a dead owner must not delete a replacement's entry (ADR-0068).
    pub fn unregister_owned(&self, name: &str, owner: &Weak<()>) -> bool {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let owned = entries
            .get(name)
            .is_some_and(|entry| Weak::ptr_eq(&entry.owner, owner));
        if owned {
            entries.remove(name);
        }
        owned
    }

    /// Whether `name` maps to an entry whose owner token is still alive. The
    /// consumer's duplicate fast-path consults this instead of `resolve`, so
    /// a dead owner's lingering entry does not veto a legal takeover
    /// (ADR-0068).
    pub fn name_taken_by_live_owner(&self, name: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .is_some_and(|entry| entry.owner.upgrade().is_some())
    }

    /// Ready tools as `(name, input_schema)` pairs; not-ready tools are
    /// hidden and dead-owner entries are pruned before listing. Order is
    /// unspecified.
    pub fn list_ready(&self) -> Vec<(String, serde_json::Value)> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune_dead(&mut entries);
        entries
            .iter()
            .filter(|(_, entry)| entry.ready.load(Ordering::SeqCst))
            .map(|(name, entry)| (name.clone(), entry.input_schema.clone()))
            .collect()
    }

    /// A snapshot of the entry for `name` (cloned schema + cloned sender), or
    /// `None` when `name` is unknown, unregistered, or its owner died (the
    /// dead entry is pruned). The dispatch layer maps `None` to a clean MCP
    /// method error; no dead channel is ever awaited.
    pub fn resolve(&self, name: &str) -> Option<ToolEntry> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune_dead(&mut entries);
        entries.get(name).cloned()
    }
}

/// Per-listener resource registry: resource URI → route sender, readiness
/// flag, and owner liveness token.
///
/// Registration is bounded by `max`; the (N+1)th distinct URI is rejected
/// with [`McpError::CapExceeded`]. A dead owner's entry is replaced on
/// conflict and pruned lazily (ADR-0068, mirroring [`McpToolRegistry`]).
/// Lookups never await (see [`McpToolRegistry`]).
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
    /// The serving route's id — the key into the bind's
    /// [`McpBindSecurity`] plan map (dispatch-time authentication, Task 2.6).
    pub route_id: String,
    /// Readiness flag — resources not yet ready are hidden from
    /// `resources/list`.
    pub ready: AtomicBool,
    /// Owner liveness token: the registering consumer holds the strong
    /// `Arc<()>`, the entry holds this `Weak`. When the token no longer
    /// upgrades the entry is dead and is pruned lazily (ADR-0068).
    pub owner: Weak<()>,
}

/// Snapshot clone for `resolve`: the `AtomicBool` is copied by value (a fresh
/// flag at the entry's current state), while the sender (an `mpsc` handle) is
/// a cheap clone.
impl Clone for ResourceEntry {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            route_id: self.route_id.clone(),
            ready: AtomicBool::new(self.ready.load(Ordering::SeqCst)),
            // Liveness is registry-internal (ADR-0068): the dispatch snapshot
            // carries an inert token, never the registering owner.
            owner: Weak::new(),
        }
    }
}

/// Removes every entry whose owner token no longer upgrades — the strong
/// `Arc<()>` was dropped without a stop, so the entry is dead (ADR-0068:
/// lazy prune). Resource-side twin of the tool registry's `prune_dead`.
fn prune_dead_resources(entries: &mut HashMap<String, ResourceEntry>) {
    entries.retain(|_, entry| entry.owner.upgrade().is_some());
}

impl McpResourceRegistry {
    /// Creates an empty resource registry with capacity `max`.
    pub fn new(max: usize) -> Self {
        Self {
            max,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Registers `uri` → `sender` for `route_id` under the liveness token
    /// `owner`.
    ///
    /// A duplicate `uri` whose entry is still owned by a live token is
    /// rejected with an [`McpError::Endpoint`] — closing the
    /// check-then-register race in the consumer, where two concurrent
    /// same-URI starts would otherwise silently replace the first
    /// registration (stranding the first route's channel). A duplicate whose
    /// owner died (the consumer dropped without a stop) is REPLACED by the
    /// newcomer and the dead registration's route id is warned
    /// (ADR-0068: replace-dead-on-conflict) — a same-URI replace does not
    /// grow the map, so it bypasses the cap check. Before the cap check,
    /// every dead-owner entry under any URI is pruned, so a crashed
    /// consumer's slots are reclaimed without waiting for an unrelated
    /// listing; the (N+1)th distinct live URI is rejected with
    /// [`McpError::CapExceeded`].
    /// # Security note
    ///
    /// Resources registered directly through this pub API carry no
    /// `RouteSecurityPlan` and are served WITHOUT authentication (public
    /// pass-through, invisible to the per-bind exposure gate).
    /// Kernel-secured registration flows through the consumer's plan
    /// registration instead.
    pub fn register(
        &self,
        uri: String,
        route_id: String,
        sender: tokio::sync::mpsc::Sender<McpResourceRead>,
        owner: Weak<()>,
    ) -> Result<(), McpError> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = entries.get(&uri) {
            if existing.owner.upgrade().is_some() {
                return Err(McpError::Endpoint(format!(
                    "resource '{uri}' is already registered"
                )));
            }
            // ADR-0068: replace-dead-on-conflict
            tracing::warn!(
                uri = %uri,
                dead_route_id = %existing.route_id,
                "replacing a dead owner's resource registration"
            );
            entries.insert(
                uri,
                ResourceEntry {
                    sender,
                    route_id,
                    ready: AtomicBool::new(false),
                    owner,
                },
            );
            return Ok(());
        }
        // Reclaim the cap slots of dead owners under every URI before the
        // cap check (ADR-0068: lazy prune). The replace branch above does not
        // grow the map, so only the fresh-URI path enforces the cap.
        prune_dead_resources(&mut entries);
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
                route_id,
                ready: AtomicBool::new(false),
                owner,
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

    /// Removes `uri` only when the registered entry carries exactly `owner`
    /// (`Weak::ptr_eq`). Returns whether a removal happened. A late stop by
    /// a dead owner must not delete a replacement's entry (ADR-0068).
    pub fn unregister_owned(&self, uri: &str, owner: &Weak<()>) -> bool {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let owned = entries
            .get(uri)
            .is_some_and(|entry| Weak::ptr_eq(&entry.owner, owner));
        if owned {
            entries.remove(uri);
        }
        owned
    }

    /// Whether `uri` maps to an entry whose owner token is still alive. The
    /// consumer's duplicate fast-path consults this instead of `resolve`, so
    /// a dead owner's lingering entry does not veto a legal takeover
    /// (ADR-0068).
    pub fn uri_taken_by_live_owner(&self, uri: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(uri)
            .is_some_and(|entry| entry.owner.upgrade().is_some())
    }

    /// Ready resource URIs; not-ready resources are hidden and dead-owner
    /// entries are pruned before listing. Order is unspecified.
    pub fn list_ready(&self) -> Vec<String> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune_dead_resources(&mut entries);
        entries
            .iter()
            .filter(|(_, entry)| entry.ready.load(Ordering::SeqCst))
            .map(|(uri, _)| uri.clone())
            .collect()
    }

    /// A snapshot of the entry for `uri` (cloned sender), or `None` when
    /// `uri` is unknown, unregistered, or its owner died (the dead entry is
    /// pruned). The dispatch layer maps `None` to a clean MCP method error;
    /// no dead channel is ever awaited.
    pub fn resolve(&self, uri: &str) -> Option<ResourceEntry> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune_dead_resources(&mut entries);
        entries.get(uri).cloned()
    }
}
