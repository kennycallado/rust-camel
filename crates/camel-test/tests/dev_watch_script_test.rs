//! Dev-watch end-to-end integration tests for `script:` `source_file`
//! closures (OpenSpec scriptfile, Task 4.1).
//!
//! Each test boots a real `CamelContext` from a tempdir route project and
//! starts the production watcher loop
//! (`camel_core::reload_watcher::watch_and_reload_with_barriers`, the same
//! loop `camel run --watch` uses) with a production-shaped provider that
//! snapshots `ScriptWatchRegistry`. Real filesystem writes then exercise the
//! reload path end to end: a script edit runs a full discovery pass, the
//! document hash (which folds script contents) changes, and the route swaps.
//!
//! The staged set in `ScriptWatchRegistry` is process-global and a discovery
//! run clears it, so tests in this binary are serialized by `SCRIPT_WATCH_LOCK`
//! (production has exactly one watcher; parallel tests would otherwise clear
//! each other's in-flight staging). Route ids are additionally unique per
//! test.
//!
//! `wait_for` / `wait_for_marker` poll under bounded `tokio::time::timeout`
//! chunks with `tokio::task::yield_now` — no sleeps.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange, Message, Value};
use camel_component_api::test_support::{TEST_LOCK_DEADLINE, acquire_deadline};
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;
use camel_core::reload_watcher::watch_and_reload_with_barriers;
use camel_language_rhai::RhaiLanguage;
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// Serializes this binary's tests: they share the process-global staged set.
static SCRIPT_WATCH_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

/// Upper bound for a single condition wait.
const WAIT_BOUND: Duration = Duration::from_secs(5);
/// Debounce window for the watcher under test.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// Length of one bounded poll chunk.
const POLL_STEP: Duration = Duration::from_millis(10);

const SCRIPT_V1: &str = "headers[\"marker\"] = \"v1\";";
const SCRIPT_V2: &str = "headers[\"marker\"] = \"v2\";";
const SCRIPT_A_V1: &str = "headers[\"marker\"] = \"a1\";";
const SCRIPT_A_V2: &str = "headers[\"marker\"] = \"a2\";";
const SCRIPT_B_V1: &str = "headers[\"marker\"] = \"b1\";";
const SCRIPT_B_V2: &str = "headers[\"marker\"] = \"b2\";";

/// Publish hook capture: `(rebuilt, removed)` per pass, forwarded to the real
/// registry so the watch set advances exactly like production.
type PublishLog = Arc<Mutex<Vec<(Vec<String>, Vec<String>)>>>;

/// Poll `cond` until it returns `true` or `bound` elapses.
async fn wait_for(mut cond: impl FnMut() -> bool, bound: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + bound;
    loop {
        // allow-test-wait: bounded by `bound`; each poll step is a 10 ms timeout chunk
        if cond() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        poll_step().await;
    }
}

/// Yield once, then park until the bounded poll chunk elapses.
async fn poll_step() {
    let chunk = async {
        tokio::task::yield_now().await;
        std::future::pending::<()>().await;
    };
    let _ = tokio::time::timeout(POLL_STEP, chunk).await;
}

/// Poll the route's marker header until it equals `expected` or `bound`
/// elapses. The exchange round-trip also retries the documented `direct:in`
/// consumer-registration race, so a swap in progress cannot fail the wait.
async fn wait_for_marker(ctx: &CamelContext, uri: &str, expected: &str, bound: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + bound;
    loop {
        // allow-test-wait: bounded by `bound`; each poll step is a 10 ms timeout chunk
        if let Ok(Some(marker)) = marker_once(ctx, uri).await
            && marker == expected
        {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        poll_step().await;
    }
}

/// A booted context plus its running watcher.
struct Harness {
    _project: tempfile::TempDir,
    project_root: PathBuf,
    ctx: CamelContext,
    shutdown: CancellationToken,
    watcher: JoinHandle<Result<(), CamelError>>,
    publish_log: PublishLog,
}

impl Harness {
    async fn stop(self) {
        self.shutdown.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), self.watcher)
            .await
            .expect("watcher must stop after shutdown")
            .expect("watcher task must not panic");
        assert!(joined.is_ok(), "watcher returned an error: {joined:?}");
        let mut ctx = self.ctx;
        ctx.stop().await.expect("context stop");
    }

    /// Number of publish-hook invocations recorded so far.
    fn publish_count(&self) -> usize {
        self.publish_log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// True when any recorded pass rebuilt `route_id`.
    fn was_rebuilt(&self, route_id: &str) -> bool {
        self.publish_log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|(rebuilt, _)| rebuilt.iter().any(|id| id == route_id))
    }
}

/// Write a route document and its named script files under `project`.
fn write_project(
    project: &tempfile::TempDir,
    documents: &[(&str, &str)],
    scripts: &[(&str, &str)],
) -> PathBuf {
    let root = project.path().to_path_buf();
    std::fs::create_dir_all(root.join("routes")).expect("create routes dir");
    std::fs::create_dir_all(root.join("scripts")).expect("create scripts dir");
    for (name, contents) in scripts {
        std::fs::write(root.join("scripts").join(name), contents).expect("write script"); // allow-unwrap
    }
    for (name, contents) in documents {
        std::fs::write(root.join("routes").join(name), contents).expect("write document"); // allow-unwrap
    }
    root
}

/// Route document referencing `script` (relative to the document's directory).
fn document(route_id: &str, from: &str, script: &str) -> String {
    format!(
        "routes:\n  - id: {route_id}\n    from: \"{from}\"\n    steps:\n      - script:\n          source_file: \"../scripts/{script}\"\n"
    )
}

/// Start a context from the route documents in `project/routes`, publish the
/// startup watch set (the production startup publication), then start the
/// watcher with the production-shaped script provider and await readiness.
async fn start_harness(project: tempfile::TempDir) -> Harness {
    let project_root = project.path().to_path_buf();
    let routes_dir = project_root.join("routes");
    let pattern = format!("{}/routes/*.yaml", project_root.display());

    let defs = camel_dsl::discover_routes(std::slice::from_ref(&pattern))
        .expect("initial discovery must succeed");
    let route_ids: Vec<String> = defs.iter().map(|def| def.route_id().to_string()).collect();
    assert!(!route_ids.is_empty(), "project must declare routes");

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(DirectComponent::new());
    if ctx.resolve_language("rhai").is_none() {
        ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
            .expect("rhai language registration");
    }
    for def in defs {
        ctx.add_route_definition(def)
            .await
            .expect("add route definition");
    }
    ctx.start().await.expect("context start");

    // Mirror the production startup publication: initial loading is
    // fail-closed, so every staged dependency belongs to a running route.
    camel_dsl::ScriptWatchRegistry::global().publish_commit(&route_ids, &[]);
    let published = camel_dsl::ScriptWatchRegistry::global().route_watch_sources();
    for route_id in &route_ids {
        assert!(
            published
                .get(route_id)
                .is_some_and(|paths| !paths.is_empty()),
            "startup publication must seed script dependencies for '{route_id}': {published:?}"
        );
    }

    let (ready_tx, mut ready_rx) = watch::channel(false);
    let (version_tx, _version_rx) = watch::channel(0u64);
    let shutdown = CancellationToken::new();
    let publish_log: PublishLog = Arc::new(Mutex::new(Vec::new()));

    let provider_ids = route_ids.clone();
    let provider = move || {
        camel_dsl::ScriptWatchRegistry::global()
            .route_watch_sources()
            .into_iter()
            .filter(|(id, _)| provider_ids.contains(id))
            .map(|(id, paths)| {
                (
                    id,
                    paths
                        .into_iter()
                        .map(|path| (path, camel_core::reload_watcher::DevSourceKind::Script))
                        .collect::<Vec<(PathBuf, camel_core::reload_watcher::DevSourceKind)>>(),
                )
            })
            .collect::<Vec<(
                String,
                Vec<(PathBuf, camel_core::reload_watcher::DevSourceKind)>,
            )>>()
    };

    let log_sink = Arc::clone(&publish_log);
    let publish = move |rebuilt: &[String], removed: &[String]| {
        log_sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((rebuilt.to_vec(), removed.to_vec()));
        camel_dsl::ScriptWatchRegistry::global().publish_commit(rebuilt, removed);
    };

    let discover_pattern = pattern.clone();
    let watcher = tokio::spawn(watch_and_reload_with_barriers(
        vec![routes_dir],
        ctx.runtime_execution_handle(),
        move || {
            camel_dsl::discover_routes(std::slice::from_ref(&discover_pattern))
                .map_err(|e| CamelError::RouteError(e.to_string()))
        },
        provider,
        publish,
        Some(shutdown.clone()),
        Duration::from_secs(5),
        DEBOUNCE,
        ready_tx,
        version_tx,
    ));

    let ready = tokio::time::timeout(WAIT_BOUND, ready_rx.wait_for(|value| *value))
        .await
        .expect("watcher readiness barrier must fire")
        .expect("watcher readiness sender must stay alive");
    assert!(*ready, "watcher readiness barrier must report ready");

    Harness {
        _project: project,
        project_root,
        ctx,
        shutdown,
        watcher,
        publish_log,
    }
}

/// Send one empty-body exchange through `uri` and read the `marker` header.
async fn marker_once(ctx: &CamelContext, uri: &str) -> Result<Option<String>, CamelError> {
    let producer = {
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component registered")
            .clone();
        let endpoint = component.create_endpoint(uri, ctx)?;
        let runtime: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
        endpoint.create_producer(runtime, &producer_ctx)?
    };
    let exchange = producer
        .oneshot(Exchange::new(Message::new(Body::Empty)))
        .await?;
    Ok(exchange
        .input
        .header("marker")
        .and_then(|value| match value {
            Value::String(marker) => Some(marker.clone()),
            _ => None,
        }))
}

/// Render the current route output, retrying only the documented `direct:in`
/// consumer-registration race. Any other failure panics.
async fn marker(ctx: &CamelContext, uri: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        // allow-test-wait: bounded 2 s retry for the documented direct:in registration race
        match marker_once(ctx, uri).await {
            Ok(Some(marker)) => return marker,
            Ok(None) => panic!("route did not set the marker header"),
            Err(err)
                if is_direct_not_registered(&err) && tokio::time::Instant::now() < deadline =>
            {
                tokio::task::yield_now().await;
            }
            Err(err) => panic!("render failed: {err}"),
        }
    }
}

fn is_direct_not_registered(err: &CamelError) -> bool {
    matches!(
        err,
        CamelError::EndpointCreationFailed(msg)
            if msg.starts_with("direct endpoint '") && msg.ends_with("' not registered")
    )
}

/// Published script dependency of `route_id` (must exist).
fn published_sources(route_id: &str) -> Vec<PathBuf> {
    camel_dsl::ScriptWatchRegistry::global()
        .route_watch_sources()
        .remove(route_id)
        .map(|paths| paths.into_iter().collect())
        .unwrap_or_default()
}

async fn route_status(ctx: &CamelContext, route_id: &str) -> String {
    ctx.runtime_route_status(route_id)
        .await
        .expect("route status query")
        .expect("route must exist")
}

/// (a) A script-only edit runs the reload pass and the route swaps to the
/// generation built from the new contents; (b) a script edit restored to its
/// previous bytes inside one debounce window leaves the route on its committed
/// generation (the folded hash is unchanged, so the route is skipped).
#[tokio::test(flavor = "multi_thread")]
async fn script_edit_swaps_route_and_restored_content_skips() {
    let _serial = acquire_deadline(
        &SCRIPT_WATCH_LOCK,
        "SCRIPT_WATCH_LOCK (dev watch script)",
        TEST_LOCK_DEADLINE,
    )
    .await;

    let route_id = "dev-watch-script-swap-restore";
    let project = tempfile::tempdir().expect("project tempdir");
    write_project(
        &project,
        &[("doc.yaml", &document(route_id, "direct:in", "enrich.rhai"))],
        &[("enrich.rhai", SCRIPT_V1)],
    );
    let harness = start_harness(project).await;
    let script_path = std::fs::canonicalize(harness.project_root.join("scripts/enrich.rhai"))
        .expect("canonicalize script");

    assert_eq!(marker(&harness.ctx, "direct:in").await, "v1");
    assert!(
        published_sources(route_id).contains(&script_path),
        "the resolved script path must be the published dependency"
    );

    // (b) Edit then restore identical bytes inside one debounce window. Both
    // writes land before the debounce closes, so discovery reads SCRIPT_V1 and
    // the folded hash equals the committed hash: the route is skipped.
    let passes_before_restore = harness.publish_count();
    std::fs::write(harness.project_root.join("scripts/enrich.rhai"), SCRIPT_V2)
        .expect("edit script");
    std::fs::write(harness.project_root.join("scripts/enrich.rhai"), SCRIPT_V1)
        .expect("restore script");
    assert!(
        wait_for(
            || harness.publish_count() > passes_before_restore,
            WAIT_BOUND
        )
        .await,
        "the restored-content batch must still run one reload pass"
    );
    let restored_pass = harness
        .publish_log
        .lock()
        .unwrap_or_else(PoisonError::into_inner)[passes_before_restore..]
        .to_vec();
    assert!(
        restored_pass.iter().all(|(rebuilt, removed)| {
            !rebuilt.contains(&route_id.to_string()) && !removed.contains(&route_id.to_string())
        }),
        "identical restored content must not swap the route, got pass outcomes: {restored_pass:?}"
    );
    assert_eq!(
        marker(&harness.ctx, "direct:in").await,
        "v1",
        "the route keeps its committed generation after the restored edit"
    );

    // (a) Script-only edit: a full reload pass, then a route swap.
    std::fs::write(harness.project_root.join("scripts/enrich.rhai"), SCRIPT_V2)
        .expect("edit script");
    assert!(
        wait_for(|| harness.was_rebuilt(route_id), WAIT_BOUND).await,
        "a script-only edit must rebuild the route"
    );
    assert!(
        wait_for_marker(&harness.ctx, "direct:in", "v2", WAIT_BOUND).await,
        "the route must swap to the generation built from the new script contents"
    );
    assert_eq!(
        route_status(&harness.ctx, route_id).await,
        "Started",
        "the route must stay running across a script-driven swap"
    );
    assert!(
        published_sources(route_id).contains(&script_path),
        "the rebuilt route keeps the same script path published"
    );

    harness.stop().await;
}

/// (c) Partial success: route A's document is edited to an unknown language
/// (route build fails) while route B's script is edited (swap succeeds) in the
/// same debounce window. B publishes its dependency and swaps.
///
/// A is covered by two separate facts — neither is "A keeps running":
///
/// - Dependency retention: A's published script dependency survives the pass.
///   A is never rebuilt (an action that reported an error is excluded from the
///   rebuilt list), and the internal `RemoveRoute` inside `apply_restart` is
///   not a `ReloadAction::Remove`, so it cannot surface as a committed
///   removal either.
/// - Route serving: A's runtime route IS stopped by the failed restart — core
///   stops it and issues that internal removal before the re-add fails. A
///   later successful pass republishes it.
#[tokio::test(flavor = "multi_thread")]
async fn partial_success_keeps_failed_route_script_dep() {
    let _serial = acquire_deadline(
        &SCRIPT_WATCH_LOCK,
        "SCRIPT_WATCH_LOCK (dev watch script)",
        TEST_LOCK_DEADLINE,
    )
    .await;

    let route_a = "dev-watch-script-partial-failed";
    let route_b = "dev-watch-script-partial-swapped";
    let project = tempfile::tempdir().expect("project tempdir");
    write_project(
        &project,
        &[
            ("a.yaml", &document(route_a, "direct:a-in", "a_v1.rhai")),
            ("b.yaml", &document(route_b, "direct:b-in", "b.rhai")),
        ],
        &[
            ("a_v1.rhai", SCRIPT_A_V1),
            ("a_v2.rhai", SCRIPT_A_V2),
            ("b.rhai", SCRIPT_B_V1),
        ],
    );
    let harness = start_harness(project).await;
    let a_v1 = std::fs::canonicalize(harness.project_root.join("scripts/a_v1.rhai"))
        .expect("canonicalize a_v1");
    let a_v2 = std::fs::canonicalize(harness.project_root.join("scripts/a_v2.rhai"))
        .expect("canonicalize a_v2");
    let b_path =
        std::fs::canonicalize(harness.project_root.join("scripts/b.rhai")).expect("canonicalize b");

    assert_eq!(marker(&harness.ctx, "direct:a-in").await, "a1");
    assert_eq!(marker(&harness.ctx, "direct:b-in").await, "b1");
    assert!(published_sources(route_a).contains(&a_v1));
    assert!(published_sources(route_b).contains(&b_path));

    // Same debounce window: A's document now references an unregistered
    // language (route build fails), B's script content changes (swap).
    std::fs::write(
        harness.project_root.join("routes/a.yaml"),
        format!(
            "routes:\n  - id: {route_a}\n    from: \"direct:a-in\"\n    steps:\n      - script:\n          language: nonexistent-lang\n          source_file: \"../scripts/a_v2.rhai\"\n"
        ),
    )
    .expect("edit route A document");
    std::fs::write(harness.project_root.join("scripts/b.rhai"), SCRIPT_B_V2)
        .expect("edit route B script");

    assert!(
        wait_for_marker(&harness.ctx, "direct:b-in", "b2", WAIT_BOUND).await,
        "the valid route must swap even though the other route's restart failed"
    );
    // A's previously published dependency is retained; the newly staged
    // a_v2.rhai was never promoted because no swap committed for A.
    let a_sources = published_sources(route_a);
    assert!(
        a_sources.contains(&a_v1),
        "the failed route retains its published script dependency, got {a_sources:?}"
    );
    assert!(
        !a_sources.contains(&a_v2),
        "the failed route must not publish its staged dependency, got {a_sources:?}"
    );
    // A never appears as rebuilt in any pass: its rebuild failed while B's
    // succeeded, so the published dependency above is a genuinely retained
    // entry, not a fresh promotion.
    let rebuilt_passes = harness
        .publish_log
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .map(|(rebuilt, _)| rebuilt.clone())
        .collect::<Vec<_>>();
    assert!(
        rebuilt_passes
            .iter()
            .any(|rebuilt| rebuilt.contains(&route_b.to_string())),
        "the valid route must be published as rebuilt, got {rebuilt_passes:?}"
    );
    assert!(
        rebuilt_passes
            .iter()
            .all(|rebuilt| !rebuilt.contains(&route_a.to_string())),
        "the compile-failed route must never be published as rebuilt, got {rebuilt_passes:?}"
    );
    // B swapped, so its dependency is published.
    assert!(
        published_sources(route_b).contains(&b_path),
        "the swapped route publishes its script dependency"
    );

    harness.stop().await;
}
