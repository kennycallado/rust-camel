//! Lifecycle handle for the external template component (ADR-0047 Stage 2,
//! Phase 4 / Tasks 4.3 + 4.4).
//!
//! **Task 4.3 (this commit)** creates the [`StartupBuildHandle`] struct shell
//! and a stub [`StepLifecycle`] impl so `TemplateEndpoint::lifecycle()`
//! compiles. **Task 4.4** fleshes out the real `start()` — open the root
//! directory, build a `ClosureSnapshot`, compile the entry, seed the
//! `SharedTemplates` cell, and construct a [`ReloadHandler`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use camel_api::{CamelError, StepLifecycle, StepShutdownReason};
use camel_component_api::RuntimeObservability;
use camel_component_api::template_reload::{
    RegistrationGuard, TemplateReloadRegistry, TemplateReloadTarget,
};
use camel_language_api::MinijinjaLimitsConfig;

use crate::closure;
use crate::config::ResolvedExternalTemplateLimits;
use crate::error::TemplateReloadError;
use crate::path_util;
use crate::reload::ReloadHandler;
use crate::template_set::{SharedTemplates, TemplateSet};

/// Lifecycle handle returned from `TemplateEndpoint::lifecycle()`.
///
/// `start()` (Task 4.4) opens the root directory once, walks the
/// dependency closure, compiles the entry into a [`TemplateSet`], swaps
/// it into [`SharedTemplates`], and installs a [`ReloadHandler`] that
/// retains the root handle for Phase-5 hot reloads. `shutdown()` is a
/// no-op until Phase 5 wires the reload-registration guard.
///
/// [`TemplateSet`]: crate::template_set::TemplateSet
/// [`SharedTemplates`]: crate::template_set::SharedTemplates
pub(crate) struct StartupBuildHandle {
    /// Compiled-set swap cell shared with the endpoint and the producer.
    pub(crate) shared: SharedTemplates,
    /// Absolute filesystem path of the entry template (parsed by
    /// `parse_template_uri`, validated at Task 2.2).
    pub(crate) entry_abs_path: PathBuf,
    /// Per-route render limits (consumed at render time).
    pub(crate) render_limits: MinijinjaLimitsConfig,
    /// Operator-resolved acquisition limits (consumed at build time).
    pub(crate) limits: ResolvedExternalTemplateLimits,
    /// Runtime observability handle stashed at `create_producer` time.
    /// None until the producer is built.
    pub(crate) rt: Option<Arc<dyn RuntimeObservability>>,
    /// Owning route id (used for log labels and metrics).
    pub(crate) route_id: String,
    /// Reload handler installed after a successful build. None until
    /// Task 4.4's `start()` runs.
    // Phase-4 lifecycle seam: filled in 4.4, read in 5.3.
    pub(crate) handler: Mutex<Option<Arc<ReloadHandler>>>,
    /// RAII registration guard for the process-global reload registry
    /// (Task 5.3). `start()` registers the handler and stores the guard
    /// here; `shutdown()` drops it, which unregisters by unique id. On a
    /// route restart a fresh guard with a new id is stored — a dropped
    /// stopped-generation guard can never evict the new registration.
    pub(crate) guard: Mutex<Option<RegistrationGuard>>,
}

impl std::fmt::Debug for StartupBuildHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartupBuildHandle")
            .field("entry_abs_path", &self.entry_abs_path)
            .field("route_id", &self.route_id)
            .field("rt_set", &self.rt.is_some())
            .field(
                "handler_set",
                &self.handler.lock().expect("poisoned").is_some(),
            )
            .field("guard_set", &self.guard.lock().expect("poisoned").is_some())
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl StepLifecycle for StartupBuildHandle {
    fn name(&self) -> &'static str {
        "template-startup"
    }

    async fn start(&self) -> Result<(), CamelError> {
        // The root directory is the entry's parent; every confined read
        // is `openat`-relative to it. A path with no parent (e.g. `/`)
        // cannot anchor a closure.
        let parent = self.entry_abs_path.parent().ok_or_else(|| {
            CamelError::from(TemplateReloadError::PathEscape(
                "entry has no parent directory".into(),
            ))
        })?;

        // The entry name registered into the Environment is the entry's
        // `file_name()` — the SAME root-relative name `build_snapshot`
        // uses to key the closure, so `compile`'s lookup resolves.
        // `to_string_lossy` matches `build_snapshot`'s key conversion
        // exactly, so a non-UTF-8 file name stays consistent across both.
        let entry_name = self.entry_abs_path.file_name().ok_or_else(|| {
            CamelError::from(TemplateReloadError::PathEscape(
                "entry has no file name".into(),
            ))
        })?;
        let entry_str = entry_name.to_string_lossy();

        // Open the root ONCE. The handle is retained in the ReloadHandler
        // below so Phase-5 reloads re-acquire snapshots against the same
        // anchor without paying for another root open.
        let (root, _root_identity) = path_util::open_root(parent).map_err(CamelError::from)?;
        let root = Arc::new(root);

        // Walk the dependency closure `openat`-relative to the root.
        let snapshot = closure::build_snapshot(&self.entry_abs_path, root.as_ref(), self.limits)
            .map_err(CamelError::from)?;

        // Compile once, under the configured render limits.
        let set = TemplateSet::compile(&snapshot, &entry_str, self.render_limits.clone())
            .map_err(CamelError::from)?;

        // Seed the shared cell with the compiled set. The empty seed is
        // never observed by a request: `start_route` (Task 3.3) awaits
        // `start()` before serving.
        self.shared.store(Arc::new(set));

        // Install the ReloadHandler at generation 0; it retains the root
        // handle for the Phase-5 reload loop. Startup is NOT a reload,
        // so `template_reloads_total` is not incremented here (Task 5.4).
        let handler = Arc::new(ReloadHandler {
            shared: Arc::clone(&self.shared),
            entry_abs_path: self.entry_abs_path.clone(),
            render_limits: self.render_limits.clone(),
            limits: self.limits,
            generation: Mutex::new(0),
            root: Arc::clone(&root),
            rt: self.rt.clone(),
            route_id: self.route_id.clone(),
        });
        *self.handler.lock().expect("handler cell poisoned") = Some(Arc::clone(&handler));

        // Register the handler into the process-global reload registry
        // (Task 5.3) so `reload_route` can reach it. The registration must
        // happen AFTER the handler is constructed and stored above so the
        // `Arc::clone` handed to the registry outlives the guard. The
        // returned guard is retained here; dropping it (in `shutdown`)
        // unregisters by unique id — RAII.
        let guard = TemplateReloadRegistry::global()
            .register(Arc::clone(&handler) as Arc<dyn TemplateReloadTarget>);
        *self.guard.lock().expect("guard cell poisoned") = Some(guard);

        Ok(())
    }

    async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
        // Drop the reload-registration guard. Both reasons (RouteStop /
        // HotSwap) mean the route is tearing down, so the handler must be
        // unregistered either way. `RegistrationGuard::drop` removes the
        // entry by its unique id — a stopped-generation guard can never
        // evict a restarted-generation registration.
        *self.guard.lock().expect("guard cell poisoned") = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    use arc_swap::ArcSwap;
    use camel_language_minijinja::ResolvedLimits;

    use crate::config::ResolvedExternalTemplateLimits;
    use crate::template_set::TemplateSet;

    fn default_limits() -> ResolvedExternalTemplateLimits {
        ResolvedExternalTemplateLimits {
            max_total_source_bytes: 1024 * 1024,
            max_include_count: 64,
            max_include_depth: 16,
            max_template_size: 1024 * 1024,
            reload_timeout_ms: 5000,
        }
    }

    /// `{ name: k }` context built without the `value!` macro (which
    /// collides with the `minijinja::value` module name in scope).
    fn ctx_name(k: &str) -> minijinja::Value {
        let mut m: BTreeMap<&str, &str> = BTreeMap::new();
        m.insert("name", k);
        minijinja::Value::from_serialize(&m)
    }

    #[tokio::test]
    async fn startup_build_compiles_and_seeds() {
        // Arrange: a tempdir with a valid entry carrying the ADR-0047
        // top-level `{% autoescape %}` wrapper so compile passes.
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = dir.path().join("page.html");
        fs::write(
            &entry,
            r#"{% autoescape "none" %}Hi {{name}}{% endautoescape %}"#,
        )
        .expect("write page");

        let shared: SharedTemplates = Arc::new(ArcSwap::from_pointee(TemplateSet::empty()));

        let handle = StartupBuildHandle {
            shared: Arc::clone(&shared),
            entry_abs_path: entry,
            render_limits: MinijinjaLimitsConfig::default(),
            limits: default_limits(),
            rt: None,
            route_id: "test-route".to_string(),
            handler: Mutex::new(None),
            guard: Mutex::new(None),
        };

        // Act
        handle.start().await.expect("start must succeed");

        // Assert: the compiled set was swapped in and renders the entry.
        let set = shared.load_full();
        let rendered = set
            .render_entry(ctx_name("k"), ResolvedLimits::default())
            .await
            .expect("render entry");
        assert_eq!(rendered, "Hi k");
    }

    #[tokio::test]
    async fn startup_build_fails_closed_on_missing_file() {
        // Arrange: parent dir exists, but the entry file does not.
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = dir.path().join("does_not_exist.html");

        let shared: SharedTemplates = Arc::new(ArcSwap::from_pointee(TemplateSet::empty()));

        let handle = StartupBuildHandle {
            shared: Arc::clone(&shared),
            entry_abs_path: entry,
            render_limits: MinijinjaLimitsConfig::default(),
            limits: default_limits(),
            rt: None,
            route_id: "test-route".to_string(),
            handler: Mutex::new(None),
            guard: Mutex::new(None),
        };

        // Act + Assert: start() fails closed with a TemplateReload error.
        let result = handle.start().await;
        assert!(
            matches!(result, Err(CamelError::TemplateReload(_))),
            "missing entry must surface as CamelError::TemplateReload, got: {result:?}"
        );

        // The shared cell must still hold the empty seed: rendering the
        // entry fails at template lookup (the empty set's entry name is
        // `""`), so a request could never observe a half-built state.
        let set = shared.load_full();
        let rendered = set
            .render_entry(ctx_name("k"), ResolvedLimits::default())
            .await;
        assert!(
            rendered.is_err(),
            "shared must remain the empty seed after a failed start"
        );
    }

    /// Build a `StartupBuildHandle` for the registration-lifecycle tests, with
    /// a UNIQUE route id (the registry is process-global) and a valid entry so
    /// `start()` succeeds.
    fn make_handle(route: &str) -> (tempfile::TempDir, StartupBuildHandle) {
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = dir.path().join("page.html");
        fs::write(
            &entry,
            r#"{% autoescape "none" %}Hi {{name}}{% endautoescape %}"#,
        )
        .expect("write page");
        let shared: SharedTemplates = Arc::new(ArcSwap::from_pointee(TemplateSet::empty()));
        let handle = StartupBuildHandle {
            shared: Arc::clone(&shared),
            entry_abs_path: entry,
            render_limits: MinijinjaLimitsConfig::default(),
            limits: default_limits(),
            rt: None,
            route_id: route.to_string(),
            handler: Mutex::new(None),
            guard: Mutex::new(None),
        };
        (dir, handle)
    }

    /// Task 5.3: `start()` registers the handler into the process-global
    /// registry and stores the RAII guard; `shutdown()` drops the guard, which
    /// unregisters by unique id. Uses a unique route id + guard-drop cleanup so
    /// it cannot collide with sibling tests on the same global singleton.
    #[tokio::test]
    async fn start_registers_shutdown_unregisters() {
        use camel_component_api::template_reload::TemplateReloadRegistry;
        let route = "test-start-reg-shutdown-unreg-5.3";
        let (_dir, handle) = make_handle(route);
        let reg = TemplateReloadRegistry::global();

        // Before start: nothing registered for this route.
        assert_eq!(reg.find_all(route).len(), 0);

        handle.start().await.expect("start");
        assert_eq!(
            reg.find_all(route).len(),
            1,
            "start() must register the handler"
        );

        handle
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("shutdown");
        assert_eq!(
            reg.find_all(route).len(),
            0,
            "shutdown() must drop the guard and unregister"
        );
    }

    /// Task 5.3: after a stop→start cycle, `start()` re-registers with a
    /// FRESH guard (new unique id). The dropped stopped-generation guard was
    /// removed by id on shutdown, so the new registration is the only one for
    /// the route — proving a stopped guard cannot evict a restarted one.
    #[tokio::test]
    async fn restart_re_registers_new_guard() {
        use camel_component_api::template_reload::TemplateReloadRegistry;
        let route = "test-restart-re-registers-new-guard-5.3";
        let (_dir, handle) = make_handle(route);
        let reg = TemplateReloadRegistry::global();

        // First lifecycle: start → register → shutdown → unregister.
        handle.start().await.expect("start #1");
        assert_eq!(reg.find_all(route).len(), 1);
        handle
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("shutdown #1");
        assert_eq!(reg.find_all(route).len(), 0);

        // Restart: start() constructs a new ReloadHandler + registers a fresh
        // guard. find_all must show exactly 1 — the old guard was dropped on
        // shutdown #1, so it cannot linger or evict the new registration.
        handle.start().await.expect("start #2");
        assert_eq!(
            reg.find_all(route).len(),
            1,
            "restart must re-register with a fresh guard"
        );

        // Cleanup: drop the restarted registration so it cannot leak to
        // sibling tests on the global singleton.
        handle
            .shutdown(StepShutdownReason::HotSwap)
            .await
            .expect("shutdown #2");
        assert_eq!(reg.find_all(route).len(), 0);
    }
}
