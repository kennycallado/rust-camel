//! Per-route script dependencies for dev watch.
//!
//! Route documents reference script files (`source_file`). The watcher must
//! observe those files, but only for routes that actually RUN: a discovery or
//! build failure must never unwatch a route that kept its previous generation.
//! This registry is the seam between discovery (which STAGES the script paths
//! it read) and the reload pass outcome (which PUBLISHES the routes that were
//! rebuilt or removed).
//!
//! Invariants:
//!
//! - Staging never touches the published set; a failed or aborted pass cannot
//!   unwatch a running route.
//! - Only a committed rebuild promotes staged paths, and only an explicitly
//!   committed removal deletes a route's published paths.
//! - A route in neither the rebuilt nor the removed list keeps its published
//!   entry (failed/skipped retention).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

/// Process-global registry of per-route script dependencies.
///
/// The global singleton is production wiring; tests build fresh instances via
/// `Box::leak` so parallel tests never observe each other's state.
#[derive(Default)]
pub struct ScriptWatchRegistry {
    /// Paths of routes that are currently running and were rebuilt (or loaded)
    /// successfully. This is the watch set.
    published: Mutex<BTreeMap<String, BTreeSet<PathBuf>>>,
    /// Paths recorded by the discovery pass in progress. Replaced at the start
    /// of every pass and promoted only for rebuilt routes.
    staged: Mutex<BTreeMap<String, BTreeSet<PathBuf>>>,
}

impl ScriptWatchRegistry {
    /// Create an empty registry.
    ///
    /// Unit tests leak these (`Box::leak`) to obtain the `&'static self` the
    /// watcher wiring needs without touching the global singleton.
    pub fn new() -> Self {
        Self::default()
    }

    /// Process-global singleton used by production wiring.
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<ScriptWatchRegistry> = OnceLock::new();
        INSTANCE.get_or_init(ScriptWatchRegistry::new)
    }

    /// Record the script paths a discovered route depends on.
    ///
    /// Staging is additive within one pass: a route id appearing in several
    /// documents (an impossible shape today, but cheap to keep total) keeps
    /// the union. Staged paths are invisible until [`Self::publish_commit`]
    /// promotes the route.
    pub fn stage(&self, route_id: &str, paths: BTreeSet<PathBuf>) {
        let mut staged = self.staged.lock().expect("script watch staged lock"); // allow-unwrap
        staged
            .entry(route_id.to_string())
            .or_default()
            .extend(paths);
    }

    /// Clear the staged set at the start of a discovery run.
    ///
    /// The published set is untouched: clearing staged state says nothing
    /// about which routes still run.
    pub fn begin_discovery_pass(&self) {
        let mut staged = self.staged.lock().expect("script watch staged lock"); // allow-unwrap
        staged.clear();
    }

    /// Publish a route's script paths immediately (file-backed loaders).
    ///
    /// `load_from_file` and `load_json_from_file` are single-document loads
    /// with no discovery/reload pass to publish later, so their routes are
    /// committed directly. An empty set removes any stale entry for the route.
    pub fn register_direct(&self, route_id: &str, paths: BTreeSet<PathBuf>) {
        let mut published = self.published.lock().expect("script watch published lock"); // allow-unwrap
        if paths.is_empty() {
            published.remove(route_id);
        } else {
            published.insert(route_id.to_string(), paths);
        }
    }

    /// Apply one reload pass outcome to the published set.
    ///
    /// - `rebuilt` route ids promote their staged paths (an empty staged set
    ///   for a rebuilt route drops its previously published paths, because the
    ///   rebuilt generation no longer references them).
    /// - `removed` route ids are deleted.
    /// - Every other route keeps its published entry unchanged, so a failed or
    ///   skipped route stays watched through its prior generation.
    pub fn publish_commit(&self, rebuilt: &[String], removed: &[String]) {
        let mut staged = self.staged.lock().expect("script watch staged lock"); // allow-unwrap
        let mut published = self.published.lock().expect("script watch published lock"); // allow-unwrap

        for route_id in rebuilt {
            // Absence means this pass staged nothing for the route (it was not
            // discovered from a document), which must not delete anything.
            if let Some(paths) = staged.remove(route_id) {
                if paths.is_empty() {
                    published.remove(route_id);
                } else {
                    published.insert(route_id.clone(), paths);
                }
            }
        }

        for route_id in removed {
            published.remove(route_id);
            staged.remove(route_id);
        }
    }

    /// Snapshot of the committed watch set, per route.
    #[must_use]
    pub fn route_watch_sources(&self) -> BTreeMap<String, BTreeSet<PathBuf>> {
        self.published
            .lock()
            .expect("script watch published lock") // allow-unwrap
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::ScriptWatchRegistry;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    /// A fresh, leaked registry so tests never observe the global singleton or
    /// each other's state.
    fn fresh_registry() -> &'static ScriptWatchRegistry {
        Box::leak(Box::new(ScriptWatchRegistry::new()))
    }

    fn paths(items: &[&str]) -> BTreeSet<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn publish_commit_promotes_staged_paths() {
        let reg = fresh_registry();
        reg.stage("r1", paths(&["a.rhai"]));

        reg.publish_commit(&["r1".to_string()], &[]);

        let sources = reg.route_watch_sources();
        assert_eq!(
            sources.get("r1"),
            Some(&paths(&["a.rhai"])),
            "a rebuilt route promotes its staged paths: {sources:?}"
        );
    }

    #[test]
    fn publish_commit_keeps_failed_route_paths() {
        let reg = fresh_registry();
        reg.register_direct("r1", paths(&["old.rhai"]));
        reg.stage("r1", paths(&["new.rhai"]));

        // Route neither rebuilt nor removed: failed or skipped.
        reg.publish_commit(&[], &[]);

        let sources = reg.route_watch_sources();
        assert_eq!(
            sources.get("r1"),
            Some(&paths(&["old.rhai"])),
            "a failed route keeps its previously published dependency: {sources:?}"
        );
    }

    #[test]
    fn staging_absence_never_deletes() {
        let reg = fresh_registry();
        reg.register_direct("r1", paths(&["x.rhai"]));

        reg.begin_discovery_pass();
        // Stage nothing at all for r1 this pass.
        reg.publish_commit(&[], &[]);

        let sources = reg.route_watch_sources();
        assert_eq!(
            sources.get("r1"),
            Some(&paths(&["x.rhai"])),
            "only an explicitly committed removal may delete a route: {sources:?}"
        );
    }

    #[test]
    fn publish_commit_drops_removed_routes() {
        let reg = fresh_registry();
        reg.register_direct("r1", paths(&["x.rhai"]));

        reg.publish_commit(&[], &["r1".to_string()]);

        let sources = reg.route_watch_sources();
        assert!(
            !sources.contains_key("r1"),
            "a committed removal deletes the route's published paths: {sources:?}"
        );
    }

    #[test]
    fn changed_script_path_publishes_new_target() {
        let reg = fresh_registry();
        reg.register_direct("r1", paths(&["old.rhai"]));
        reg.stage("r1", paths(&["new.rhai"]));

        reg.publish_commit(&["r1".to_string()], &[]);

        let sources = reg.route_watch_sources();
        assert_eq!(
            sources.get("r1"),
            Some(&paths(&["new.rhai"])),
            "a rebuilt route replaces (not merges) its published paths: {sources:?}"
        );
    }

    #[test]
    fn load_test_watch_set_contains_resolved_script_path() {
        let project = tempfile::tempdir().expect("project tempdir"); // allow-unwrap
        let routes_dir = project.path().join("routes");
        let scripts_dir = project.path().join("scripts");
        std::fs::create_dir_all(&routes_dir).expect("create routes dir"); // allow-unwrap
        std::fs::create_dir_all(&scripts_dir).expect("create scripts dir"); // allow-unwrap
        let script = scripts_dir.join("enrich.rhai");
        std::fs::write(&script, "headers[\"enriched\"] = \"yes\";").expect("write script"); // allow-unwrap
        let route_file = routes_dir.join("doc.yaml");
        std::fs::write(
            &route_file,
            "routes:\n  - id: script-watch-load-test-unit\n    from: \"direct:start\"\n    steps:\n      - script:\n          source_file: \"../scripts/enrich.rhai\"\n",
        )
        .expect("write route document"); // allow-unwrap

        let routes = crate::load_from_file(&route_file).expect("load route file"); // allow-unwrap
        assert_eq!(routes.len(), 1, "fixture declares exactly one route");

        let sources = ScriptWatchRegistry::global().route_watch_sources();
        let published = sources
            .get("script-watch-load-test-unit")
            .expect("load_from_file must publish the route's script dependency"); // allow-unwrap
        let canonical = std::fs::canonicalize(&script).expect("canonicalize script"); // allow-unwrap
        assert!(
            published.contains(&canonical),
            "resolved script path {canonical:?} must be published, got {published:?}"
        );
    }
}
