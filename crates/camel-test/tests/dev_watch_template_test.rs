//! Dev-watch end-to-end integration tests for `template:` closure files
//! (OpenSpec 476-template-proto-dev-reload, Task 4.1).
//!
//! Each test boots a real `CamelContext` with a `direct:in → template:file:`
//! route and starts the production watcher wiring
//! (`camel_core::reload_watcher::watch_and_reload_with_barriers`, the same
//! loop `camel run --watch` uses) with the production-shaped dev provider that
//! snapshots the global `TemplateReloadRegistry`. Real filesystem writes then
//! exercise the reload path end to end.
//!
//! `wait_for` polls under bounded `tokio::time::timeout` chunks with
//! `tokio::task::yield_now` — no sleeps.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::template_reload::TemplateReloadRegistry;
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_core::reload_watcher::DevSourceKind;
use camel_core::reload_watcher::watch_and_reload_with_barriers;
use camel_core::{CamelContext, InMemoryRuntimeStore, RuntimeEvent};
use camel_template::{AutoescapeMode, TemplateComponent};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// No-op script publication hook: this scenario watches template assets only.
fn ignore_script_publish(_rebuilt: &[String], _removed: &[String]) {}

/// Upper bound for a single condition wait. Real notify delivery plus the
/// debounce window fits comfortably; a miss is a failed reload.
const WAIT_BOUND: Duration = Duration::from_secs(5);
/// Debounce window for the watcher under test.
const DEBOUNCE: Duration = Duration::from_millis(300);

const PAGE_V1: &str = "page-v1 {% include \"part/head.jinja\" %}";
const PAGE_V2: &str = "page-v2 {% include \"part/head.jinja\" %}";
const PAGE_V3: &str = "page-v3 {% include \"part/head.jinja\" %}";
const PAGE_INVALID: &str = "{% if broken %}no endif";
const HEAD_V1: &str = "head-v1";
const HEAD_V2: &str = "head-v2";

/// Poll `cond` until it returns `true` or `bound` elapses.
///
/// Time advances through bounded `timeout` chunks; a `yield_now` keeps the
/// runtime scheduling promptly. No sleep calls, so `lint-test-sleep` stays
/// clean.
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
        let chunk = async {
            tokio::task::yield_now().await;
            std::future::pending::<()>().await;
        };
        let _ = tokio::time::timeout(Duration::from_millis(10), chunk).await;
    }
}

/// A booted context plus its running watcher.
struct Harness {
    _root_dir: tempfile::TempDir,
    _routes_dir: tempfile::TempDir,
    _version_rx: watch::Receiver<u64>,
    root: PathBuf,
    ctx: CamelContext,
    /// The context's append-only lifecycle-event store, used to prove that a
    /// template reload never stops/starts the route.
    store: InMemoryRuntimeStore,
    shutdown: CancellationToken,
    watcher: JoinHandle<Result<(), CamelError>>,
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

    /// Number of lifecycle events recorded so far (appended monotonically).
    async fn event_count(&self) -> usize {
        self.store.snapshot_events().await.len()
    }

    /// Lifecycle transitions recorded for `route_id` strictly after the first
    /// `since` events. Any stop/start cycle or reload event lands here.
    async fn transitions_since(&self, route_id: &str, since: usize) -> Vec<RuntimeEvent> {
        self.store
            .snapshot_events()
            .await
            .into_iter()
            .skip(since)
            .filter(|event| event_route_id(event) == Some(route_id))
            .collect()
    }
}

/// Build the template root (`page.jinja` including `part/head.jinja`), start a
/// `direct:in → template:file:page.jinja` route, then start the dev watcher and
/// await its readiness barrier before returning.
async fn start_harness(route_id: &str, page: &str, head: &str) -> Harness {
    let root_dir = tempfile::tempdir().expect("template root tempdir");
    let root = root_dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("part")).expect("create part dir");
    std::fs::write(root.join("page.jinja"), page).expect("write page");
    std::fs::write(root.join("part/head.jinja"), head).expect("write head");

    let routes_dir = tempfile::tempdir().expect("routes tempdir");

    let store = InMemoryRuntimeStore::default();
    let mut ctx = CamelContext::builder()
        .runtime_store(store.clone())
        .build()
        .await
        .expect("context build");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(
        TemplateComponent::default()
            .with_root(root.clone())
            .with_default_autoescape(AutoescapeMode::None),
    );
    let route = RouteBuilder::from("direct:in")
        .route_id(route_id)
        .to("template:file:page.jinja".to_string())
        .build()
        .expect("route build");
    ctx.add_route_definition(route)
        .await
        .expect("add route definition");
    ctx.start().await.expect("context start");

    assert!(
        !TemplateReloadRegistry::global()
            .find_all(route_id)
            .is_empty(),
        "route start must register a template reload target before the watcher derives its map"
    );

    let (ready_tx, mut ready_rx) = watch::channel(false);
    let (version_tx, version_rx) = watch::channel(0u64);
    let shutdown = CancellationToken::new();
    let provider_route = route_id.to_string();
    let provider = move || {
        TemplateReloadRegistry::global()
            .route_watch_sources()
            .into_iter()
            .filter(|(id, _)| id == &provider_route)
            .map(|(id, paths)| {
                (
                    id,
                    paths
                        .into_iter()
                        .map(|path| (path, DevSourceKind::Template))
                        .collect::<Vec<(PathBuf, DevSourceKind)>>(),
                )
            })
            .collect::<Vec<(String, Vec<(PathBuf, DevSourceKind)>)>>()
    };
    let watch_dirs = vec![routes_dir.path().to_path_buf()];
    let watcher = tokio::spawn(watch_and_reload_with_barriers(
        watch_dirs,
        ctx.runtime_execution_handle(),
        || Ok(Vec::new()),
        provider,
        ignore_script_publish,
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
        _root_dir: root_dir,
        _routes_dir: routes_dir,
        _version_rx: version_rx,
        root,
        ctx,
        store,
        shutdown,
        watcher,
    }
}

/// Render one empty-body exchange through the route's `direct:in` producer.
async fn render_once(ctx: &CamelContext) -> Result<String, CamelError> {
    let producer = {
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component registered")
            .clone();
        let endpoint = component.create_endpoint("direct:in", ctx)?;
        let runtime: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
        endpoint.create_producer(runtime, &producer_ctx)?
    };
    let exchange = producer
        .oneshot(Exchange::new(Message::new(Body::Empty)))
        .await?;
    match &exchange.input.body {
        Body::Text(text) => Ok(text.clone()),
        Body::Bytes(bytes) => Ok(String::from_utf8_lossy(bytes).into_owned()),
        other => panic!("expected a rendered text body, got: {other:?}"),
    }
}

/// Render the current route output, retrying only the documented `direct:in`
/// consumer-registration race. Any other failure panics.
async fn render(ctx: &CamelContext) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        // allow-test-wait: bounded 2 s retry for the documented direct:in registration race
        match render_once(ctx).await {
            Ok(body) => return body,
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

/// Committed generation of the (single) target registered for `route_id`.
fn generation(route_id: &str) -> u64 {
    TemplateReloadRegistry::global()
        .find_all(route_id)
        .first()
        .map(|target| target.current_generation())
        .expect("route must have a registered template reload target")
}

async fn route_status(ctx: &CamelContext, route_id: &str) -> String {
    ctx.runtime_route_status(route_id)
        .await
        .expect("route status query")
        .expect("route must exist")
}

/// Route id carried by a lifecycle event.
fn event_route_id(event: &RuntimeEvent) -> Option<&str> {
    match event {
        RuntimeEvent::RouteRegistered { route_id }
        | RuntimeEvent::RouteStartRequested { route_id }
        | RuntimeEvent::RouteStarted { route_id }
        | RuntimeEvent::RouteFailed { route_id, .. }
        | RuntimeEvent::RouteStopped { route_id }
        | RuntimeEvent::RouteSuspended { route_id }
        | RuntimeEvent::RouteResumed { route_id }
        | RuntimeEvent::RouteReloaded { route_id }
        | RuntimeEvent::RouteRemoved { route_id } => Some(route_id),
    }
}

/// Append-only capture sink for WARN+ records. Installed once per test binary
/// so events emitted from watcher tasks (running on other runtime threads) are
/// observed even though `set_default` is thread-local.
fn shared_warn_capture() -> Arc<Mutex<Vec<u8>>> {
    #[derive(Clone)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    static BUFFER: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    BUFFER
        .get_or_init(|| {
            let buffer = Arc::new(Mutex::new(Vec::new()));
            let _ = tracing_subscriber::fmt()
                .with_max_level(tracing_subscriber::filter::LevelFilter::WARN)
                .with_ansi(false)
                .with_writer(SharedWriter(Arc::clone(&buffer)))
                .try_init();
            buffer
        })
        .clone()
}

/// True when a single captured log line contains every `needle`.
///
/// Requiring both needles on the SAME line ties the route marker to the error
/// rendering, so an unrelated line cannot satisfy a multi-part assertion.
fn captured_event_with(buffer: &Arc<Mutex<Vec<u8>>>, needles: &[&str]) -> bool {
    let bytes = buffer.lock().expect("log buffer poisoned");
    String::from_utf8_lossy(&bytes)
        .lines()
        .any(|line| needles.iter().all(|needle| line.contains(needle)))
}

/// Editing the entry changes the rendered output without restarting the route
/// (generation advances monotonically while the route records no lifecycle
/// transition at all).
#[tokio::test(flavor = "multi_thread")]
async fn template_edit_changes_output_without_restart() {
    let route_id = "dev-watch-template-edit";
    let harness = start_harness(route_id, PAGE_V1, HEAD_V1).await;

    let initial = render(&harness.ctx).await;
    assert!(initial.contains("page-v1"), "initial render: {initial:?}");
    assert!(initial.contains("head-v1"), "initial include: {initial:?}");
    let status_before = route_status(&harness.ctx, route_id).await;
    assert_eq!(
        status_before, "Started",
        "route must be serving before edit"
    );
    let gen_before = generation(route_id);
    // Baseline after the route is fully started: every lifecycle event already
    // emitted is behind this mark; anything after it happened in the window.
    let events_before = harness.event_count().await;

    std::fs::write(harness.root.join("page.jinja"), PAGE_V2).expect("edit page");

    assert!(
        wait_for(|| generation(route_id) > gen_before, WAIT_BOUND).await,
        "entry edit must commit a new generation"
    );

    let updated = render(&harness.ctx).await;
    assert!(updated.contains("page-v2"), "new entry output: {updated:?}");
    assert!(
        updated.contains("head-v1"),
        "unchanged include still renders: {updated:?}"
    );

    let status_after = route_status(&harness.ctx, route_id).await;
    assert_eq!(
        status_after, status_before,
        "a template reload must not restart the route"
    );
    // Events are append-only, so a stop/start cycle hidden between the two
    // status snapshots still leaves its RouteStopped/RouteStarted trail here.
    let transitions = harness.transitions_since(route_id, events_before).await;
    assert!(
        transitions.is_empty(),
        "a template reload must not stop, restart or otherwise transition the route; \
         observed lifecycle events: {transitions:?}"
    );
    assert_eq!(
        generation(route_id),
        gen_before + 1,
        "one entry edit must commit exactly one generation"
    );

    harness.stop().await;
}

/// Editing only a nested include reloads the route and surfaces the include
/// change in the rendered output.
#[tokio::test(flavor = "multi_thread")]
async fn nested_include_edit_triggers_reload() {
    let route_id = "dev-watch-template-nested";
    let harness = start_harness(route_id, PAGE_V1, HEAD_V1).await;

    let initial = render(&harness.ctx).await;
    assert!(initial.contains("head-v1"), "initial include: {initial:?}");
    let gen_before = generation(route_id);

    std::fs::write(harness.root.join("part/head.jinja"), HEAD_V2).expect("edit nested include");

    assert!(
        wait_for(|| generation(route_id) > gen_before, WAIT_BOUND).await,
        "nested include edit must commit a new generation"
    );

    let updated = render(&harness.ctx).await;
    assert!(
        updated.contains("head-v2"),
        "include change renders: {updated:?}"
    );
    assert!(
        updated.contains("page-v1"),
        "entry is unchanged: {updated:?}"
    );

    harness.stop().await;
}

/// Two writes inside one debounce window coalesce into exactly one reload: the
/// generation delta is exactly one, not merely stable output.
#[tokio::test(flavor = "multi_thread")]
async fn rapid_edits_coalesce_in_debounce() {
    let route_id = "dev-watch-template-coalesce";
    let harness = start_harness(route_id, PAGE_V1, HEAD_V1).await;

    let initial = render(&harness.ctx).await;
    assert!(initial.contains("page-v1"), "initial render: {initial:?}");
    let gen_before = generation(route_id);

    // Back-to-back writes with no await between them: both land inside one
    // debounce window.
    std::fs::write(harness.root.join("page.jinja"), PAGE_V2).expect("first rapid edit");
    std::fs::write(harness.root.join("page.jinja"), PAGE_V3).expect("second rapid edit");

    assert!(
        wait_for(|| generation(route_id) > gen_before, WAIT_BOUND).await,
        "the coalesced batch must commit one reload"
    );

    // Settle well past the debounce window and the reload itself. A duplicate
    // dispatch (a second batch) would bump the generation again.
    assert!(
        !wait_for(|| false, DEBOUNCE * 3).await,
        "the bounded settle window must expire"
    );
    assert_eq!(
        generation(route_id),
        gen_before + 1,
        "rapid edits inside one debounce window must coalesce into exactly one reload"
    );

    let updated = render(&harness.ctx).await;
    assert!(
        updated.contains("page-v3"),
        "the last write wins after coalescing: {updated:?}"
    );

    harness.stop().await;
}

/// A failed reload keeps the last good output and the route alive; a later
/// valid edit recovers. The failure is proven by a captured WARN event whose
/// error is the `CamelError::TemplateReload` variant (rendered as
/// `Template reload failed:`), not by stable output alone.
#[tokio::test(flavor = "multi_thread")]
async fn failed_reload_keeps_output_and_route_alive() {
    let route_id = "dev-watch-template-failed";
    let harness = start_harness(route_id, PAGE_V1, HEAD_V1).await;
    let logs = shared_warn_capture();

    let initial = render(&harness.ctx).await;
    assert!(initial.contains("page-v1"), "initial render: {initial:?}");
    let gen_before = generation(route_id);

    std::fs::write(harness.root.join("page.jinja"), PAGE_INVALID).expect("write invalid syntax");

    // Bind BOTH the route marker and the `CamelError::TemplateReload` display
    // prefix on the same event line. A generic dispatch failure (any other
    // CamelError variant) does not render "Template reload failed:" and must
    // not satisfy this test.
    let failure_marker = format!("reload dispatch failed for route '{route_id}'");
    assert!(
        wait_for(
            || captured_event_with(&logs, &[failure_marker.as_str(), "Template reload failed:"]),
            WAIT_BOUND
        )
        .await,
        "a TemplateReload dispatch failure must be logged for route '{route_id}'"
    );

    assert_eq!(
        generation(route_id),
        gen_before,
        "a failed reload must not commit a generation"
    );
    let retained = render(&harness.ctx).await;
    assert!(
        retained.contains("page-v1"),
        "last good output must keep rendering: {retained:?}"
    );
    assert_eq!(
        route_status(&harness.ctx, route_id).await,
        "Started",
        "the route must stay alive after a failed reload"
    );

    std::fs::write(harness.root.join("page.jinja"), PAGE_V2).expect("write valid recovery");

    assert!(
        wait_for(|| generation(route_id) > gen_before, WAIT_BOUND).await,
        "a later valid edit must recover with a committed reload"
    );
    let recovered = render(&harness.ctx).await;
    assert!(
        recovered.contains("page-v2"),
        "recovered output must render the valid template: {recovered:?}"
    );

    harness.stop().await;
}
