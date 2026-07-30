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

use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;
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
