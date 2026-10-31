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
use std::task::{Context, Poll};

use camel_api::{Body, CamelError, Exchange};
use camel_language_minijinja::ResolvedLimits;
use camel_language_minijinja::engine::build_context_bounded;
use tower::Service;

use crate::error::TemplateReloadError;
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
#[derive(Clone)]
pub(crate) struct TemplateProducer {
    templates: SharedTemplates,
    render_limits: ResolvedLimits,
}

impl TemplateProducer {
    /// Build a producer bound to the operator-configured compiled set and
    /// the resolved render limits.
    pub(crate) fn new(templates: SharedTemplates, render_limits: ResolvedLimits) -> Self {
        Self {
            templates,
            render_limits,
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
        // the await.
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
    /// On render failure the producer emits a WARN log carrying the render
    /// phase, the operator-configured entry name, and a best-effort error
    /// class, then returns `CamelError::ProcessorError` exactly as before.
    /// The log carries no error string and no exchange payload or context:
    /// minijinja interpolates operand values into some error displays, so the
    /// full error Display flows exclusively on the `ProcessorError` surface.
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

        let rendered = match set.render_entry(context, self.render_limits).await {
            Ok(rendered) => rendered,
            Err(err) => {
                let template = set.entry_name();
                let class = match &err {
                    TemplateReloadError::Render(msg) => render_error_class(msg),
                    _ => "other",
                };
                tracing::warn!(
                    target: "camel::template",
                    phase = "render",
                    template = %template,
                    class,
                    "template render failed; body left unchanged; diagnostics live in \
                     logs (HTTP consumers return an empty 500 body by policy)"
                );
                return Err(CamelError::ProcessorError(err.to_string()));
            }
        };

        // Body is replaced ONLY on the success path. A failure above
        // returns without touching `exchange.input.body`, so the
        // body-unchanged-on-error contract holds.
        exchange.input.body = Body::from(rendered);
        Ok(())
    }
}

/// Best-effort classification of a render error message for the render-failure
/// WARN log. The check order matters: camel-owned sub-phase prefixes
/// (`template lookup:`, timeout, `spawn_blocking join:`, `non-utf8 output:`)
/// come first, then the minijinja kind descriptions, with `other` as the
/// fallback. The full message stays on the `ProcessorError` surface and never
/// enters the log, so a misclassification only affects the `class` field.
fn render_error_class(msg: &str) -> &'static str {
    if msg.contains("template lookup:") {
        "lookup"
    } else if msg.contains("minijinja execution timeout") {
        "timeout"
    } else if msg.contains("spawn_blocking join:") {
        "join"
    } else if msg.contains("non-utf8 output:") {
        "utf8"
    } else if msg.contains("unknown filter") {
        "filter"
    } else if msg.contains("unknown function") {
        "function"
    } else if msg.contains("undefined") {
        "undefined"
    } else if msg.contains("syntax error") {
        "syntax"
    } else {
        "other"
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

    /// Install a bare registry as the process-global tracing default, once per
    /// test binary.
    ///
    /// Guards capture tests against callsite-interest poisoning: `tracing`
    /// caches each callsite's `Interest` process-wide from its FIRST macro
    /// execution, evaluated against the executing thread's dispatcher. A
    /// subscriber-less sibling test that hits a shared `warn!` callsite first
    /// caches `Interest::never`, so a later thread-local `set_default` capture
    /// silently drops events. The global registry heals prior poison and floors
    /// future rebuilds at `sometimes` (convention:
    /// `docs/testing/tracing-capture-guards.md`).
    fn ensure_global_tracing_default() {
        static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if INIT.set(()).is_ok() {
            let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
        }
    }

    /// Flattens an event's structured fields plus its message into a single
    /// `key=value key=value ...` line for substring assertions.
    ///
    /// `%value` (Display) and message fields arrive through `record_debug`;
    /// string shorthand fields arrive through `record_str` and are kept
    /// unquoted so assertions can match `class=undefined` directly.
    struct FieldCollector {
        line: String,
    }

    impl FieldCollector {
        fn push(&mut self, field: &tracing::field::Field, rendered: &str) {
            if !self.line.is_empty() {
                self.line.push(' ');
            }
            self.line.push_str(field.name());
            self.line.push('=');
            self.line.push_str(rendered);
        }
    }

    impl tracing::field::Visit for FieldCollector {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.push(field, value);
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.push(field, &format!("{value:?}"));
        }
    }

    /// `tracing_subscriber` layer recording WARN events as flattened lines.
    struct CaptureLayer {
        events: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl<C> tracing_subscriber::Layer<C> for CaptureLayer
    where
        C: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, C>,
        ) {
            if *event.metadata().level() != tracing::Level::WARN {
                return;
            }
            let mut collector = FieldCollector {
                line: String::new(),
            };
            event.record(&mut collector);
            self.events
                .lock()
                .expect("capture buffer lock")
                .push(collector.line);
        }
    }

    /// Install a thread-local subscriber recording WARN lines; returns the
    /// shared buffer and the default-subscriber guard.
    ///
    /// `set_default` (guard-scoped, thread-local) is used instead of
    /// `with_default` because the guard must span the `.await` points of an
    /// async test body; `#[tokio::test]` polls the future on the test thread,
    /// so the thread-local subscriber observes the producer's warn event.
    fn capture_warns() -> (
        Arc<std::sync::Mutex<Vec<String>>>,
        tracing::subscriber::DefaultGuard,
    ) {
        use tracing_subscriber::layer::SubscriberExt as _;
        use tracing_subscriber::util::SubscriberInitExt as _;

        ensure_global_tracing_default();
        let events: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let layer = CaptureLayer {
            events: Arc::clone(&events),
        };
        let guard = tracing_subscriber::registry().with(layer).set_default();
        (events, guard)
    }

    /// Build a `SharedTemplates` seeded with a single-entry `ClosureSnapshot`
    /// compiled to the entry name `entry_name`.
    fn make_templates(entry_name: &str, source: &str) -> SharedTemplates {
        let snap = ClosureSnapshot::from_single_entry(entry_name, source.as_bytes().to_vec());
        let set = TemplateSet::compile(&snap, entry_name, MinijinjaLimitsConfig::default(), None)
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
        let mut producer = TemplateProducer::new(templates, ResolvedLimits::default());

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
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());

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
        let err_msg = match &result {
            Err(CamelError::ProcessorError(msg)) => msg.clone(),
            other => panic!(
                "strict-undefined is a data-plane render error and must surface as \
                 CamelError::ProcessorError (NOT CamelError::TemplateReload, which is \
                 reserved for the control-plane startup-build and hot-reload paths in \
                 lifecycle.rs / reload.rs), got: {other:?}"
            ),
        };
        assert!(
            err_msg.starts_with("template render failed:"),
            "render failures must carry the render-variant display prefix, got: {err_msg}"
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
        let producer = TemplateProducer::new(templates, small_limits);

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
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());

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
        let mut producer = TemplateProducer::new(templates, ResolvedLimits::default());

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
        let producer = TemplateProducer::new(templates, tight_limits);

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
        // `TemplateReloadError::Render("render: {e}")` (template_set.rs),
        // which render_into surfaces as `CamelError::ProcessorError`. Assert
        // on the render-variant display prefix and the camel-owned `render:`
        // sub-phase prefix (both stable across minijinja versions) rather
        // than minijinja-internal error strings, which a `cargo update` could
        // rephrase. The real contract — fail-closed + body unchanged — is
        // already pinned by the variant assertion above and the body equality
        // below; this prefix check adds provenance that the render path (not
        // e.g. fuel) tripped. Without the cap, the render would report success
        // with ~10 KiB of output.
        assert!(
            err_msg.contains("template render failed: render:"),
            "output-cap failure must originate from the render path \
             (render_entry prefixes); got: {err_msg}"
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
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());

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
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());

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
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());

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
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());

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
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());

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
    /// is `"template render failed: render: unknown function: ..."` — the
    /// camel-owned `render:` prefix is stable across minijinja versions.
    #[tokio::test]
    async fn producer_rejects_unknown_function_call() {
        let templates = make_templates(
            "evil.html",
            r#"{% autoescape "none" %}{{ evil_global_function() }}{% endautoescape %}"#,
        );
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());

        let original_body = "original-payload".to_string();
        let mut exchange = Exchange::new(Message::new(Body::from(original_body.clone())));

        let result = producer.render_into(&mut exchange).await;
        let err_msg = match &result {
            Err(CamelError::ProcessorError(msg)) => msg.clone(),
            other => panic!(
                "unknown function call must surface as CamelError::ProcessorError \
                 (data-plane render error), got: {other:?}"
            ),
        };
        assert!(
            err_msg.starts_with("template render failed:"),
            "unknown-function failure must carry the render-variant display prefix, \
             got: {err_msg}"
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

    /// Pins the best-effort classifier for every render error class the
    /// `render_entry` sub-phase prefixes and minijinja kind descriptions can
    /// produce. The order is intentional: camel-owned sub-phase prefixes are
    /// checked before minijinja text, and `other` is the fallback. A
    /// misclassification is cosmetic only — the full message never enters the
    /// log and stays on the `ProcessorError` surface.
    #[test]
    fn render_error_class_maps_known_classes() {
        let cases = [
            ("template lookup: x", "lookup"),
            ("minijinja execution timeout", "timeout"),
            ("minijinja spawn_blocking join: x", "join"),
            ("non-utf8 output: x", "utf8"),
            ("render: unknown filter: f", "filter"),
            ("render: unknown function: f", "function"),
            ("render: undefined value: v", "undefined"),
            ("render: syntax error: x", "syntax"),
            ("render: something else", "other"),
        ];
        for (msg, expected) in cases {
            assert_eq!(
                render_error_class(msg),
                expected,
                "classifier must map {msg:?} to {expected:?}"
            );
        }
    }

    /// A render failure must emit exactly one WARN carrying the render phase,
    /// the operator-configured entry name, and the best-effort class — and
    /// must never include exchange payload.
    #[tokio::test]
    async fn producer_render_failure_logs_render_phase() {
        let templates = make_templates(
            "boom.html",
            r#"{% autoescape "none" %}{{undefined_var}}{% endautoescape %}"#,
        );
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());
        let mut exchange = make_exchange_with_body("original-payload-secret");

        let (warns, _guard) = capture_warns();
        let result = producer.render_into(&mut exchange).await;
        assert!(
            matches!(result, Err(CamelError::ProcessorError(_))),
            "render failure must stay a ProcessorError, got: {result:?}"
        );

        let warns = warns.lock().expect("capture buffer lock");
        assert_eq!(
            warns.len(),
            1,
            "exactly one WARN expected for one render failure, got: {warns:?}"
        );
        let line = &warns[0];
        for needle in [
            "phase=render",
            "template=boom.html",
            "class=undefined",
            "template render failed",
        ] {
            assert!(
                line.contains(needle),
                "render-failure WARN must contain {needle:?}, got: {line}"
            );
        }
        assert!(
            !line.contains("original-payload-secret"),
            "render-failure WARN must never contain exchange payload, got: {line}"
        );
    }

    /// An unknown filter fails at render time; the WARN must name the entry
    /// and classify the failure as a filter error.
    #[tokio::test]
    async fn producer_unknown_filter_logs_filter_class() {
        let templates = make_templates(
            "filter.html",
            r#"{% autoescape "none" %}{{ x | nosuchfilter }}{% endautoescape %}"#,
        );
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());
        let mut exchange = make_exchange_with_body("payload");

        let (warns, _guard) = capture_warns();
        let result = producer.render_into(&mut exchange).await;
        assert!(
            result.is_err(),
            "unknown filter must fail the render, got: {result:?}"
        );

        let warns = warns.lock().expect("capture buffer lock");
        assert_eq!(
            warns.len(),
            1,
            "exactly one WARN expected for one render failure, got: {warns:?}"
        );
        let line = &warns[0];
        assert!(
            line.contains("template=filter.html"),
            "WARN must name the failing entry, got: {line}"
        );
        assert!(
            line.contains("class=filter"),
            "WARN must classify an unknown filter, got: {line}"
        );
    }

    /// The producer WARN must never carry the error Display: minijinja
    /// interpolates operand values into some error details (for example
    /// `unable to calculate {lhs} // {rhs}` on integer division by zero), so
    /// logging the Display would leak exchange-derived payload into the log.
    /// The full detail stays on the `CamelError::ProcessorError` surface only.
    #[tokio::test]
    async fn producer_warn_omits_value_bearing_error_detail() {
        let templates = make_templates(
            "leak.html",
            r#"{% autoescape "none" %}{{ (body | int) // 0 }}{% endautoescape %}"#,
        );
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());
        let mut exchange = make_exchange_with_body("867530912345");

        let (warns, _guard) = capture_warns();
        let result = producer.render_into(&mut exchange).await;

        let err_msg = match &result {
            Err(CamelError::ProcessorError(msg)) => msg.clone(),
            other => panic!(
                "division by zero must surface as CamelError::ProcessorError, got: {other:?}"
            ),
        };
        // Boundary: the full error detail (which embeds the payload value)
        // travels on the ProcessorError surface, never in the log.
        assert!(
            err_msg.contains("867530912345"),
            "ProcessorError must keep the full error detail, got: {err_msg}"
        );

        let warns = warns.lock().expect("capture buffer lock");
        assert_eq!(
            warns.len(),
            1,
            "exactly one WARN expected for one render failure, got: {warns:?}"
        );
        let line = &warns[0];
        assert!(
            !line.contains("867530912345"),
            "render-failure WARN must never contain exchange payload, got: {line}"
        );
        assert!(
            !line.contains("unable to calculate"),
            "render-failure WARN must never contain the value-bearing error display, got: {line}"
        );
    }

    /// The success path must not emit any WARN.
    #[tokio::test]
    async fn producer_success_logs_no_warn() {
        let templates = make_templates(
            "echo.html",
            r#"{% autoescape "none" %}body={{body}}{% endautoescape %}"#,
        );
        let producer = TemplateProducer::new(templates, ResolvedLimits::default());
        let mut exchange = make_exchange_with_body("hello");

        let (warns, _guard) = capture_warns();
        producer
            .render_into(&mut exchange)
            .await
            .expect("successful render");

        let warns = warns.lock().expect("capture buffer lock");
        assert!(
            warns.is_empty(),
            "successful render must emit no WARN, got: {warns:?}"
        );
    }
}
