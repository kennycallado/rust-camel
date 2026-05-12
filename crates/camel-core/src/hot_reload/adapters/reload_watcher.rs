//! File-watching hot-reload loop.
//!
//! Watches YAML/JSON route files for changes and triggers pipeline reloads.
//!
//! # Circular dependency avoidance
//!
//! `camel-core` cannot depend on `camel-dsl` (which already depends on
//! `camel-core`). Route discovery is therefore injected via a closure:
//!
//! ```ignore
//! watch_and_reload(dirs, controller, || discover_routes(&patterns)).await?;
//! ```
//!
//! This keeps `camel-core` decoupled while letting `camel-cli` (which has
//! access to both) wire the pieces together.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use camel_api::CamelError;

use crate::context::RuntimeExecutionHandle;
use crate::hot_reload::application::reload::FunctionReloadContext;
use crate::hot_reload::application::{
    compute_reload_actions_from_runtime_snapshot, execute_reload_actions,
};
use crate::lifecycle::application::route_definition::RouteDefinition;

pub struct ReloadWatcher;

impl ReloadWatcher {
    pub async fn watch_and_reload<F>(
        watch_dirs: Vec<PathBuf>,
        controller: RuntimeExecutionHandle,
        discover_fn: F,
        shutdown: Option<CancellationToken>,
        drain_timeout: Duration,
        debounce: Duration,
    ) -> Result<(), CamelError>
    where
        F: Fn() -> Result<Vec<RouteDefinition>, CamelError> + Send + 'static,
    {
        watch_and_reload(
            watch_dirs,
            controller,
            discover_fn,
            shutdown,
            drain_timeout,
            debounce,
        )
        .await
    }

    pub fn resolve_watch_dirs(patterns: &[String]) -> Vec<PathBuf> {
        resolve_watch_dirs(patterns)
    }
}

/// Watch directories and reload pipelines when YAML files change.
///
/// * `watch_dirs` — directories to monitor (non-recursive watch on each).
/// * `controller` — live route controller to apply reload actions to.
/// * `discover_fn` — called after each debounced file-change event to load
///   the current set of route definitions from disk.
/// * `shutdown` — optional token; cancel it to stop the watcher gracefully.
///
/// The function runs indefinitely until the channel is closed, the shutdown
/// token is cancelled, or a fatal initialisation error occurs.
///
/// # Errors
///
/// Returns `Err` only on fatal initialisation failure (watcher cannot be
/// created, or a directory cannot be watched). Per-route reload errors are
/// logged as warnings and do not terminate the loop.
pub async fn watch_and_reload<F>(
    watch_dirs: Vec<PathBuf>,
    controller: RuntimeExecutionHandle,
    discover_fn: F,
    shutdown: Option<CancellationToken>,
    drain_timeout: Duration,
    debounce: Duration,
) -> Result<(), CamelError>
where
    F: Fn() -> Result<Vec<RouteDefinition>, CamelError> + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(64);

    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.blocking_send(res);
        },
        notify::Config::default(),
    )
    .map_err(|e| CamelError::RouteError(format!("Failed to create file watcher: {e}")))?;

    if watch_dirs.is_empty() {
        tracing::warn!("hot-reload: no directories to watch");
    }

    for dir in &watch_dirs {
        watcher.watch(dir, RecursiveMode::Recursive).map_err(|e| {
            CamelError::RouteError(format!("Failed to watch directory {dir:?}: {e}"))
        })?;
        tracing::info!("hot-reload: watching {:?}", dir);
    }

    // Debounce duration to coalesce rapid successive file events from editors
    // (e.g., when saving a file, multiple events may fire in quick succession).
    // debounce duration is passed by the caller (configured via CamelConfig.watch_debounce_ms)

    // Helper: check if shutdown was requested
    let is_cancelled = || shutdown.as_ref().map(|t| t.is_cancelled()).unwrap_or(false);

    loop {
        if is_cancelled() {
            tracing::info!("hot-reload: shutdown requested — stopping watcher");
            return Ok(());
        }

        // Wait for the first relevant event.
        let triggered = loop {
            // Select between an incoming event and a cancellation signal.
            let recv_fut = rx.recv();
            if let Some(token) = shutdown.as_ref() {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        tracing::info!("hot-reload: shutdown requested — stopping watcher");
                        return Ok(());
                    }
                    msg = recv_fut => {
                        match msg {
                            None => return Ok(()), // channel closed
                            Some(Err(e)) => {
                                tracing::warn!("hot-reload: watcher error: {e}");
                                continue;
                            }
                            Some(Ok(event)) => {
                                if is_reload_event(&event) {
                                    break true;
                                }
                            }
                        }
                    }
                }
            } else {
                match recv_fut.await {
                    None => return Ok(()), // channel closed — watcher dropped
                    Some(Err(e)) => {
                        tracing::warn!("hot-reload: watcher error: {e}");
                        continue;
                    }
                    Some(Ok(event)) => {
                        if is_reload_event(&event) {
                            break true;
                        }
                    }
                }
            }
        };

        if !triggered {
            continue;
        }

        // Drain further events within the debounce window.
        let deadline = tokio::time::Instant::now() + debounce;
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(_)) => continue,   // consume
                Ok(None) => return Ok(()), // channel closed
                Err(_elapsed) => break,    // debounce window expired
            }
        }
        tracing::info!("hot-reload: file change detected — reloading routes");

        // Discover new definitions from disk.
        let new_defs = match discover_fn() {
            Ok(defs) => defs,
            Err(e) => {
                tracing::warn!("hot-reload: route discovery failed: {e}");
                continue;
            }
        };

        // Compute the diff from runtime projection route IDs only.
        let runtime_route_ids = match controller.runtime_route_ids().await {
            Ok(route_ids) => route_ids,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "hot-reload: failed to list runtime routes; skipping this reload cycle"
                );
                continue;
            }
        };
        let mut runtime_hashes: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        for id in &runtime_route_ids {
            if let Some(hash) = controller.route_source_hash(id).await {
                runtime_hashes.insert(id.clone(), hash);
            }
        }

        let actions = compute_reload_actions_from_runtime_snapshot(
            &new_defs,
            &runtime_route_ids,
            &|route_id: &str| runtime_hashes.get(route_id).copied(),
        );

        if actions.is_empty() {
            tracing::debug!("hot-reload: no route changes detected");
            continue;
        }

        tracing::info!("hot-reload: applying {} reload action(s)", actions.len());

        let function_ctx = controller.function_invoker().map(|invoker| {
            let generation = invoker.begin_reload();
            FunctionReloadContext {
                invoker,
                generation,
            }
        });
        let errors = execute_reload_actions(
            actions,
            new_defs,
            &controller,
            drain_timeout,
            function_ctx.as_ref(),
        )
        .await;
        for err in &errors {
            tracing::warn!(
                "hot-reload: error on route '{}' ({}): {}",
                err.route_id,
                err.action,
                err.error
            );
        }
    }
}

/// Returns `true` if this notify event should trigger a reload.
///
/// Only events affecting `.yaml`, `.yml`, or `.json` files are considered, to avoid
/// triggering reloads on editor swap files (`.swp`, `~`, `.tmp`, etc.).
fn is_reload_event(event: &Event) -> bool {
    let has_yaml = event.paths.iter().any(|p| {
        p.extension()
            .map(|e| e == "yaml" || e == "yml" || e == "json")
            .unwrap_or(false)
    });
    has_yaml
        && matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        )
}

/// Extract unique existing parent directories from glob patterns.
///
/// Given patterns like `"routes/*.yaml"` or `"/abs/**/*.yaml"`, walks up
/// from each path until the first non-glob segment and collects the
/// resulting existing directories.
pub fn resolve_watch_dirs(patterns: &[String]) -> Vec<PathBuf> {
    let mut dirs: HashSet<PathBuf> = HashSet::new();

    for pattern in patterns {
        let path = Path::new(pattern.as_str());

        let dir = if path.is_dir() {
            path.to_path_buf()
        } else {
            // Walk toward root until no glob characters remain.
            let mut candidate = path;
            loop {
                let s = candidate.to_string_lossy();
                if s.contains('*') || s.contains('?') || s.contains('[') {
                    match candidate.parent() {
                        Some(p) => candidate = p,
                        None => break,
                    }
                } else {
                    break;
                }
            }
            candidate.to_path_buf()
        };

        if dir.as_os_str().is_empty() {
            // Relative pattern with no parent — watch current dir
            dirs.insert(PathBuf::from("."));
        } else if dir.exists() {
            dirs.insert(dir);
        }
    }

    dirs.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use notify::{Event, EventKind};
    use tokio_util::sync::CancellationToken;

    use crate::CamelContext;

    use super::{ReloadWatcher, is_reload_event, resolve_watch_dirs, watch_and_reload};

    fn make_event(paths: &[&str], kind: EventKind) -> Event {
        Event {
            kind,
            paths: paths.iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        }
    }

    #[test]
    fn is_reload_event_accepts_yaml() {
        let ev = make_event(
            &["routes/test.yaml"],
            EventKind::Create(notify::event::CreateKind::File),
        );
        assert!(is_reload_event(&ev));
    }

    #[test]
    fn is_reload_event_accepts_yml() {
        let ev = make_event(
            &["routes/test.yml"],
            EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
        );
        assert!(is_reload_event(&ev));
    }

    #[test]
    fn is_reload_event_accepts_json() {
        let ev = make_event(
            &["routes/test.json"],
            EventKind::Create(notify::event::CreateKind::File),
        );
        assert!(is_reload_event(&ev));
    }

    #[test]
    fn is_reload_event_rejects_other_extensions() {
        let ev = make_event(
            &["routes/test.toml"],
            EventKind::Create(notify::event::CreateKind::File),
        );
        assert!(!is_reload_event(&ev));
    }

    #[test]
    fn is_reload_event_rejects_swap_files() {
        let ev = make_event(
            &["routes/.test.yaml.swp"],
            EventKind::Create(notify::event::CreateKind::File),
        );
        assert!(!is_reload_event(&ev));
    }

    #[test]
    fn is_reload_event_accepts_remove_kind() {
        let ev = make_event(
            &["routes/test.yaml"],
            EventKind::Remove(notify::event::RemoveKind::File),
        );
        assert!(is_reload_event(&ev));
    }

    #[test]
    fn is_reload_event_rejects_non_reload_kind_even_with_yaml() {
        let ev = make_event(
            &["routes/test.yaml"],
            EventKind::Access(notify::event::AccessKind::Read),
        );
        assert!(!is_reload_event(&ev));
    }

    #[test]
    fn is_reload_event_accepts_when_any_path_matches() {
        let ev = make_event(
            &["routes/test.tmp", "routes/real.json"],
            EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
        );
        assert!(is_reload_event(&ev));
    }

    #[test]
    fn resolve_watch_dirs_returns_existing_dir_pattern_and_ignores_missing() {
        let root = std::env::temp_dir().join(format!(
            "camel-reload-watch-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before unix epoch")
                .as_nanos()
        ));
        let existing = root.join("routes");
        fs::create_dir_all(&existing).expect("create temp dir");

        let patterns = vec![
            existing.to_string_lossy().to_string(),
            root.join("missing").to_string_lossy().to_string(),
        ];

        let dirs = resolve_watch_dirs(&patterns);

        assert!(dirs.contains(&existing));
        assert_eq!(dirs.len(), 1);

        fs::remove_dir_all(&root).expect("cleanup temp dir");
    }

    #[test]
    fn resolve_watch_dirs_walks_up_glob_segments() {
        let root = std::env::temp_dir().join(format!(
            "camel-reload-watch-glob-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before unix epoch")
                .as_nanos()
        ));
        let nested = root.join("a").join("b");
        fs::create_dir_all(&nested).expect("create nested temp dir");

        let pattern = nested
            .join("**")
            .join("*.yaml")
            .to_string_lossy()
            .to_string();

        let dirs = resolve_watch_dirs(&[pattern]);

        assert!(dirs.contains(&nested));

        fs::remove_dir_all(&root).expect("cleanup temp dir");
    }

    #[test]
    fn resolve_watch_dirs_uses_current_dir_for_relative_no_parent_pattern() {
        let dirs = resolve_watch_dirs(&["*.yaml".to_string()]);
        assert_eq!(dirs, vec![PathBuf::from(".")]);
    }

    #[tokio::test]
    async fn watch_and_reload_returns_immediately_when_shutdown_is_cancelled() {
        let token = CancellationToken::new();
        token.cancel();

        let ctx = CamelContext::builder().build().await.unwrap();
        let result = watch_and_reload(
            vec![],
            ctx.runtime_execution_handle(),
            || Ok(vec![]),
            Some(token),
            Duration::from_millis(1),
            Duration::from_millis(1),
        )
        .await;

        assert!(result.is_ok());
    }

    #[test]
    fn is_reload_event_rejects_no_extension() {
        let ev = make_event(
            &["routes/Makefile"],
            EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
        );
        assert!(!is_reload_event(&ev));
    }

    #[test]
    fn is_reload_event_rejects_any_event_with_no_matching_kind() {
        let ev = make_event(&["routes/test.yaml"], EventKind::Other);
        assert!(!is_reload_event(&ev));
    }

    #[test]
    fn is_reload_event_rejects_empty_paths() {
        let ev = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![],
            attrs: Default::default(),
        };
        assert!(!is_reload_event(&ev));
    }

    #[test]
    fn resolve_watch_dirs_deduplicates_same_directory() {
        let root = std::env::temp_dir().join(format!(
            "camel-reload-watch-dedup-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp dir");

        let patterns = vec![
            root.join("*.yaml").to_string_lossy().to_string(),
            root.join("*.yml").to_string_lossy().to_string(),
        ];

        let dirs = resolve_watch_dirs(&patterns);
        assert_eq!(dirs.len(), 1);
        assert!(dirs.contains(&root));

        fs::remove_dir_all(&root).expect("cleanup temp dir");
    }

    #[test]
    fn resolve_watch_dirs_handles_absolute_glob_pattern() {
        let root = std::env::temp_dir().join(format!(
            "camel-reload-watch-abs-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before unix epoch")
                .as_nanos()
        ));
        let nested = root.join("routes").join("sub");
        fs::create_dir_all(&nested).expect("create nested temp dir");

        let pattern = root
            .join("routes")
            .join("**")
            .join("*.yaml")
            .to_string_lossy()
            .to_string();

        let dirs = resolve_watch_dirs(&[pattern]);
        assert!(dirs.iter().any(|d| d.starts_with(root.join("routes"))));

        fs::remove_dir_all(&root).expect("cleanup temp dir");
    }

    #[test]
    fn resolve_watch_dirs_empty_patterns_returns_empty() {
        let dirs = resolve_watch_dirs(&[]);
        assert!(dirs.is_empty());
    }

    #[test]
    fn resolve_watch_dirs_ignores_nonexistent_parent_of_glob() {
        let dirs = resolve_watch_dirs(&["/nonexistent/path/**/*.yaml".to_string()]);
        assert!(dirs.is_empty());
    }

    #[test]
    fn reload_watcher_struct_is_unit() {
        let _ = ReloadWatcher;
    }
}
