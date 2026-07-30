//! End-to-end reload integration tests for the `template:` scheme
//! (ADR-0047 Stage 2, Phase 5 / Task 5.5).
//!
//! These tests drive a REAL `CamelContext` whose `direct:in` route feeds
//! exchanges through one or two `to("template:file:///...")` producers, then
//! exercise the Phase-5 hot-reload control plane against the SAME on-disk
//! sources and assert the ADR-0047 reload acceptance criteria:
//!
//! - **AC5 (atomic swap)** — a valid on-disk change swapped in via reload is
//!   observed by the very next render, and only the next render (`reload_valid_swaps_atomic`).
//! - **AC6 (last-good retention)** — a reload whose new sources fail to compile
//!   returns `Err` and the prior good set keeps serving (`reload_invalid_retains_last_good`).
//! - **AC8 (compile-once / no hot-path FS I/O)** — after a successful reload,
//!   deleting the source file does not break subsequent renders: the compiled
//!   set lives in memory (`reload_valid_swaps_atomic`, delete-after-reload step).
//! - **Route-scoped reload atomicity (spec)** — a route with two
//!   `template:` producers reloads atomically across both: if EITHER target's
//!   build fails, NEITHER commits (`reload_multi_producer_all_or_nothing`).
//!
//! ## Reload issuance boundary
//!
//! `RuntimeCommand::ReloadTemplates { route_id, .. }` is dispatched by the
//! `RuntimeBus` intercept (Task 5.4) to
//! `TemplateReloadRegistry::global().reload_route(route_id)`. The intercept +
//! dedup-bypass + journal-skip is covered by `crates/camel-core/tests/
//! template_reload_test.rs` (Task 5.4) with a `FakeTarget`. The
//! `CamelContext::execute_runtime_command` seam is `pub(crate)`, so these
//! integration tests call the SAME `reload_route` the intercept calls — proving
//! the REAL `ReloadHandler` build/validate/commit against REAL files end-to-end
//! (the part the FakeTarget test does not cover).
//!
//! Harness mirrors `template_render_integration.rs` (Task 4.5): register
//! `TemplateComponent`, build a `direct:in → template:` route, start (which
//! awaits each endpoint's `StepLifecycle::start()` → open root → closure →
//! compile → seed → register reload target), and send exchanges via the
//! `direct:` request-reply producer. Shared helpers (`start_template_route`,
//! `send_title`, `body_text`) live in `tests/common/mod.rs`. `send_title`
//! retries on the `direct:in` consumer-registration race (see
//! `tests/common/mod.rs` for the rationale).

use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::template_reload::TemplateReloadRegistry;
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;
use camel_template::TemplateComponent;

mod common;

// ── Single-template page variants ───────────────────────────────────────────
//
// All carry the ADR-0047 top-level `{% autoescape "html" %}` wrapper so the
// startup build and a valid reload both compile. The rendered body differs
// between v1 and v2, so a reload-driven swap is observable from the body.

/// v1 page — renders `<h1>v1 <title></h1>`.
const PAGE_V1: &str = "{% autoescape \"html\" %}<h1>v1 {{headers.title}}</h1>{% endautoescape %}";

/// v2 page — renders `<h1>v2 <title></h1>`. Distinguishable from v1.
const PAGE_V2: &str = "{% autoescape \"html\" %}<h1>v2 {{headers.title}}</h1>{% endautoescape %}";

/// Genuinely invalid source: an unclosed `{% if %}` block (a minijinja compile
/// error) that ALSO drops the top-level `{% autoescape %}` wrapper (so the
/// ADR-0047 wrapper guard rejects it too). Either gate fails the build, so the
/// reload of this content MUST return `Err` and the prior good set is retained.
const PAGE_INVALID: &str = "{% if broken %}no endif, no autoescape wrapper";

// ── Multi-producer page variants ────────────────────────────────────────────
//
// Producer A renders from the exchange header; producer B composes over A's
// rendered body via `{{body}}` (the inbound body exposed by `build_context_bounded`).
// The final route body is therefore `B…:A…:<title>`, so a swap of EITHER
// producer is observable in the rendered output.

/// Producer A, generation 1 — renders `A1:<title>`.
const A_V1: &str = "{% autoescape \"none\" %}A1:{{headers.title}}{% endautoescape %}";
/// Producer A, generation 2 — renders `A2:<title>`.
const A_V2: &str = "{% autoescape \"none\" %}A2:{{headers.title}}{% endautoescape %}";
/// Producer A, generation 3 — renders `A3:<title>` (a VALID change used in the
/// all-or-nothing step to prove A is NOT swapped when B fails).
const A_V3: &str = "{% autoescape \"none\" %}A3:{{headers.title}}{% endautoescape %}";
/// Producer B, generation 1 — composes over A's body: `B1:{{body}}`.
const B_V1: &str = "{% autoescape \"none\" %}B1:{{body}}{% endautoescape %}";
/// Producer B, generation 2 — composes over A's body: `B2:{{body}}`.
const B_V2: &str = "{% autoescape \"none\" %}B2:{{body}}{% endautoescape %}";

/// Build and start a context with one route carrying TWO `template:` producers:
/// `direct:in → template:<a_uri> → template:<b_uri>`. Each `To(template:)` step
/// creates its own endpoint and lifecycle handle, so both register a reload
/// target under the SAME `route_id` (set via `ComponentContext::route_id()`
/// during route compilation). `reload_route(route_id)` therefore reaches both.
async fn start_two_template_route(
    a_uri: &str,
    b_uri: &str,
    route_id: &str,
) -> Result<CamelContext, camel_api::CamelError> {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(TemplateComponent::default());
    let route = RouteBuilder::from("direct:in")
        .route_id(route_id)
        .to(a_uri.to_string())
        .to(b_uri.to_string())
        .build()
        .expect("route build");
    ctx.add_route_definition(route)
        .await
        .expect("add_route_definition");
    ctx.start().await?;
    Ok(ctx)
}

/// Issue a template reload for `route_id` through the SAME path the
/// `RuntimeCommand::ReloadTemplates` intercept uses (Task 5.4):
/// `TemplateReloadRegistry::global().reload_route`. Returns the underlying
/// `Result` so a test can assert on success/failure.
async fn reload(route_id: &str) -> Result<(), camel_api::CamelError> {
    TemplateReloadRegistry::global()
        .reload_route(route_id)
        .await
}

/// Generations of the reload targets registered for `route_id`, in registration
/// order. Used to assert all-or-nothing at the per-target commit level: a
/// successful reload bumps every target's generation, a failed reload bumps none.
fn generations(route_id: &str) -> Vec<u64> {
    TemplateReloadRegistry::global()
        .find_all(route_id)
        .iter()
        .map(|t| t.current_generation())
        .collect()
}

/// AC5 / AC8: a valid on-disk change swapped in via reload is served by the
/// very next render (atomic swap), and the compiled set is held in memory —
/// after the reload, deleting the source file does not break renders (no
/// hot-path filesystem I/O).
#[tokio::test(flavor = "multi_thread")]
async fn reload_valid_swaps_atomic() {
    let route_id = "t-reload-swaps-atomic";
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("page.html");
    std::fs::write(&entry, PAGE_V1).expect("write v1");
    let uri = format!("template:file://{}", entry.display());

    let mut ctx = common::start_template_route(&uri, route_id)
        .await
        .expect("context must start for a valid template");

    // Baseline: the startup build compiled v1.
    let out = common::send_title(&ctx, "Hi").await;
    assert_eq!(
        common::body_text(&out),
        "<h1>v1 Hi</h1>",
        "initial render must be v1"
    );

    // Mutate the on-disk source to v2, then reload.
    std::fs::write(&entry, PAGE_V2).expect("overwrite v2");
    reload(route_id)
        .await
        .expect("reload of a valid change must succeed");

    // AC5 (atomic swap): the next render reflects v2 with no half-built state.
    let out = common::send_title(&ctx, "Hi").await;
    assert_eq!(
        common::body_text(&out),
        "<h1>v2 Hi</h1>",
        "render after a valid reload must reflect the new set (atomic swap)"
    );

    // AC8 (compile-once / no hot-path FS I/O): the compiled set lives in
    // memory. Remove the source file AFTER a successful reload; subsequent
    // renders MUST still succeed using the in-memory compiled set. If the hot
    // path re-read the source, this render would fail.
    std::fs::remove_file(&entry).expect("remove source after reload");

    let out = common::send_title(&ctx, "Hi").await;
    assert_eq!(
        common::body_text(&out),
        "<h1>v2 Hi</h1>",
        "render after source deletion must still use the in-memory compiled set"
    );

    let _ = ctx.stop().await;
}

/// AC6: a reload whose new sources fail to compile returns `Err` and the route
/// keeps serving the LAST GOOD set. Neither the generation nor the rendered
/// output changes across the failed reload.
#[tokio::test(flavor = "multi_thread")]
async fn reload_invalid_retains_last_good() {
    let route_id = "t-reload-invalid-retains";
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("page.html");
    std::fs::write(&entry, PAGE_V1).expect("write v1");
    let uri = format!("template:file://{}", entry.display());

    let mut ctx = common::start_template_route(&uri, route_id)
        .await
        .expect("context must start for a valid template");

    // Establish a known good set: v1, then a successful reload to v2.
    std::fs::write(&entry, PAGE_V2).expect("overwrite v2");
    reload(route_id)
        .await
        .expect("reload to v2 must succeed (precondition)");
    let out = common::send_title(&ctx, "Hi").await;
    assert_eq!(
        common::body_text(&out),
        "<h1>v2 Hi</h1>",
        "last good set must be v2 before the invalid reload"
    );
    let gen_before = generations(route_id);
    assert_eq!(gen_before.len(), 1, "one reload target for this route");
    let good_gen = gen_before[0];

    // Overwrite with genuinely invalid source and reload — must fail.
    std::fs::write(&entry, PAGE_INVALID).expect("overwrite invalid");
    let result = reload(route_id).await;
    assert!(
        result.is_err(),
        "reload of invalid source must return Err, got {result:?}"
    );

    // AC6 (last-good retention): the generation did NOT bump and the route
    // keeps serving the last good set (v2).
    let gen_after = generations(route_id);
    assert_eq!(
        gen_after, gen_before,
        "generation must be unchanged by a failed reload (last good retained)"
    );
    assert_eq!(gen_after[0], good_gen, "no commit on failed reload");

    let out = common::send_title(&ctx, "Hi").await;
    assert_eq!(
        common::body_text(&out),
        "<h1>v2 Hi</h1>",
        "render after a failed reload must retain the last good set (AC6)"
    );

    let _ = ctx.stop().await;
}

/// AC10: a route with TWO `template:` producers reloads all-or-nothing. When
/// EITHER producer's build fails, NEITHER commits — not even the producer whose
/// file was valid. Asserted both at the generation level (per-target commit
/// counters) and at the rendered-output level (the composed body).
#[tokio::test(flavor = "multi_thread")]
async fn reload_multi_producer_all_or_nothing() {
    let route_id = "t-reload-multi-all-or-nothing";
    let dir = tempfile::tempdir().expect("tempdir");
    let entry_a = dir.path().join("a.html");
    let entry_b = dir.path().join("b.html");
    std::fs::write(&entry_a, A_V1).expect("write a v1");
    std::fs::write(&entry_b, B_V1).expect("write b v1");
    let uri_a = format!("template:file://{}", entry_a.display());
    let uri_b = format!("template:file://{}", entry_b.display());

    let mut ctx = start_two_template_route(&uri_a, &uri_b, route_id)
        .await
        .expect("context must start for two valid templates");

    // Baseline: A renders `A1:Hi`, B composes → `B1:A1:Hi`. Two reload targets
    // are registered under this route_id (one per endpoint).
    let out = common::send_title(&ctx, "Hi").await;
    assert_eq!(
        common::body_text(&out),
        "B1:A1:Hi",
        "initial composed render"
    );
    let baseline = generations(route_id);
    assert_eq!(
        baseline.len(),
        2,
        "two template producers must register two reload targets under one route_id"
    );

    // Mutate BOTH to valid v2 → reload succeeds and BOTH swap.
    std::fs::write(&entry_a, A_V2).expect("overwrite a v2");
    std::fs::write(&entry_b, B_V2).expect("overwrite b v2");
    reload(route_id)
        .await
        .expect("reload of two valid changes must succeed");
    let out = common::send_title(&ctx, "Hi").await;
    assert_eq!(
        common::body_text(&out),
        "B2:A2:Hi",
        "both producers swapped: A→A2, B→B2"
    );
    let after_ok = generations(route_id);
    assert_eq!(
        after_ok,
        vec![baseline[0] + 1, baseline[1] + 1],
        "both targets committed on the successful reload"
    );

    // All-or-nothing: mutate A to a VALID v3 but make B INVALID. The reload
    // MUST fail (B's build errors), and NEITHER commits — A stays on v2 even
    // though its file was valid, because B's failure aborted the build phase.
    std::fs::write(&entry_a, A_V3).expect("overwrite a v3 (valid)");
    std::fs::write(&entry_b, PAGE_INVALID).expect("overwrite b invalid");
    let result = reload(route_id).await;
    assert!(
        result.is_err(),
        "reload must fail when any producer's build fails, got {result:?}"
    );

    // Generation proof: A did NOT bump (its v3 file was valid, but B's failure
    // aborted before commit). Both retain their post-v2 generation.
    let after_err = generations(route_id);
    assert_eq!(
        after_err, after_ok,
        "neither target committed on the failed reload (all-or-nothing)"
    );

    // Render proof: A still serves v2 (NOT v3), B still composes over v2.
    let out = common::send_title(&ctx, "Hi").await;
    assert_eq!(
        common::body_text(&out),
        "B2:A2:Hi",
        "both producers retain their prior sets after a failed reload (AC10)"
    );

    let _ = ctx.stop().await;
}
