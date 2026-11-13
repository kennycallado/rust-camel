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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use camel_api::{CamelError, RuntimeCommand};
use camel_component_api::template_reload::TemplateReloadRegistry;

use crate::context::RuntimeExecutionHandle;
use crate::hot_reload::application::reload::{FunctionReloadContext, ReloadError};
use crate::hot_reload::application::{
    compute_reload_actions_from_runtime_snapshot, execute_reload_actions,
};
use crate::hot_reload::domain::ReloadAction;
use crate::lifecycle::application::route_definition::RouteDefinition;

pub struct ReloadWatcher;

impl ReloadWatcher {
    #[allow(clippy::too_many_arguments)]
    pub async fn watch_and_reload<F, D, P>(
        watch_dirs: Vec<PathBuf>,
        controller: RuntimeExecutionHandle,
        discover_fn: F,
        dev_provider: D,
        publish_script_sources: P,
        shutdown: Option<CancellationToken>,
        drain_timeout: Duration,
        debounce: Duration,
    ) -> Result<(), CamelError>
    where
        F: Fn() -> Result<Vec<RouteDefinition>, CamelError> + Send + Sync + 'static,
        D: Fn() -> Vec<(String, Vec<(PathBuf, DevSourceKind)>)> + Send + Sync + 'static,
        P: Fn(&[String], &[String]) + Send + Sync + 'static,
    {
        watch_and_reload(
            watch_dirs,
            controller,
            discover_fn,
            dev_provider,
            publish_script_sources,
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

/// Watch directories and reload pipelines when route files or dev-target
/// sources change.
///
/// * `watch_dirs` — route root directories to monitor recursively.
/// * `controller` — live route controller to apply reload actions to.
/// * `discover_fn` — called after each debounced route-file event to load
///   the current set of route definitions from disk.
/// * `dev_provider` — synchronous snapshot of committed dev targets as
///   `(route_id, [(path, kind)])` pairs. Parent directories of every source are
///   watched (non-recursively) so asset edits can trigger a reload: template
///   sources dispatch a per-route template reload, script sources trigger a
///   full reload pass.
/// * `publish_script_sources` — called once after every non-aborted reload pass
///   with the route ids rebuilt and removed by it. The script watch registry
///   promotes staged dependencies for rebuilt routes and drops removed ones.
/// * `shutdown` — optional token; cancel it to stop the watcher gracefully.
///
/// The function runs indefinitely until the channel is closed, the shutdown
/// token is cancelled, or a fatal initialisation error occurs.
///
/// # Errors
///
/// Returns `Err` only on fatal initialisation failure (watcher cannot be
/// created, or a route directory cannot be watched). Per-route reload errors
/// are logged as warnings and do not terminate the loop.
#[allow(clippy::too_many_arguments)]
pub async fn watch_and_reload<F, D, P>(
    watch_dirs: Vec<PathBuf>,
    controller: RuntimeExecutionHandle,
    discover_fn: F,
    dev_provider: D,
    publish_script_sources: P,
    shutdown: Option<CancellationToken>,
    drain_timeout: Duration,
    debounce: Duration,
) -> Result<(), CamelError>
where
    F: Fn() -> Result<Vec<RouteDefinition>, CamelError> + Send + Sync + 'static,
    D: Fn() -> Vec<(String, Vec<(PathBuf, DevSourceKind)>)> + Send + Sync + 'static,
    P: Fn(&[String], &[String]) + Send + Sync + 'static,
{
    watch_and_reload_inner(
        watch_dirs,
        controller,
        discover_fn,
        dev_provider,
        publish_script_sources,
        shutdown,
        drain_timeout,
        debounce,
        None,
        None,
    )
    .await
}

/// Test-only entry point that exposes the watcher readiness and subscription
/// reconciliation barriers. Production callers use [`watch_and_reload`].
///
/// Available to this crate's unit tests and to downstream integration tests
/// that enable the `test-support` feature. The readiness signal fires after
/// the initial dev-target subscriptions and reconciliation are applied, so a
/// real-filesystem test can await it before its first write.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn watch_and_reload_with_barriers<F, D, P>(
    watch_dirs: Vec<PathBuf>,
    controller: RuntimeExecutionHandle,
    discover_fn: F,
    dev_provider: D,
    publish_script_sources: P,
    shutdown: Option<CancellationToken>,
    drain_timeout: Duration,
    debounce: Duration,
    ready_tx: watch::Sender<bool>,
    reconciliation_tx: watch::Sender<u64>,
) -> Result<(), CamelError>
where
    F: Fn() -> Result<Vec<RouteDefinition>, CamelError> + Send + Sync + 'static,
    D: Fn() -> Vec<(String, Vec<(PathBuf, DevSourceKind)>)> + Send + Sync + 'static,
    P: Fn(&[String], &[String]) + Send + Sync + 'static,
{
    watch_and_reload_inner(
        watch_dirs,
        controller,
        discover_fn,
        dev_provider,
        publish_script_sources,
        shutdown,
        drain_timeout,
        debounce,
        Some(ready_tx),
        Some(reconciliation_tx),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn watch_and_reload_inner<F, D, P>(
    watch_dirs: Vec<PathBuf>,
    controller: RuntimeExecutionHandle,
    discover_fn: F,
    dev_provider: D,
    publish_script_sources: P,
    shutdown: Option<CancellationToken>,
    drain_timeout: Duration,
    debounce: Duration,
    ready_tx: Option<watch::Sender<bool>>,
    reconciliation_tx: Option<watch::Sender<u64>>,
) -> Result<(), CamelError>
where
    F: Fn() -> Result<Vec<RouteDefinition>, CamelError> + Send + Sync + 'static,
    D: Fn() -> Vec<(String, Vec<(PathBuf, DevSourceKind)>)> + Send + Sync + 'static,
    P: Fn(&[String], &[String]) + Send + Sync + 'static,
{
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>(64);

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

    let route_roots: Vec<PathBuf> = watch_dirs.iter().map(|dir| normalize_path(dir)).collect();

    for dir in &watch_dirs {
        watcher.watch(dir, RecursiveMode::Recursive).map_err(|e| {
            CamelError::RouteError(format!("Failed to watch directory {dir:?}: {e}"))
        })?;
        tracing::info!("hot-reload: watching {:?}", dir);
    }

    // Initial dev-target parent subscription. Reconciling from an empty set
    // keeps the initial pass and every later pass on one code path.
    let initial_map = DevTargetMap::from_sources(&dev_provider());
    let mut subscribed: HashSet<PathBuf> = HashSet::new();
    let initial_parents = wanted_dev_parents(&initial_map, &route_roots);
    reconcile_subscriptions(&mut watcher, &mut subscribed, initial_parents.clone());

    // Readiness barrier: signal only after the initial subscriptions AND the
    // initial reconciliation are applied. Real-fs tests await this before any
    // write, so no edit can precede a subscription.
    if let Some(tx) = &reconciliation_tx {
        let _ = tx.send(1);
    }
    if let Some(tx) = &ready_tx {
        let _ = tx.send(true);
    }

    // Re-derivation channel: `run_reload_loop` publishes the wanted parent
    // dirs after every re-derivation; the reconciliation loop below applies
    // them to the watcher.
    let (parents_tx, parents_rx) = watch::channel(initial_parents);

    // Route-file reload pass: discovery + diff + action execution. Returns the
    // ids rebuilt and removed by this pass; early-continue paths abort.
    let discover = Arc::new(discover_fn);
    let reload_pass = {
        let controller = controller.clone();
        move || -> BoxFuture<'static, Result<PassOutcome, CamelError>> {
            let controller = controller.clone();
            let discover = Arc::clone(&discover);
            Box::pin(async move {
                let new_defs = match discover() {
                    Ok(defs) => defs,
                    Err(e) => {
                        tracing::warn!("hot-reload: route discovery failed: {e}");
                        return Ok(PassOutcome::aborted_pass());
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
                        return Ok(PassOutcome::aborted_pass());
                    }
                };
                let mut runtime_hashes: HashMap<String, u64> = HashMap::new();
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
                    return Ok(PassOutcome::default());
                }

                tracing::info!("hot-reload: applying {} reload action(s)", actions.len());

                let function_ctx = controller.function_invoker().map(|invoker| {
                    let generation = invoker.begin_reload();
                    FunctionReloadContext {
                        invoker,
                        generation,
                    }
                });
                // Only successfully rebuilt routes are covered by this pass.
                // Skip and Remove do not rebuild, and failed actions leave the
                // prior generation in place, so their assets still need a
                // template dispatch.
                let actions_for_derivation = actions.clone();
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
                Ok(PassOutcome {
                    rebuilt: derive_rebuilt_route_ids(&actions_for_derivation, &errors),
                    removed: derive_removed_route_ids(&actions_for_derivation, &errors),
                    aborted: false,
                })
            })
        }
    };

    // Per-route template reload dispatch, bounded by the registry deadline plus
    // the route-drain timeout.
    let dispatch_reload = {
        let controller = controller.clone();
        let shutdown = shutdown.clone();
        move |route_id: &str| -> BoxFuture<'static, Result<(), CamelError>> {
            let controller = controller.clone();
            let shutdown = shutdown.clone();
            let route_id = route_id.to_string();
            Box::pin(async move {
                let bound = reload_dispatch_bound(&route_id, drain_timeout);
                let command = RuntimeCommand::ReloadTemplates {
                    route_id: route_id.clone(),
                    command_id: next_reload_command_id(&route_id),
                    causation_id: None,
                };
                bounded_dispatch(
                    async move {
                        controller
                            .execute_runtime_command(command)
                            .await
                            .map(|_result| ())
                    },
                    bound,
                    shutdown.as_ref(),
                )
                .await
            })
        }
    };

    // Re-derive the dev-target map from the registry on every batch.
    let dev_map_provider = move || DevTargetMap::from_sources(&dev_provider());

    let reload_loop = run_reload_loop(
        rx,
        watch_dirs,
        controller,
        &dev_map_provider,
        &reload_pass,
        &publish_script_sources,
        &dispatch_reload,
        parents_tx,
        shutdown.clone(),
        debounce,
    );
    let reconciliation = run_subscription_reconciliation(
        watcher,
        subscribed,
        parents_rx,
        reconciliation_tx,
        shutdown,
    );
    tokio::try_join!(reload_loop, reconciliation)?;
    Ok(())
}

/// Parent directories the watcher must subscribe for a dev-target snapshot.
///
/// Paths are normalized and sorted (deterministic output), and directories
/// already covered by a recursive route-root watch are removed.
fn wanted_dev_parents(map: &DevTargetMap, route_roots: &[PathBuf]) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> = route_roots
        .iter()
        .map(|root| normalize_path(root))
        .collect();
    let mut parents: Vec<PathBuf> = map
        .parent_dirs()
        .into_iter()
        .map(|parent| normalize_path(&parent))
        .filter(|parent| !roots.iter().any(|root| parent.starts_with(root)))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    parents.sort();
    parents
}

/// Publish a parent-dir snapshot after every re-derivation.
///
/// Unchanged snapshots are published too: reconciliation is where failed
/// best-effort subscriptions get retried, so suppressing a snapshot would also
/// suppress those retries.
fn publish_parents(tx: &watch::Sender<Vec<PathBuf>>, map: &DevTargetMap, route_roots: &[PathBuf]) {
    let _ = tx.send(wanted_dev_parents(map, route_roots));
}

/// Add missing dev-target parent subscriptions and (best-effort) drop stale
/// ones. Subscription errors are logged, not fatal: the watcher keeps running
/// with whatever subscriptions it has.
///
/// The `subscribed` set records only *successful* operations, so a failed add
/// is retried on the next reconciliation (the dir is not in the set) and a
/// failed drop is retried too (the dir stays in the set). Successful operations
/// are skipped, which keeps repeated reconciliation cheap.
fn reconcile_subscriptions(
    watcher: &mut RecommendedWatcher,
    subscribed: &mut HashSet<PathBuf>,
    wanted: Vec<PathBuf>,
) {
    let wanted_set: HashSet<&Path> = wanted.iter().map(PathBuf::as_path).collect();

    for dir in &wanted {
        if subscribed.contains(dir) {
            continue;
        }
        match watcher.watch(dir, RecursiveMode::NonRecursive) {
            Ok(()) => {
                subscribed.insert(dir.clone());
                tracing::info!("hot-reload: watching dev-target dir {:?}", dir);
            }
            Err(e) => {
                tracing::warn!("hot-reload: failed to watch dev-target dir {dir:?}: {e}")
            }
        }
    }

    let stale: Vec<PathBuf> = subscribed
        .iter()
        .filter(|dir| !wanted_set.contains(dir.as_path()))
        .cloned()
        .collect();
    for dir in stale {
        match watcher.unwatch(&dir) {
            Ok(()) => {
                subscribed.remove(&dir);
                tracing::debug!("hot-reload: dropped dev-target subscription {:?}", dir);
            }
            Err(e) => {
                tracing::warn!("hot-reload: failed to drop dev-target dir {dir:?}: {e}")
            }
        }
    }
}

/// Apply every parent-dir snapshot published by [`run_reload_loop`].
async fn run_subscription_reconciliation(
    mut watcher: RecommendedWatcher,
    mut subscribed: HashSet<PathBuf>,
    mut parents_rx: watch::Receiver<Vec<PathBuf>>,
    reconciliation_tx: Option<watch::Sender<u64>>,
    shutdown: Option<CancellationToken>,
) -> Result<(), CamelError> {
    loop {
        let changed = if let Some(token) = shutdown.as_ref() {
            tokio::select! {
                biased;
                _ = token.cancelled() => return Ok(()),
                changed = parents_rx.changed() => changed,
            }
        } else {
            parents_rx.changed().await
        };

        if changed.is_err() {
            return Ok(());
        }

        let wanted = parents_rx.borrow_and_update().clone();
        reconcile_subscriptions(&mut watcher, &mut subscribed, wanted);
        if let Some(tx) = &reconciliation_tx {
            let next = *tx.borrow() + 1;
            let _ = tx.send(next);
        }
    }
}

/// Sequence for template-reload command ids, mirroring the context lifecycle
/// command-id pattern (`context:{op}:{route_id}:{boot_nonce}:{seq}`).
static RELOAD_COMMAND_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_reload_command_id(route_id: &str) -> String {
    let seq = RELOAD_COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("hot-reload:reload-templates:{route_id}:{seq}")
}

/// Dispatch bound for a template reload: the tightest registry deadline for
/// `route_id` plus the route-drain timeout.
fn reload_dispatch_bound(route_id: &str, drain_timeout: Duration) -> Duration {
    let registry_deadline = TemplateReloadRegistry::global()
        .find_all(route_id)
        .iter()
        .map(|target| target.reload_timeout())
        .min()
        .unwrap_or_else(|| Duration::from_millis(5000));
    registry_deadline + drain_timeout
}

/// Run a dispatch future under a hard time bound, biased against shutdown.
///
/// On timeout or cancellation the future is dropped, so a slow reload can
/// never commit late. A cancelled shutdown is reported with the same
/// `TemplateReload("dispatch bound elapsed")` error as a timeout — the caller
/// logs and continues; the outer loop observes the cancel on its next
/// iteration and stops.
pub(crate) async fn bounded_dispatch<F>(
    fut: F,
    bound: Duration,
    shutdown: Option<&CancellationToken>,
) -> Result<(), CamelError>
where
    F: std::future::Future<Output = Result<(), CamelError>>,
{
    let timed = tokio::time::timeout(bound, fut);
    match shutdown {
        Some(token) => {
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    Err(CamelError::TemplateReload("dispatch bound elapsed".to_string()))
                }
                result = timed => match result {
                    Ok(inner) => inner,
                    Err(_elapsed) => {
                        Err(CamelError::TemplateReload("dispatch bound elapsed".to_string()))
                    }
                },
            }
        }
        None => match timed.await {
            Ok(inner) => inner,
            Err(_elapsed) => Err(CamelError::TemplateReload(
                "dispatch bound elapsed".to_string(),
            )),
        },
    }
}

/// Outcome of one route-file reload pass.
///
/// `rebuilt` drives template dispatch suppression: a rebuilt route's assets are
/// covered by the pass. `removed` lists routes whose `Remove` action executed
/// without error, so the script watch registry can drop them. `aborted` marks a
/// pass that ended before computing a diff (discovery or runtime snapshot
/// failure); an aborted pass must not be published, because it says nothing
/// about which routes still run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PassOutcome {
    /// Route ids successfully rebuilt by this pass.
    pub(crate) rebuilt: Vec<String>,
    /// Route ids removed by this pass (successful `Remove` actions only).
    pub(crate) removed: Vec<String>,
    /// `true` when the pass ended early without a diff.
    pub(crate) aborted: bool,
}

impl PassOutcome {
    /// An aborted pass: no diff was computed, so nothing may be published.
    fn aborted_pass() -> Self {
        Self {
            rebuilt: Vec::new(),
            removed: Vec::new(),
            aborted: true,
        }
    }
}

/// Route ids successfully rebuilt by one reload pass.
///
/// `Add`, `Swap` and `Restart` rebuild the route, so their ids are candidates.
/// `Skip` and `Remove` do not rebuild anything. A candidate whose action
/// reported an error is excluded because the failed action left the prior
/// generation in place — its assets still need a template dispatch.
pub(crate) fn derive_rebuilt_route_ids(
    actions: &[ReloadAction],
    errors: &[ReloadError],
) -> Vec<String> {
    let failed: HashSet<&str> = errors.iter().map(|err| err.route_id.as_str()).collect();
    actions
        .iter()
        .filter_map(|action| match action {
            ReloadAction::Swap { route_id }
            | ReloadAction::Restart { route_id }
            | ReloadAction::Add { route_id } => Some(route_id),
            ReloadAction::Skip { .. } | ReloadAction::Remove { .. } => None,
        })
        .filter(|route_id| !failed.contains(route_id.as_str()))
        .cloned()
        .collect()
}

/// Route ids removed by one reload pass.
///
/// Only `Remove` actions count. A `Remove` that reported an error did not stop
/// the route, so it keeps running and its script targets must be retained.
pub(crate) fn derive_removed_route_ids(
    actions: &[ReloadAction],
    errors: &[ReloadError],
) -> Vec<String> {
    let failed: HashSet<&str> = errors.iter().map(|err| err.route_id.as_str()).collect();
    actions
        .iter()
        .filter_map(|action| match action {
            ReloadAction::Remove { route_id } => Some(route_id),
            ReloadAction::Swap { .. }
            | ReloadAction::Restart { .. }
            | ReloadAction::Add { .. }
            | ReloadAction::Skip { .. } => None,
        })
        .filter(|route_id| !failed.contains(route_id.as_str()))
        .cloned()
        .collect()
}

/// Debounced watch loop extracted from [`watch_and_reload`] so tests can drive
/// it with a synthetic event source.
///
/// The loop classifies each debounced batch: route files and script sources run
/// `reload_pass` once, template edits dispatch a per-route template reload, and
/// unrelated events are ignored. A successful, non-aborted pass publishes its
/// [`PassOutcome`] through `publish_script_sources` so the script watch
/// registry can promote or drop per-route script dependencies.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_reload_loop(
    mut event_rx: mpsc::Receiver<notify::Result<Event>>,
    route_roots: Vec<PathBuf>,
    _controller: RuntimeExecutionHandle,
    dev_provider: &(dyn Fn() -> DevTargetMap + Send + Sync),
    reload_pass: &(dyn Fn() -> BoxFuture<'static, Result<PassOutcome, CamelError>> + Send + Sync),
    publish_script_sources: &(dyn Fn(&[String], &[String]) + Send + Sync),
    dispatch_reload: &(dyn Fn(&str) -> BoxFuture<'static, Result<(), CamelError>> + Send + Sync),
    parents_tx: watch::Sender<Vec<PathBuf>>,
    shutdown: Option<CancellationToken>,
    debounce: Duration,
) -> Result<(), CamelError> {
    let is_cancelled = || shutdown.as_ref().map(|t| t.is_cancelled()).unwrap_or(false);
    let mut dev_map = dev_provider();

    loop {
        if is_cancelled() {
            tracing::debug!("hot-reload: shutdown requested — stopping watcher");
            return Ok(());
        }

        // Wait for the first classified event. The select only produces the
        // next message; loop control stays outside so `break`/`continue` are
        // never captured by the select macro.
        let mut batch: Vec<(EventClass, Event)> = loop {
            let msg = if let Some(token) = shutdown.as_ref() {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        tracing::debug!("hot-reload: shutdown requested — stopping watcher");
                        return Ok(());
                    }
                    msg = event_rx.recv() => msg,
                }
            } else {
                event_rx.recv().await
            };

            match msg {
                None => return Ok(()), // channel closed — watcher dropped
                Some(Err(e)) => {
                    tracing::warn!("hot-reload: watcher error: {e}");
                }
                Some(Ok(event)) => {
                    if let Some(class) = classify_event(&event, &dev_map, &route_roots) {
                        break vec![(class, event)];
                    }
                }
            }
        };

        // Drain further events within the debounce window.
        let deadline = tokio::time::Instant::now() + debounce;
        loop {
            let received = if let Some(token) = shutdown.as_ref() {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        tracing::debug!("hot-reload: shutdown requested — stopping watcher");
                        return Ok(());
                    }
                    result = tokio::time::timeout_at(deadline, event_rx.recv()) => result,
                }
            } else {
                tokio::time::timeout_at(deadline, event_rx.recv()).await
            };

            match received {
                Ok(Some(Ok(event))) => {
                    if let Some(class) = classify_event(&event, &dev_map, &route_roots) {
                        batch.push((class, event));
                    }
                }
                Ok(Some(Err(e))) => tracing::warn!("hot-reload: watcher error: {e}"),
                Ok(None) => return Ok(()), // channel closed
                Err(_elapsed) => break,    // debounce window expired
            }
        }

        let has_full_pass_trigger = batch
            .iter()
            .any(|(class, _)| matches!(*class, EventClass::RouteFile | EventClass::ScriptSource));
        let mut pending: BTreeSet<String> = BTreeSet::new();
        for (class, event) in &batch {
            if *class == EventClass::DevTarget {
                for path in &event.paths {
                    pending.extend(dev_map.routes_for(path));
                }
            }
        }

        let mut rebuilt: HashSet<String> = HashSet::new();
        if has_full_pass_trigger {
            tracing::info!("hot-reload: file change detected — reloading routes");
            match reload_pass().await {
                Ok(outcome) => {
                    rebuilt = outcome.rebuilt.iter().cloned().collect();
                    // An aborted pass computed no diff, so it says nothing
                    // about which routes still run: publishing it would let the
                    // registry react to a phantom empty outcome.
                    if !outcome.aborted {
                        publish_script_sources(&outcome.rebuilt, &outcome.removed);
                    }
                }
                Err(err) => tracing::warn!("hot-reload: reload pass failed: {err}"),
            }
            // Re-derive after the route pass: a rebuilt route's assets are
            // covered by the pass; surviving target routes still dispatch.
            dev_map = dev_provider();
            publish_parents(&parents_tx, &dev_map, &route_roots);
        } else {
            tracing::info!(
                "hot-reload: dev target change detected — dispatching {} target route(s)",
                pending.len()
            );
            dev_map = dev_provider();
            publish_parents(&parents_tx, &dev_map, &route_roots);
        }

        // Dispatch only pending routes that were not successfully rebuilt and
        // still declare a target in the refreshed map.
        pending.retain(|route_id| !rebuilt.contains(route_id) && dev_map.has_targets_for(route_id));

        for route_id in pending {
            match dispatch_reload(route_id.as_str()).await {
                Ok(()) => {
                    // A committed reload may change the route's dependency
                    // closure (for example a template now includes a file from
                    // a new directory), so re-derive and let the wrapper
                    // reconcile subscriptions right away. Publishing per
                    // commit means a later pending route that stalls can no
                    // longer delay an earlier route's new parents.
                    dev_map = dev_provider();
                    publish_parents(&parents_tx, &dev_map, &route_roots);
                }
                Err(err) => {
                    tracing::warn!(
                        "hot-reload: reload dispatch failed for route '{route_id}': {err}"
                    );
                }
            }
        }
    }
}

/// Classification outcome for a single notify event.
///
/// `None` (no class) is the ignore outcome: the event matches neither a
/// dev-target source nor a route file.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventClass {
    /// YAML/JSON route definition under a configured route root.
    RouteFile,
    /// Any template asset (template, proto, include, …) declared by a reload
    /// target. Dispatched per route as `ReloadTemplates`.
    DevTarget,
    /// Script file (`.rhai`, `.js`, …) declared as a route script source.
    /// Triggers a full reload pass so the DSL can re-read the file.
    ScriptSource,
}

/// Kind of a watched dev source.
///
/// Templates are reloaded per route (`ReloadTemplates`); scripts feed the
/// script watch registry and trigger a full route reload pass so discovery
/// re-reads the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DevSourceKind {
    /// Template/proto/include asset owned by a template reload target.
    Template,
    /// Script file read by a route's `script:` step (and friends).
    Script,
}

/// Path → route-id edges for one snapshot of committed dev-target sources,
/// split by [`DevSourceKind`].
///
/// Paths are normalized on insert and lookup (see [`normalize_path`]) so a
/// relative source and an absolute event path for the same file compare equal.
/// Template accessors (`contains_path`, `routes_for`, `has_targets_for`) only
/// see template entries; `script_routes_for` only sees script entries.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Default, Clone)]
pub(crate) struct DevTargetMap {
    template_path_to_routes: HashMap<PathBuf, HashSet<String>>,
    script_path_to_routes: HashMap<PathBuf, HashSet<String>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl DevTargetMap {
    /// Build the map from `(route_id, (path, kind))` pairs.
    pub(crate) fn from_sources(sources: &[(String, Vec<(PathBuf, DevSourceKind)>)]) -> Self {
        let mut map = Self::default();
        for (route_id, paths) in sources {
            for (path, kind) in paths {
                let by_path = match kind {
                    DevSourceKind::Template => &mut map.template_path_to_routes,
                    DevSourceKind::Script => &mut map.script_path_to_routes,
                };
                by_path
                    .entry(normalize_path(path))
                    .or_default()
                    .insert(route_id.clone());
            }
        }
        map
    }

    /// Returns `true` when `path` is a tracked template source.
    pub(crate) fn contains_path(&self, path: &Path) -> bool {
        self.template_path_to_routes
            .contains_key(&normalize_path(path))
    }

    /// Route ids that declare `path` as a template source; empty when the path
    /// is not tracked.
    pub(crate) fn routes_for(&self, path: &Path) -> HashSet<String> {
        self.template_path_to_routes
            .get(&normalize_path(path))
            .cloned()
            .unwrap_or_default()
    }

    /// Returns `true` when `path` is a tracked script source.
    pub(crate) fn contains_script_path(&self, path: &Path) -> bool {
        self.script_path_to_routes
            .contains_key(&normalize_path(path))
    }

    /// Route ids that declare `path` as a script source; empty when the path is
    /// not tracked.
    pub(crate) fn script_routes_for(&self, path: &Path) -> HashSet<String> {
        self.script_path_to_routes
            .get(&normalize_path(path))
            .cloned()
            .unwrap_or_default()
    }

    /// Returns `true` when `route_id` still declares at least one tracked
    /// template source.
    pub(crate) fn has_targets_for(&self, route_id: &str) -> bool {
        self.template_path_to_routes
            .values()
            .any(|routes| routes.contains(route_id))
    }

    /// Unique parent directories of every tracked source, template or script.
    ///
    /// Directories already subscribed as route roots are NOT removed here;
    /// that subtraction belongs to the watcher wrapper.
    pub(crate) fn parent_dirs(&self) -> Vec<PathBuf> {
        self.template_path_to_routes
            .keys()
            .chain(self.script_path_to_routes.keys())
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Returns `true` when no sources of either kind are tracked.
    pub(crate) fn is_empty(&self) -> bool {
        self.template_path_to_routes.is_empty() && self.script_path_to_routes.is_empty()
    }
}

/// Normalize a path to an absolute, canonical path when possible.
///
/// Relative paths are first joined onto the current directory. Existing
/// paths are canonicalized so watcher events and registered sources compare
/// equal across symlinked roots (notably macOS `/var` vs `/private/var`,
/// where `tempfile` and notify disagree). For paths that no longer exist,
/// including notify `Remove` events, canonicalization fails; in that case
/// `.`/`..` are resolved textually as a deterministic fallback. Task 2.2
/// must pass `watch_dirs` through the same normalization (here) rather than
/// mixing raw and canonical roots.
#[cfg_attr(not(test), allow(dead_code))] // consumed by the extracted loop (Task 2.2)
fn normalize_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    };

    if let Ok(canonical) = std::fs::canonicalize(&absolute) {
        return canonical;
    }

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Returns `true` for create/modify/remove event kinds.
fn is_reload_kind(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Returns `true` when `path` is a YAML/JSON file under one of `route_roots`.
fn is_route_file_path(path: &Path, route_roots: &[PathBuf]) -> bool {
    let is_route_extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext, "yaml" | "yml" | "json"))
        .unwrap_or(false);
    if !is_route_extension {
        return false;
    }

    let normalized = normalize_path(path);
    route_roots
        .iter()
        .any(|root| normalized.starts_with(normalize_path(root)))
}

/// Classify a notify event against the dev-target map and route roots.
///
/// Dev-target paths take precedence over the route-file extension check, so a
/// tracked `.yaml` template source is treated as a dev target. A tracked
/// script source yields [`EventClass::ScriptSource`]; scripts are checked after
/// templates because a path cannot be both kinds of target for the same route,
/// and before the route-file extension check because a script source may itself
/// be a `.yaml`/`.yml`/`.json` file under a route root.
/// Events in asset-only directories (outside every route root) return `None`
/// and must be ignored.
#[cfg_attr(not(test), allow(dead_code))]
fn classify_event(
    event: &Event,
    dev_map: &DevTargetMap,
    route_roots: &[PathBuf],
) -> Option<EventClass> {
    if !is_reload_kind(event) {
        return None;
    }

    if event.paths.iter().any(|path| dev_map.contains_path(path)) {
        return Some(EventClass::DevTarget);
    }

    if event
        .paths
        .iter()
        .any(|path| dev_map.contains_script_path(path))
    {
        return Some(EventClass::ScriptSource);
    }

    if event
        .paths
        .iter()
        .any(|path| is_route_file_path(path, route_roots))
    {
        return Some(EventClass::RouteFile);
    }

    None
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
    use std::any::Any;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use async_trait::async_trait;
    use camel_component_api::template_reload::{
        TemplateReloadRegistry, TemplateReloadStaged, TemplateReloadTarget,
    };
    use futures::future::BoxFuture;
    use notify::{Event, EventKind};
    use tokio::sync::{mpsc, watch};
    use tokio_util::sync::CancellationToken;

    use crate::CamelContext;
    use crate::context::RuntimeExecutionHandle;
    use crate::hot_reload::application::execute_reload_actions;
    use crate::hot_reload::domain::ReloadAction;
    use crate::lifecycle::application::route_definition::RouteDefinition;
    use camel_api::CamelError;

    use super::{
        DevSourceKind, DevTargetMap, EventClass, PassOutcome, ReloadWatcher, bounded_dispatch,
        classify_event, derive_rebuilt_route_ids, derive_removed_route_ids, is_reload_kind,
        is_route_file_path, normalize_path, publish_parents, reload_dispatch_bound,
        resolve_watch_dirs, run_reload_loop, watch_and_reload, watch_and_reload_with_barriers,
    };

    type ReloadPass =
        Box<dyn Fn() -> BoxFuture<'static, Result<PassOutcome, CamelError>> + Send + Sync>;
    type DispatchReload =
        Box<dyn Fn(&str) -> BoxFuture<'static, Result<(), CamelError>> + Send + Sync>;

    /// Concrete staged marker for the test target.
    struct FakeStaged;

    impl TemplateReloadStaged for FakeStaged {
        fn into_any(self: Box<Self>) -> Box<dyn Any> {
            self
        }
    }

    /// Test reload target with a committed source list that may grow on commit.
    ///
    /// `watch_sources` reports only the committed list. A commit installs the
    /// pending list, mirroring real targets that re-resolve their dependency
    /// closure and report the new one afterwards.
    struct FakeReloadTarget {
        route: String,
        sources: Mutex<Vec<PathBuf>>,
        pending: Mutex<Vec<PathBuf>>,
        generation: AtomicU64,
        commits: Arc<AtomicUsize>,
    }

    impl FakeReloadTarget {
        fn new(route: &str, sources: Vec<PathBuf>) -> Arc<Self> {
            Arc::new(Self {
                route: route.to_string(),
                sources: Mutex::new(sources.clone()),
                pending: Mutex::new(sources),
                generation: AtomicU64::new(0),
                commits: Arc::new(AtomicUsize::new(0)),
            })
        }

        fn as_dyn(self: &Arc<Self>) -> Arc<dyn TemplateReloadTarget> {
            let concrete: Arc<Self> = Arc::clone(self);
            concrete
        }

        /// Install the pending list without a commit (simulates a route pass
        /// re-deriving from a freshly committed closure).
        fn install_pending(&self) {
            let pending = self.pending.lock().expect("pending poisoned").clone();
            *self.sources.lock().expect("sources poisoned") = pending;
            self.generation.fetch_add(1, Ordering::SeqCst);
        }

        fn set_pending(&self, sources: Vec<PathBuf>) {
            *self.pending.lock().expect("pending poisoned") = sources;
        }
    }

    #[async_trait]
    impl TemplateReloadTarget for FakeReloadTarget {
        fn route_id(&self) -> &str {
            &self.route
        }
        fn reload_timeout(&self) -> Duration {
            Duration::from_secs(5)
        }
        fn current_generation(&self) -> u64 {
            self.generation.load(Ordering::SeqCst)
        }
        async fn build(&self) -> Result<(Box<dyn TemplateReloadStaged>, u64), CamelError> {
            Ok((Box::new(FakeStaged), self.generation.load(Ordering::SeqCst)))
        }
        fn commit(&self, _staged: Box<dyn TemplateReloadStaged>) {
            let pending = self.pending.lock().expect("pending poisoned").clone();
            *self.sources.lock().expect("sources poisoned") = pending;
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.commits.fetch_add(1, Ordering::SeqCst);
        }
        fn watch_sources(&self) -> Vec<PathBuf> {
            self.sources.lock().expect("sources poisoned").clone()
        }
    }

    /// Yield-spin until `cond` holds or `bound` elapses. No sleeps (lint).
    async fn wait_for_condition<F>(mut cond: F, bound: Duration)
    where
        F: FnMut() -> bool,
    {
        let start = std::time::Instant::now();
        loop {
            if cond() {
                return;
            }
            assert!(
                std::time::Instant::now() < start + bound,
                "condition not met within {bound:?}"
            );
            let _ = tokio::time::timeout(Duration::from_millis(1), tokio::task::yield_now()).await;
        }
    }

    async fn await_ready(mut ready_rx: watch::Receiver<bool>) {
        let ready = tokio::time::timeout(Duration::from_secs(2), ready_rx.wait_for(|v| *v))
            .await
            .expect("watcher-ready barrier did not fire");
        assert!(ready.is_ok(), "watcher-ready sender dropped");
    }

    async fn await_version_after(version_rx: &mut watch::Receiver<u64>, base: u64) {
        let bumped = tokio::time::timeout(
            Duration::from_secs(2),
            version_rx.wait_for(|version| *version > base),
        )
        .await
        .expect("reconciliation version bump did not arrive");
        assert!(bumped.is_ok(), "reconciliation sender dropped");
    }

    #[allow(clippy::type_complexity)]
    fn spawn_watcher<F>(
        controller: RuntimeExecutionHandle,
        watch_dirs: Vec<PathBuf>,
        route_id: String,
        discover_fn: F,
    ) -> (
        CancellationToken,
        watch::Receiver<bool>,
        watch::Receiver<u64>,
        tokio::task::JoinHandle<Result<(), CamelError>>,
    )
    where
        F: Fn() -> Result<Vec<RouteDefinition>, CamelError> + Send + Sync + 'static,
    {
        let (ready_tx, ready_rx) = watch::channel(false);
        let (version_tx, version_rx) = watch::channel(0u64);
        let shutdown = CancellationToken::new();
        let provider = move || {
            template_kind_sources(
                TemplateReloadRegistry::global()
                    .route_watch_sources()
                    .into_iter()
                    .filter(|(route, _)| route.as_str() == route_id.as_str())
                    .collect::<Vec<(String, Vec<PathBuf>)>>(),
            )
        };
        let handle = tokio::spawn(watch_and_reload_with_barriers(
            watch_dirs,
            controller,
            discover_fn,
            provider,
            noop_publish,
            Some(shutdown.clone()),
            Duration::from_millis(50),
            Duration::from_millis(20),
            ready_tx,
            version_tx,
        ));
        (shutdown, ready_rx, version_rx, handle)
    }

    async fn stop_watcher(
        shutdown: CancellationToken,
        handle: tokio::task::JoinHandle<Result<(), CamelError>>,
    ) {
        shutdown.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("watcher did not stop")
            .expect("watcher task panicked");
        assert!(result.is_ok(), "watcher returned an error: {result:?}");
    }

    fn fresh_registry() -> &'static TemplateReloadRegistry {
        Box::leak(Box::new(TemplateReloadRegistry::default()))
    }

    /// Sets a flag when dropped, proving a parked dispatch future was dropped.
    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn modify_event(path: &str) -> Event {
        make_event(&[path], modify_content())
    }

    /// Let the spawned-side loop future run to its next await point under a
    /// paused clock. `yield_now` is a finite scheduler yield, not a sleep.
    async fn settle() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    fn reload_pass_counter(calls: Arc<AtomicUsize>, rebuilt: Vec<String>) -> ReloadPass {
        Box::new(move || {
            let calls = Arc::clone(&calls);
            let rebuilt = rebuilt.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(PassOutcome {
                    rebuilt,
                    removed: Vec::new(),
                    aborted: false,
                })
            })
        })
    }

    /// Publish hook that ignores every outcome. Integration scenarios that do
    /// not exercise script sources pass this.
    fn noop_publish(_rebuilt: &[String], _removed: &[String]) {}

    /// Recorded `(rebuilt, removed)` pairs from a publish hook.
    type PublishLog = Arc<Mutex<Vec<(Vec<String>, Vec<String>)>>>;
    /// Publish hook accepting one pass outcome.
    type PublishHook = Box<dyn Fn(&[String], &[String]) + Send + Sync>;

    /// Publish hook that records every `(rebuilt, removed)` pair it receives.
    fn publish_logger(calls: PublishLog) -> PublishHook {
        Box::new(move |rebuilt: &[String], removed: &[String]| {
            calls
                .lock()
                .expect("publish log poisoned")
                .push((rebuilt.to_vec(), removed.to_vec()));
        })
    }

    /// Dispatch bound the loop tests exercise: the production bound helper
    /// applied to the injected route id (registry deadline plus drain), so the
    /// loggers below run through `bounded_dispatch` under the same bound the
    /// watcher uses instead of an unbounded test-local future.
    fn test_dispatch_bound(route_id: &str) -> Duration {
        reload_dispatch_bound(route_id, Duration::ZERO)
    }

    fn dispatch_logger(
        calls: Arc<Mutex<Vec<String>>>,
        shutdown: CancellationToken,
    ) -> DispatchReload {
        Box::new(move |route_id: &str| {
            let calls = Arc::clone(&calls);
            let route_id = route_id.to_string();
            let shutdown = shutdown.clone();
            Box::pin(async move {
                let bound = test_dispatch_bound(&route_id);
                bounded_dispatch(
                    async move {
                        calls.lock().expect("dispatch log poisoned").push(route_id);
                        Ok(())
                    },
                    bound,
                    Some(&shutdown),
                )
                .await
            })
        })
    }

    fn make_event(paths: &[&str], kind: EventKind) -> Event {
        Event {
            kind,
            paths: paths.iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn normalize_path_resolves_symlinked_existing_paths() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("temp dir");
        let real = tmp.path().join("real");
        let link = tmp.path().join("link");
        fs::create_dir(&real).expect("create real dir");
        symlink(&real, &link).expect("create symlink");

        assert_eq!(normalize_path(&link), normalize_path(&real));
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
            Vec::<(String, Vec<(PathBuf, DevSourceKind)>)>::new,
            noop_publish,
            Some(token),
            Duration::from_millis(1),
            Duration::from_millis(1),
        )
        .await;

        assert!(result.is_ok());
    }

    #[test]
    fn is_route_file_path_rejects_non_reload_extension() {
        let roots = vec![PathBuf::from("routes")];
        let ev = make_event(
            &["routes/test.toml"],
            EventKind::Create(notify::event::CreateKind::File),
        );
        assert!(!is_route_file_path(&ev.paths[0], &roots));
    }

    #[test]
    fn is_route_file_path_rejects_editor_swap_file() {
        let roots = vec![PathBuf::from("routes")];
        let ev = make_event(
            &["routes/route.yaml.swp"],
            EventKind::Create(notify::event::CreateKind::File),
        );
        assert!(!is_route_file_path(&ev.paths[0], &roots));
    }

    #[test]
    fn is_route_file_path_rejects_path_without_extension() {
        let roots = vec![PathBuf::from("routes")];
        let ev = make_event(
            &["routes/Makefile"],
            EventKind::Create(notify::event::CreateKind::File),
        );
        assert!(!is_route_file_path(&ev.paths[0], &roots));
    }

    #[test]
    fn is_route_file_path_accepts_yml_and_json_under_route_root() {
        let roots = vec![PathBuf::from("routes")];
        for path in ["routes/route.yml", "routes/route.json"] {
            let ev = make_event(&[path], EventKind::Create(notify::event::CreateKind::File));
            assert!(
                is_route_file_path(&ev.paths[0], &roots),
                "{path} should be a route file"
            );
        }
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

    fn make_dev_map(sources: &[(&str, &[&str])]) -> DevTargetMap {
        let owned: Vec<(String, Vec<(PathBuf, DevSourceKind)>)> = sources
            .iter()
            .map(|(route, paths)| {
                (
                    (*route).to_string(),
                    paths
                        .iter()
                        .map(|path| (PathBuf::from(path), DevSourceKind::Template))
                        .collect(),
                )
            })
            .collect();
        DevTargetMap::from_sources(&owned)
    }

    /// Build a dev map from `(route, path, kind)` triples.
    fn make_kinded_dev_map(sources: &[(&str, &str, DevSourceKind)]) -> DevTargetMap {
        let owned: Vec<(String, Vec<(PathBuf, DevSourceKind)>)> = sources
            .iter()
            .map(|(route, path, kind)| ((*route).to_string(), vec![(PathBuf::from(path), *kind)]))
            .collect();
        DevTargetMap::from_sources(&owned)
    }

    /// Wrap a plain template watch snapshot as kind-tagged sources.
    fn template_kind_sources(
        sources: Vec<(String, Vec<PathBuf>)>,
    ) -> Vec<(String, Vec<(PathBuf, DevSourceKind)>)> {
        sources
            .into_iter()
            .map(|(route, paths)| {
                (
                    route,
                    paths
                        .into_iter()
                        .map(|path| (path, DevSourceKind::Template))
                        .collect(),
                )
            })
            .collect()
    }

    fn modify_content() -> EventKind {
        EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Content,
        ))
    }

    #[test]
    fn is_reload_kind_accepts_mutations_and_rejects_access() {
        assert!(is_reload_kind(&make_event(
            &["routes/route.yaml"],
            EventKind::Remove(notify::event::RemoveKind::File),
        )));
        assert!(is_reload_kind(&make_event(
            &["routes/route.yaml"],
            modify_content(),
        )));
        assert!(is_reload_kind(&make_event(
            &["routes/route.yaml"],
            EventKind::Create(notify::event::CreateKind::File),
        )));
        assert!(!is_reload_kind(&make_event(
            &["routes/route.yaml"],
            EventKind::Access(notify::event::AccessKind::Read),
        )));
    }

    #[test]
    fn classify_dev_target_path_takes_precedence_over_extension() {
        let dev_map = make_dev_map(&[("t1", &["routes/defs.yaml"])]);
        let ev = make_event(&["routes/defs.yaml"], modify_content());
        let roots = vec![PathBuf::from("routes")];

        assert_eq!(
            classify_event(&ev, &dev_map, &roots),
            Some(EventClass::DevTarget)
        );
    }

    #[test]
    fn classify_route_file_when_not_tracked() {
        let dev_map = DevTargetMap::default();
        let ev = make_event(
            &["routes/other.yaml"],
            EventKind::Create(notify::event::CreateKind::File),
        );
        let roots = vec![PathBuf::from("routes")];

        assert_eq!(
            classify_event(&ev, &dev_map, &roots),
            Some(EventClass::RouteFile)
        );
    }

    #[test]
    fn unrelated_yaml_outside_route_roots_is_ignored() {
        let dev_map = DevTargetMap::default();
        let ev = make_event(
            &["assets/data.yaml"],
            EventKind::Create(notify::event::CreateKind::File),
        );
        let roots = vec![PathBuf::from("routes")];

        let class = classify_event(&ev, &dev_map, &roots);
        assert_ne!(class, Some(EventClass::RouteFile));
        assert_ne!(class, Some(EventClass::DevTarget));
        assert_eq!(class, None);
    }

    #[test]
    fn dev_map_limited_to_declared_sources() {
        let a = PathBuf::from("/work/site/a.jinja");
        let b = PathBuf::from("/work/site/b.jinja");
        let c = PathBuf::from("/work/site/part/c.jinja");
        let schema = PathBuf::from("/work/proto/schema.proto");
        let import = PathBuf::from("/work/proto/import.proto");
        let undeclared = PathBuf::from("/work/proto/never_declared.proto");

        let sources = vec![
            (
                "t1".to_string(),
                vec![
                    (a.clone(), DevSourceKind::Template),
                    (b.clone(), DevSourceKind::Template),
                    (c.clone(), DevSourceKind::Template),
                ],
            ),
            (
                "v1".to_string(),
                vec![
                    (schema.clone(), DevSourceKind::Template),
                    (import.clone(), DevSourceKind::Template),
                ],
            ),
        ];
        let map = DevTargetMap::from_sources(&sources);

        assert!(DevTargetMap::default().is_empty());
        assert!(!map.is_empty());
        for path in [&a, &b, &c, &schema, &import] {
            assert!(map.contains_path(path), "expected tracked path {path:?}");
        }
        assert!(!map.contains_path(&undeclared));

        let routes = map.routes_for(&schema);
        assert_eq!(routes.len(), 1);
        assert!(routes.contains("v1"));
    }

    #[test]
    fn dev_map_parent_dirs_unique() {
        let sources = vec![
            (
                "t1".to_string(),
                vec![
                    (
                        PathBuf::from("/work/site/page.jinja"),
                        DevSourceKind::Template,
                    ),
                    (
                        PathBuf::from("/work/site/other.jinja"),
                        DevSourceKind::Template,
                    ),
                ],
            ),
            (
                "v1".to_string(),
                vec![(
                    PathBuf::from("/work/proto/schema.proto"),
                    DevSourceKind::Template,
                )],
            ),
        ];

        let dirs = DevTargetMap::from_sources(&sources).parent_dirs();

        assert_eq!(dirs.len(), 2, "expected unique parent dirs, got {dirs:?}");
        assert!(dirs.contains(&PathBuf::from("/work/site")));
        assert!(dirs.contains(&PathBuf::from("/work/proto")));
    }

    // ── run_reload_loop (synthetic event source) ────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn dev_target_event_dispatches_per_route() {
        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");

        let debounce = Duration::from_millis(50);
        let dev_map = make_dev_map(&[("t1", &["templates/page.jinja"])]);
        let provider = move || dev_map.clone();
        let reload_calls = Arc::new(AtomicUsize::new(0));
        let reload_pass = reload_pass_counter(Arc::clone(&reload_calls), Vec::new());
        let dispatched = Arc::new(Mutex::new(Vec::new()));
        let token = CancellationToken::new();
        let dispatch = dispatch_logger(Arc::clone(&dispatched), token.clone());

        let (tx, rx) = mpsc::channel(64);
        let (parents_tx, _parents_rx) = watch::channel(Vec::new());
        let loop_fut = run_reload_loop(
            rx,
            vec![PathBuf::from("routes")],
            ctx.runtime_execution_handle(),
            &provider,
            &reload_pass,
            &noop_publish,
            &dispatch,
            parents_tx,
            Some(token.clone()),
            debounce,
        );

        let driver = async {
            tx.send(Ok(modify_event("templates/page.jinja")))
                .await
                .expect("send event");
            settle().await;
            tokio::time::advance(debounce).await;
            settle().await;
            token.cancel();
        };

        let (loop_result, ()) = futures::future::join(loop_fut, driver).await;
        assert!(loop_result.is_ok());
        assert_eq!(
            reload_calls.load(Ordering::SeqCst),
            0,
            "dev-target batch must not run the route pass"
        );
        assert_eq!(
            *dispatched.lock().expect("dispatch log poisoned"),
            vec!["t1".to_string()]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn debounce_coalesces_rapid_edits() {
        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");

        let debounce = Duration::from_millis(50);
        let dev_map = make_dev_map(&[("t1", &["templates/page.jinja"])]);
        let provider = move || dev_map.clone();
        let reload_calls = Arc::new(AtomicUsize::new(0));
        let reload_pass = reload_pass_counter(Arc::clone(&reload_calls), Vec::new());
        let dispatched = Arc::new(Mutex::new(Vec::new()));
        let token = CancellationToken::new();
        let dispatch = dispatch_logger(Arc::clone(&dispatched), token.clone());

        let (tx, rx) = mpsc::channel(64);
        let (parents_tx, _parents_rx) = watch::channel(Vec::new());
        let loop_fut = run_reload_loop(
            rx,
            vec![PathBuf::from("routes")],
            ctx.runtime_execution_handle(),
            &provider,
            &reload_pass,
            &noop_publish,
            &dispatch,
            parents_tx,
            Some(token.clone()),
            debounce,
        );

        let driver = async {
            for _ in 0..3 {
                tx.send(Ok(modify_event("templates/page.jinja")))
                    .await
                    .expect("send event");
            }
            settle().await;
            tokio::time::advance(debounce).await;
            settle().await;
            token.cancel();
        };

        let (loop_result, ()) = futures::future::join(loop_fut, driver).await;
        assert!(loop_result.is_ok());
        assert_eq!(
            *dispatched.lock().expect("dispatch log poisoned"),
            vec!["t1".to_string()],
            "rapid edits within one debounce window must yield one dispatch"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn route_without_targets_skipped_and_loop_continues() {
        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");

        let debounce = Duration::from_millis(50);
        let dev_map = make_dev_map(&[("t1", &["templates/page.jinja"])]);
        let provider = move || dev_map.clone();
        let reload_calls = Arc::new(AtomicUsize::new(0));
        let reload_pass = reload_pass_counter(Arc::clone(&reload_calls), Vec::new());
        let dispatched = Arc::new(Mutex::new(Vec::new()));
        let token = CancellationToken::new();
        let dispatch = dispatch_logger(Arc::clone(&dispatched), token.clone());

        let (tx, rx) = mpsc::channel(64);
        let (parents_tx, _parents_rx) = watch::channel(Vec::new());
        let loop_fut = run_reload_loop(
            rx,
            vec![PathBuf::from("routes")],
            ctx.runtime_execution_handle(),
            &provider,
            &reload_pass,
            &noop_publish,
            &dispatch,
            parents_tx,
            Some(token.clone()),
            debounce,
        );

        let driver = async {
            // Untracked, non-route file: classified as neither, so it must be
            // ignored without starting a debounce window or dispatching.
            tx.send(Ok(modify_event("assets/unrelated.jinja")))
                .await
                .expect("send unrelated event");
            settle().await;
            tx.send(Ok(modify_event("templates/page.jinja")))
                .await
                .expect("send tracked event");
            settle().await;
            tokio::time::advance(debounce).await;
            settle().await;
            token.cancel();
        };

        let (loop_result, ()) = futures::future::join(loop_fut, driver).await;
        assert!(loop_result.is_ok());
        assert_eq!(
            reload_calls.load(Ordering::SeqCst),
            0,
            "target-less events must not run the route pass"
        );
        assert_eq!(
            *dispatched.lock().expect("dispatch log poisoned"),
            vec!["t1".to_string()],
            "the loop must keep processing after an unmatched event"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dispatch_timeout_drops_without_late_commit() {
        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");

        let debounce = Duration::from_millis(50);
        let bound = Duration::from_millis(100);
        let dev_map = make_dev_map(&[("t1", &["templates/page.jinja"])]);
        let provider = move || dev_map.clone();
        let reload_pass = reload_pass_counter(Arc::new(AtomicUsize::new(0)), Vec::new());

        let token = CancellationToken::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let dispatch: DispatchReload = {
            let calls = Arc::clone(&calls);
            let dropped = Arc::clone(&dropped);
            let token = token.clone();
            Box::new(move |_route_id: &str| {
                let call_index = calls.fetch_add(1, Ordering::SeqCst);
                let dropped = Arc::clone(&dropped);
                let token = token.clone();
                Box::pin(async move {
                    if call_index == 0 {
                        let guard = DropFlag(dropped);
                        bounded_dispatch(
                            async move {
                                let _guard = guard;
                                futures::future::pending::<Result<(), CamelError>>().await
                            },
                            bound,
                            Some(&token),
                        )
                        .await
                    } else {
                        Ok(())
                    }
                }) as BoxFuture<'static, Result<(), CamelError>>
            })
        };

        let (tx, rx) = mpsc::channel(64);
        let (parents_tx, _parents_rx) = watch::channel(Vec::new());
        let loop_fut = run_reload_loop(
            rx,
            vec![PathBuf::from("routes")],
            ctx.runtime_execution_handle(),
            &provider,
            &reload_pass,
            &noop_publish,
            &dispatch,
            parents_tx,
            Some(token.clone()),
            debounce,
        );

        let driver = async {
            tx.send(Ok(modify_event("templates/page.jinja")))
                .await
                .expect("send first event");
            settle().await;
            tokio::time::advance(debounce).await;
            settle().await;
            // First dispatch is parked; advance past the production bound.
            tokio::time::advance(bound).await;
            settle().await;
            // The loop must still process a subsequent event.
            tx.send(Ok(modify_event("templates/page.jinja")))
                .await
                .expect("send second event");
            settle().await;
            tokio::time::advance(debounce).await;
            settle().await;
            token.cancel();
        };

        let (loop_result, ()) = futures::future::join(loop_fut, driver).await;
        assert!(loop_result.is_ok());
        assert!(
            dropped.load(Ordering::SeqCst),
            "timed-out dispatch future must be dropped, not left to commit later"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the loop must continue after a dispatch timeout"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_responsive_during_dispatch() {
        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");

        let debounce = Duration::from_millis(50);
        let dev_map = make_dev_map(&[("t1", &["templates/page.jinja"])]);
        let provider = move || dev_map.clone();
        let reload_pass = reload_pass_counter(Arc::new(AtomicUsize::new(0)), Vec::new());

        let token = CancellationToken::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let dispatch: DispatchReload = {
            let calls = Arc::clone(&calls);
            let dropped = Arc::clone(&dropped);
            let token = token.clone();
            Box::new(move |_route_id: &str| {
                calls.fetch_add(1, Ordering::SeqCst);
                let guard = DropFlag(Arc::clone(&dropped));
                let token = token.clone();
                Box::pin(async move {
                    bounded_dispatch(
                        async move {
                            let _guard = guard;
                            futures::future::pending::<Result<(), CamelError>>().await
                        },
                        Duration::from_secs(60),
                        Some(&token),
                    )
                    .await
                }) as BoxFuture<'static, Result<(), CamelError>>
            })
        };

        let (tx, rx) = mpsc::channel(64);
        let (parents_tx, _parents_rx) = watch::channel(Vec::new());
        let loop_fut = run_reload_loop(
            rx,
            vec![PathBuf::from("routes")],
            ctx.runtime_execution_handle(),
            &provider,
            &reload_pass,
            &noop_publish,
            &dispatch,
            parents_tx,
            Some(token.clone()),
            debounce,
        );

        let driver = async {
            tx.send(Ok(modify_event("templates/page.jinja")))
                .await
                .expect("send event");
            settle().await;
            tokio::time::advance(debounce).await;
            settle().await;
            // Dispatch is parked on a 60 s bound; cancellation must win long
            // before that bound elapses in virtual time. Return the instant
            // the cancel fired so the assertion below can prove the loop
            // stopped via the cancellation path, not the timeout path.
            token.cancel();
            tokio::time::Instant::now()
        };

        let (loop_result, cancelled_at) = futures::future::join(loop_fut, driver).await;
        let cancel_latency = tokio::time::Instant::now() - cancelled_at;
        assert!(loop_result.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(
            dropped.load(Ordering::SeqCst),
            "cancellation must drop the in-flight dispatch future"
        );
        assert!(
            cancel_latency < Duration::from_secs(30),
            "loop must stop via cancellation, not the 60 s dispatch timeout (took {cancel_latency:?})"
        );
    }

    /// Drive one debounced batch through the extracted loop and return the
    /// route ids it dispatched, in dispatch order.
    async fn drive_one_batch(
        dev_map: DevTargetMap,
        reload_pass: ReloadPass,
        events: Vec<Event>,
    ) -> Vec<String> {
        drive_one_batch_with_provider(move || dev_map.clone(), reload_pass, events).await
    }

    async fn drive_one_batch_with_provider<P>(
        provider: P,
        reload_pass: ReloadPass,
        events: Vec<Event>,
    ) -> Vec<String>
    where
        P: Fn() -> DevTargetMap + Send + Sync + 'static,
    {
        drive_one_batch_with_hook(provider, reload_pass, &noop_publish, events).await
    }

    async fn drive_one_batch_with_hook<P>(
        provider: P,
        reload_pass: ReloadPass,
        publish_script_sources: &(dyn Fn(&[String], &[String]) + Send + Sync),
        events: Vec<Event>,
    ) -> Vec<String>
    where
        P: Fn() -> DevTargetMap + Send + Sync + 'static,
    {
        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");

        let debounce = Duration::from_millis(50);
        let dispatched = Arc::new(Mutex::new(Vec::new()));
        let token = CancellationToken::new();
        let dispatch = dispatch_logger(Arc::clone(&dispatched), token.clone());

        let (tx, rx) = mpsc::channel(64);
        let (parents_tx, _parents_rx) = watch::channel(Vec::new());
        let loop_fut = run_reload_loop(
            rx,
            vec![PathBuf::from("routes")],
            ctx.runtime_execution_handle(),
            &provider,
            &reload_pass,
            publish_script_sources,
            &dispatch,
            parents_tx,
            Some(token.clone()),
            debounce,
        );

        let driver = async {
            for event in events {
                tx.send(Ok(event)).await.expect("send event");
            }
            settle().await;
            tokio::time::advance(debounce).await;
            settle().await;
            token.cancel();
        };

        let (loop_result, ()) = futures::future::join(loop_fut, driver).await;
        assert!(loop_result.is_ok());
        dispatched.lock().expect("dispatch log poisoned").clone()
    }

    fn route_file_event(path: &str) -> Event {
        make_event(&[path], EventKind::Create(notify::event::CreateKind::File))
    }

    #[tokio::test(start_paused = true)]
    async fn mixed_batch_dispatches_for_surviving_unrebuilt_route() {
        let dev_map = make_dev_map(&[("A", &["templates/a.jinja"]), ("B", &["templates/b.jinja"])]);
        let reload_pass = reload_pass_counter(Arc::new(AtomicUsize::new(0)), vec!["B".to_string()]);

        let dispatched = drive_one_batch(
            dev_map,
            reload_pass,
            vec![
                modify_event("templates/a.jinja"),
                route_file_event("routes/b.yaml"),
            ],
        )
        .await;

        assert_eq!(dispatched, vec!["A".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn mixed_batch_same_route_no_double_dispatch() {
        let dev_map = make_dev_map(&[("B", &["templates/b.jinja"])]);
        let reload_pass = reload_pass_counter(Arc::new(AtomicUsize::new(0)), vec!["B".to_string()]);

        let dispatched = drive_one_batch(
            dev_map,
            reload_pass,
            vec![
                modify_event("templates/b.jinja"),
                route_file_event("routes/b.yaml"),
            ],
        )
        .await;

        assert!(
            dispatched.is_empty(),
            "rebuilt route B must not receive a second dispatch, got {dispatched:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn partial_survival_skipped_and_failed_still_dispatch() {
        let dev_map = make_dev_map(&[
            ("A", &["templates/a.jinja"]),
            ("B", &["templates/b.jinja"]),
            ("C", &["templates/c.jinja"]),
        ]);
        // A was skipped and B failed to rebuild; only C was rebuilt.
        let reload_pass = reload_pass_counter(Arc::new(AtomicUsize::new(0)), vec!["C".to_string()]);

        let dispatched = drive_one_batch(
            dev_map,
            reload_pass,
            vec![
                modify_event("templates/a.jinja"),
                modify_event("templates/b.jinja"),
                modify_event("templates/c.jinja"),
                route_file_event("routes/c.yaml"),
            ],
        )
        .await;

        assert_eq!(dispatched, vec!["A".to_string(), "B".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_discovery_still_dispatches_pending() {
        let dev_map = make_dev_map(&[("A", &["templates/a.jinja"]), ("B", &["templates/b.jinja"])]);
        let reload_pass: ReloadPass = Box::new(|| {
            Box::pin(async { Err(CamelError::Config("discovery failed".to_string())) })
        });

        let dispatched = drive_one_batch(
            dev_map,
            reload_pass,
            vec![
                modify_event("templates/a.jinja"),
                modify_event("templates/b.jinja"),
                route_file_event("routes/b.yaml"),
            ],
        )
        .await;

        assert_eq!(dispatched, vec!["A".to_string(), "B".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn no_action_pass_still_dispatches() {
        let dev_map = make_dev_map(&[("A", &["templates/a.jinja"])]);
        let reload_pass = reload_pass_counter(Arc::new(AtomicUsize::new(0)), Vec::new());

        let dispatched = drive_one_batch(
            dev_map,
            reload_pass,
            vec![
                modify_event("templates/a.jinja"),
                route_file_event("routes/a.yaml"),
            ],
        )
        .await;

        assert_eq!(dispatched, vec!["A".to_string()]);
    }

    // ── script sources (Task 3.1) ───────────────────────────────────────────

    #[test]
    fn classify_script_path_yields_script_source_class() {
        let dev_map = make_kinded_dev_map(&[("s1", "scripts/enrich.rhai", DevSourceKind::Script)]);
        let script_path = PathBuf::from("scripts/enrich.rhai");
        assert!(
            dev_map.script_routes_for(&script_path).contains("s1"),
            "the script source must map back to its route"
        );
        assert!(
            dev_map.routes_for(&script_path).is_empty(),
            "script sources must not appear as template targets"
        );

        let ev = make_event(&["scripts/enrich.rhai"], modify_content());
        let roots = vec![PathBuf::from("routes")];

        assert_eq!(
            classify_event(&ev, &dev_map, &roots),
            Some(EventClass::ScriptSource)
        );
    }

    #[test]
    fn classify_yaml_script_path_yields_script_source_not_route_file() {
        // A `.yaml` script source under a route root matches both the script
        // registry and the route-file extension check; the registry must win.
        let dev_map =
            make_kinded_dev_map(&[("s1", "routes/scripts/render.yaml", DevSourceKind::Script)]);
        let script_path = PathBuf::from("routes/scripts/render.yaml");
        assert!(
            dev_map.script_routes_for(&script_path).contains("s1"),
            "the script source must map back to its route"
        );
        assert!(
            dev_map.routes_for(&script_path).is_empty(),
            "script sources must not appear as template targets"
        );

        let ev = make_event(&["routes/scripts/render.yaml"], modify_content());
        let roots = vec![PathBuf::from("routes")];

        assert_eq!(
            classify_event(&ev, &dev_map, &roots),
            Some(EventClass::ScriptSource),
            "a tracked .yaml script source must not classify as RouteFile or DevTarget"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn script_event_triggers_full_reload_pass() {
        let dev_map = make_kinded_dev_map(&[("s1", "scripts/enrich.rhai", DevSourceKind::Script)]);
        let reload_calls = Arc::new(AtomicUsize::new(0));
        let reload_pass = reload_pass_counter(Arc::clone(&reload_calls), Vec::new());

        let dispatched = drive_one_batch(
            dev_map,
            reload_pass,
            vec![modify_event("scripts/enrich.rhai")],
        )
        .await;

        assert_eq!(
            reload_calls.load(Ordering::SeqCst),
            1,
            "a script-only batch must run the full reload pass once"
        );
        assert!(
            dispatched.is_empty(),
            "a script source must not dispatch ReloadTemplates, got {dispatched:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn template_event_still_dispatches_reload_templates() {
        let dev_map = make_dev_map(&[("t1", &["templates/page.jinja"])]);
        let reload_calls = Arc::new(AtomicUsize::new(0));
        let reload_pass = reload_pass_counter(Arc::clone(&reload_calls), Vec::new());

        let dispatched = drive_one_batch(
            dev_map,
            reload_pass,
            vec![modify_event("templates/page.jinja")],
        )
        .await;

        assert_eq!(
            reload_calls.load(Ordering::SeqCst),
            0,
            "a template-only batch must keep the per-route dispatch path"
        );
        assert_eq!(dispatched, vec!["t1".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn script_and_route_edit_in_one_batch_single_pass() {
        let dev_map = make_kinded_dev_map(&[("s1", "scripts/enrich.rhai", DevSourceKind::Script)]);
        let reload_calls = Arc::new(AtomicUsize::new(0));
        let reload_pass = reload_pass_counter(Arc::clone(&reload_calls), Vec::new());

        drive_one_batch(
            dev_map,
            reload_pass,
            vec![
                modify_event("scripts/enrich.rhai"),
                route_file_event("routes/a.yaml"),
            ],
        )
        .await;

        assert_eq!(
            reload_calls.load(Ordering::SeqCst),
            1,
            "a mixed script+route batch must run exactly one reload pass"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn publish_hook_called_with_outcome_after_pass() {
        // A successful Remove appears in `removed`; a failed one does not,
        // because the route keeps running and keeps its script targets.
        let actions = vec![
            ReloadAction::Remove {
                route_id: "removed-ok".into(),
            },
            ReloadAction::Remove {
                route_id: "removed-failed".into(),
            },
        ];
        let errors = vec![super::ReloadError {
            route_id: "removed-failed".into(),
            action: "remove".into(),
            error: CamelError::RouteError("removal failed".to_string()),
        }];
        assert_eq!(
            derive_removed_route_ids(&actions, &errors),
            vec!["removed-ok".to_string()],
            "a failed removal must stay out of the removed set"
        );

        // Non-aborted pass: the hook sees exactly one (rebuilt, removed) pair.
        let published = Arc::new(Mutex::new(Vec::new()));
        let hook = publish_logger(Arc::clone(&published));
        let outcome = PassOutcome {
            rebuilt: vec!["rebuilt-1".to_string()],
            removed: vec!["removed-ok".to_string()],
            aborted: false,
        };
        let reload_pass: ReloadPass = Box::new(move || {
            let outcome = outcome.clone();
            Box::pin(async move { Ok(outcome) })
        });
        let dev_map = make_kinded_dev_map(&[("s1", "scripts/enrich.rhai", DevSourceKind::Script)]);
        let _ = drive_one_batch_with_hook(
            move || dev_map.clone(),
            reload_pass,
            &*hook,
            vec![modify_event("scripts/enrich.rhai")],
        )
        .await;
        assert_eq!(
            *published.lock().expect("publish log poisoned"),
            vec![(
                vec!["rebuilt-1".to_string()],
                vec!["removed-ok".to_string()]
            )],
            "a non-aborted pass must publish its outcome exactly once"
        );

        // Aborted pass: never publishes. A discovery failure says nothing
        // about which routes still run.
        let aborted_published = Arc::new(Mutex::new(Vec::new()));
        let aborted_hook = publish_logger(Arc::clone(&aborted_published));
        let aborted_pass: ReloadPass = Box::new(|| {
            Box::pin(async {
                Ok(PassOutcome {
                    rebuilt: Vec::new(),
                    removed: Vec::new(),
                    aborted: true,
                })
            })
        });
        let dev_map = make_kinded_dev_map(&[("s1", "scripts/enrich.rhai", DevSourceKind::Script)]);
        let _ = drive_one_batch_with_hook(
            move || dev_map.clone(),
            aborted_pass,
            &*aborted_hook,
            vec![modify_event("scripts/enrich.rhai")],
        )
        .await;
        assert!(
            aborted_published
                .lock()
                .expect("publish log poisoned")
                .is_empty(),
            "an aborted pass must never publish"
        );
    }

    /// Covers the production rebuilt-id derivation end-to-end against the real
    /// reload executor: only Add, Swap and Restart routes that did not error
    /// survive; Skip, Remove and the failed Swap are filtered out.
    #[tokio::test]
    async fn production_derivation_keeps_only_successful_rebuilds() {
        use camel_component_timer::TimerComponent;

        let mut ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");
        ctx.register_component(TimerComponent::new());
        ctx.start().await.expect("context start");

        // Seed the routes that Swap, Restart, Skip and Remove act on.
        for route_id in ["swapped", "restarted", "skipped", "removed"] {
            ctx.add_route_definition(
                RouteDefinition::new("timer:tick?period=50", Vec::new()).with_route_id(route_id),
            )
            .await
            .expect("seed route");
        }

        let actions = vec![
            ReloadAction::Add {
                route_id: "added".into(),
            },
            ReloadAction::Swap {
                route_id: "swapped".into(),
            },
            ReloadAction::Restart {
                route_id: "restarted".into(),
            },
            ReloadAction::Skip {
                route_id: "skipped".into(),
            },
            ReloadAction::Remove {
                route_id: "removed".into(),
            },
            // No matching definition: the executor reports an error, so this
            // candidate must not count as rebuilt.
            ReloadAction::Swap {
                route_id: "failed".into(),
            },
        ];
        let new_defs = vec![
            RouteDefinition::new("timer:tick?period=50", Vec::new()).with_route_id("added"),
            RouteDefinition::new("timer:tick?period=50", Vec::new()).with_route_id("swapped"),
            RouteDefinition::new("timer:tick?period=60", Vec::new()).with_route_id("restarted"),
        ];

        let errors = execute_reload_actions(
            actions.clone(),
            new_defs,
            &ctx.runtime_execution_handle(),
            Duration::from_secs(5),
            None,
        )
        .await;
        assert_eq!(
            errors.len(),
            1,
            "only the definition-less Swap may fail, got {errors:?}"
        );
        assert_eq!(errors[0].route_id, "failed");

        let mut rebuilt = derive_rebuilt_route_ids(&actions, &errors);
        rebuilt.sort();

        assert_eq!(
            rebuilt,
            vec![
                "added".to_string(),
                "restarted".to_string(),
                "swapped".to_string()
            ],
            "Skip, Remove and the errored rebuild must be excluded"
        );

        ctx.stop().await.expect("context stop");
    }

    #[tokio::test(start_paused = true)]
    async fn route_file_event_keeps_existing_behavior() {
        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");

        let debounce = Duration::from_millis(50);
        let dev_map = DevTargetMap::default();
        let provider = move || dev_map.clone();
        let reload_calls = Arc::new(AtomicUsize::new(0));
        let reload_pass = reload_pass_counter(Arc::clone(&reload_calls), Vec::new());
        let dispatched = Arc::new(Mutex::new(Vec::new()));
        let token = CancellationToken::new();
        let dispatch = dispatch_logger(Arc::clone(&dispatched), token.clone());

        let (tx, rx) = mpsc::channel(64);
        let (parents_tx, _parents_rx) = watch::channel(Vec::new());
        let loop_fut = run_reload_loop(
            rx,
            vec![PathBuf::from("routes")],
            ctx.runtime_execution_handle(),
            &provider,
            &reload_pass,
            &noop_publish,
            &dispatch,
            parents_tx,
            Some(token.clone()),
            debounce,
        );

        let driver = async {
            tx.send(Ok(make_event(
                &["routes/x.yaml"],
                EventKind::Create(notify::event::CreateKind::File),
            )))
            .await
            .expect("send route-file event");
            settle().await;
            tokio::time::advance(debounce).await;
            settle().await;
            token.cancel();
        };

        let (loop_result, ()) = futures::future::join(loop_fut, driver).await;
        assert!(loop_result.is_ok());
        assert_eq!(
            reload_calls.load(Ordering::SeqCst),
            1,
            "a route-file event must run the route pass exactly once"
        );
        assert!(
            dispatched.lock().expect("dispatch log poisoned").is_empty(),
            "no dev-target dispatch without a target map entry"
        );
    }

    // ── Task 2.4: re-derivation + subscription reconciliation ───────────────

    #[test]
    fn publish_parents_repeats_unchanged_snapshot() {
        let map = make_dev_map(&[("A", &["templates/a.jinja"])]);
        let route_roots: Vec<PathBuf> = Vec::new();
        let (tx, mut rx) = watch::channel(Vec::new());

        publish_parents(&tx, &map, &route_roots);
        let first = rx.borrow_and_update().clone();
        assert!(!first.is_empty(), "first publish must carry the parent dir");

        publish_parents(&tx, &map, &route_roots);
        assert!(
            rx.has_changed().expect("parents sender alive"),
            "an unchanged snapshot must still be published so failed \
             subscriptions retry"
        );
    }

    #[tokio::test]
    async fn watcher_ready_barrier() {
        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");
        let (shutdown, ready_rx, version_rx, handle) = spawn_watcher(
            ctx.runtime_execution_handle(),
            Vec::new(),
            "ready-barrier-route".to_string(),
            || Ok(vec![]),
        );

        await_ready(ready_rx).await;
        assert!(
            *version_rx.borrow() >= 1,
            "initial reconciliation must be applied before the readiness signal"
        );

        stop_watcher(shutdown, handle).await;
    }

    #[tokio::test(start_paused = true)]
    async fn failed_route_pass_retains_surviving_targets() {
        let reg = fresh_registry();
        let good = FakeReloadTarget::new(
            "failed-pass-good",
            vec![PathBuf::from("templates/good.jinja")],
        );
        let bad = FakeReloadTarget::new(
            "failed-pass-bad",
            vec![PathBuf::from("templates/bad.jinja")],
        );
        let _good = reg.register(good.as_dyn());
        let _bad = reg.register(bad.as_dyn());

        let provider =
            move || DevTargetMap::from_sources(&template_kind_sources(reg.route_watch_sources()));
        let reload_pass: ReloadPass = Box::new(|| {
            Box::pin(async { Err(CamelError::Config("discovery failed".to_string())) })
        });

        let dispatched = drive_one_batch_with_provider(
            provider,
            reload_pass,
            vec![
                modify_event("templates/good.jinja"),
                route_file_event("routes/x.yaml"),
            ],
        )
        .await;

        assert_eq!(
            dispatched,
            vec!["failed-pass-good".to_string()],
            "the surviving target must dispatch even after a failed route pass"
        );
        let map = DevTargetMap::from_sources(&template_kind_sources(reg.route_watch_sources()));
        assert!(
            !map.routes_for(&PathBuf::from("templates/good.jinja"))
                .is_empty(),
            "good route source must survive re-derivation"
        );
        assert!(
            !map.routes_for(&PathBuf::from("templates/bad.jinja"))
                .is_empty(),
            "bad route source must survive re-derivation"
        );
    }

    #[tokio::test]
    async fn rederive_after_committed_reload_picks_up_new_closure_file() {
        let reg = fresh_registry();
        let old = PathBuf::from("templates/page.jinja");
        let new = PathBuf::from("templates/part/head.jinja");
        let target = FakeReloadTarget::new("rederive-route", vec![old.clone()]);
        target.set_pending(vec![old.clone(), new.clone()]);
        let _guard = reg.register(target.as_dyn());

        let provider =
            move || DevTargetMap::from_sources(&template_kind_sources(reg.route_watch_sources()));
        let reload_pass = reload_pass_counter(Arc::new(AtomicUsize::new(0)), Vec::new());
        let dispatch: DispatchReload = Box::new(move |_route_id: &str| {
            Box::pin(async move { reg.reload_route("rederive-route").await })
        });

        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");
        let token = CancellationToken::new();
        let shutdown = token.clone();
        let (tx, rx) = mpsc::channel(64);
        let (parents_tx, mut parents_rx) = watch::channel(Vec::new());
        let handle = tokio::spawn(async move {
            run_reload_loop(
                rx,
                vec![PathBuf::from("routes")],
                ctx.runtime_execution_handle(),
                &provider,
                &reload_pass,
                &noop_publish,
                &dispatch,
                parents_tx,
                Some(shutdown),
                Duration::from_millis(20),
            )
            .await
        });

        tx.send(Ok(modify_event("templates/page.jinja")))
            .await
            .expect("send tracked event");

        let published = tokio::time::timeout(
            Duration::from_secs(2),
            parents_rx.wait_for(|dirs| dirs.iter().any(|dir| dir.ends_with("templates/part"))),
        )
        .await
        .expect("post-commit re-derivation must publish the new parent dir");
        assert!(published.is_ok(), "parents channel closed");

        let map = DevTargetMap::from_sources(&template_kind_sources(reg.route_watch_sources()));
        assert!(
            !map.routes_for(&new).is_empty(),
            "re-derived map must route the newly committed closure file"
        );

        token.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("loop did not stop")
            .expect("loop task panicked");
        assert!(result.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn per_commit_rederivation_publishes_before_later_route_stalls() {
        let reg = fresh_registry();
        let tmp = tempfile::tempdir().expect("temp dir");
        let a_old = tmp.path().join("awatch/page.jinja");
        let a_new = tmp.path().join("awatch/grown/part.jinja");
        let a_new_parent = tmp.path().join("awatch/grown");
        let b_source = tmp.path().join("bwatch/page.jinja");

        let route_a = "a-growth-route";
        let route_b = "b-stall-route";

        // Route A grows its closure into a new parent dir when it commits.
        let target_a = FakeReloadTarget::new(route_a, vec![a_old.clone()]);
        target_a.set_pending(vec![a_old.clone(), a_new.clone()]);
        let _guard_a = reg.register(target_a.as_dyn());

        // Route B never changes its closure; it only stalls on dispatch.
        let target_b = FakeReloadTarget::new(route_b, vec![b_source.clone()]);
        let _guard_b = reg.register(target_b.as_dyn());

        let provider =
            move || DevTargetMap::from_sources(&template_kind_sources(reg.route_watch_sources()));
        let reload_pass = reload_pass_counter(Arc::new(AtomicUsize::new(0)), Vec::new());

        let token = CancellationToken::new();
        let (started_tx, mut started_rx) = watch::channel(false);
        let (release_tx, release_rx) = watch::channel(false);
        let b_completed = Arc::new(AtomicBool::new(false));

        let dispatch: DispatchReload = {
            let b_completed = Arc::clone(&b_completed);
            let shutdown = token.clone();
            Box::new(move |route_id: &str| {
                if route_id == route_b {
                    let _ = started_tx.send(true);
                    let mut released = release_rx.clone();
                    let b_completed = Arc::clone(&b_completed);
                    let shutdown = shutdown.clone();
                    Box::pin(async move {
                        // Park under the production dispatch bound; the test
                        // releases B only after asserting A's publication.
                        let result = bounded_dispatch(
                            async move {
                                let _ = released.wait_for(|released| *released).await;
                                Ok(())
                            },
                            test_dispatch_bound(route_b),
                            Some(&shutdown),
                        )
                        .await;
                        b_completed.store(true, Ordering::SeqCst);
                        result
                    })
                } else {
                    let route_id = route_id.to_string();
                    Box::pin(async move { reg.reload_route(&route_id).await })
                }
            })
        };

        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");
        let (tx, rx) = mpsc::channel(64);
        let (parents_tx, parents_rx) = watch::channel(Vec::new());
        let debounce = Duration::from_millis(20);
        let loop_fut = run_reload_loop(
            rx,
            Vec::new(),
            ctx.runtime_execution_handle(),
            &provider,
            &reload_pass,
            &noop_publish,
            &dispatch,
            parents_tx,
            Some(token.clone()),
            debounce,
        );

        let driver = async {
            tx.send(Ok(modify_event(a_old.to_str().expect("utf-8 test path"))))
                .await
                .expect("send A event");
            tx.send(Ok(modify_event(
                b_source.to_str().expect("utf-8 test path"),
            )))
            .await
            .expect("send B event");
            settle().await;
            tokio::time::advance(debounce).await;
            settle().await;

            // A is dispatched first (BTreeSet order) and B is now parked in
            // its bounded dispatch. Waiting for B to start proves the loop
            // reached it without advancing past A's commit.
            let started = started_rx.wait_for(|started| *started).await;
            assert!(started.is_ok(), "route B dispatch never started");

            let published = parents_rx.borrow().clone();
            assert!(
                published.iter().any(|dir| dir == &a_new_parent),
                "A's new parent must be published before B completes, got {published:?}"
            );
            assert!(
                !b_completed.load(Ordering::SeqCst),
                "B must still be parked when A's growth is published"
            );

            release_tx.send(true).expect("release B");
            settle().await;
            token.cancel();
        };

        let (loop_result, ()) = futures::future::join(loop_fut, driver).await;
        assert!(loop_result.is_ok());
        assert!(
            b_completed.load(Ordering::SeqCst),
            "released B dispatch must complete"
        );
    }

    #[tokio::test]
    async fn replace_then_edit_still_dispatches() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let source = tmp.path().join("page.jinja");
        fs::write(&source, "v1").expect("seed source");

        let route = "replace-then-edit-route";
        let target = FakeReloadTarget::new(route, vec![source.clone()]);
        let commits = Arc::clone(&target.commits);
        let _guard = TemplateReloadRegistry::global().register(target.as_dyn());

        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");
        let (shutdown, ready_rx, _version_rx, handle) = spawn_watcher(
            ctx.runtime_execution_handle(),
            Vec::new(),
            route.to_string(),
            || Ok(vec![]),
        );
        await_ready(ready_rx).await;

        // Atomic replace: write a sibling then rename it over the target.
        let sibling = tmp.path().join("page.jinja.replacement");
        fs::write(&sibling, "v2").expect("write sibling");
        fs::rename(&sibling, &source).expect("rename over target");
        wait_for_condition(
            || commits.load(Ordering::SeqCst) >= 1,
            Duration::from_secs(3),
        )
        .await;

        fs::write(&source, "v3").expect("edit target");
        wait_for_condition(
            || commits.load(Ordering::SeqCst) >= 2,
            Duration::from_secs(3),
        )
        .await;
        assert_eq!(
            commits.load(Ordering::SeqCst),
            2,
            "replace then edit must yield exactly two dispatches"
        );

        stop_watcher(shutdown, handle).await;
    }

    #[tokio::test]
    async fn new_parent_subscribed_after_closure_growth() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let watch_a = tmp.path().join("watch_a");
        let watch_b = tmp.path().join("watch_b");
        fs::create_dir_all(&watch_a).expect("create watch_a");
        fs::create_dir_all(&watch_b).expect("create watch_b");
        let a_source = watch_a.join("page.jinja");
        let b_source = watch_b.join("new.jinja");
        fs::write(&a_source, "a1").expect("seed a");
        fs::write(&b_source, "b1").expect("seed b");

        let route = "growth-target-route";
        let target = FakeReloadTarget::new(route, vec![a_source.clone()]);
        target.set_pending(vec![a_source.clone(), b_source.clone()]);
        let commits = Arc::clone(&target.commits);
        let _guard = TemplateReloadRegistry::global().register(target.as_dyn());

        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");
        let (shutdown, ready_rx, mut version_rx, handle) = spawn_watcher(
            ctx.runtime_execution_handle(),
            Vec::new(),
            route.to_string(),
            || Ok(vec![]),
        );
        await_ready(ready_rx).await;
        let base_version = *version_rx.borrow();

        fs::write(&a_source, "a2").expect("edit tracked source");
        wait_for_condition(
            || commits.load(Ordering::SeqCst) >= 1,
            Duration::from_secs(3),
        )
        .await;
        assert_eq!(
            commits.load(Ordering::SeqCst),
            1,
            "first edit must dispatch exactly once"
        );

        // The commit grew the closure into watch_b/, so the wrapper must have
        // reconciled a subscription there before we write.
        await_version_after(&mut version_rx, base_version).await;

        fs::write(&b_source, "b2").expect("edit newly watched source");
        wait_for_condition(
            || commits.load(Ordering::SeqCst) >= 2,
            Duration::from_secs(3),
        )
        .await;

        stop_watcher(shutdown, handle).await;
    }

    #[tokio::test]
    async fn route_pass_growth_subscribes_new_parent() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let watch_a = tmp.path().join("watch_a");
        let watch_b = tmp.path().join("watch_b");
        let routes = tmp.path().join("routes");
        fs::create_dir_all(&watch_a).expect("create watch_a");
        fs::create_dir_all(&watch_b).expect("create watch_b");
        fs::create_dir_all(&routes).expect("create routes");
        let a_source = watch_a.join("page.jinja");
        let b_source = watch_b.join("new.jinja");
        fs::write(&a_source, "a1").expect("seed a");
        fs::write(&b_source, "b1").expect("seed b");

        let route = "route-pass-growth-route";
        let target = FakeReloadTarget::new(route, vec![a_source.clone()]);
        target.set_pending(vec![a_source.clone(), b_source.clone()]);
        let commits = Arc::clone(&target.commits);
        let _guard = TemplateReloadRegistry::global().register(target.as_dyn());

        // The fake's committed closure grows when the route pass runs.
        let discover_target = Arc::clone(&target);
        let discover = move || {
            discover_target.install_pending();
            Ok(vec![])
        };

        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("context build");
        let (shutdown, ready_rx, mut version_rx, handle) = spawn_watcher(
            ctx.runtime_execution_handle(),
            vec![routes.clone()],
            route.to_string(),
            discover,
        );
        await_ready(ready_rx).await;
        let base_version = *version_rx.borrow();

        fs::write(routes.join("r.yaml"), "routes: []").expect("write route file");
        await_version_after(&mut version_rx, base_version).await;

        // The route-pass re-derivation must have reconciled watch_b/.
        fs::write(&b_source, "b2").expect("edit newly watched source");
        wait_for_condition(
            || commits.load(Ordering::SeqCst) >= 1,
            Duration::from_secs(3),
        )
        .await;

        stop_watcher(shutdown, handle).await;
    }
}
