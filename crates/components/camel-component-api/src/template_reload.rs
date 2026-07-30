//! Process-global registry of template reload targets + the erased reload
//! target contract.
//!
//! This is the dependency-inversion seam (mirror of
//! [`crate::tls_source::TlsReloadRegistry`]): it lives in `camel-component-api`
//! so that both `camel-core` (the RuntimeBus) and `camel-template` (the
//! ReloadHandler impl) can see it, WITHOUT `camel-core` depending on
//! `camel-template`.
//!
//! The ONLY reload path is [`TemplateReloadRegistry::reload_route`], which is
//! all-or-nothing: it builds every target for a route, validates every staged
//! generation, then commits — guaranteeing atomicity structurally.

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::CamelError;

/// Erased staged-set marker with an owned-downcast accessor.
///
/// `Box<dyn TemplateReloadStaged>` has no inherent `downcast`; `into_any`
/// returns `Box<dyn Any>`, which does. This is the standard object-downcast
/// idiom: a concrete staged type `T` impls `into_any` by returning `self`
/// (`Box<T>` coerces to `Box<dyn Any>` when `T: 'static`). Consumers then call
/// `staged.into_any().downcast::<T>()`.
pub trait TemplateReloadStaged: Send {
    /// Convert this boxed trait object into a `Box<dyn Any>` for downcasting.
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

/// A built staged set paired with the generation read at build time.
type StagedBuild = (Box<dyn TemplateReloadStaged>, u64);

/// One reload target per registered template producer endpoint.
///
/// There is NO single-producer `reload()` on this trait — the only reload path
/// is [`TemplateReloadRegistry::reload_route`]. `commit` is infallible (`()`)
/// because it is only ever reached after `reload_route` has validated every
/// staged generation, so all-or-nothing holds structurally.
#[async_trait]
pub trait TemplateReloadTarget: Send + Sync {
    /// Route id this target serves.
    fn route_id(&self) -> &str;
    /// Per-target deadline for a reload. `reload_route` uses the TIGHTEST
    /// (minimum) across all targets for the route, so registration order does
    /// not define the route deadline.
    fn reload_timeout(&self) -> Duration;
    /// Current committed generation (bumped on each successful commit).
    fn current_generation(&self) -> u64;
    /// Build a staged set against the current sources. Returns the staged set
    /// and the generation read at build time. May fail (invalid source); the
    /// prior set is retained on failure.
    async fn build(&self) -> Result<(Box<dyn TemplateReloadStaged>, u64), CamelError>;
    /// Commit a previously-built staged set (infallible). Only called by
    /// `reload_route` after validation, so no recheck is needed here.
    fn commit(&self, staged: Box<dyn TemplateReloadStaged>);
}

/// A registered target plus its unique id.
struct RegisteredTarget {
    id: u64,
    target: Arc<dyn TemplateReloadTarget>,
}

/// Monotonic id source for registrations (unique per process).
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Process-global registry of template reload targets.
pub struct TemplateReloadRegistry {
    handlers: Mutex<Vec<RegisteredTarget>>,
    route_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Default for TemplateReloadRegistry {
    fn default() -> Self {
        Self {
            handlers: Mutex::new(Vec::new()),
            route_locks: Mutex::new(HashMap::new()),
        }
    }
}

impl TemplateReloadRegistry {
    /// Process-global singleton (mirror of `TlsReloadRegistry::global`).
    pub fn global() -> &'static TemplateReloadRegistry {
        static INSTANCE: OnceLock<TemplateReloadRegistry> = OnceLock::new();
        INSTANCE.get_or_init(TemplateReloadRegistry::default)
    }

    /// Register a target. The returned guard unregisters on drop (RAII).
    ///
    /// Only callable on the [`global`](Self::global) singleton: the guard
    /// retains a `&'static` reference so it can evict its entry on drop from
    /// any context.
    pub fn register(&'static self, target: Arc<dyn TemplateReloadTarget>) -> RegistrationGuard {
        let id = next_id();
        {
            let mut guard = self
                .handlers
                .lock()
                .expect("TemplateReloadRegistry handlers lock poisoned"); // allow-unwrap
            guard.push(RegisteredTarget { id, target });
        }
        RegistrationGuard { id, registry: self }
    }

    /// All targets registered for `route_id`, in registration order.
    /// `pub` so integration tests in `camel-template` can assert registration.
    pub fn find_all(&self, route_id: &str) -> Vec<Arc<dyn TemplateReloadTarget>> {
        let guard = self
            .handlers
            .lock()
            .expect("TemplateReloadRegistry handlers lock poisoned"); // allow-unwrap
        guard
            .iter()
            .filter(|t| t.target.route_id() == route_id)
            .map(|t| Arc::clone(&t.target))
            .collect()
    }

    /// Remove the registration with this id (called by [`RegistrationGuard`]'s
    /// Drop). Removes by `id`, NOT route_id — a stopped-generation guard cannot
    /// evict a restarted-generation registration.
    fn remove(&self, id: u64) {
        let mut guard = self
            .handlers
            .lock()
            .expect("TemplateReloadRegistry handlers lock poisoned"); // allow-unwrap
        guard.retain(|t| t.id != id);
    }

    /// Get-or-insert the per-route async lock. The std guard is dropped before
    /// this returns, so the returned `Arc<tokio::sync::Mutex<_>>` is the only
    /// thing held across `.await` (no std lock held across await).
    fn route_lock(&self, route_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut guard = self
            .route_locks
            .lock()
            .expect("TemplateReloadRegistry route_locks lock poisoned"); // allow-unwrap
        guard
            .entry(route_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Reload ALL targets for `route_id` — all-or-nothing.
    ///
    /// Acquires the per-route `tokio::sync::Mutex` (serializing concurrent
    /// `reload_route` for the SAME route; different routes run in parallel).
    /// Since there is no other reload path, no concurrent generation bump is
    /// possible. Phases, in strict order:
    ///
    /// 1. **Build** — run every target's `build` concurrently; if ANY returns
    ///    `Err`, abort (nothing committed).
    /// 2. **Validate** — every staged `read_gen` must equal the target's
    ///    `current_generation`; any mismatch aborts (nothing committed).
    /// 3. **Commit** — only reached if all builds and validations succeeded;
    ///    `commit` is infallible, so atomicity is structural.
    ///
    /// The whole sequence is bounded by the tightest target deadline; a
    /// timeout returns `Err` and the dropped build futures never commit.
    pub async fn reload_route(&self, route_id: &str) -> Result<(), CamelError> {
        // Serialize concurrent reload_route for the SAME route. Different routes
        // get distinct locks and run in parallel. Holding this tokio::sync::Mutex
        // across the build/commit awaits is intentional and safe.
        let route_lock = self.route_lock(route_id);
        let _route_guard = route_lock.lock().await;

        let targets = self.find_all(route_id);
        if targets.is_empty() {
            return Err(CamelError::Config(format!(
                "no template target for route '{route_id}'"
            )));
        }

        // TIGHTEST deadline wins — registration order must not set the deadline.
        let timeout = targets
            .iter()
            .map(|t| t.reload_timeout())
            .min()
            .unwrap_or(Duration::from_millis(5000));

        tokio::time::timeout(timeout, async {
            // Build phase: run every build concurrently. join_all preserves
            // input order, so targets[i] aligns with staged[i].
            let built = futures::future::join_all(targets.iter().map(|t| t.build())).await;
            // ANY Err → abort; nothing committed (all-or-nothing).
            let staged: Vec<StagedBuild> = built.into_iter().collect::<Result<_, _>>()?;

            // Validate phase (structural stale-guard). Under the per-route
            // mutex with no other reload path this never fires in practice —
            // it is the guarantee that a delayed stale build cannot swap.
            for (target, (_set, read_gen)) in targets.iter().zip(&staged) {
                if *read_gen != target.current_generation() {
                    return Err(CamelError::TemplateReload("stale generation".to_string()));
                }
            }

            // Commit phase (infallible). Only reached if every build and every
            // validation succeeded.
            for (target, (set, _)) in targets.into_iter().zip(staged) {
                target.commit(set);
            }
            Ok(())
        })
        .await
        .map_err(|_| CamelError::TemplateReload("reload timeout".to_string()))?
    }
}

/// RAII guard returned by [`TemplateReloadRegistry::register`]. Removes the
/// target on drop by its unique `id` (NOT route_id) — so a stopped-generation
/// guard cannot evict a restarted-generation registration.
pub struct RegistrationGuard {
    id: u64,
    registry: &'static TemplateReloadRegistry,
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        self.registry.remove(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Concrete staged type carrying the generation read at build time.
    struct FakeStaged {
        read_generation: u64,
    }
    impl TemplateReloadStaged for FakeStaged {
        // `Box<FakeStaged>` coerces to `Box<dyn Any>` (FakeStaged: 'static).
        fn into_any(self: Box<Self>) -> Box<dyn Any> {
            self
        }
    }

    /// What a fake's `build()` should do.
    #[derive(Clone)]
    enum BuildMode {
        /// Succeed, returning the current generation.
        Ok,
        /// Fail immediately.
        Err,
        /// Sleep before succeeding (for the timeout test).
        Sleep(Duration),
        /// Read G, then bump current to G+1 and return read_gen=G (stale).
        Stale,
    }

    /// Shared, observable state for a fake target.
    #[derive(Default)]
    struct FakeState {
        generation: AtomicU64,
        commit_calls: AtomicUsize,
        build_calls: AtomicUsize,
    }

    struct FakeTarget {
        route: String,
        timeout: Duration,
        state: Arc<FakeState>,
        mode: StdMutex<BuildMode>,
        /// Optional event recorder for the serialize test.
        events: Option<Arc<StdMutex<Vec<&'static str>>>>,
    }

    impl FakeTarget {
        fn new(route: &str) -> Arc<Self> {
            Arc::new(Self {
                route: route.to_string(),
                timeout: Duration::from_secs(5),
                state: Arc::new(FakeState::default()),
                mode: StdMutex::new(BuildMode::Ok),
                events: None,
            })
        }

        fn set_mode(&self, mode: BuildMode) {
            *self.mode.lock().unwrap() = mode;
        }

        /// Coerce to the erased trait-object Arc that `register` expects.
        fn as_dyn(self: &Arc<Self>) -> Arc<dyn TemplateReloadTarget> {
            // Bind to a concrete-typed local so CoerceUnsized fires at the
            // return site (inference would otherwise pin `Arc::clone` to the
            // trait object and fail).
            let concrete: Arc<Self> = Arc::clone(self);
            concrete
        }
    }

    #[async_trait]
    impl TemplateReloadTarget for FakeTarget {
        fn route_id(&self) -> &str {
            &self.route
        }
        fn reload_timeout(&self) -> Duration {
            self.timeout
        }
        fn current_generation(&self) -> u64 {
            self.state.generation.load(Ordering::SeqCst)
        }
        async fn build(&self) -> Result<(Box<dyn TemplateReloadStaged>, u64), CamelError> {
            self.state.build_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(ev) = &self.events {
                ev.lock().unwrap().push("start");
            }
            let mode = self.mode.lock().unwrap().clone();
            match mode {
                BuildMode::Err => {
                    if let Some(ev) = &self.events {
                        ev.lock().unwrap().push("end");
                    }
                    return Err(CamelError::TemplateReload("fake build failed".to_string()));
                }
                BuildMode::Sleep(d) => {
                    tokio::time::sleep(d).await;
                }
                BuildMode::Ok | BuildMode::Stale => {
                    // yield to encourage interleaving when not serialized.
                    tokio::task::yield_now().await;
                }
            }
            let read_gen = match mode {
                // Read G, then bump current to G+1 BEFORE returning, so the
                // validate phase sees read_gen(G) != current(G+1).
                BuildMode::Stale => self.state.generation.fetch_add(1, Ordering::SeqCst),
                _ => self.state.generation.load(Ordering::SeqCst),
            };
            if let Some(ev) = &self.events {
                ev.lock().unwrap().push("end");
            }
            Ok((
                Box::new(FakeStaged {
                    read_generation: read_gen,
                }),
                read_gen,
            ))
        }
        fn commit(&self, staged: Box<dyn TemplateReloadStaged>) {
            // Exercise the downcast idiom end-to-end.
            let concrete = staged.into_any().downcast::<FakeStaged>().unwrap();
            assert_eq!(
                concrete.read_generation,
                self.state.generation.load(Ordering::SeqCst)
            );
            self.state.commit_calls.fetch_add(1, Ordering::SeqCst);
            self.state.generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn registry_register_find_all_remove() {
        let reg = TemplateReloadRegistry::global();
        let route = "test-register-find-all-remove";
        let target = FakeTarget::new(route);
        let _guard = reg.register(target.as_dyn());
        assert_eq!(reg.find_all(route).len(), 1);
        drop(_guard);
        assert_eq!(reg.find_all(route).len(), 0);
    }

    #[tokio::test]
    async fn reload_route_all_or_nothing() {
        let reg = TemplateReloadRegistry::global();
        let route = "test-all-or-nothing";
        let ok = FakeTarget::new(route);
        let err = FakeTarget::new(route);
        err.set_mode(BuildMode::Err);
        let g1 = reg.register(ok.as_dyn());
        let g2 = reg.register(err.as_dyn());

        let res = reg.reload_route(route).await;
        assert!(res.is_err(), "expected reload to fail");
        assert_eq!(
            ok.state.commit_calls.load(Ordering::SeqCst),
            0,
            "OK target must NOT be committed"
        );
        assert_eq!(
            err.state.commit_calls.load(Ordering::SeqCst),
            0,
            "Err target must NOT be committed"
        );
        assert_eq!(
            ok.state.generation.load(Ordering::SeqCst),
            0,
            "prior generation retained"
        );
        drop(g1);
        drop(g2);
    }

    #[tokio::test]
    async fn reload_route_commits_all_on_success() {
        let reg = TemplateReloadRegistry::global();
        let route = "test-commits-all-on-success";
        let a = FakeTarget::new(route);
        let b = FakeTarget::new(route);
        let ga = reg.register(a.as_dyn());
        let gb = reg.register(b.as_dyn());

        let res = reg.reload_route(route).await;
        assert!(res.is_ok(), "expected reload to succeed: {:?}", res);
        assert_eq!(a.state.commit_calls.load(Ordering::SeqCst), 1);
        assert_eq!(b.state.commit_calls.load(Ordering::SeqCst), 1);
        assert_eq!(a.state.generation.load(Ordering::SeqCst), 1);
        assert_eq!(b.state.generation.load(Ordering::SeqCst), 1);
        drop(ga);
        drop(gb);
    }

    #[tokio::test]
    async fn reload_route_timeout_no_commit() {
        let reg = TemplateReloadRegistry::global();
        let route = "test-timeout-no-commit";
        // Tighten the deadline (40ms) and make build overrun it (2s sleep).
        let slow = Arc::new(FakeTarget {
            route: route.to_string(),
            timeout: Duration::from_millis(40),
            state: Arc::new(FakeState::default()),
            mode: StdMutex::new(BuildMode::Sleep(Duration::from_millis(2_000))),
            events: None,
        });
        let g = reg.register(slow.as_dyn());

        let res = reg.reload_route(route).await;
        assert!(
            matches!(res, Err(CamelError::TemplateReload(_))),
            "expected TemplateReload timeout error, got {res:?}"
        );
        assert_eq!(
            slow.state.commit_calls.load(Ordering::SeqCst),
            0,
            "commit must never be called on timeout"
        );
        drop(g);
    }

    #[tokio::test]
    async fn reload_route_rejects_stale_no_commit() {
        let reg = TemplateReloadRegistry::global();
        let route = "test-rejects-stale-no-commit";
        let target = FakeTarget::new(route);
        target.set_mode(BuildMode::Stale);
        let g = reg.register(target.as_dyn());

        let res = reg.reload_route(route).await;
        assert!(
            matches!(res, Err(CamelError::TemplateReload(_))),
            "expected TemplateReload stale error, got {res:?}"
        );
        assert_eq!(
            target.state.commit_calls.load(Ordering::SeqCst),
            0,
            "commit must never be called on stale rejection"
        );
        drop(g);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_route_serializes_concurrent() {
        let reg = TemplateReloadRegistry::global();
        let route = "test-serializes-concurrent";
        let events: Arc<StdMutex<Vec<&'static str>>> = Arc::new(StdMutex::new(Vec::new()));
        let target = Arc::new(FakeTarget {
            route: route.to_string(),
            timeout: Duration::from_secs(5),
            state: Arc::new(FakeState::default()),
            mode: StdMutex::new(BuildMode::Ok),
            events: Some(Arc::clone(&events)),
        });
        let g = reg.register(target.as_dyn());

        // Spawn two concurrent reload_route for the SAME route.
        let h1 = tokio::spawn(async move { reg.reload_route(route).await });
        let h2 = tokio::spawn(async move { reg.reload_route(route).await });
        let (r1, r2) = tokio::join!(h1, h2);
        r1.unwrap().unwrap();
        r2.unwrap().unwrap();

        // If serialized by the per-route mutex, build calls never interleave:
        // the sequence must be start,end,start,end (NOT start,start,end,end).
        let evs = events.lock().unwrap().clone();
        assert_eq!(
            evs,
            vec!["start", "end", "start", "end"],
            "per-route mutex must serialize concurrent reload_route"
        );
        assert_eq!(target.state.commit_calls.load(Ordering::SeqCst), 2);
        drop(g);
    }
}
