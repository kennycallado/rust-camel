//! Reload handler for the external template component (ADR-0047 Stage 2,
//! Phase 5).
//!
//! [`ReloadHandler`] is the per-route reload target. Task 5.1 (this commit)
//! adds the staged-set type ([`StagedSet`]) and the `build` /
//! `current_generation` / `commit` triad. Task 5.3 wires this into the
//! [`camel_component_api::template_reload::TemplateReloadTarget`] trait and the
//! process-global registry.
//!
//! **Build / commit split (Critical 4):** the synchronous FS read + compile
//! run inside `tokio::task::spawn_blocking`, which CANNOT be interrupted by
//! `tokio::time::timeout`. The outer `reload_route` (Task 5.2) bounds
//! [`ReloadHandler::build`] with a timeout; if it fires, the blocking task is
//! detached and never reaches [`ReloadHandler::commit`], so no partial state is
//! committed.

use std::any::Any;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_component_api::template_reload::{TemplateReloadStaged, TemplateReloadTarget};
use camel_language_api::MinijinjaLimitsConfig;

use crate::closure;
use crate::config::ResolvedExternalTemplateLimits;
use crate::error::TemplateReloadError;
use crate::path_util::OwnedHandle;
use crate::template_set::{SharedTemplates, TemplateSet};

/// Hot-reload handler installed in `StartupBuildHandle::handler` after a
/// successful startup build (Task 4.4). Drives a generation counter, the
/// retained root handle, and the build / commit reload steps that atomically
/// swap `shared`.
///
/// Task 5.1 (this commit) adds [`StagedSet`] + `build` / `current_generation`
/// / `commit`. The reload loop and registry registration land in Task 5.3.
pub(crate) struct ReloadHandler {
    /// Compiled-set swap cell shared with the endpoint and the producer.
    pub(crate) shared: SharedTemplates,
    /// Absolute path of the entry template (the closure seed in the reload
    /// build).
    pub(crate) entry_abs_path: PathBuf,
    /// Per-route render limits (consumed at render time; cloned into each
    /// reload build).
    pub(crate) render_limits: MinijinjaLimitsConfig,
    /// Operator-resolved acquisition limits (consumed at build time).
    pub(crate) limits: ResolvedExternalTemplateLimits,
    /// Monotonic generation counter. READ by `build` (capturing the build-time
    /// generation); bumped ONLY by `commit`. The split is what lets
    /// `reload_route` (Task 5.2) validate all staged generations before any
    /// commit (all-or-nothing).
    pub(crate) generation: Mutex<u64>,
    /// Kernel handle to the open root directory (the `openat` anchor for
    /// confined traversal). Retained from `StartupBuildHandle::start()` so
    /// reloads re-acquire snapshots against the same anchor without a second
    /// root open.
    pub(crate) root: Arc<OwnedHandle>,
    /// Owning route id (used for log labels and metrics).
    pub(crate) route_id: String,
}

impl std::fmt::Debug for ReloadHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReloadHandler")
            .field("entry_abs_path", &self.entry_abs_path)
            .field("route_id", &self.route_id)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// A compiled template set paired with the generation that was current when it
/// was built. Produced ONLY by [`ReloadHandler::build`], consumed ONLY by
/// [`ReloadHandler::commit`]; the pairing lets `reload_route` (Task 5.2)
/// reject a staged set whose build-time generation has since been superseded.
///
/// `read_generation` is carried for structural completeness and future
/// diagnostics; [`ReloadHandler::commit`] is infallible and does NOT re-check
/// it — the validate phase in `reload_route` re-checks the generation returned
/// by `build`.
#[allow(dead_code)] // Phase-5: read_generation is not re-read in commit; the
// validate phase in reload_route (Task 5.2) uses build's
// returned u64 instead.
pub(crate) struct StagedSet {
    set: TemplateSet,
    read_generation: u64,
}

impl TemplateReloadStaged for StagedSet {
    // `Box<StagedSet>` coerces to `Box<dyn Any>` (StagedSet: 'static) — the
    // standard object-downcast idiom (see `TemplateReloadStaged::into_any`).
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl ReloadHandler {
    /// Build a staged set against the current on-disk sources.
    ///
    /// The bounded FS read ([`closure::build_snapshot`]) and the compile
    /// ([`TemplateSet::compile`]) are SYNCHRONOUS CPU/IO work, so they run
    /// inside `tokio::task::spawn_blocking`: the blocking task CANNOT be
    /// interrupted by `tokio::time::timeout` (Critical 4). The outer
    /// `reload_route` (Task 5.2) bounds this call with a timeout; a timeout
    /// fire detaches the blocking task, which never returns a staged set and
    /// so never reaches [`Self::commit`].
    ///
    /// On success this READS the current generation (capturing it as the staged
    /// `read_generation`) but does NOT store or increment — only
    /// [`Self::commit`] mutates.
    ///
    /// # Errors
    ///
    /// Returns `Err(CamelError::TemplateReload(..))` on any acquire / compile
    /// failure or on a `spawn_blocking` join failure. On error the prior set is
    /// retained: neither `generation` nor `shared` is touched.
    pub async fn build(&self) -> Result<(Box<dyn TemplateReloadStaged>, u64), CamelError> {
        // Clone every input the blocking task needs so the closure is
        // 'static + Send: the root Arc, the entry path, the Copy limits, and
        // the render limits.
        let root = Arc::clone(&self.root);
        let entry = self.entry_abs_path.clone();
        let limits = self.limits;
        let render_limits = self.render_limits.clone();

        let join =
            tokio::task::spawn_blocking(move || -> Result<TemplateSet, TemplateReloadError> {
                let snapshot = closure::build_snapshot(&entry, root.as_ref(), limits)?;
                // The entry name registered into the Environment must equal the
                // name `build_snapshot` keyed the closure under (the entry's
                // `file_name()`), so compile's lookup resolves — mirrors
                // `StartupBuildHandle::start()` exactly.
                let entry_str = entry
                    .file_name()
                    .ok_or_else(|| {
                        TemplateReloadError::PathEscape("entry has no file name".into())
                    })?
                    .to_string_lossy()
                    .into_owned();
                TemplateSet::compile(&snapshot, &entry_str, render_limits)
            });

        // Any failure — acquire/compile OR a join error — returns WITHOUT
        // storing; the prior set is retained (all-or-nothing).
        let set = join
            .await
            .map_err(|e| {
                CamelError::TemplateReload(format!("reload build spawn_blocking join error: {e}"))
            })?
            .map_err(CamelError::from)?;

        // READ the build-time generation; do NOT store or increment. `commit`
        // is the only mutator. This split is what enables the all-or-nothing
        // validate phase in `reload_route` (Task 5.2).
        let read_generation = *self.generation.lock().expect("generation poisoned"); // allow-unwrap
        Ok((
            Box::new(StagedSet {
                set,
                read_generation,
            }),
            read_generation,
        ))
    }

    /// Current committed generation. Bumped exactly once per successful
    /// [`Self::commit`].
    pub fn current_generation(&self) -> u64 {
        *self.generation.lock().expect("generation poisoned") // allow-unwrap
    }

    /// Commit a previously-built staged set — INFALLIBLE.
    ///
    /// Only ever called by `reload_route` AFTER it has validated every staged
    /// generation, so all-or-nothing holds structurally. The downcast is safe
    /// because the ONLY producer of a `StagedSet` is this handler's
    /// [`Self::build`], and the ONLY consumer is this `commit`.
    pub fn commit(&self, staged: Box<dyn TemplateReloadStaged>) {
        let concrete = staged
            .into_any()
            .downcast::<StagedSet>()
            .expect("staged type matches its builder"); // allow-unwrap
        *self.generation.lock().expect("generation poisoned") += 1; // allow-unwrap
        self.shared.store(Arc::new(concrete.set));
    }
}

/// ReloadHandler is one `TemplateReloadTarget` per registered template producer
/// endpoint (Task 5.3). Every method delegates to the inherent impl above via
/// fully-qualified calls (e.g. `ReloadHandler::build(self)`), so there is no
/// resolution ambiguity with the trait methods being defined.
#[async_trait]
impl TemplateReloadTarget for ReloadHandler {
    fn route_id(&self) -> &str {
        &self.route_id
    }

    fn reload_timeout(&self) -> Duration {
        Duration::from_millis(self.limits.reload_timeout_ms)
    }

    fn current_generation(&self) -> u64 {
        ReloadHandler::current_generation(self)
    }

    async fn build(&self) -> Result<(Box<dyn TemplateReloadStaged>, u64), CamelError> {
        ReloadHandler::build(self).await
    }

    fn commit(&self, staged: Box<dyn TemplateReloadStaged>) {
        ReloadHandler::commit(self, staged);
    }
}

#[cfg(test)]
mod tests {
    use super::{ReloadHandler, SharedTemplates};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use arc_swap::ArcSwap;
    use camel_language_api::MinijinjaLimitsConfig;
    use camel_language_minijinja::ResolvedLimits;

    use crate::closure;
    use crate::config::ResolvedExternalTemplateLimits;
    use crate::path_util::OwnedHandle;
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

    /// `{ name: k }` context built without the `value!` macro (which collides
    /// with the `minijinja::value` module name in scope).
    fn ctx_name(k: &str) -> minijinja::Value {
        let mut m: BTreeMap<&str, &str> = BTreeMap::new();
        m.insert("name", k);
        minijinja::Value::from_serialize(&m)
    }

    /// Build a `ReloadHandler` seeded with a compiled `entry_content` set (the
    /// initial S0). Returns the tempdir (kept alive), the handler, the shared
    /// cell, and the entry path so a test can mutate the on-disk source. The
    /// handler retains the same open root handle used to seed S0.
    #[allow(clippy::type_complexity)]
    fn seed_handler(
        entry_content: &str,
    ) -> (
        tempfile::TempDir,
        Arc<ReloadHandler>,
        SharedTemplates,
        PathBuf,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = dir.path().join("page.html");
        fs::write(&entry, entry_content).expect("write entry");

        let (root, _id) = crate::path_util::open_root(dir.path()).expect("open root");
        let root: Arc<OwnedHandle> = Arc::new(root);

        // Seed S0: build a snapshot + compile under the same root handle the
        // handler will reuse for reloads.
        let snapshot = closure::build_snapshot(&entry, root.as_ref(), default_limits())
            .expect("seed snapshot");
        let entry_str = entry
            .file_name()
            .expect("entry file name")
            .to_string_lossy()
            .into_owned();
        let set = TemplateSet::compile(&snapshot, &entry_str, MinijinjaLimitsConfig::default())
            .expect("seed compile");
        let shared: SharedTemplates = Arc::new(ArcSwap::from_pointee(set));

        let handler = Arc::new(ReloadHandler {
            shared: Arc::clone(&shared),
            entry_abs_path: entry.clone(),
            render_limits: MinijinjaLimitsConfig::default(),
            limits: default_limits(),
            generation: Mutex::new(0),
            root: Arc::clone(&root),
            route_id: "test-reload".to_string(),
        });
        (dir, handler, shared, entry)
    }

    #[tokio::test]
    async fn reload_build_does_not_store_on_compile_error() {
        let v1 = r#"{% autoescape "none" %}v1{{name}}{% endautoescape %}"#;
        let (_dir, handler, shared, entry) = seed_handler(v1);
        let gen_before = handler.current_generation();

        // Make the on-disk source invalid: no top-level autoescape wrapper.
        fs::write(&entry, "broken {{name}").expect("write broken source");

        let result = handler.build().await;
        assert!(result.is_err(), "build of invalid source must return Err");
        // build() must NOT increment generation or store anything.
        assert_eq!(handler.current_generation(), gen_before);

        // The shared cell still holds the seeded S0, which renders v1.
        let set = shared.load_full();
        let rendered = set
            .render_entry(ctx_name("k"), ResolvedLimits::default())
            .await
            .expect("render S0");
        assert_eq!(rendered, "v1k");
    }

    #[tokio::test]
    async fn reload_commit_swaps_on_valid_change() {
        let v1 = r#"{% autoescape "none" %}v1{{name}}{% endautoescape %}"#;
        let v2 = r#"{% autoescape "none" %}v2{{name}}{% endautoescape %}"#;
        let (_dir, handler, shared, entry) = seed_handler(v1);
        let gen_before = handler.current_generation();

        // Mutate the on-disk source to v2.
        fs::write(&entry, v2).expect("write v2");

        let (staged, read_gen) = handler.build().await.expect("build v2");
        // build() captures (but does not bump) generation.
        assert_eq!(read_gen, gen_before);

        handler.commit(staged);

        // commit() bumped the generation exactly once.
        assert_eq!(handler.current_generation(), gen_before + 1);

        // The shared cell now renders v2.
        let set = shared.load_full();
        let rendered = set
            .render_entry(ctx_name("k"), ResolvedLimits::default())
            .await
            .expect("render v2");
        assert_eq!(rendered, "v2k");
    }

    #[tokio::test]
    async fn reload_commit_is_infallible() {
        let v1 = r#"{% autoescape "none" %}v1{{name}}{% endautoescape %}"#;
        let v2 = r#"{% autoescape "none" %}v2{{name}}{% endautoescape %}"#;
        let (_dir, handler, shared, entry) = seed_handler(v1);
        let gen_before = handler.current_generation();

        fs::write(&entry, v2).expect("write v2");
        let (staged, _read_gen) = handler.build().await.expect("build v2");

        // commit() returns unit — compile-time proof of infallibility.
        let returned: () = handler.commit(staged);
        assert_eq!(returned, ());

        // Generation bumped and the set swapped to v2.
        assert_eq!(handler.current_generation(), gen_before + 1);
        let set = shared.load_full();
        let rendered = set
            .render_entry(ctx_name("k"), ResolvedLimits::default())
            .await
            .expect("render v2");
        assert_eq!(rendered, "v2k");
    }

    /// Task 5.3: `ReloadHandler` implements `TemplateReloadTarget`, and the
    /// trait's `build` / `commit` delegate to the inherent impls so a reload
    /// swaps the set and bumps the generation. Exercises the erased
    /// `dyn TemplateReloadTarget` dispatch path end-to-end (no registry).
    #[tokio::test]
    async fn reload_handler_impls_target() {
        use camel_component_api::template_reload::TemplateReloadTarget;

        let v1 = r#"{% autoescape "none" %}v1{{name}}{% endautoescape %}"#;
        let v2 = r#"{% autoescape "none" %}v2{{name}}{% endautoescape %}"#;
        let (_dir, handler, shared, entry) = seed_handler(v1);
        let gen_before = handler.current_generation();
        assert_eq!(TemplateReloadTarget::route_id(&*handler), "test-reload");
        assert_eq!(
            TemplateReloadTarget::reload_timeout(&*handler),
            std::time::Duration::from_millis(5000)
        );
        assert_eq!(
            TemplateReloadTarget::current_generation(&*handler),
            gen_before
        );

        fs::write(&entry, v2).expect("write v2");

        // Drive build + commit THROUGH the trait object surface.
        let (staged, read_gen) = TemplateReloadTarget::build(&*handler)
            .await
            .expect("trait build v2");
        assert_eq!(read_gen, gen_before);
        TemplateReloadTarget::commit(&*handler, staged);

        // Generation bumped exactly once and the shared cell swapped to v2.
        assert_eq!(handler.current_generation(), gen_before + 1);
        let set = shared.load_full();
        let rendered = set
            .render_entry(ctx_name("k"), ResolvedLimits::default())
            .await
            .expect("render v2");
        assert_eq!(rendered, "v2k");
    }
}
