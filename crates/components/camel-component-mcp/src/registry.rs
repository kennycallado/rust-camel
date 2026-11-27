//! Process-global registry mapping a bind address to its single shared MCP
//! Streamable-HTTP listener (spec: MCP server shared listener registry).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::sync::OnceCell;

use camel_api::security_policy::{AccessMode, RouteSecurityPlan};
use camel_auth::ProviderRegistry;

use crate::config::McpServerConfig;
use crate::error::McpError;
use crate::skills::McpSkillCatalog;
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
    /// the rmcp adapter mounted on the slot's listener. Carried across
    /// eviction: a consumer registers its plan BEFORE `get_or_spawn`, so the
    /// respawned listener must mount the same book or it would come up with
    /// no plan and `protected_seen` false (fail-open for the skills catalog
    /// and an unauthenticated tool route). Dead owners are pruned lazily by
    /// `plan_for`/`plans_snapshot`, so no stale live plan survives.
    security: Arc<McpBindSecurity>,
    /// Single-flight skills load for THIS slot generation (holistic review
    /// round 4): the flight lives with the slot, so eviction drops it and a
    /// respawn walks the tree again instead of replaying a stale catalog.
    /// A cancelled waiter leaves the flight in place; a retry reuses it.
    skills_flight: Mutex<Option<Arc<SkillsFlight>>>,
    /// The settled catalog for THIS slot generation, keyed by the root it was
    /// loaded from. Also dropped by eviction, so a respawn observes on-disk
    /// changes.
    skills_catalog: Mutex<Option<(PathBuf, Arc<McpSkillCatalog>)>>,
}

impl BindSlot {
    fn with_counter(
        carried_count: Option<Arc<AtomicUsize>>,
        carried_security: Option<Arc<McpBindSecurity>>,
    ) -> Self {
        Self {
            spawn_count: carried_count.unwrap_or_else(|| Arc::new(AtomicUsize::new(0))),
            cell: OnceCell::new(),
            security: carried_security.unwrap_or_else(|| Arc::new(McpBindSecurity::new())),
            skills_flight: Mutex::new(None),
            skills_catalog: Mutex::new(None),
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
    /// Sticky record of whether this bind has committed to protection
    /// (Task 2.2). Only [`Self::commit_protection`] sets it — a plan
    /// registration alone never latches, because a start that fails before a
    /// listener exists must not hide the catalog of a later Public listener.
    /// Once set it is never cleared, so a stopped protected route cannot make
    /// the skills catalog public again.
    protected_seen: AtomicBool,
}

impl McpBindSecurity {
    /// Creates an empty security book (no plans, no provider snapshot).
    pub(crate) fn new() -> Self {
        Self {
            plans: Mutex::new(HashMap::new()),
            providers: Mutex::new(None),
            protected_seen: AtomicBool::new(false),
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
        // Commit the winning plan atomically with the takeover (holistic
        // review round 4): the lock is still held, so a concurrent loser
        // cannot observe the inserted plan without the commitment that came
        // with it.
        self.commit_protection_locked(&plans);
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

    /// Commits this bind to protection when a live non-`Public` plan is
    /// present.
    ///
    /// Caller must hold the `plans` lock, so a takeover can commit the
    /// winning plan atomically with its insertion (holistic review round 4).
    fn commit_protection_locked(&self, plans: &HashMap<String, OwnedPlan>) {
        if plans.values().any(|entry| {
            entry.owner.upgrade().is_some() && !matches!(entry.plan.access_mode, AccessMode::Public)
        }) {
            self.protected_seen.store(true, Ordering::SeqCst);
        }
    }

    /// Commits this bind to protection when a live non-`Public` plan is
    /// registered on it: the skills catalog must stay authentication-gated for
    /// the rest of the bind's lifetime.
    ///
    /// Called by [`McpServerRegistry::spawn`] BEFORE the serve task is spawned
    /// (so no request can be accepted before the latch is set) and by
    /// [`Self::register_plan_takeover`] for the winning plan on a shared
    /// listener. Plain [`Self::register_plan`] never latches: a start that
    /// fails before any listener exists leaves the latch false, so a later
    /// successful Public listener serves the catalog.
    ///
    /// Sticky by construction — there is no clear path. A live listener's
    /// latch deliberately outlives the protected routes that committed it.
    /// Dead owners do not commit: a plan whose consumer is gone protects
    /// nothing.
    pub fn commit_protection(&self) {
        let plans = self
            .plans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.commit_protection_locked(&plans);
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

    /// Whether this bind has ever held a non-`Public` plan (Task 2.2).
    ///
    /// Once true it stays true for the listener slot's lifetime, so the
    /// skills catalog stays authentication-gated even after every protected
    /// route stops.
    pub fn protected_seen(&self) -> bool {
        self.protected_seen.load(Ordering::SeqCst)
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
            .or_insert_with(|| Arc::new(BindSlot::with_counter(None, None)))
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
            // ServerRegistry dead-server eviction). rc-nyxti: the signal is
            // the serve-future drop guard (`serve_alive`, camel-ws rc-onm5b
            // pattern) — it flips on EVERY death mode (normal exit, serve
            // error, abort, and runtime-drop cancellation, polled or not),
            // covering the runtime-dropped-task class whose JoinHandle never
            // reports completion; the old `monitor_task.is_finished()` signal
            // missed exactly that. The spawn counter AND the security book are
            // carried forward: a respawn continues the count instead of
            // resetting it, and it mounts the SAME book the consumer
            // registered its plan on before `get_or_spawn` — a fresh book
            // would come up with no plan and `protected_seen` false, opening
            // the skills catalog and the route to unauthenticated callers.
            let mut carried: Option<Arc<AtomicUsize>> = None;
            let mut carried_security: Option<Arc<McpBindSecurity>> = None;
            if let Some(slot) = guard.get(bind)
                && let Some(handle) = slot.cell.get()
                && !handle.serve_alive.load(Ordering::Acquire)
            {
                tracing::debug!(bind, "evicting dead MCP server entry (serve task gone)");
                carried = Some(slot.spawn_count.clone());
                carried_security = Some(slot.security.clone());
                guard.remove(bind);
            }
            guard
                .entry(bind.to_string())
                .or_insert_with(|| Arc::new(BindSlot::with_counter(carried, carried_security)))
                .clone()
        };

        if let Some(existing) = slot.cell.get() {
            Self::check_conflict(bind, existing, cfg)?;
            return Ok(existing.clone());
        }

        // Load the catalog BEFORE the slot is initialized and before any
        // socket is bound: a bad root must fail the start without installing
        // a handle, and the listener must never come up with a background
        // load in flight. The flight and the settled catalog live in this
        // slot generation, so concurrent first-starts share one load, a
        // cancelled waiter leaves the flight for its retry, and eviction
        // drops both so a respawn re-reads the tree (holistic review
        // round 4).
        let skills = match cfg.skills_root.as_deref() {
            Some(root) => Some(Self::load_skills_for_slot(&slot, PathBuf::from(root)).await?),
            None => None,
        };

        let handle = slot
            .cell
            .get_or_try_init(|| {
                Self::spawn(
                    bind.to_string(),
                    cfg.clone(),
                    slot.spawn_count.clone(),
                    slot.security.clone(),
                    skills.clone(),
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
        if existing.cfg.skills_root != cfg.skills_root {
            return Err(McpError::Endpoint(format!(
                "conflicting bind '{bind}': skills_root differs from the existing shared listener"
            )));
        }
        Ok(())
    }

    /// Bind the address, mount the rmcp server adapter service, and spawn
    /// the serve loop. Process-lifetime: the listener is only torn down when
    /// the serve loop dies (then evicted and respawned by `get_or_spawn`).
    ///
    /// Both transports pre-bind and pre-construct the listener, then commit
    /// the protection latch, and only then spawn the serve task (holistic
    /// review round 4): no request can be accepted before the latch is set.
    /// Plain binds use `TcpListener` + `axum::serve`; TLS binds use
    /// `axum_server::from_tcp_rustls` over a synchronously pre-bound
    /// `std::net::TcpListener`.
    async fn spawn(
        bind: String,
        cfg: McpServerConfig,
        spawn_count: Arc<AtomicUsize>,
        security: Arc<McpBindSecurity>,
        skills: Option<Arc<McpSkillCatalog>>,
    ) -> Result<Arc<McpListenerHandle>, McpError> {
        // The catalog was already loaded (slot-owned single-flight) by
        // `get_or_spawn`; `spawn` only mounts it. That keeps the load — and
        // its failure — strictly before the bind, while never walking the
        // tree from a Tokio worker or holding the slot's init future open
        // across a blocking read.

        // The registries are `Arc`ed because both the handle (route
        // registration/unregistration) and the rmcp server adapter
        // (`server/discover`, dispatch) share one registry per listener; the
        // security book rides the same shape (Task 2.6 per-invocation
        // kernel authentication).
        let tool_registry = Arc::new(McpToolRegistry::new(cfg.effective_max_tools()));
        let resource_registry = Arc::new(McpResourceRegistry::new(cfg.effective_max_resources()));

        // rc-nyxti: accept-loop liveness, flipped `false` by a drop guard
        // captured into the serve future below (camel-ws rc-onm5b pattern)
        // — the direct eviction signal read by `get_or_spawn`.
        let serve_alive: Arc<AtomicBool> = Arc::new(AtomicBool::new(true));

        let (local_addr, monitor_task) = if let Some(tls) = &cfg.tls {
            let rustls_config = load_tls_config(&tls.cert_path, &tls.key_path)?;
            let addr: SocketAddr = bind.parse().map_err(|_| {
                McpError::Endpoint(format!(
                    "bind '{bind}' is not an IP:port literal (hostnames are not allowed)"
                ))
            })?;
            // Pre-bind the TCP socket synchronously: the bind error belongs
            // to spawn, and the resolved address must exist before the router
            // (host validation) and the protection commit.
            let listener = std::net::TcpListener::bind(addr)
                .map_err(|e| McpError::Endpoint(format!("failed to bind {bind}: {e}")))?;
            listener.set_nonblocking(true).map_err(|e| {
                McpError::Endpoint(format!("failed to set nonblocking on {bind}: {e}"))
            })?;
            let local_addr = listener.local_addr().map_err(|e| {
                McpError::Endpoint(format!("failed to read local address for {bind}: {e}"))
            })?;
            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(rustls_config));
            let app = crate::adapter::server::mcp_router(
                tool_registry.clone(),
                resource_registry.clone(),
                security.clone(),
                skills.clone(),
                &bind,
                local_addr,
                cfg.allowed_hosts.clone(),
            );
            let server = axum_server::from_tcp_rustls(listener, tls_config).map_err(|e| {
                McpError::Endpoint(format!("failed to construct TLS listener for {bind}: {e}"))
            })?;
            // Task 2.2: commit before the serve task exists, so the first
            // accepted request already sees the latch.
            security.commit_protection();
            // The serve loop IS the monitored task (no separate wrapper):
            // the JoinHandle stays the test kill seam via `abort()`, while
            // rc-nyxti made the drop guard inside this future (not the
            // handle's completion) the `get_or_spawn` eviction signal.
            let alive_guard = ServeAliveGuard(Arc::clone(&serve_alive));
            let monitor_task = tokio::spawn(async move {
                // rc-nyxti: dropped with this future on every death mode —
                // flips `serve_alive` for lazy eviction.
                let _alive = alive_guard;
                if let Err(e) = server
                    .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                    .await
                {
                    tracing::warn!(error = %e, "MCP server listener exited");
                }
            });
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
                skills.clone(),
                &bind,
                local_addr,
                cfg.allowed_hosts.clone(),
            );
            // Task 2.2: commit before the serve task can accept a request.
            security.commit_protection();
            // Same serve-loop-as-monitored-task shape as the TLS branch.
            // `into_make_service_with_connect_info` feeds the rejection warn
            // layer's peer field (spec: rejection warn names the peer).
            let alive_guard = ServeAliveGuard(Arc::clone(&serve_alive));
            let monitor_task = tokio::spawn(async move {
                // rc-nyxti: dropped with this future on every death mode —
                // flips `serve_alive` for lazy eviction.
                let _alive = alive_guard;
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
            skills,
            cfg,
            spawn_count,
            monitor_task,
            serve_alive,
        }))
    }
}

/// Number of catalog loader threads started in this process (tests only).
///
/// The single-flight guarantee is "one walk per root", so the tests assert a
/// delta of exactly one across a concurrent/cancelled load pair.
#[cfg(test)]
static SKILLS_LOAD_STARTS: AtomicUsize = AtomicUsize::new(0);

/// A failed catalog load in cloneable form: every waiter on one shared load
/// observes the same typed outcome, and the two variants the loader produces
/// survive the round trip back into [`McpError`].
#[derive(Clone)]
enum SkillsLoadError {
    /// The root (or a path under it) resolved outside the canonical root.
    PathEscape { path: String },
    /// The tree could not be walked, or a skill was unusable.
    Load {
        skill: Option<String>,
        reason: String,
    },
}

impl From<McpError> for SkillsLoadError {
    fn from(error: McpError) -> Self {
        match error {
            McpError::SkillPathEscape { path } => Self::PathEscape { path },
            McpError::SkillLoad { skill, reason } => Self::Load { skill, reason },
            // `McpSkillCatalog::load` produces only the two variants above;
            // fold any other into a load failure rather than lose it.
            other => Self::Load {
                skill: None,
                reason: other.to_string(),
            },
        }
    }
}

impl From<SkillsLoadError> for McpError {
    fn from(error: SkillsLoadError) -> Self {
        match error {
            SkillsLoadError::PathEscape { path } => McpError::SkillPathEscape { path },
            SkillsLoadError::Load { skill, reason } => McpError::SkillLoad { skill, reason },
        }
    }
}

/// One slot-owned, generation-scoped single-flight skills load: the settled
/// outcome (if any), the wake-up for its waiters, and the exactly-one-loader
/// flag.
///
/// The flight lives in the [`BindSlot`], not in a process-global map: eviction
/// drops it with the generation, so a respawn walks the tree again instead of
/// replaying a stale catalog (holistic review round 4).
struct SkillsFlight {
    root: PathBuf,
    result: Mutex<Option<Result<Arc<McpSkillCatalog>, SkillsLoadError>>>,
    notify: Notify,
    started: AtomicBool,
}

impl SkillsFlight {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            result: Mutex::new(None),
            notify: Notify::new(),
            started: AtomicBool::new(false),
        }
    }

    /// Waits for the one loader this flight is allowed to start.
    ///
    /// The walk runs on a plain OS thread so it outlives a cancelled waiter:
    /// the future awaiting it can be dropped and the flight stays in the
    /// slot, so a retry awaits the same load instead of starting a second
    /// walk.
    async fn wait(self: &Arc<Self>) -> Result<Arc<McpSkillCatalog>, McpError> {
        loop {
            // Register interest (pin + enable) BEFORE reading the result:
            // `notify_waiters` only wakes futures already registered, so a
            // notification landing between the read and the await would
            // otherwise be lost and park this waiter forever.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let settled = self
                .result
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(settled) = settled {
                return settled.map_err(Into::into);
            }

            if !self.started.swap(true, Ordering::SeqCst) {
                let flight = Arc::clone(self);
                std::thread::spawn(move || {
                    #[cfg(test)]
                    SKILLS_LOAD_STARTS.fetch_add(1, Ordering::SeqCst);
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        McpSkillCatalog::load(&flight.root)
                    }))
                    .unwrap_or_else(|_| {
                        Err(McpError::SkillLoad {
                            skill: None,
                            reason: "skills loader panicked".into(),
                        })
                    })
                    .map(Arc::new)
                    .map_err(SkillsLoadError::from);
                    *flight
                        .result
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(outcome);
                    flight.notify.notify_waiters();
                });
            }

            notified.await;
        }
    }
}

impl McpServerRegistry {
    /// Loads (or joins) the catalog for this slot generation, single-flight
    /// and safe under cancellation.
    ///
    /// Both the flight and the settled catalog live in the [`BindSlot`], so
    /// eviction (a new listener generation) drops them and a respawn re-reads
    /// the tree. A cancelled waiter leaves the flight in the slot and its
    /// retry reuses it; a failed flight retires only itself — waiters already
    /// on it observe the typed error, and new callers start a fresh flight.
    async fn load_skills_for_slot(
        slot: &BindSlot,
        root: PathBuf,
    ) -> Result<Arc<McpSkillCatalog>, McpError> {
        let cached = slot
            .skills_catalog
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some((cached_root, catalog)) = cached
            && cached_root == root
        {
            return Ok(catalog);
        }

        let flight = {
            let mut slot_flight = slot
                .skills_flight
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match slot_flight.as_ref() {
                Some(existing) if existing.root == root => Arc::clone(existing),
                _ => {
                    let flight = Arc::new(SkillsFlight::new(root.clone()));
                    *slot_flight = Some(Arc::clone(&flight));
                    flight
                }
            }
        };

        // Cancellation-safe: the flight stays in the slot if this future is
        // dropped here, so a retry joins the same load.
        let result = flight.wait().await;

        match result {
            Ok(catalog) => {
                *slot
                    .skills_catalog
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some((root, Arc::clone(&catalog)));
                let mut slot_flight = slot
                    .skills_flight
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if slot_flight
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &flight))
                {
                    *slot_flight = None;
                }
                Ok(catalog)
            }
            Err(error) => {
                let mut slot_flight = slot
                    .skills_flight
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if slot_flight
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &flight))
                {
                    *slot_flight = None;
                }
                Err(error)
            }
        }
    }
}

/// Drop guard that flips `serve_alive` to `false` when the serve future is
/// gone (rc-nyxti, camel-ws rc-onm5b pattern). Captured into the serve
/// future by value — so it is part of the future's state from construction,
/// before the first poll — which makes runtime-drop cancellation (the task
/// dropped without ever being polled, or parked mid-accept at runtime
/// shutdown) flip the flag just like a normal exit, serve error, or abort
/// does.
struct ServeAliveGuard(Arc<AtomicBool>);

impl Drop for ServeAliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
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
    /// The listener's skills catalog, loaded once at spawn from the
    /// configured `skills_root` (spec: MCP Skills extension is an opt-in
    /// server capability). `None` when the config declares no root — the
    /// extension stays off. `Arc`-shared with the rmcp server adapter.
    pub skills: Option<Arc<McpSkillCatalog>>,
    /// The server config this listener was spawned with (conflict detection).
    pub cfg: McpServerConfig,
    /// Spawn counter — total times this bind has been spawned across
    /// dead-server respawns (shared with the registry slot so eviction doesn't
    /// reset it).
    pub spawn_count: Arc<AtomicUsize>,
    /// JoinHandle for the spawned serve loop. rc-nyxti: no longer the
    /// eviction signal (a runtime-dropped task's JoinHandle never reports
    /// completion — eviction reads `serve_alive` instead); retained as the
    /// kill seam used by tests (`abort()` stops the listener directly).
    pub monitor_task: tokio::task::JoinHandle<()>,
    /// Accept-loop liveness (rc-nyxti): flipped to `false` by a drop guard
    /// captured into the serve future, so it fires on EVERY death mode —
    /// normal exit, serve error, abort, and runtime-drop cancellation,
    /// polled or not. `get_or_spawn` reads this as the dead-server
    /// eviction signal; it is strictly more reliable than the JoinHandle's
    /// completion (a runtime-dropped task never reports completion, and a
    /// joiner arriving after serve-future death but before completion would
    /// rejoin the corpse).
    pub serve_alive: Arc<AtomicBool>,
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
            .field("skills", &self.skills.as_ref().map(|catalog| catalog.len()))
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
    /// The compiled [`RouteSecurityPlan`] captured at registration — the
    /// catalog classification input (`None` = public pass-through
    /// registration, listed to every caller).
    pub plan: Option<RouteSecurityPlan>,
    /// Readiness flag — tools not yet ready are hidden from `tools/list`.
    pub ready: AtomicBool,
    /// Owner liveness token: the registering consumer holds the strong
    /// `Arc<()>`, the entry holds this `Weak`. When the token no longer
    /// upgrades the entry is dead and is pruned lazily (ADR-0068).
    pub owner: Weak<()>,
}

/// Snapshot clone for `resolve`: the `AtomicBool` is copied by value (a fresh
/// flag at the entry's current state), while the sender (an `mpsc` handle),
/// schema, and compiled plan are cheap clones.
impl Clone for ToolEntry {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            input_schema: self.input_schema.clone(),
            route_id: self.route_id.clone(),
            plan: self.plan.clone(),
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

/// One ready tool as catalog discovery sees it: name, input schema, serving
/// route id, and the compiled plan captured at registration.
///
/// Crate-private snapshot shape (Task 2.1): the adapter's catalog filter
/// classifies each entry by `route_id`/`plan`, while `list_ready` keeps its
/// public `(name, input_schema)` projection.
pub(crate) struct ReadyTool {
    /// Registered tool name.
    pub name: String,
    /// Declared JSON Schema for the tool's arguments.
    pub input_schema: serde_json::Value,
    /// The serving route's id (key into the bind security book).
    pub route_id: String,
    /// The compiled plan captured at registration; `None` = public
    /// pass-through registration (listed to every caller).
    pub plan: Option<RouteSecurityPlan>,
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
    /// the liveness token `owner`, carrying no plan.
    ///
    /// Delegates to [`Self::register_with_plan`] with `plan = None`.
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
    /// An entry registered through this API carries no plan, so catalog
    /// discovery lists it to every caller. Dispatch still follows the bind
    /// security book, so a direct-drive route with a registered dispatch
    /// plan stays authenticated at call time. Use
    /// [`Self::register_with_plan`] to hand the entry its compiled plan.
    pub fn register(
        &self,
        name: String,
        route_id: String,
        sender: tokio::sync::mpsc::Sender<McpToolInvocation>,
        input_schema: serde_json::Value,
        owner: Weak<()>,
    ) -> Result<(), McpError> {
        self.register_with_plan(name, route_id, sender, input_schema, owner, None)
    }

    /// Registers `name` → (`sender`, `input_schema`) for `route_id` under
    /// the liveness token `owner`, storing `plan` — the compiled
    /// [`RouteSecurityPlan`] captured at registration.
    ///
    /// `plan` is the catalog classification contract: `None` marks a public
    /// pass-through registration that every caller sees in `tools/list`,
    /// while `Some(plan)` lets catalog discovery classify the entry by its
    /// access mode. Dispatch is unchanged — it always follows the bind
    /// security book ([`McpBindSecurity`]), not this copy.
    ///
    /// Duplicate rejection, dead-owner replacement, lazy pruning, and the
    /// `max` cap are identical to [`Self::register`].
    pub fn register_with_plan(
        &self,
        name: String,
        route_id: String,
        sender: tokio::sync::mpsc::Sender<McpToolInvocation>,
        input_schema: serde_json::Value,
        owner: Weak<()>,
        plan: Option<RouteSecurityPlan>,
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
                    plan,
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
                plan,
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

    /// Crate-private ready snapshot: every ready tool with its serving route
    /// id and compiled plan. Dead-owner entries are pruned before listing and
    /// not-ready tools are hidden. Order is unspecified.
    pub(crate) fn ready_tools(&self) -> Vec<ReadyTool> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune_dead(&mut entries);
        entries
            .iter()
            .filter(|(_, entry)| entry.ready.load(Ordering::SeqCst))
            .map(|(name, entry)| ReadyTool {
                name: name.clone(),
                input_schema: entry.input_schema.clone(),
                route_id: entry.route_id.clone(),
                plan: entry.plan.clone(),
            })
            .collect()
    }

    /// Ready tools as `(name, input_schema)` pairs; not-ready tools are
    /// hidden and dead-owner entries are pruned before listing. Order is
    /// unspecified.
    pub fn list_ready(&self) -> Vec<(String, serde_json::Value)> {
        self.ready_tools()
            .into_iter()
            .map(|tool| (tool.name, tool.input_schema))
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
    /// The compiled [`RouteSecurityPlan`] captured at registration — the
    /// catalog classification input (`None` = public pass-through
    /// registration, listed to every caller).
    pub plan: Option<RouteSecurityPlan>,
    /// Readiness flag — resources not yet ready are hidden from
    /// `resources/list`.
    pub ready: AtomicBool,
    /// Owner liveness token: the registering consumer holds the strong
    /// `Arc<()>`, the entry holds this `Weak`. When the token no longer
    /// upgrades the entry is dead and is pruned lazily (ADR-0068).
    pub owner: Weak<()>,
}

/// Snapshot clone for `resolve`: the `AtomicBool` is copied by value (a fresh
/// flag at the entry's current state), while the sender (an `mpsc` handle)
/// and compiled plan are cheap clones.
impl Clone for ResourceEntry {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            route_id: self.route_id.clone(),
            plan: self.plan.clone(),
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

/// One ready resource as catalog discovery sees it: URI, serving route id,
/// and the compiled plan captured at registration.
///
/// Crate-private snapshot shape (Task 2.1): the adapter's catalog filter
/// classifies each entry by `route_id`/`plan`, while `list_ready` keeps its
/// public URI-only projection.
pub(crate) struct ReadyResource {
    /// Registered resource URI.
    pub uri: String,
    /// The serving route's id (key into the bind security book).
    pub route_id: String,
    /// The compiled plan captured at registration; `None` = public
    /// pass-through registration (listed to every caller).
    pub plan: Option<RouteSecurityPlan>,
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
    /// `owner`, carrying no plan.
    ///
    /// Delegates to [`Self::register_with_plan`] with `plan = None`.
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
    /// An entry registered through this API carries no plan, so catalog
    /// discovery lists it to every caller. Dispatch still follows the bind
    /// security book, so a direct-drive route with a registered dispatch
    /// plan stays authenticated at call time. Use
    /// [`Self::register_with_plan`] to hand the entry its compiled plan.
    pub fn register(
        &self,
        uri: String,
        route_id: String,
        sender: tokio::sync::mpsc::Sender<McpResourceRead>,
        owner: Weak<()>,
    ) -> Result<(), McpError> {
        self.register_with_plan(uri, route_id, sender, owner, None)
    }

    /// Registers `uri` → `sender` for `route_id` under the liveness token
    /// `owner`, storing `plan` — the compiled [`RouteSecurityPlan`] captured
    /// at registration.
    ///
    /// `plan` is the catalog classification contract: `None` marks a public
    /// pass-through registration that every caller sees in
    /// `resources/list`, while `Some(plan)` lets catalog discovery classify
    /// the entry by its access mode. Dispatch is unchanged — it always
    /// follows the bind security book ([`McpBindSecurity`]), not this copy.
    ///
    /// Duplicate rejection, dead-owner replacement, lazy pruning, and the
    /// `max` cap are identical to [`Self::register`].
    pub fn register_with_plan(
        &self,
        uri: String,
        route_id: String,
        sender: tokio::sync::mpsc::Sender<McpResourceRead>,
        owner: Weak<()>,
        plan: Option<RouteSecurityPlan>,
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
                uri = %camel_api::redact::redact_url_fail_closed(&uri),
                dead_route_id = %existing.route_id,
                "replacing a dead owner's resource registration"
            );
            entries.insert(
                uri,
                ResourceEntry {
                    sender,
                    route_id,
                    plan,
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
                plan,
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

    /// Crate-private ready snapshot: every ready resource URI with its
    /// serving route id and compiled plan. Dead-owner entries are pruned
    /// before listing and not-ready resources are hidden. Order is
    /// unspecified.
    pub(crate) fn ready_resources(&self) -> Vec<ReadyResource> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune_dead_resources(&mut entries);
        entries
            .iter()
            .filter(|(_, entry)| entry.ready.load(Ordering::SeqCst))
            .map(|(uri, entry)| ReadyResource {
                uri: uri.clone(),
                route_id: entry.route_id.clone(),
                plan: entry.plan.clone(),
            })
            .collect()
    }

    /// Ready resource URIs; not-ready resources are hidden and dead-owner
    /// entries are pruned before listing. Order is unspecified.
    pub fn list_ready(&self) -> Vec<String> {
        self.ready_resources()
            .into_iter()
            .map(|resource| resource.uri)
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    use camel_api::security_policy::{AccessMode, TransportId};

    /// A Public plan — the pass-through shape catalog discovery lists to
    /// every caller. Used to prove the plan captured at registration rides
    /// the ready snapshot.
    fn public_plan() -> RouteSecurityPlan {
        RouteSecurityPlan {
            access_mode: AccessMode::Public,
            provider_ref: None,
            transport: TransportId::Mcp,
            credential_sources: vec![],
            audience_binding: None,
        }
    }

    /// A non-`Public` plan: its (live) registration is what commits a bind to
    /// protection.
    fn protected_plan() -> RouteSecurityPlan {
        RouteSecurityPlan {
            access_mode: AccessMode::Authenticated,
            provider_ref: None,
            transport: TransportId::Mcp,
            credential_sources: vec![],
            audience_binding: None,
        }
    }

    #[test]
    fn commit_protection_ignores_dead_owners() {
        let security = McpBindSecurity::new();
        let owner = Arc::new(());
        security.register_plan("r1", protected_plan(), None, Arc::downgrade(&owner));
        drop(owner);

        security.commit_protection();

        assert!(
            !security.protected_seen(),
            "a dead owner's plan protects nothing and must not commit the latch"
        );
    }

    #[test]
    fn commit_protection_latches_on_a_live_protected_plan() {
        let security = McpBindSecurity::new();
        let owner = Arc::new(());
        security.register_plan("r1", protected_plan(), None, Arc::downgrade(&owner));
        assert!(
            !security.protected_seen(),
            "registration alone must not latch (a failed start cannot hide a later listener)"
        );

        security.commit_protection();
        assert!(security.protected_seen(), "a live protected plan commits");

        security.unregister_plan("r1");
        security.commit_protection();
        assert!(
            security.protected_seen(),
            "the committed latch is sticky and never cleared"
        );
    }

    #[test]
    fn commit_protection_ignores_public_plans() {
        let security = McpBindSecurity::new();
        let owner = Arc::new(());
        security.register_plan("r1", public_plan(), None, Arc::downgrade(&owner));

        security.commit_protection();

        assert!(
            !security.protected_seen(),
            "a Public plan must never commit protection"
        );
    }

    /// Holistic review round 4: the winning takeover must commit the latch
    /// atomically with its insertion — no separate `commit_protection` call
    /// may be required for a shared listener's later protected route.
    #[test]
    fn takeover_commits_protection_atomically() {
        let security = McpBindSecurity::new();
        let public_owner = Arc::new(());
        let protected_owner = Arc::new(());
        security.register_plan("r", public_plan(), None, Arc::downgrade(&public_owner));
        assert!(
            !security.protected_seen(),
            "registering a Public plan must not latch"
        );

        security.register_plan_takeover(
            "r",
            protected_plan(),
            None,
            Arc::downgrade(&protected_owner),
        );

        assert!(
            security.protected_seen(),
            "the winning non-Public takeover must commit the latch while holding the plans lock"
        );
    }

    /// Unique temp directory for the loader tests (removed by the test body's
    /// `Drop`-free cleanup; the crate has no `tempfile` dev-dep).
    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(tag: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "camel-mcp-registry-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("code-review")).expect("create skills root");
            std::fs::write(
                path.join("code-review/SKILL.md"),
                "---\nname: code-review\ndescription: Review code.\n---\n",
            )
            .expect("write SKILL.md");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Serializes the loader tests: [`SKILLS_LOAD_STARTS`] is process-global,
    /// so a concurrent loader test in this binary would pollute the delta.
    static LOADER_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn skills_load_is_single_flight_across_cancellation() {
        let _serial = LOADER_TEST_LOCK.lock().await; // allow-test-wait: peer loader test releases it with no blocking work
        let root = TestRoot::new("single-flight");
        let slot = BindSlot::with_counter(None, None);
        let before = SKILLS_LOAD_STARTS.load(Ordering::SeqCst);

        // A zero deadline drops the load future at (or right after) its first
        // poll: whether or not it managed to start the thread, the flight
        // stays in the slot, so the retry joins it instead of walking the
        // tree a second time.
        let _ = tokio::time::timeout(
            std::time::Duration::ZERO,
            McpServerRegistry::load_skills_for_slot(&slot, root.path().to_path_buf()),
        )
        .await;

        let catalog = McpServerRegistry::load_skills_for_slot(&slot, root.path().to_path_buf())
            .await
            .expect("the retry must observe the shared load");
        assert_eq!(catalog.len(), 1, "the catalog must hold the one skill");
        assert_eq!(
            SKILLS_LOAD_STARTS.load(Ordering::SeqCst) - before,
            1,
            "exactly one loader thread may start for one root across a cancelled waiter and its retry"
        );
    }

    #[tokio::test]
    async fn failed_skills_load_is_not_cached() {
        let _serial = LOADER_TEST_LOCK.lock().await; // allow-test-wait: peer loader test releases it with no blocking work
        let missing =
            std::env::temp_dir().join(format!("camel-mcp-registry-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        let slot = BindSlot::with_counter(None, None);

        let error = McpServerRegistry::load_skills_for_slot(&slot, missing.clone())
            .await
            .expect_err("a missing root must fail the load");
        assert!(
            matches!(error, McpError::SkillLoad { .. }),
            "expected SkillLoad, got {error:?}"
        );

        // The failed flight retired itself: the same root is retried with a
        // fresh flight instead of replaying the cached error (which would
        // also block a retry after a fix).
        let error = McpServerRegistry::load_skills_for_slot(&slot, missing)
            .await
            .expect_err("the retry must attempt a fresh load");
        assert!(
            matches!(error, McpError::SkillLoad { .. }),
            "expected SkillLoad on retry, got {error:?}"
        );
    }

    #[test]
    fn ready_snapshot_carries_route_id_and_plan() {
        let registry = McpToolRegistry::new(8);
        let owner = Arc::new(());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        registry
            .register_with_plan(
                "t".to_string(),
                "route-t".to_string(),
                tx,
                serde_json::json!({}),
                Arc::downgrade(&owner),
                Some(public_plan()),
            )
            .expect("registration must succeed");
        registry.mark_ready("t");

        let ready = registry.ready_tools();
        assert_eq!(ready.len(), 1, "exactly one ready tool");
        assert_eq!(ready[0].route_id, "route-t");
        let plan = ready[0]
            .plan
            .as_ref()
            .expect("plan captured at registration");
        assert!(
            matches!(&plan.access_mode, AccessMode::Public),
            "the snapshot must carry the registered plan's access mode"
        );
    }

    #[test]
    fn register_without_plan_carries_none() {
        let registry = McpToolRegistry::new(8);
        let owner = Arc::new(());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        registry
            .register(
                "t".to_string(),
                "route-t".to_string(),
                tx,
                serde_json::json!({}),
                Arc::downgrade(&owner),
            )
            .expect("registration must succeed");
        registry.mark_ready("t");

        let ready = registry.ready_tools();
        assert_eq!(ready.len(), 1, "exactly one ready tool");
        assert!(
            ready[0].plan.is_none(),
            "a plan-less registration must carry None"
        );
    }

    #[test]
    fn ready_resources_snapshot_carries_route_id_and_plan() {
        let registry = McpResourceRegistry::new(8);
        let owner = Arc::new(());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        registry
            .register_with_plan(
                "crm://x".to_string(),
                "route-r".to_string(),
                tx,
                Arc::downgrade(&owner),
                Some(public_plan()),
            )
            .expect("registration must succeed");
        registry.mark_ready("crm://x");

        let ready = registry.ready_resources();
        assert_eq!(ready.len(), 1, "exactly one ready resource");
        assert_eq!(ready[0].uri, "crm://x");
        assert_eq!(ready[0].route_id, "route-r");
        let plan = ready[0]
            .plan
            .as_ref()
            .expect("plan captured at registration");
        assert!(
            matches!(&plan.access_mode, AccessMode::Public),
            "the snapshot must carry the registered plan's access mode"
        );
    }

    #[test]
    fn list_ready_still_projects_name_and_schema() {
        let registry = McpToolRegistry::new(8);
        let owner = Arc::new(());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let schema = serde_json::json!({"type": "object"});
        registry
            .register(
                "t".to_string(),
                "route-t".to_string(),
                tx,
                schema.clone(),
                Arc::downgrade(&owner),
            )
            .expect("registration must succeed");
        registry.mark_ready("t");

        assert_eq!(
            registry.list_ready(),
            vec![("t".to_string(), schema)],
            "list_ready must keep its (name, schema) projection"
        );
    }
}
