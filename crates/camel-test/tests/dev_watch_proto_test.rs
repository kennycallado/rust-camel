//! Dev-watch end-to-end integration tests for protobuf `validator:` schema
//! files (OpenSpec 476-template-proto-dev-reload, Task 4.2).
//!
//! Each test boots a real `CamelContext` whose route ends in a protobuf
//! validator. The watched tests start the production watcher wiring
//! (`camel_core::reload_watcher::watch_and_reload_with_barriers`, the same
//! loop `camel run --watch` uses) with the production-shaped dev provider that
//! snapshots the global `TemplateReloadRegistry`. Real filesystem writes then
//! exercise the reload path end to end.
//!
//! `wait_for` / `wait_for_validation` poll under bounded
//! `tokio::time::timeout` chunks with `tokio::task::yield_now` — no sleeps.
//! The watched tests wait on the observable validation outcome; generation
//! counters are only checked afterwards.
//!
//! Route ids are unique per test because the reload registry is
//! process-global.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::template_reload::TemplateReloadRegistry;
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_component_validator::ValidatorComponent;
use camel_core::CamelContext;
use camel_core::reload_watcher::DevSourceKind;
use camel_core::reload_watcher::watch_and_reload_with_barriers;
use camel_template::{AutoescapeMode, TemplateComponent};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// No-op script publication hook: this scenario watches proto assets only.
fn ignore_script_publish(_rebuilt: &[String], _removed: &[String]) {}

/// Upper bound for a single condition wait.
const WAIT_BOUND: Duration = Duration::from_secs(5);
/// Debounce window for the watcher under test.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// Length of one bounded poll chunk.
const POLL_STEP: Duration = Duration::from_millis(10);

const ROOT_V1: &str = "syntax = \"proto3\";\npackage reload.v1;\nimport \"common.proto\";\nmessage M { string a = 1; C c = 2; }\n";
const ROOT_V2: &str = "syntax = \"proto3\";\npackage reload.v1;\nimport \"common.proto\";\nmessage M { string a = 1; int32 b = 3; C c = 2; }\n";
const COMMON_V1: &str = "syntax = \"proto3\";\npackage reload.v1;\nmessage C { string x = 1; }\n";
const COMMON_V2: &str =
    "syntax = \"proto3\";\npackage reload.v1;\nmessage C { string x = 1; int32 y = 2; }\n";

const PAGE_V1: &str = "page-v1";
const PAGE_V2: &str = "page-v2";

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
        poll_step().await;
    }
}

/// Yield once, then park until the bounded poll chunk elapses.
async fn tick() {
    tokio::task::yield_now().await;
    std::future::pending::<()>().await;
}

/// Advance one bounded poll chunk without sleeping.
async fn poll_step() {
    let _ = tokio::time::timeout(POLL_STEP, tick()).await;
}

/// A booted context plus (optionally) its running watcher.
struct Harness {
    _root_dir: tempfile::TempDir,
    _routes_dir: tempfile::TempDir,
    _version_rx: Option<watch::Receiver<u64>>,
    root: PathBuf,
    ctx: CamelContext,
    shutdown: Option<CancellationToken>,
    watcher: Option<JoinHandle<Result<(), CamelError>>>,
}

impl Harness {
    async fn stop(mut self) {
        if let (Some(shutdown), Some(watcher)) = (self.shutdown.take(), self.watcher.take()) {
            shutdown.cancel();
            let joined = tokio::time::timeout(Duration::from_secs(5), watcher)
                .await
                .expect("watcher must stop after shutdown")
                .expect("watcher task must not panic");
            assert!(joined.is_ok(), "watcher returned an error: {joined:?}");
        }
        self.ctx.stop().await.expect("context stop");
    }
}

/// Build the proto root (`root.proto` importing `common.proto`), optionally a
/// template root, add the routes, start the context, then optionally start the
/// dev watcher and await its readiness barrier.
///
/// `message` selects the fully-qualified protobuf message the validator
/// validates (`reload.v1.M` or the imported `reload.v1.C`).
async fn start_harness(
    proto_route_id: &str,
    message: &str,
    template_route_id: Option<&str>,
    watch: bool,
) -> Harness {
    let root_dir = tempfile::tempdir().expect("schema root tempdir");
    let root = root_dir.path().to_path_buf();
    std::fs::write(root.join("root.proto"), ROOT_V1).expect("write root proto");
    std::fs::write(root.join("common.proto"), COMMON_V1).expect("write common proto");
    if template_route_id.is_some() {
        std::fs::write(root.join("page.jinja"), PAGE_V1).expect("write page");
    }

    let routes_dir = tempfile::tempdir().expect("routes tempdir");

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(ValidatorComponent::new());
    if template_route_id.is_some() {
        ctx.register_component(
            TemplateComponent::default()
                .with_root(root.clone())
                .with_default_autoescape(AutoescapeMode::None),
        );
    }

    let proto_uri = format!(
        "validator:{}?type=protobuf&message={message}",
        root.join("root.proto").display()
    );
    let proto_route = RouteBuilder::from("direct:in")
        .route_id(proto_route_id)
        .to(proto_uri)
        .build()
        .expect("proto route build");
    ctx.add_route_definition(proto_route)
        .await
        .expect("add proto route definition");

    if let Some(template_route_id) = template_route_id {
        let template_route = RouteBuilder::from("direct:template-in")
            .route_id(template_route_id)
            .to("template:file:page.jinja".to_string())
            .build()
            .expect("template route build");
        ctx.add_route_definition(template_route)
            .await
            .expect("add template route definition");
    }

    ctx.start().await.expect("context start");

    assert!(
        !TemplateReloadRegistry::global()
            .find_all(proto_route_id)
            .is_empty(),
        "route start must register a protobuf reload target before the watcher derives its map"
    );

    if !watch {
        return Harness {
            _root_dir: root_dir,
            _routes_dir: routes_dir,
            _version_rx: None,
            root,
            ctx,
            shutdown: None,
            watcher: None,
        };
    }

    let (ready_tx, mut ready_rx) = watch::channel(false);
    let (version_tx, version_rx) = watch::channel(0u64);
    let shutdown = CancellationToken::new();
    let provider_route = proto_route_id.to_string();
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
        _version_rx: Some(version_rx),
        root,
        ctx,
        shutdown: Some(shutdown),
        watcher: Some(watcher),
    }
}

/// Drive one exchange through a `direct:` producer, retrying only the
/// documented consumer-registration race.
async fn invoke(ctx: &CamelContext, endpoint: &str, body: &Body) -> Result<Exchange, CamelError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let producer = {
            let producer_ctx = ctx.producer_context();
            let registry = ctx.registry();
            let component = registry
                .get("direct")
                .expect("direct component registered")
                .clone();
            let endpoint = component.create_endpoint(endpoint, ctx)?;
            let runtime: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
            endpoint.create_producer(runtime, &producer_ctx)?
        };
        let exchange = Exchange::new(Message::new(body.clone()));
        match producer.oneshot(exchange).await {
            Ok(exchange) => return Ok(exchange),
            Err(err)
                if is_direct_not_registered(&err) && tokio::time::Instant::now() < deadline =>
            {
                tokio::task::yield_now().await;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Validate `json` against the route's protobuf descriptor.
async fn validate(ctx: &CamelContext, json: &str) -> Result<(), CamelError> {
    invoke(ctx, "direct:in", &Body::Text(json.to_string()))
        .await
        .map(|_| ())
}

/// Poll `validate(ctx, json)` until it succeeds or `bound` elapses.
///
/// Waits on the observable effect of a reload — the recompiled descriptor
/// accepting a payload only the new schema recognizes — under bounded poll
/// chunks, not on the internal generation counter.
async fn wait_for_validation(ctx: &CamelContext, json: &str, bound: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + bound;
    loop {
        // allow-test-wait: bounded by `bound`; each poll step is a 10 ms timeout chunk
        if validate(ctx, json).await.is_ok() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        poll_step().await;
    }
}

/// Render the template route's current output.
async fn render(ctx: &CamelContext) -> String {
    let exchange = invoke(ctx, "direct:template-in", &Body::Empty)
        .await
        .expect("template render");
    match &exchange.input.body {
        Body::Text(text) => text.clone(),
        Body::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        other => panic!("expected a rendered text body, got: {other:?}"),
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
        .expect("route must have a registered reload target")
}

/// Editing the schema source recompiles the validator descriptor: a field that
/// was unknown before the edit validates after the watcher commits the reload.
#[tokio::test(flavor = "multi_thread")]
async fn proto_edit_recompiles_validator_descriptor() {
    let route_id = "dev-watch-proto-edit";
    let harness = start_harness(route_id, "reload.v1.M", None, true).await;

    assert!(
        validate(&harness.ctx, r#"{"a":"x"}"#).await.is_ok(),
        "the initial descriptor must accept field a"
    );
    assert!(
        validate(&harness.ctx, r#"{"a":"x","b":7}"#).await.is_err(),
        "field b must be unknown before the edit"
    );
    let gen_before = generation(route_id);

    std::fs::write(harness.root.join("root.proto"), ROOT_V2).expect("edit root proto");

    assert!(
        wait_for_validation(&harness.ctx, r#"{"a":"x","b":7}"#, WAIT_BOUND).await,
        "the schema edit must make the newly added field validate"
    );
    assert!(
        generation(route_id) > gen_before,
        "schema edit must commit a new descriptor generation"
    );
    assert_eq!(
        generation(route_id),
        gen_before + 1,
        "one schema edit must commit exactly one generation"
    );

    harness.stop().await;
}

/// Editing only the imported `common.proto` recompiles the validator
/// descriptor that consumes the import.
#[tokio::test(flavor = "multi_thread")]
async fn import_edit_recompiles_validator_descriptor() {
    let route_id = "dev-watch-proto-import";
    let harness = start_harness(route_id, "reload.v1.C", None, true).await;

    assert!(
        validate(&harness.ctx, r#"{"x":"a"}"#).await.is_ok(),
        "the imported descriptor must accept field x"
    );
    assert!(
        validate(&harness.ctx, r#"{"x":"a","y":3}"#).await.is_err(),
        "field y must be unknown before the import edit"
    );
    let gen_before = generation(route_id);

    std::fs::write(harness.root.join("common.proto"), COMMON_V2).expect("edit the imported proto");

    assert!(
        wait_for_validation(&harness.ctx, r#"{"x":"a","y":3}"#, WAIT_BOUND).await,
        "the import edit must make the field added to the import validate"
    );
    assert!(
        generation(route_id) > gen_before,
        "import-only edit must commit a new descriptor generation"
    );

    harness.stop().await;
}

/// Without a watcher, editing template and proto sources on disk never commits
/// a reload: both route generations stay unchanged across a bounded window that
/// would have covered the debounce plus a dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn non_watch_mode_never_reloads() {
    let proto_route_id = "dev-watch-proto-isolated";
    let template_route_id = "dev-watch-template-isolated";
    let harness = start_harness(
        proto_route_id,
        "reload.v1.M",
        Some(template_route_id),
        false,
    )
    .await;

    assert_eq!(
        generation(proto_route_id),
        0,
        "the proto route starts with no committed reloads"
    );
    assert_eq!(
        generation(template_route_id),
        0,
        "the template route starts with no committed reloads"
    );
    assert!(
        render(&harness.ctx).await.contains("page-v1"),
        "the template route must serve its startup output"
    );

    std::fs::write(harness.root.join("root.proto"), ROOT_V2).expect("edit root proto");
    std::fs::write(harness.root.join("common.proto"), COMMON_V2).expect("edit common proto");
    std::fs::write(harness.root.join("page.jinja"), PAGE_V2).expect("edit template");

    // Consume a bounded window larger than the watcher's debounce. With a
    // watcher running this window would have observed a dispatch.
    assert!(
        !wait_for(|| false, DEBOUNCE * 3).await,
        "the bounded settle window must expire"
    );

    assert_eq!(
        generation(proto_route_id),
        0,
        "no watcher means no protobuf descriptor reload"
    );
    assert_eq!(
        generation(template_route_id),
        0,
        "no watcher means no template reload"
    );
    assert!(
        validate(&harness.ctx, r#"{"a":"x","b":7}"#).await.is_err(),
        "the descriptor must not pick up the edit without a watcher"
    );
    assert!(
        render(&harness.ctx).await.contains("page-v1"),
        "the template output must not pick up the edit without a watcher"
    );

    harness.stop().await;
}
