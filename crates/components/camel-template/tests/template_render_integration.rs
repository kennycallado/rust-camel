//! End-to-end integration tests for the `template:` scheme (ADR-0047 Stage 2,
//! Phase 4 / Task 4.5).
//!
//! These tests drive a REAL `CamelContext`: a `direct:in` route feeds exchanges
//! through a `to("template:file:///...")` producer. Starting the context awaits
//! each route's `start_route`, which in turn awaits the template endpoint's
//! `StepLifecycle::start()` — so the startup build (open root → closure →
//! compile → seed) is exercised here, not stubbed.
//!
//! Harness mirrors `camel-validator`'s `yaml_dsl_e2e` integration test
//! (`CamelContext::builder()` + manual component registration + a `direct:`
//! request-reply producer), adapted to `RouteBuilder` for programmatic routes.
//!
//! Shared helpers (`start_template_route`, `send_title`, `body_text`) live in
//! `tests/common/mod.rs` and are reused by `template_reload_integration.rs`.
//! `send_title` retries on the `direct:in` consumer-registration race
//! (see `tests/common/mod.rs` for the rationale).

use camel_api::CamelError;
use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::Component;
use camel_component_api::NoOpComponentContext;
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;
use camel_template::config::autoescape_mode_from_toml;
use camel_template::{TemplateBundleConfig, TemplateComponent};

mod common;

/// The page template: an HTML-escaped heading rendered from the `title` header.
/// The render context exposes headers under `headers` (see
/// `build_context_bounded` in `camel-language-minijinja`), so the header is
/// `{{headers.title}}`. Under `{% autoescape "html" %}` the value `Hi` has no
/// special characters, so the rendered output is exactly `<h1>Hi</h1>`.
const PAGE_TEMPLATE: &str =
    "{% autoescape \"html\" %}<h1>{{headers.title}}</h1>{% endautoescape %}";

/// AC1 / AC2: an exchange sent through a real route renders the operator
/// template end-to-end, replaces the body, and preserves headers.
#[tokio::test(flavor = "multi_thread")]
async fn template_renders_end_to_end() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("page.html");
    std::fs::write(&entry, PAGE_TEMPLATE).expect("write page");
    let uri = format!("template:file://{}", entry.display());

    let mut ctx = common::start_template_route(&uri, "t-render")
        .await
        .expect("context must start for a valid template");

    let out = common::send_title(&ctx, "Hi").await;
    assert_eq!(common::body_text(&out), "<h1>Hi</h1>", "rendered body");
    // Headers must be preserved across render (zero-override contract).
    assert_eq!(
        out.input.headers.get("title"),
        Some(&Value::String("Hi".to_string())),
        "title header must be preserved"
    );

    let _ = ctx.stop().await;
}

/// AC4 / AC9: the compiled set is held in memory after `start()`. Once the
/// route is running, the source file is deleted; two subsequent renders must
/// still succeed with the compiled output, proving the hot path does ZERO
/// filesystem I/O (compile-once invariant).
#[tokio::test(flavor = "multi_thread")]
async fn template_compile_once_no_hot_io() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("page.html");
    std::fs::write(&entry, PAGE_TEMPLATE).expect("write page");
    let uri = format!("template:file://{}", entry.display());

    let mut ctx = common::start_template_route(&uri, "t-nohot")
        .await
        .expect("context must start for a valid template");

    // AFTER start() completes the compiled set is in memory — remove the
    // source file. If the hot path re-read it, both renders below would fail.
    std::fs::remove_file(&entry).expect("remove source after start");

    let first = common::send_title(&ctx, "Hi").await;
    assert_eq!(
        common::body_text(&first),
        "<h1>Hi</h1>",
        "first render after delete"
    );

    let second = common::send_title(&ctx, "Hi").await;
    assert_eq!(
        common::body_text(&second),
        "<h1>Hi</h1>",
        "second render after delete — hot path must not touch the filesystem"
    );

    let _ = ctx.stop().await;
}

/// AC3 / AC7: a route pointing at a non-existent template must fail closed.
/// `start()` awaits the endpoint lifecycle `start()`, which fails on the
/// missing file; the route serves no requests.
#[tokio::test(flavor = "multi_thread")]
async fn missing_template_fails_route_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Entry file is intentionally NOT created.
    let missing = dir.path().join("missing.html");
    let uri = format!("template:file://{}", missing.display());

    let route_id = "t-missing".to_string();
    let start_result = common::start_template_route(&uri, &route_id).await;

    // Fail closed: either start() propagates the lifecycle error, or the route
    // enters the `Failed` status. Either way it serves no requests.
    let (failed_closed, detail) = match start_result {
        Err(e) => (true, format!("start error: {e}")),
        Ok(mut ctx) => {
            let status = ctx.runtime_route_status(&route_id).await.ok().flatten();
            let closed = status.as_deref() == Some("Failed");
            let _ = ctx.stop().await;
            (closed, format!("start Ok; route status = {status:?}"))
        }
    };
    assert!(failed_closed, "missing template must fail closed; {detail}");
}

/// Bundle-config end-to-end: a tightened `max-template-size` configured via
/// the bundle's TOML config shape is enforced when the route starts. The
/// existing tests register `TemplateComponent::default()`, so the
/// non-default limit path is not exercised anywhere else.
///
/// Approach: `TemplateBundle::config` is private, so `register_all` cannot
/// be driven from an integration test (it needs a `ComponentRegistrar`, and
/// `Arc<dyn Component>` cannot be re-registered into `CamelContext` because
/// the public `register_component` takes a concrete `C: Component`). This
/// test instead deserializes the same TOML the bundle accepts into
/// `TemplateBundleConfig` and constructs the same `TemplateComponent` the
/// bundle's `register_all` would build — i.e.
/// `TemplateComponent::new(cfg.limits, cfg.render_limits)`. That proves the
/// bundle's config shape flows into a real route and that the configured
/// acquisition limit (`max-template-size = 4`) is enforced.
#[tokio::test(flavor = "multi_thread")]
async fn bundle_enforces_configured_limits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("page.html");
    // 18 bytes of payload — well above the tightened 4-byte bound.
    std::fs::write(&entry, "<h1>oversize</h1>").expect("write page");
    let uri = format!("template:file://{}", entry.display());

    // Same TOML shape `TemplateBundle::from_toml` accepts; the inner table
    // is deserialized into `TemplateBundleConfig` (kebab-case,
    // deny_unknown_fields) exactly as the bundle does.
    let bundle_toml = r#"
[limits]
max-template-size = 4
"#;
    let cfg: TemplateBundleConfig =
        toml::from_str(bundle_toml).expect("bundle config must deserialize");

    // Mirror `TemplateBundle::register_all` line-for-line:
    //   let component = TemplateComponent::new(self.config.limits, self.config.render_limits);
    //   ctx.register_component_dyn(Arc::new(component));
    let component = TemplateComponent::new(cfg.limits, cfg.render_limits);

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(component);
    let route = RouteBuilder::from("direct:in")
        .route_id("t-bundle-tight")
        .to(uri)
        .build()
        .expect("route build");
    ctx.add_route_definition(route)
        .await
        .expect("add_route_definition");

    // Fail closed: the closure reader rejects the >4-byte template with
    // `TemplateReloadError::BoundExceeded("max_template_size")` which maps
    // to `CamelError::TemplateReload(_)` via the `From` impl in
    // `crate::error`. The route-level start may wrap the inner variant in
    // `CamelError::RouteError(_)` (lifecycle recovery); either way the
    // bound name must appear in the chain, proving the configured limit
    // was the one that tripped.
    let start_result = ctx.start().await;
    let detail = match start_result {
        Err(e) => format!("{e}"),
        Ok(_) => panic!("tightened max-template-size must fail closed, but start() returned Ok"),
    };
    assert!(
        detail.contains("max_template_size") || detail.contains("max-template-size"),
        "expected max-template-size failure in start error chain, got: {detail}"
    );
}

/// Parse a `[components.template]` document and build the configured
/// component exactly as `TemplateBundle::register_all` would. The inner table
/// is deserialized into `TemplateBundleConfig` because the top-level document
/// (with the `components` wrapper) does not fit its `deny_unknown_fields`.
fn configured_component(toml_doc: &str) -> TemplateComponent {
    let doc: toml::Value = toml::from_str(toml_doc).expect("config TOML");
    let table = doc
        .get("components")
        .and_then(|v| v.get("template"))
        .expect("components.template sub-table")
        .clone();
    let cfg: TemplateBundleConfig = table.try_into().expect("bundle config must deserialize");
    let mode = cfg
        .default_autoescape
        .as_ref()
        .map(autoescape_mode_from_toml)
        .transpose()
        .expect("valid default-autoescape")
        .expect("test config sets default-autoescape");
    let mut component =
        TemplateComponent::new(cfg.limits, cfg.render_limits).with_default_autoescape(mode);
    if let Some(root) = cfg.root {
        component = component.with_root(root);
    }
    component
}

/// Build a component whose `[components.template]` table carries only the
/// confinement `root` (plus the mandatory `default-autoescape = "none"`
/// fixture value the helper's TOML shape requires).
fn root_component(root: &std::path::Path) -> TemplateComponent {
    configured_component(&format!(
        r#"
[components.template]
root = "{}"
default-autoescape = "none"
"#,
        root.display()
    ))
}

/// Register `direct:in → template:<uri>` against a fresh context and return
/// the context plus the `add_route_definition` result.
///
/// Endpoint construction happens inside `add_route_definition` (see
/// `CamelContext::add_route_definition`), so construction-time rejections
/// surface there — wrapped as `CamelError::RouteError` by the registration
/// layer. Filesystem failures surface later from `ctx.start()`.
async fn context_with_route(
    component: TemplateComponent,
    route_id: &str,
    uri: &str,
) -> (CamelContext, Result<(), CamelError>) {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(component);
    let route = RouteBuilder::from("direct:in")
        .route_id(route_id)
        .to(uri.to_string())
        .build()
        .expect("route build");
    let registration = ctx.add_route_definition(route).await;
    (ctx, registration)
}

/// Route-level construction proof: `add_route_definition` must reject the
/// route and the registration layer must wrap the original `CamelError::Config`
/// diagnostic inside `CamelError::RouteError`. Returns the wrapped message.
async fn registration_expect_route_error(
    component: TemplateComponent,
    route_id: &str,
    uri: &str,
) -> String {
    let (_ctx, registration) = context_with_route(component, route_id, uri).await;
    match registration {
        Err(CamelError::RouteError(message)) => message,
        Err(other) => panic!("expected CamelError::RouteError, got {other:?}"),
        Ok(()) => panic!("route registration must be rejected for {uri:?}"),
    }
}

/// Route-level filesystem proof: a fixture whose filesystem state violates
/// confinement must fail closed during the awaited startup. Returns the
/// propagated error text for diagnostic assertions.
async fn start_expect_err(component: TemplateComponent, route_id: &str, uri: &str) -> String {
    let (mut ctx, registration) = context_with_route(component, route_id, uri).await;
    registration.expect("route registration must succeed for a filesystem-failure fixture");
    match ctx.start().await {
        Ok(()) => {
            let status = ctx.runtime_route_status(route_id).await.ok().flatten();
            let _ = ctx.stop().await;
            panic!("start must fail closed for {uri:?}; route status = {status:?}");
        }
        Err(error) => {
            let _ = ctx.stop().await;
            error.to_string()
        }
    }
}

/// `default-autoescape = "html"` reaches the compile path through the
/// component → endpoint → lifecycle chain: a block-less `.txt` entry (the
/// name-based MiniJinja callback cannot escape a `.txt` name) renders an
/// HTML-escaped header payload.
#[tokio::test(flavor = "multi_thread")]
async fn render_with_default_html_config_escapes_payload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("page.txt");
    // No `{% autoescape %}` statement: only the configured default can make
    // this compile.
    std::fs::write(&entry, "Hi {{ headers.title }}").expect("write page");
    let uri = format!("template:file://{}", entry.display());

    let component = configured_component(
        r#"
[components.template]
default-autoescape = "html"
"#,
    );

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(component);
    let route = RouteBuilder::from("direct:in")
        .route_id("t-default-html")
        .to(uri)
        .build()
        .expect("route build");
    ctx.add_route_definition(route)
        .await
        .expect("add_route_definition");

    // The startup compile must accept the block-less entry because the
    // configured HTML default wraps it.
    ctx.start()
        .await
        .expect("context must start with the configured default");

    let out = common::send_title(&ctx, "<b>").await;
    let body = common::body_text(&out);
    assert!(
        body.contains("&lt;b&gt;"),
        "configured default must HTML-escape the payload, got: {body:?}"
    );
    assert!(
        !body.contains("<b>"),
        "raw payload must not survive the configured HTML wrap, got: {body:?}"
    );

    let _ = ctx.stop().await;
}

/// FEATURE (Task 1.4): a relative entry resolves under the configured root,
/// not against the process CWD, and nested namesakes do not collide.
///
/// `templates/page.html` (nested) and `page.html` (root-level) both exist with
/// distinct markers. The route spells the nested path; the rendered body must
/// carry the nested marker and never the root-level one.
#[tokio::test(flavor = "multi_thread")]
async fn relative_entry_under_root_renders_nested_namesake() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("templates")).expect("create templates dir");
    std::fs::write(root.join("templates/page.html"), "nested-{{headers.title}}")
        .expect("write nested page");
    std::fs::write(root.join("page.html"), "root-level-{{headers.title}}")
        .expect("write root-level page");

    let component = configured_component(&format!(
        r#"
[components.template]
root = "{}"
default-autoescape = "none"
"#,
        root.display()
    ));

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(component);
    let route = RouteBuilder::from("direct:in")
        .route_id("t-relative-nested-namesake")
        .to("template:file:templates/page.html".to_string())
        .build()
        .expect("route build");
    ctx.add_route_definition(route)
        .await
        .expect("add_route_definition");
    ctx.start()
        .await
        .expect("context must start with a configured root");

    let out = common::send_title(&ctx, "x").await;
    let body = common::body_text(&out);
    assert!(
        body.contains("nested-"),
        "relative entry must resolve the nested namesake, got: {body:?}"
    );
    assert!(
        !body.contains("root-level-"),
        "relative entry must not fall back to the root-level namesake, got: {body:?}"
    );

    let _ = ctx.stop().await;
}

/// FEATURE (Task 1.4): `{% include %}` targets resolve against the configured
/// root, not the entry's parent directory.
///
/// `partials/shared.html` (root-anchored) and `templates/partials/shared.html`
/// (parent-anchored) both exist. The entry under `templates/` includes
/// `partials/shared.html`; the rendered body must carry the root-anchored
/// marker and never the parent-anchored one.
#[tokio::test(flavor = "multi_thread")]
async fn include_resolves_from_configured_root_not_entry_parent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("templates/partials")).expect("create parent partials dir");
    std::fs::create_dir_all(root.join("partials")).expect("create root partials dir");
    std::fs::write(
        root.join("templates/entry.html"),
        r#"entry-{% include "partials/shared.html" %}"#,
    )
    .expect("write entry");
    std::fs::write(
        root.join("partials/shared.html"),
        "root-anchored-{{headers.title}}",
    )
    .expect("write root-anchored partial");
    std::fs::write(
        root.join("templates/partials/shared.html"),
        "parent-anchored-{{headers.title}}",
    )
    .expect("write parent-anchored partial");

    let component = configured_component(&format!(
        r#"
[components.template]
root = "{}"
default-autoescape = "none"
"#,
        root.display()
    ));

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(component);
    let route = RouteBuilder::from("direct:in")
        .route_id("t-include-root-anchored")
        .to("template:file:templates/entry.html".to_string())
        .build()
        .expect("route build");
    ctx.add_route_definition(route)
        .await
        .expect("add_route_definition");
    ctx.start()
        .await
        .expect("context must start with a configured root");

    let out = common::send_title(&ctx, "x").await;
    let body = common::body_text(&out);
    assert!(
        body.contains("entry-"),
        "entry marker must render, got: {body:?}"
    );
    assert!(
        body.contains("root-anchored-"),
        "include must resolve from the configured root, got: {body:?}"
    );
    assert!(
        !body.contains("parent-anchored-"),
        "include must not resolve from the entry's parent directory, got: {body:?}"
    );

    let _ = ctx.stop().await;
}

/// REGRESSION-LOCK: with no configured root, a fully absolute entry still
/// renders. This is the historic `EntryParent` anchor row and must never
/// regress when the configured-root path is added.
#[tokio::test(flavor = "multi_thread")]
async fn absolute_uri_without_root_still_works() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("page.html");
    std::fs::write(
        &entry,
        "{% autoescape \"none\" %}absolute-{{headers.title}}{% endautoescape %}",
    )
    .expect("write page");
    let uri = format!("template:file://{}", entry.display());

    let mut ctx = common::start_template_route(&uri, "t-absolute-no-root")
        .await
        .expect("absolute entry without a configured root must render");

    let out = common::send_title(&ctx, "x").await;
    let body = common::body_text(&out);
    assert!(
        body.contains("absolute-"),
        "absolute entry must render without a configured root, got: {body:?}"
    );

    let _ = ctx.stop().await;
}

/// REGRESSION-LOCK: the positive precedence row — a root is configured AND the
/// absolute entry is inside that canonical root, so it renders.
#[tokio::test(flavor = "multi_thread")]
async fn absolute_inside_root_with_root_configured_renders() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(dir.path()).expect("canonicalize root");
    let entry = root.join("page.html");
    std::fs::write(&entry, "inside-root-{{headers.title}}").expect("write page");
    let uri = format!("template:file://{}", entry.display());

    let (mut ctx, registration) =
        context_with_route(root_component(&root), "t-absolute-inside-root", &uri).await;
    registration.expect("absolute entry inside the configured root must register");
    ctx.start()
        .await
        .expect("absolute entry inside the configured root must start");

    let out = common::send_title(&ctx, "x").await;
    let body = common::body_text(&out);
    assert!(
        body.contains("inside-root-"),
        "absolute entry inside the configured root must render, got: {body:?}"
    );

    let _ = ctx.stop().await;
}

/// REGRESSION-LOCK (Task 1.2 shim): a relative URI with no configured root is a
/// construction-time configuration error. Asserted EXACTLY at the
/// `create_endpoint` seam, then proven through the registration layer.
#[tokio::test(flavor = "multi_thread")]
async fn relative_uri_without_root_rejected() {
    let component = TemplateComponent::default();

    match component.create_endpoint("template:file:page.html", &NoOpComponentContext) {
        Err(CamelError::Config(message)) => assert_eq!(
            message,
            "relative template URI requires [components.template] root to be configured"
        ),
        Err(other) => panic!("expected CamelError::Config, got {other:?}"),
        Ok(_) => panic!("relative URI without a configured root must be rejected"),
    }

    let message = registration_expect_route_error(
        TemplateComponent::default(),
        "t-relative-no-root",
        "template:file:page.html",
    )
    .await;
    assert!(
        message
            .contains("relative template URI requires [components.template] root to be configured"),
        "route-level error must carry the original diagnostic, got: {message}"
    );
}

/// REGRESSION-LOCK: the URI parser rejects `..` in the canonical relative form
/// before any filesystem access, even when a root is configured. Asserted at
/// the `create_endpoint` seam, then through the registration layer.
#[tokio::test(flavor = "multi_thread")]
async fn dotdot_relative_uri_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let component = TemplateComponent::default().with_root(dir.path().to_path_buf());

    match component.create_endpoint("template:file:../escape.html", &NoOpComponentContext) {
        Err(CamelError::Config(message)) => assert!(
            message.contains("'..' component"),
            "diagnostic must name the rejected '..' component, got: {message}"
        ),
        Err(other) => panic!("expected CamelError::Config, got {other:?}"),
        Ok(_) => panic!("'..' relative URI must be rejected"),
    }

    let message = registration_expect_route_error(
        TemplateComponent::default().with_root(dir.path().to_path_buf()),
        "t-dotdot-relative",
        "template:file:../escape.html",
    )
    .await;
    assert!(
        message.contains("'..' component"),
        "route-level error must carry the original diagnostic, got: {message}"
    );
}

/// REGRESSION-LOCK: `O_NOFOLLOW` rejects a leaf symlink inside the configured
/// root during the closure walk, even when the link target itself is inside
/// the root.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn symlink_leaf_entry_rejected() {
    use std::os::unix::fs as unix_fs;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("page.html"), "real-page").expect("write real page");
    unix_fs::symlink("page.html", root.join("link.html")).expect("create leaf symlink");

    let message = start_expect_err(
        root_component(root),
        "t-symlink-leaf",
        "template:file:link.html",
    )
    .await;

    assert!(
        message.contains("path escapes"),
        "leaf symlink must be rejected as a path escape, got: {message}"
    );
    assert!(
        message.contains("link.html"),
        "diagnostic must name the rejected entry, got: {message}"
    );
}

/// REGRESSION-LOCK: `O_DIRECTORY | O_NOFOLLOW` rejects a symlinked intermediate
/// component during the closure walk, even when the link target is a real
/// directory inside the root.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn symlink_intermediate_component_rejected() {
    use std::os::unix::fs as unix_fs;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("partials-real")).expect("create real partials dir");
    std::fs::write(root.join("partials-real/page.html"), "real-page").expect("write real page");
    unix_fs::symlink("partials-real", root.join("partials")).expect("create intermediate symlink");

    let message = start_expect_err(
        root_component(root),
        "t-symlink-intermediate",
        "template:file:partials/page.html",
    )
    .await;

    assert!(
        message.contains("path escapes"),
        "intermediate symlink must be rejected as a path escape, got: {message}"
    );
    assert!(
        message.contains("partials"),
        "diagnostic must name the rejected component, got: {message}"
    );
}

/// REGRESSION-LOCK: a symlinked include target that resolves outside the
/// configured root is rejected during the closure walk. The outside target
/// EXISTS, so the failure is confinement, not a missing file.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn symlink_closure_target_outside_rejected() {
    use std::os::unix::fs as unix_fs;

    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside_file = outside.path().join("real.html");
    std::fs::write(&outside_file, "outside").expect("write outside target");

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("a.html"), r#"{% include "link.html" %}"#).expect("write entry");
    unix_fs::symlink(&outside_file, root.join("link.html")).expect("create closure symlink");

    let message = start_expect_err(
        root_component(root),
        "t-symlink-closure",
        "template:file:a.html",
    )
    .await;

    assert!(
        message.contains("path escapes"),
        "outside closure symlink must be rejected as a path escape, got: {message}"
    );
    assert!(
        message.contains("link.html"),
        "diagnostic must name the rejected include target, got: {message}"
    );
}

/// FEATURE: an absolute entry that canonicalizes outside the configured root is
/// rejected at startup with a diagnostic naming both the entry and the
/// canonical root.
#[tokio::test(flavor = "multi_thread")]
async fn absolute_entry_outside_root_rejected() {
    let root_dir = tempfile::tempdir().expect("root tempdir");
    let root = std::fs::canonicalize(root_dir.path()).expect("canonicalize root");
    let outside_dir = tempfile::tempdir().expect("outside tempdir");
    let outside = std::fs::canonicalize(outside_dir.path()).expect("canonicalize outside");
    let entry = outside.join("page.html");
    std::fs::write(&entry, "outside-entry").expect("write outside entry");
    let uri = format!("template:file://{}", entry.display());

    let message = start_expect_err(root_component(&root), "t-absolute-outside-root", &uri).await;

    assert!(
        message.contains(&entry.display().to_string()),
        "diagnostic must name the escaping entry, got: {message}"
    );
    assert!(
        message.contains(&root.display().to_string()),
        "diagnostic must name the canonical root, got: {message}"
    );
    assert!(
        message.contains("not spelled under the canonical root"),
        "diagnostic must be the lexical-strip rejection for an absolute entry outside the root, got: {message}"
    );
}

/// FEATURE: a configured root that does not exist is rejected at startup with a
/// diagnostic explicitly naming the configured root.
#[tokio::test(flavor = "multi_thread")]
async fn missing_root_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nope");

    let message = start_expect_err(
        root_component(&missing),
        "t-missing-root",
        "template:file:page.html",
    )
    .await;

    assert!(
        message.contains("configured template root"),
        "diagnostic must name the configured root concept, got: {message}"
    );
    assert!(
        message.contains(&missing.display().to_string()),
        "diagnostic must name the missing root path, got: {message}"
    );
}

/// REGRESSION-LOCK: an include target bearing `..` is rejected during the
/// closure walk, now anchored at the canonical root. The outside file EXISTS,
/// so the failure is confinement, not a missing file.
#[tokio::test(flavor = "multi_thread")]
async fn include_outside_root_rejected() {
    let enclosing = tempfile::tempdir().expect("enclosing tempdir");
    let outside = enclosing.path().join("outside.html");
    std::fs::write(&outside, "outside-partial").expect("write outside partial");
    assert!(
        outside.exists(),
        "outside include target must exist so the failure is confinement, not ENOENT"
    );

    let root = enclosing.path().join("root");
    std::fs::create_dir_all(&root).expect("create root dir");
    std::fs::write(
        root.join("entry.html"),
        r#"{% include "../outside.html" %}"#,
    )
    .expect("write entry");

    let message = start_expect_err(
        root_component(&root),
        "t-include-outside",
        "template:file:entry.html",
    )
    .await;

    assert!(
        message.contains("path escapes"),
        "'..' include must be rejected as a path escape, got: {message}"
    );
    assert!(
        message.contains("../outside.html"),
        "diagnostic must name the rejected include target, got: {message}"
    );
}
