//! Tower `Service<Exchange>` render semantics for the external template
//! component (ADR-0047 Stage 2, Phase 4 / Task 4.2).
//!
//! [`TemplateProducer`] holds the operator-configured compiled
//! [`SharedTemplates`] and renders the entry against a per-Exchange context
//! built from body + headers + properties. The producer is zero-override: the
//! entry and root always come from the operator-supplied compiled set — no
//! header or property of the inbound Exchange is ever consulted to choose
//! them.
//!
//! All observable behaviour lives in the private [`render_into`] seam so
//! tests can drive it against a borrowed `Exchange` and assert the
//! body-unchanged-on-error contract directly.
//!
//! [`SharedTemplates`]: crate::template_set::SharedTemplates
//! [`render_into`]: TemplateProducer::render_into

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use camel_api::{Body, CamelError, Exchange};
use camel_component_api::RuntimeObservability;
use camel_language_minijinja::ResolvedLimits;
use camel_language_minijinja::engine::build_context_bounded;
use tower::Service;

use crate::template_set::SharedTemplates;

/// Tower `Service<Exchange>` that renders the operator-configured compiled
/// template set against each inbound exchange.
///
/// # Body Contract
///
/// **Input:** the inbound exchange body is the `{{body}}` of the rendering
/// context. `Body::Stream` is rejected (matching the inline engine's S4
/// guard).
///
/// **Output:** the exchange `input.body` is replaced by the rendered output
/// as `Body::Text`. Headers and properties are preserved unchanged. On
/// render failure (strict-undefined, fuel, output limit, timeout, join)
/// the body is NOT mutated — the inbound exchange reaches the caller
/// byte-identical to what was submitted.
///
/// # Zero Override (CRITICAL)
///
/// The entry and root template are taken from the operator-configured
/// [`SharedTemplates`] at startup. **No** header or property of the inbound
/// exchange is ever consulted to choose the entry, the root, or the
/// template source. The `producer_ignores_override_header` test pins this
/// property.
//
// ponytail: rt/route_id deferred to bd rc-d3pj (RuntimeBus metrics port).
// In this final state the producer stores rt/route_id but never reads them on
// the render hot path; rc-d3pj wires them into the per-route failure
// counter. The struct + new() stay as-is so the constructor signature in
// endpoint.rs is not churned; only the read-side remains pending.
#[derive(Clone)]
pub(crate) struct TemplateProducer {
    templates: SharedTemplates,
    render_limits: ResolvedLimits,
    /// Runtime observability handle for Phase-5 metrics (deferred to
    /// bd rc-d3pj). Stored now, read later.
    #[allow(dead_code)] // read in rc-d3pj — see ponytail note above
    rt: Option<Arc<dyn RuntimeObservability>>,
    /// Route id for Phase-5 metrics (deferred to bd rc-d3pj).
    #[allow(dead_code)] // read in rc-d3pj — see ponytail note above
    route_id: String,
}

impl TemplateProducer {
    /// Build a producer bound to the operator-configured compiled set and
    /// the resolved render limits. The `rt`/`route_id` are reserved for
    /// Phase 5 metrics emission (bd rc-d3pj).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        templates: SharedTemplates,
        render_limits: ResolvedLimits,
        rt: Option<Arc<dyn RuntimeObservability>>,
        route_id: impl Into<String>,
    ) -> Self {
        Self {
            templates,
            render_limits,
            rt,
            route_id: route_id.into(),
        }
    }
}

impl Service<Exchange> for TemplateProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    /// Always ready. The compiled set is loaded inside `call` via
    /// `ArcSwap::load_full`, so a mid-flight hot-swap is safe without
    /// a backpressure handshake.
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        // `render_into` is the testable seam — it takes `&mut Exchange`
        // so a failed render leaves the exchange byte-identical to the
        // caller's input. `Service::call` is a thin delegate.
        //
        // The producer is `Clone`; cloning moves an owned copy into
        // the future so the async work holds its own data (`'static`)
        // and does not need to borrow `&mut self` for the duration of
        // the await. The clone preserves every field — including
        // `rt` and `route_id` — so Phase 5 metrics wiring does not
        // silently observe `None` / `""` on the hot path.
        let producer = self.clone();
        Box::pin(async move {
            producer.render_into(&mut exchange).await?;
            Ok(exchange)
        })
    }
}

impl TemplateProducer {
    /// Render the operator-configured entry into `exchange.input.body`.
    ///
    /// On `Ok`, the body is replaced with the rendered output. On `Err`,
    /// the body is NOT mutated and the inbound exchange is returned
    /// byte-identical to the caller — the test
    /// `producer_leaves_body_on_render_error` pins this property by
    /// observing the borrowed `&mut Exchange` directly.
    ///
    /// The entry and root are taken from `self.templates.load_full()`
    /// (operator-configured). No header or property of the inbound
    /// exchange is consulted — the `producer_ignores_override_header`
    /// test pins this.
    #[allow(dead_code)] // private fn; exercised only by `mod tests` below.
    async fn render_into(&self, exchange: &mut Exchange) -> Result<(), CamelError> {
        // Body contract: reject Body::Stream the same way the inline
        // engine does (S4). Other variants are accepted; the context
        // builder serialises them via the same `BodyAsJson` shape the
        // inline engine uses.
        //
        // S9 max-context-size is enforced inside
        // `build_context_bounded` — the same function the inline
        // engine calls. Reusing the engine's function (rather than
        // re-deriving the context shape here) keeps the S9 bound
        // intact and prevents DRY drift; the Task 4.2 first cut
        // duplicated the builder and silently dropped the bound.
        if matches!(exchange.input.body, Body::Stream(_)) {
            return Err(CamelError::ProcessorError(
                "template producer cannot render Body::Stream; add `stream_cache` upstream"
                    .to_string(),
            ));
        }

        let context = build_context_bounded(exchange, self.render_limits.max_context_size)
            .map_err(|e| CamelError::ProcessorError(e.to_string()))?;

        // Load the current compiled set atomically. Hot-reload (Phase 5)
        // will swap `self.templates`; `load_full` snapshots the current
        // set for the lifetime of this render.
        let set = self.templates.load_full();

        let rendered = set
            .render_entry(context, self.render_limits)
            .await
            .map_err(|e| CamelError::ProcessorError(e.to_string()))?;

        // Body is replaced ONLY on the success path. A failure above
        // returns without touching `exchange.input.body`, so the
        // body-unchanged-on-error contract holds.
        exchange.input.body = Body::from(rendered);
        Ok(())
    }
}

// `TemplateReloadError` is the internal data-plane render error. It surfaces
// as `CamelError::ProcessorError(_)` (the data-plane variant) so operators
// and `OnException` policies can distinguish per-request render failures
// from the control-plane `CamelError::TemplateReload(_)` used by the
// startup-build and hot-reload paths in `lifecycle.rs` / `reload.rs`. The
// conversion lives in `crate::error`; the producer intentionally bypasses
// that `From` impl here (which maps to `TemplateReload`) because render-time
// is the data plane, not the control plane.

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use arc_swap::ArcSwap;
    use camel_api::{Message, Value};
    use camel_language_api::MinijinjaLimitsConfig;
    use serde_json::json;

    use crate::closure::ClosureSnapshot;
    use crate::template_set::TemplateSet;

    /// Build a `SharedTemplates` seeded with a single-entry `ClosureSnapshot`
    /// compiled to the entry name `entry_name`.
    fn make_templates(entry_name: &str, source: &str) -> SharedTemplates {
        let snap = ClosureSnapshot::from_single_entry(entry_name, source.as_bytes().to_vec());
        let set = TemplateSet::compile(&snap, entry_name, MinijinjaLimitsConfig::default())
            .expect("compile");
        Arc::new(ArcSwap::from_pointee(set))
    }

    fn make_exchange_with_body(body: &str) -> Exchange {
        let mut msg = Message::new(Body::from(body.to_string()));
        msg.headers
            .insert("X-Keep".to_string(), Value::String("kept".to_string()));
        Exchange::new(msg)
    }

    fn body_string(exchange: &Exchange) -> Option<String> {
        match &exchange.input.body {
            Body::Text(s) => Some(s.clone()),
            Body::Bytes(b) => Some(String::from_utf8_lossy(b).to_string()),
            _ => None,
        }
    }

    #[tokio::test]
    async fn producer_replaces_body_on_success() {
        // The entry echoes `{{body}}` — the rendered output is the
        // body's string content.
        let templates = make_templates(
            "echo.html",
            r#"{% autoescape "none" %}body={{body}}{% endautoescape %}"#,
        );
        let mut producer =
            TemplateProducer::new(templates, ResolvedLimits::default(), None, "test-route");

        let exchange = make_exchange_with_body("hello");
        let result = producer.call(exchange).await.expect("call ok");
        assert_eq!(
            body_string(&result).as_deref(),
            Some("body=hello"),
            "body must be replaced by rendered output"
        );
        // Headers must be preserved unchanged.
        assert_eq!(
            result.input.headers.get("X-Keep").and_then(|v| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("kept"),
            "headers must be preserved across render"
        );
    }

    #[tokio::test]
    async fn producer_leaves_body_on_render_error() {
        // The entry references `undefined_var`, which is undefined under
        // strict-undefined. Compile succeeds; render fails.
        let templates = make_templates(
            "boom.html",
            r#"{% autoescape "none" %}{{undefined_var}}{% endautoescape %}"#,
        );
        let producer =
            TemplateProducer::new(templates, ResolvedLimits::default(), None, "test-route");

        // Mutable exchange so the test can observe that the body is
        // byte-identical after the failed render.
        let original_body = "original-payload".to_string();
        let mut exchange = Exchange::new(Message::new(Body::from(original_body.clone())));

        // Seed a header so we can also assert headers are untouched.
        exchange
            .input
            .headers
            .insert("X-Keep".to_string(), Value::String("kept".to_string()));

        let result = producer.render_into(&mut exchange).await;
        assert!(
            matches!(result, Err(CamelError::ProcessorError(_))),
            "strict-undefined is a data-plane render error and must surface as \
             CamelError::ProcessorError (NOT CamelError::TemplateReload, which is \
             reserved for the control-plane startup-build and hot-reload paths in \
             lifecycle.rs / reload.rs), got: {result:?}"
        );

        // Body must be byte-identical to the inbound body — the
        // `render_into` borrow lets the test observe the contract
        // directly.
        match &exchange.input.body {
            Body::Text(s) => {
                assert_eq!(s, &original_body, "body must be unchanged on render error")
            }
            other => panic!("expected unchanged Text body, got: {other:?}"),
        }

        // Headers must also be preserved (the producer never mutates
        // headers in the success or error path).
        assert_eq!(
            exchange.input.headers.get("X-Keep").and_then(|v| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("kept"),
        );
    }

    #[tokio::test]
    async fn producer_rejects_oversize_context() {
        // S9 — the inline engine's `build_context_bounded` enforces
        // `max_context_size` BEFORE any rendering happens. The external
        // producer must inherit the same bound (ADR-0047 Stage 2
        // invariant: templates written against the inline language
        // render identically against the external component, including
        // their security bounds). The Task 4.2 first cut dropped the
        // bound by re-deriving the context builder without it; this
        // test pins the restored property by setting
        // `max_context_size` to a value the body alone exceeds and
        // asserting the producer rejects the render with the S9 error
        // BEFORE the body is mutated.
        let small_limits = ResolvedLimits {
            max_context_size: 16,
            ..ResolvedLimits::default()
        };
        let templates = make_templates(
            "echo.html",
            r#"{% autoescape "none" %}{{body}}{% endautoescape %}"#,
        );
        let producer = TemplateProducer::new(templates, small_limits, None, "test-route");

        // 64 bytes of payload — well above the 16-byte context bound.
        // The S9 bound (or the LimitedWriter measurement pass) trips
        // before any allocation or rendering.
        let original_body = "x".repeat(64);
        let mut exchange = Exchange::new(Message::new(Body::from(original_body.clone())));

        let result = producer.render_into(&mut exchange).await;
        assert!(
            matches!(result, Err(CamelError::ProcessorError(_))),
            "S9 context-overflow must map to CamelError::ProcessorError, got: {result:?}"
        );
        // S9 trips before render, so the body-unchanged-on-error
        // contract still holds.
        match &exchange.input.body {
            Body::Text(s) => assert_eq!(
                s, &original_body,
                "body must be byte-identical after S9 rejection"
            ),
            other => panic!("expected unchanged Text body, got: {other:?}"),
        }
    }

    /// Mirrors Apache Camel's `VelocityConcurrentTest`: 10 producers × pool 5
    /// asserting `assertNoDuplicates(body())`. Our `TemplateProducer` uses an
    /// `ArcSwap` swap cell (lock-free load on the hot path) cloned by every
    /// producer clone, and `render_entry` hands an `Arc<Environment>` to
    /// `spawn_blocking`. The combination SHOULD be thread-safe, but we have
    /// zero concurrent-load coverage — this test exercises the
    /// `ArcSwap::load_full` + `spawn_blocking` paths simultaneously from
    /// many tasks, asserting no data race and no panic.
    ///
    /// `Service::call` takes `&mut self`, so each task clones the producer.
    /// All clones share the same `SharedTemplates` cell, so each task
    /// exercises the `ArcSwap::load_full` snapshot path against the same
    /// compiled set. If `load_full` or `render_entry` were not thread-safe,
    /// the test would either panic or return a mismatched body.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn producer_handles_concurrent_renders() {
        // The entry echoes `{{body}}` under autoescape="none" so the
        // rendered output is exactly `user=<body>`. Each task uses a
        // DISTINCT body — if the render path races, two tasks will
        // observe each other's bodies.
        let templates = make_templates(
            "echo.html",
            r#"{% autoescape "none" %}user={{body}}{% endautoescape %}"#,
        );
        let producer = TemplateProducer::new(
            templates,
            ResolvedLimits::default(),
            None,
            "test-concurrent-route",
        );

        // 20 tasks is well above the 10 Apache Camel uses; on this
        // hardware it's enough to keep all 4 worker threads busy
        // contending on the ArcSwap cell while the spawn_blocking
        // pool runs the renders.
        const N: usize = 20;
        let mut handles = Vec::with_capacity(N);
        for i in 0..N {
            // Each task gets its own clone (Clone is derived, all
            // clones share the same `SharedTemplates` Arc).
            let mut producer = producer.clone();
            let body = format!("user-{i}");
            handles.push(tokio::spawn(async move {
                let exchange = make_exchange_with_body(&body);
                let result = producer.call(exchange).await.expect("call ok");
                let rendered = body_string(&result).expect("rendered body");
                let expected = format!("user=user-{i}");
                assert_eq!(
                    rendered, expected,
                    "task {i} observed wrong body — concurrent render raced"
                );
                expected
            }));
        }

        // Drain every task. A panic inside any `tokio::spawn` future
        // surfaces here as a `JoinError`; we surface it as a test
        // failure rather than letting the test pass silently.
        let outputs = futures::future::join_all(handles).await;
        let mut seen: Vec<String> = Vec::with_capacity(N);
        for (i, join) in outputs.into_iter().enumerate() {
            seen.push(
                join.unwrap_or_else(|e| panic!("task {i} panicked during concurrent render: {e}")),
            );
        }

        // The multiset of seen outputs equals the expected multiset:
        // no body was duplicated, dropped, or cross-contaminated.
        let mut expected: Vec<String> = (0..N).map(|i| format!("user=user-{i}")).collect();
        seen.sort();
        expected.sort();
        assert_eq!(
            seen, expected,
            "concurrent renders must not drop, duplicate, or cross bodies"
        );
    }

    #[tokio::test]
    async fn producer_ignores_override_header() {
        // The operator-configured entry is a simple echo of `{{body}}`
        // registered under the name `op-entry`. Even if the inbound
        // exchange carries an `X-Template-File` header pointing
        // elsewhere, the producer must render `op-entry` — the entry
        // and root are operator-fixed at startup.
        let templates = make_templates(
            "op-entry",
            r#"{% autoescape "none" %}{{body}}{% endautoescape %}"#,
        );
        let mut producer =
            TemplateProducer::new(templates, ResolvedLimits::default(), None, "test-route");

        let mut exchange = make_exchange_with_body("op-output");
        // Adversarial header — must be ignored entirely.
        exchange.input.headers.insert(
            "X-Template-File".to_string(),
            Value::String("/etc/passwd".to_string()),
        );
        // Extra noise header — also ignored.
        exchange.input.headers.insert(
            "CamelTemplateRoot".to_string(),
            Value::String("evil-root".to_string()),
        );

        let result = producer.call(exchange).await.expect("call ok");
        assert_eq!(
            body_string(&result).as_deref(),
            Some("op-output"),
            "producer must render the operator-configured entry, \
             ignoring X-Template-File / CamelTemplateRoot headers"
        );
        // The override headers remain present in the inbound headers
        // map (the producer does not strip them) — but they had ZERO
        // effect on the rendered output.
        let keep: HashMap<_, _> = result
            .input
            .headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        assert!(keep.contains_key("X-Template-File"));
    }

    /// S6 — output-size cap is enforced on the RENDER path (not just
    /// registered). Mirrors Apache Camel's bounded-output tests, which
    /// prove Velocity/FreeMarker do not OOM on a template that produces
    /// more output than the configured limit.
    ///
    /// The contract being pinned here is the LimitedWriter contract: the
    /// writer aborts mid-write the moment `max_output_size` is exceeded,
    /// the render fails with an io error, the producer maps the error
    /// to `CamelError::ProcessorError`, and the body is left unchanged.
    /// If the contract drifts (e.g. the LimitedWriter is removed or the
    /// render accumulates into an unbounded buffer first) the test
    /// either OOMs or the assertion on the error message fails.
    #[tokio::test]
    async fn producer_fails_closed_on_output_cap() {
        // 1 KiB cap; the template below emits ~10 KiB of 'x' bytes —
        // 10× over the limit, well past the abort point.
        const OUTPUT_CAP: usize = 1024;
        let tight_limits = ResolvedLimits {
            max_output_size: OUTPUT_CAP,
            ..ResolvedLimits::default()
        };
        // The for loop is cheap on fuel (default 100k) — only the
        // LimitedWriter trips, not fuel or recursion.
        let source = r#"{% autoescape "none" %}{% for i in range(0, 10000) %}x{% endfor %}{% endautoescape %}"#;
        let templates = make_templates("flood.html", source);
        let producer = TemplateProducer::new(templates, tight_limits, None, "test-output-cap");

        let original_body = "original-payload".to_string();
        let mut exchange = Exchange::new(Message::new(Body::from(original_body.clone())));

        let result = producer.render_into(&mut exchange).await;
        let err_msg = match &result {
            Err(CamelError::ProcessorError(msg)) => msg.clone(),
            other => panic!(
                "output-cap overflow must surface as CamelError::ProcessorError, got: {other:?}"
            ),
        };
        // The LimitedWriter trips and render_entry wraps the failure as
        // `TemplateReloadError::Compile("render: {e}")` (template_set.rs),
        // which render_into surfaces as `CamelError::ProcessorError`. Assert
        // on the camel-owned `render:` prefix (stable across minijinja
        // versions) rather than minijinja-internal error strings, which a
        // `cargo update` could rephrase. The real contract — fail-closed +
        // body unchanged — is already pinned by the variant assertion above
        // and the body equality below; this prefix check adds provenance
        // that the render path (not e.g. fuel) tripped. Without the cap,
        // the render would report success with ~10 KiB of output.
        assert!(
            err_msg.contains("render:"),
            "output-cap failure must originate from the render path \
             (render_entry prefix); got: {err_msg}"
        );

        // Body-unchanged-on-error contract still holds on the S6 path.
        match &exchange.input.body {
            Body::Text(s) => assert_eq!(
                s, &original_body,
                "body must be byte-identical after S6 output-cap rejection"
            ),
            other => panic!("expected unchanged Text body, got: {other:?}"),
        }
    }

    /// G1 / T1 — pins the `Body::Json` arm of `BodyAsJson`.
    ///
    /// Apache Camel's `VelocityBodyAsDomainObjectTest` /
    /// `FreemarkerBodyAsDomainObjectTest` pass a POJO body and access its
    /// getters (`$body.name`) from the template. Our analogue: `Body::Json`
    /// serialises through `BodyAsJson` as the structured value itself
    /// (`v.serialize(s)`), so `{{ body.first }}` resolves a real field
    /// rather than a string-char. This is the only test exercising that
    /// arm — without it a regression that flattened JSON to a string would
    /// be invisible.
    #[tokio::test]
    async fn producer_exposes_structured_json_body_fields() {
        let templates = make_templates(
            "dom.html",
            r#"{% autoescape "none" %}{{ body.first }} {{ body.last }}{% endautoescape %}"#,
        );
        let producer =
            TemplateProducer::new(templates, ResolvedLimits::default(), None, "test-route");

        let mut exchange = Exchange::new(Message::new(Body::Json(json!({
            "first": "Claus",
            "last": "Ibsen"
        }))));

        producer
            .render_into(&mut exchange)
            .await
            .expect("structured JSON body must render");
        assert_eq!(
            body_string(&exchange).as_deref(),
            Some("Claus Ibsen"),
            "Body::Json fields must be addressable as body.<field> in the template"
        );
    }

    /// G2 / T2 Case A — empty body, template never references `{{ body }}`.
    ///
    /// `Body::Empty` serialises to an empty string (the `BodyAsJson::Empty`
    /// arm calls `serialize_str("")`, see rc-wnqj). A template that does NOT
    /// dereference `body` must render fine: the static text is the whole
    /// output. Pins the safe half of the empty-string ×
    /// `UndefinedBehavior::Strict` interaction — `body` is present in the
    /// context (as an empty string), it is simply never read.
    #[tokio::test]
    async fn producer_renders_static_template_with_empty_body() {
        let templates = make_templates(
            "static.html",
            r#"{% autoescape "none" %}static-only, no body ref{% endautoescape %}"#,
        );
        let producer =
            TemplateProducer::new(templates, ResolvedLimits::default(), None, "test-route");

        let mut exchange = Exchange::new(Message::new(Body::Empty));

        producer
            .render_into(&mut exchange)
            .await
            .expect("a template that never touches body must render with Body::Empty");
        assert_eq!(
            body_string(&exchange).as_deref(),
            Some("static-only, no body ref"),
            "static text must render verbatim regardless of an empty body"
        );
    }

    /// G2 / T2 Case B — empty body, template DOES reference `{{ body }}`.
    ///
    /// Historically the highest-risk blind spot in the audit (§4):
    /// `Body::Empty` → `serialize_none` → JSON `null`, rendered under
    /// `UndefinedBehavior::Strict`, emitted the literal string `"none"`
    /// into HTML/SSR output. Strict-undefined did NOT trip because the
    /// variable `body` IS defined in the context (it held a none value);
    /// strict-undefined only errors on absent variables.
    ///
    /// Reversed by rc-wnqj: `Body::Empty` now serialises to an empty
    /// string (`serialize_str("")`), so `{{ body }}` renders `""` — not
    /// `"none"`, not an error. A template like `<p>{{ body }}</p>` with
    /// an empty inbound body now yields `<p></p>`. Operators no longer
    /// need to guard empty bodies with `{% if body is defined and body %}…`
    /// for the common case.
    #[tokio::test]
    async fn producer_renders_empty_string_for_referenced_empty_body() {
        let templates = make_templates(
            "echo.html",
            r#"{% autoescape "none" %}{{ body }}{% endautoescape %}"#,
        );
        let producer =
            TemplateProducer::new(templates, ResolvedLimits::default(), None, "test-route");

        let mut exchange = Exchange::new(Message::new(Body::Empty));

        producer.render_into(&mut exchange).await.expect(
            "referencing an empty-string body does NOT trip strict-undefined; \
                     the variable is defined",
        );
        assert_eq!(
            body_string(&exchange).as_deref(),
            Some(""),
            "Body::Empty renders an empty string (rc-wnqj reversal), \
             NOT the literal 'none' and NOT a ProcessorError"
        );
    }

    /// G7 / T3 — pins the `Body::Bytes` arm of `BodyAsJson` and the
    /// lossy-UTF-8 contract. `Body::Bytes` serialises via
    /// `serialize_str(&String::from_utf8_lossy(b))`: valid UTF-8 passes
    /// through verbatim; invalid byte sequences become U+FFFD (replacement
    /// char) and the render SUCCEEDS. This guards against a regression to
    /// strict `from_utf8` (which would panic or error on a binary body) and
    /// documents that binary payloads are lossy-coerced, never rejected.
    #[tokio::test]
    async fn producer_renders_bytes_body_lossy_utf8() {
        // --- valid UTF-8: lossy is identity ---
        let templates = make_templates(
            "echo.html",
            r#"{% autoescape "none" %}{{ body }}{% endautoescape %}"#,
        );
        let producer =
            TemplateProducer::new(templates, ResolvedLimits::default(), None, "test-route");

        let mut exchange = Exchange::new(Message::new(Body::Bytes(
            "héllo".as_bytes().to_vec().into(),
        )));
        producer
            .render_into(&mut exchange)
            .await
            .expect("valid UTF-8 bytes body must render");
        assert_eq!(
            body_string(&exchange).as_deref(),
            Some("héllo"),
            "valid UTF-8 bytes must render verbatim"
        );

        // --- invalid UTF-8: lossy replacement, render still succeeds ---
        let mut exchange = Exchange::new(Message::new(Body::Bytes(
            // 0xff / 0xfe are not valid UTF-8 lead bytes → U+FFFD each;
            // 0x00 is a valid (null) codepoint. Together they prove no
            // panic and no rejection on binary-ish payloads.
            vec![0xff, 0xfe, 0x00].into(),
        )));
        producer
            .render_into(&mut exchange)
            .await
            .expect("invalid UTF-8 bytes must render via lossy replacement, not error");
        let rendered = body_string(&exchange).expect("rendered body must be text");
        assert!(
            rendered.contains('\u{FFFD}'),
            "invalid UTF-8 bytes must surface as U+FFFD replacement chars (lossy contract); \
             got {rendered:?}"
        );
    }

    /// G14 / T4 — pins the `exchangeProperty` context key (the third key
    /// in `build_context_bounded`'s map, alongside `body` and `headers`).
    /// The spelling is the singular Camel-canonical `exchangeProperty`
    /// (confirmed in `engine.rs`'s `BTreeMap`). Apache Camel's
    /// `ValuesInProperties`-style suites assert a property is reachable
    /// from the template; this is the rust-camel analogue.
    #[tokio::test]
    async fn producer_exposes_exchange_property_key() {
        let templates = make_templates(
            "prop.html",
            r#"{% autoescape "none" %}{{ exchangeProperty.item }}{% endautoescape %}"#,
        );
        let producer =
            TemplateProducer::new(templates, ResolvedLimits::default(), None, "test-route");

        let mut exchange = Exchange::new(Message::new(Body::Empty));
        exchange.set_property("item", Value::String("7".into()));

        producer
            .render_into(&mut exchange)
            .await
            .expect("exchangeProperty.<key> must resolve from exchange properties");
        assert_eq!(
            body_string(&exchange).as_deref(),
            Some("7"),
            "exchange properties must be addressable as exchangeProperty.<name>"
        );
    }

    /// S8 — no host callables (ADR-0047 §8). The template engine exposes zero
    /// functions, methods, or globals that a template author could invoke to
    /// reach the host runtime. `compile` registers ONLY template entries via
    /// `add_template_owned` (template_set.rs:114); it NEVER calls
    /// `add_function`, `add_filter`, or `set_globals`. A template that invokes
    /// an unknown function (e.g. `{{ evil_global_function() }}`) MUST FAIL at
    /// render — it must NOT silently no-op and must NOT reach any host code.
    ///
    /// This is the LAST coverage gap before the change can re-validate gates
    /// and merge (e_opus audit gap G10, LOW). The observed error (probed live)
    /// is `"template compilation failed: render: unknown function: ..."` — the
    /// camel-owned `render:` prefix is stable across minijinja versions.
    #[tokio::test]
    async fn producer_rejects_unknown_function_call() {
        let templates = make_templates(
            "evil.html",
            r#"{% autoescape "none" %}{{ evil_global_function() }}{% endautoescape %}"#,
        );
        let producer =
            TemplateProducer::new(templates, ResolvedLimits::default(), None, "test-route");

        let original_body = "original-payload".to_string();
        let mut exchange = Exchange::new(Message::new(Body::from(original_body.clone())));

        let result = producer.render_into(&mut exchange).await;
        assert!(
            matches!(result, Err(CamelError::ProcessorError(_))),
            "unknown function call must surface as CamelError::ProcessorError \
             (data-plane render error), got: {result:?}"
        );

        // Body must be byte-unchanged after the failed render (fail-closed, S6
        // — same as other render-error tests).
        match &exchange.input.body {
            Body::Text(s) => assert_eq!(
                s, &original_body,
                "body must be byte-identical after S8 unknown-function rejection"
            ),
            other => panic!("expected unchanged Text body, got: {other:?}"),
        }
    }
}
