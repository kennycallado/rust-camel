//! File-watching hot-reload loop.
//!
//! Watches YAML route files for changes and triggers pipeline reloads.
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
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| {
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
        let actions = compute_reload_actions_from_runtime_snapshot(&new_defs, &runtime_route_ids);

        if actions.is_empty() {
            tracing::debug!("hot-reload: no route changes detected");
            continue;
        }

        tracing::info!("hot-reload: applying {} reload action(s)", actions.len());

        let errors = execute_reload_actions(actions, new_defs, &controller, drain_timeout).await;
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
/// Only events affecting `.yaml` or `.yml` files are considered, to avoid
/// triggering reloads on editor swap files (`.swp`, `~`, `.tmp`, etc.).
fn is_reload_event(event: &Event) -> bool {
    let has_yaml = event.paths.iter().any(|p| {
        p.extension()
            .map(|e| e == "yaml" || e == "yml")
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
