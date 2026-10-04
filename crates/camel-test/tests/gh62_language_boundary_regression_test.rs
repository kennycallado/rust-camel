//! GH #62 language value-boundary regression suite, repro 1 (task 3.1).
//!
//! Pins the end-to-end pipeline behavior fixed by the `language-value-boundary`
//! change: a failing rhai expression in a route step must fail the step loudly
//! with a typed `CamelError::ExpressionFailed` (class `arithmetic` for
//! `parse_float` on a non-numeric string), never silently degrade to `()` and
//! let the route continue to the next step.
//!
//! The YAML below is embedded byte-for-byte from GH #62 (hallazgo 1). The
//! route is compiled through the real `camel-dsl` YAML pipeline and executed
//! through the `camel-test` harness (`direct:` consumer → steps → producer),
//! not through a hand-built processor table.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_api::{Body, BoxProcessor, CamelError, Exchange, ExpressionErrorClass, Message, Value};
use camel_component_api::test_support::acquire_deadline;
use camel_core::LanguageRegistryError;
use camel_dsl::parse_yaml;
use camel_language_js::JsLanguage;
use camel_language_jsonpath::JsonPathLanguage;
use camel_language_rhai::RhaiLanguage;
use camel_test::CamelTestContext;
use tower::ServiceExt;

/// GH #62 repro 1, verbatim (issue text, hallazgo 1). The first step evaluates
/// a rhai expression that fails `arithmetic`; the second step must never run.
const GH62_REPRO1_YAML: &str = r#"routes:
  - id: probe
    from: "direct:probe"
    steps:
      - set_property:
          name: x
          rhai: |-
            "no-es-un-numero".parse_float()
      - set_body:
          rhai: |-
            "x=" + property("x").to_string()
"#;

fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

fn ensure_rhai_registered(ctx: &mut camel_core::CamelContext) {
    match ctx.register_language("rhai", Box::new(RhaiLanguage::new())) {
        Ok(()) | Err(LanguageRegistryError::AlreadyRegistered { .. }) => {}
    }
}

/// Register `jsonpath` before any `add_route` (the audit R1 filter resolves the
/// language registry at add time).
fn ensure_jsonpath_registered(ctx: &mut camel_core::CamelContext) {
    match ctx.register_language("jsonpath", Box::new(JsonPathLanguage::new())) {
        Ok(()) | Err(LanguageRegistryError::AlreadyRegistered { .. }) => {}
    }
}

/// Register `js` before any `add_route` (the audit R5 `script:` step resolves
/// the language registry at add time).
fn ensure_js_registered(ctx: &mut camel_core::CamelContext) {
    match ctx.register_language("js", Box::new(JsLanguage::new())) {
        Ok(()) | Err(LanguageRegistryError::AlreadyRegistered { .. }) => {}
    }
}

/// Build a producer for a `direct:` endpoint. The context-lock guard is dropped
/// when this returns, so callers can `.await` the oneshot without holding the
/// lock (clippy::await_holding_lock).
async fn build_direct_producer(
    h: &CamelTestContext,
    endpoint_uri: &str,
    context_label: &str,
) -> BoxProcessor {
    let ctx = acquire_deadline(h.ctx(), context_label, Duration::from_secs(10)).await;
    let producer_ctx = ctx.producer_context();
    let registry = ctx.registry();
    let component = registry
        .get("direct")
        .expect("direct component not registered");
    let endpoint = component
        .create_endpoint(endpoint_uri, &*ctx)
        .expect("failed to create direct endpoint");
    endpoint
        .create_producer(test_rt(), &producer_ctx)
        .expect("failed to create direct producer")
}

/// `true` when the error is the direct component's "consumer not yet
/// registered" rejection: the route consumer's registration task runs in a
/// `tokio::spawn` and may lag `start()` on slow CI runners.
fn direct_not_registered(err: &CamelError) -> bool {
    matches!(
        err,
        CamelError::EndpointCreationFailed(msg)
            if msg.starts_with("direct endpoint '") && msg.ends_with("' not registered")
    )
}

/// Send to a `direct:` endpoint, retrying while the route consumer's spawned
/// registration task has not yet registered the endpoint (the context lock is
/// released between attempts so registration can proceed). Returns the raw
/// oneshot result — never panics on a failed pipeline, and needs no post-start
/// sleep (lint-test-sleep hygiene). Bounded by a 5s deadline
/// (lint-unbounded-wait).
async fn send_when_registered(
    h: &CamelTestContext,
    endpoint_uri: &str,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let attempt = match tokio::time::timeout_at(deadline, async {
            let producer =
                build_direct_producer(h, endpoint_uri, "camel context (send_when_registered)")
                    .await;
            producer.oneshot(exchange.clone()).await
        })
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                return Err(CamelError::ProcessorError(format!(
                    "direct endpoint '{endpoint_uri}': send_when_registered deadline elapsed"
                )));
            }
        };
        match attempt {
            Err(err) if direct_not_registered(&err) && tokio::time::Instant::now() < deadline => {
                tokio::task::yield_now().await;
            }
            other => return other,
        }
    }
}

/// Register rhai before any `add_route` (route compilation resolves the
/// language registry at add time).
async fn register_rhai(h: &CamelTestContext) {
    let mut guard = acquire_deadline(
        h.ctx(),
        "camel context (register_rhai)",
        Duration::from_secs(10),
    )
    .await;
    ensure_rhai_registered(&mut guard);
}

/// Register jsonpath before any `add_route`.
async fn register_jsonpath(h: &CamelTestContext) {
    let mut guard = acquire_deadline(
        h.ctx(),
        "camel context (register_jsonpath)",
        Duration::from_secs(10),
    )
    .await;
    ensure_jsonpath_registered(&mut guard);
}

/// Register js before any `add_route`.
async fn register_js(h: &CamelTestContext) {
    let mut guard = acquire_deadline(
        h.ctx(),
        "camel context (register_js)",
        Duration::from_secs(10),
    )
    .await;
    ensure_js_registered(&mut guard);
}

/// Extracts the `ExpressionFailed` fields for assertions, panicking with the
/// full error when the pipeline unexpectedly produced something else.
fn expect_expression_failed(
    err: &CamelError,
) -> (String, String, String, String, ExpressionErrorClass) {
    match err {
        CamelError::ExpressionFailed {
            language,
            route_id,
            step_id,
            verb,
            class,
            ..
        } => (
            language.clone(),
            route_id.clone(),
            step_id.clone(),
            verb.clone(),
            *class,
        ),
        other => panic!("expected CamelError::ExpressionFailed, got: {other:?}"),
    }
}

/// GH #62 repro 1: the route must fail loudly at `set_property` with an
/// `ExpressionFailed` of class `arithmetic`; `set_body` must never execute.
///
/// A route-level failure drops the exchange, so the direct producer's oneshot
/// returns the typed error directly. The `verb`/`step_id` pin that the failure
/// happened at step 0 (`set_property#0`), proving the downstream `set_body`
/// (which would have produced `x=...`) never ran.
#[tokio::test(flavor = "multi_thread")]
async fn gh62_repro1_error_fails_step_loudly() {
    let h = CamelTestContext::builder().with_direct().build().await;
    register_rhai(&h).await;

    for route in parse_yaml(GH62_REPRO1_YAML).expect("GH #62 repro 1 YAML must parse") {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let outcome = send_when_registered(&h, "direct:probe", Exchange::default()).await;
    let err = match outcome {
        Err(err) => err,
        Ok(ex) => panic!(
            "GH #62 repro 1 must fail loudly, but the route completed with body {:?}",
            ex.input.body.as_text()
        ),
    };

    let (language, route_id, step_id, verb, class) = expect_expression_failed(&err);
    assert_eq!(language, "rhai");
    assert_eq!(route_id, "probe");
    assert_eq!(
        step_id, "set_property#0",
        "the failure must land on the first step, before set_body"
    );
    assert_eq!(verb, "set_property");
    assert_eq!(
        class,
        ExpressionErrorClass::Arithmetic,
        "`\"no-es-un-numero\".parse_float()` must classify as arithmetic"
    );

    h.stop().await;
}

/// GH #62 repro 1 wrapped in `do_try`/`catch {exception: [ExpressionFailed]}`:
/// the catch must run, the route must complete (Handled), and the caught error
/// carried on the exchange into the catch body must be class `arithmetic`.
#[tokio::test(flavor = "multi_thread")]
async fn gh62_repro1_do_try_catches_expression_failed() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    register_rhai(&h).await;

    // Same two verbatim steps, wrapped in do_try/catch and routed to a mock so
    // the caught exchange (with its error state) is observable.
    let wrapped = r#"routes:
  - id: probe-wrapped
    from: "direct:probe-wrapped"
    steps:
      - do_try:
          steps:
            - set_property:
                name: x
                rhai: |-
                  "no-es-un-numero".parse_float()
            - set_body:
                rhai: |-
                  "x=" + property("x").to_string()
          catch:
            - exception: ["ExpressionFailed"]
              disposition: handled
              steps:
                - to: "mock:caught"
"#;
    for route in parse_yaml(wrapped).expect("wrapped do_try YAML must parse") {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let outcome = send_when_registered(&h, "direct:probe-wrapped", Exchange::default()).await;
    assert!(
        outcome.is_ok(),
        "Handled catch must let the route complete, got: {outcome:?}"
    );

    let caught = h.mock().get_endpoint("caught").unwrap();
    caught.await_exchanges(1, Duration::from_secs(5)).await;

    let received = caught.get_received_exchanges().await;
    assert_eq!(received.len(), 1, "catch body must run exactly once");
    let caught_err = received[0]
        .error
        .as_ref()
        .expect("caught exchange must carry the original ExpressionFailed");
    let (language, route_id, _step_id, verb, class) = expect_expression_failed(caught_err);
    assert_eq!(language, "rhai");
    assert_eq!(route_id, "probe-wrapped");
    assert_eq!(verb, "set_property");
    assert_eq!(class, ExpressionErrorClass::Arithmetic);

    // The try body aborted before set_body, so no `x=...` body was produced.
    assert_eq!(
        received[0].input.body.as_text(),
        None,
        "set_body must not run once set_property failed"
    );

    h.stop().await;
}

/// GH #62 repro 1 in-script try STATEMENT form (E7): a caught in-script
/// evaluation error is fully suppressible. The script assigns the fallback 42
/// and returns it, so the route completes with property `x == 42` and body
/// `x=42` — no route error.
#[tokio::test(flavor = "multi_thread")]
async fn gh62_in_script_try_statement_suppresses_error() {
    let h = CamelTestContext::builder().with_direct().build().await;
    register_rhai(&h).await;

    let script_try = r#"routes:
  - id: probe-try-statement
    from: "direct:probe-try-statement"
    steps:
      - set_property:
          name: x
          rhai: |-
            let x = (); try { x = "no".parse_float(); } catch (err) { x = 42; } x
      - set_body:
          rhai: |-
            "x=" + property("x").to_string()
"#;
    for route in parse_yaml(script_try).expect("in-script try YAML must parse") {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let outcome = send_when_registered(&h, "direct:probe-try-statement", Exchange::default()).await;
    let ex = match outcome {
        Ok(ex) => ex,
        Err(err) => {
            panic!("in-script try/catch must suppress the error, but the route failed: {err:?}")
        }
    };

    assert_eq!(
        ex.properties.get("x").and_then(|v| v.as_i64()),
        Some(42),
        "in-script catch fallback must set property x = 42"
    );
    assert_eq!(
        ex.input.body.as_text(),
        Some("x=42"),
        "downstream set_body must observe the fallback property"
    );

    h.stop().await;
}

/// GH #62 repro 2, verbatim (issue text, hallazgo 2). A rhai map literal set as
/// a property must round-trip across steps as a native rhai map (a JSON
/// object), never degrade to its string representation.
const GH62_REPRO2_YAML: &str = r#"routes:
  - id: probe
    from: "direct:probe"
    steps:
      - set_property:
          name: m
          rhai: |-
            #{ "a": 1, "b": 2 }
      - set_body:
          rhai: |-
            let x = property("m");
            "type=" + type_of(x) + " str=" + x.to_string()
"#;

/// GH #62 repro 2 cross-step indexing shape: the property created by the first
/// step must remain indexable by key in a later step (the exact failure mode
/// reported in the issue was `property("m")["a"]` failing after a round-trip).
const GH62_REPRO2_INDEX_YAML: &str = r#"routes:
  - id: probe-index
    from: "direct:probe-index"
    steps:
      - set_property:
          name: m
          rhai: |-
            #{ "a": 1, "b": 2 }
      - set_body:
          rhai: |-
            property("m")["a"]
"#;

/// GH #62 repro 2: the map literal must survive the value boundary. The body
/// produced by the second step must report `type_of(x) == "map"` (not
/// `"string"`), and the property `m` on the final exchange must be the JSON
/// object `{"a":1,"b":2}` — not a stringified `#{...}`.
#[tokio::test(flavor = "multi_thread")]
async fn gh62_repro2_map_round_trips_as_map() {
    let h = CamelTestContext::builder().with_direct().build().await;
    register_rhai(&h).await;

    for route in parse_yaml(GH62_REPRO2_YAML).expect("GH #62 repro 2 YAML must parse") {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let ex = send_when_registered(&h, "direct:probe", Exchange::default())
        .await
        .expect("GH #62 repro 2 must complete");

    let body = ex
        .input
        .body
        .as_text()
        .expect("set_body must produce a text body");
    assert!(
        body.starts_with("type=map"),
        "map property must round-trip as a rhai map, got body: {body:?}"
    );

    let stored = ex.properties.get("m").expect("property m must be set");
    assert!(
        stored.is_object(),
        "property m must stay a JSON object, got: {stored:?}"
    );
    let expected: Value = r#"{"a":1,"b":2}"#.parse().expect("expected JSON literal");
    assert_eq!(
        stored, &expected,
        "property m must round-trip as {{\"a\":1,\"b\":2}}"
    );

    h.stop().await;
}

/// GH #62 repro 2 cross-step indexing: after the first step stores property `m`,
/// a later step must index it by key. `property("m")["a"]` must yield the
/// integer `1` (Body::Json(1)), proving the map was not flattened to a string.
#[tokio::test(flavor = "multi_thread")]
async fn gh62_repro2_property_indexing_works() {
    let h = CamelTestContext::builder().with_direct().build().await;
    register_rhai(&h).await;

    for route in parse_yaml(GH62_REPRO2_INDEX_YAML).expect("GH #62 repro 2 index YAML must parse") {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let ex = send_when_registered(&h, "direct:probe-index", Exchange::default())
        .await
        .expect("GH #62 repro 2 index must complete");

    assert_eq!(
        ex.input.body,
        Body::Json(Value::from(1_i64)),
        "property(\"m\")[\"a\"] must index the map and yield 1, got: {:?}",
        ex.input.body
    );

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Mission-340 language-boundary audit repros (task 3.3)
//
// Origin: `.opencode/fleet/language-boundary-audit-matrix-20261002.md` section 2
// (repro R1 = jsonpath filter swallow; repro R5 = js thrown-message leak), plus
// GH #62. R2 (xpath Inf-to-Null) is intentionally NOT here: it needs the
// deferred xpath V2 refusal and moves with that follow-up change.
// ---------------------------------------------------------------------------

/// Audit repro R1: a Text body that is not JSON reaches a `jsonpath` filter
/// inside `do_try/catch {exception: [ExpressionFailed]}`. The error must
/// surface as a typed `ExpressionFailed` (language `jsonpath`, verb `filter`)
/// and the catch must fire exactly once — the pre-fix tree silently dropped the
/// exchange and the catch never ran.
const AUDIT_R1_YAML: &str = r#"routes:
  - id: "audit-r1-jsonpath-filter"
    from: "direct:audit-r1"
    steps:
      - set_body:
          value: "this is not JSON"
      - do_try:
          steps:
            - filter:
                jsonpath: "$.x"
                steps:
                  - to: "mock:passed"
          catch:
            - exception: ["ExpressionFailed"]
              disposition: handled
              steps:
                - to: "mock:caught"
"#;

/// Audit repro R1 end-to-end through the real YAML pipeline.
#[tokio::test(flavor = "multi_thread")]
async fn audit_r1_jsonpath_filter_error_caught() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    register_jsonpath(&h).await;

    for route in parse_yaml(AUDIT_R1_YAML).expect("audit R1 YAML must parse") {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let outcome = send_when_registered(&h, "direct:audit-r1", Exchange::default()).await;
    assert!(
        outcome.is_ok(),
        "Handled catch must let the jsonpath route complete, got: {outcome:?}"
    );

    let caught = h
        .mock()
        .get_endpoint("caught")
        .expect("caught endpoint exists");
    caught.await_exchanges(1, Duration::from_secs(5)).await;
    caught.assert_exchange_count(1).await;

    let received = caught.get_received_exchanges().await;
    assert_eq!(received.len(), 1, "catch body must run exactly once");
    let caught_err = received[0]
        .error
        .as_ref()
        .expect("caught exchange must carry the jsonpath ExpressionFailed");
    let (language, route_id, _step_id, verb, class) = expect_expression_failed(caught_err);
    assert_eq!(language, "jsonpath", "the typed error must name jsonpath");
    assert_eq!(route_id, "audit-r1-jsonpath-filter");
    assert_eq!(verb, "filter");
    assert_eq!(
        class,
        ExpressionErrorClass::Conversion,
        "a Text body that is not JSON must classify as conversion"
    );

    h.stop().await;
}

/// Audit repro R5: a real YAML `script:` step in `js` throws
/// `new Error("LEAKED-" + camel.body)` with body `SECRETVAL`. The engine
/// message embeds exchange data; the redaction boundary must keep it out of
/// every observable surface — returned error Display/Debug, captured handler
/// logs, and the DLC-style route-error payload.
///
/// Route A (log-only handler) propagates the typed error so the caller can
/// observe it, and the route handler logs the error Debug — the exact surface
/// where the pre-fix tree printed `LEAKED-SECRETVAL`. Route B forwards the
/// failed exchange to a mock dead-letter endpoint.
const AUDIT_R5_YAML: &str = r#"routes:
  - id: "audit-r5-js-leak-logged"
    from: "direct:audit-r5-logged"
    error_handler: {}
    steps:
      - script:
          language: js
          source: 'throw new Error("LEAKED-" + camel.body);'
  - id: "audit-r5-js-leak-dlc"
    from: "direct:audit-r5-dlc"
    error_handler:
      dead_letter_channel: "mock:dlc"
    steps:
      - script:
          language: js
          source: 'throw new Error("LEAKED-" + camel.body);'
"#;

/// The four redaction surfaces checked by [`audit_r5_js_leak_redacted_end_to_end`].
struct AuditR5Surfaces {
    error_display: String,
    error_debug: String,
    dlc_message: String,
    dlc_error_debug: String,
}

async fn run_audit_r5_case() -> AuditR5Surfaces {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    register_js(&h).await;

    for route in parse_yaml(AUDIT_R5_YAML).expect("audit R5 YAML must parse") {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let secret = Exchange::new(Message::new("SECRETVAL"));

    // (a) Log-only handler route: the failure propagates (typed error is
    // observable) AND the handler logs the error Debug.
    let err = send_when_registered(&h, "direct:audit-r5-logged", secret.clone())
        .await
        .expect_err("audit R5 js throw must fail the step loudly");
    let error_display = err.to_string();
    let error_debug = format!("{err:?}");

    match &err {
        CamelError::ExpressionFailed {
            language,
            route_id,
            step_id,
            verb,
            class,
            position,
            ..
        } => {
            assert_eq!(language, "js");
            assert_eq!(route_id, "audit-r5-js-leak-logged");
            assert_eq!(verb, "script");
            assert_eq!(*class, ExpressionErrorClass::Runtime);
            assert!(
                step_id.starts_with("script#"),
                "step id {step_id:?} must carry the script verb"
            );
            assert!(position.is_none(), "js exposes no source position: {err}");
        }
        other => panic!("expected ExpressionFailed, got {other:?}"),
    }

    // (b) Dead-letter route: the failed exchange is forwarded to the mock DLC
    // endpoint with the original error attached.
    let _ = send_when_registered(&h, "direct:audit-r5-dlc", secret).await;
    let dlc = h.mock().get_endpoint("dlc").expect("dlc endpoint exists");
    dlc.await_exchanges(1, Duration::from_secs(5)).await;
    dlc.assert_exchange_count(1).await;
    let received = dlc.get_received_exchanges().await;
    let dlc_message = received[0]
        .property(camel_api::exchange::PROPERTY_EXCEPTION_MESSAGE)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let dlc_error_debug = format!("{:?}", received[0].error);

    // Probe: proves the tracing capture is wired (non-empty, attributable).
    tracing::warn!("audit-r5-capture-probe");

    h.stop().await;

    AuditR5Surfaces {
        error_display,
        error_debug,
        dlc_message,
        dlc_error_debug,
    }
}

/// Captures `tracing` output so the audit R5 leak assertion can observe the
/// handler's error record (same writer idiom as the camel-core redaction test).
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Audit repro R5 end-to-end: the exchange-data leak must not appear in the
/// returned error, the captured handler logs, or the DLC route-error payload.
#[test]
fn audit_r5_js_leak_redacted_end_to_end() {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing_subscriber::filter::LevelFilter::WARN)
        .without_time()
        .with_writer(buffer.clone())
        .finish();

    let surfaces = {
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = subscriber.set_default();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(run_audit_r5_case())
    };

    // (a) Returned error Display and Debug.
    for (label, text) in [
        ("error Display", &surfaces.error_display),
        ("error Debug", &surfaces.error_debug),
    ] {
        assert!(!text.contains("LEAKED-"), "{label} leaked LEAKED-: {text}");
        assert!(
            !text.contains("SECRETVAL"),
            "{label} leaked SECRETVAL: {text}"
        );
    }

    // (b) Captured handler logs: non-empty (probe present + the route handler
    // logged the real error) and redacted.
    let logs = String::from_utf8_lossy(&buffer.0.lock().expect("log buffer lock")).to_string();
    assert!(
        logs.contains("audit-r5-capture-probe"),
        "log capture must be wired (probe missing); captured: {logs}"
    );
    assert!(
        logs.contains("ExpressionFailed"),
        "handler must have logged the route error; captured: {logs}"
    );
    assert!(
        !logs.contains("LEAKED-"),
        "captured logs leaked LEAKED-: {logs}"
    );
    assert!(
        !logs.contains("SECRETVAL"),
        "captured logs leaked SECRETVAL: {logs}"
    );

    // (c) DLC-style route-error payload. Assert the payload is non-empty and
    // carries the real error first, so the redaction assertions below cannot
    // pass vacuously.
    assert!(
        !surfaces.dlc_message.is_empty(),
        "DLC must carry a non-empty CamelExceptionMessage"
    );
    assert!(
        surfaces.dlc_error_debug.contains("ExpressionFailed"),
        "DLC error Debug must carry the route failure: {}",
        surfaces.dlc_error_debug
    );
    assert!(
        !surfaces.dlc_message.contains("LEAKED-"),
        "DLC CamelExceptionMessage leaked LEAKED-: {}",
        surfaces.dlc_message
    );
    assert!(
        !surfaces.dlc_message.contains("SECRETVAL"),
        "DLC CamelExceptionMessage leaked SECRETVAL: {}",
        surfaces.dlc_message
    );
    assert!(
        !surfaces.dlc_error_debug.contains("LEAKED-"),
        "DLC error Debug leaked LEAKED-: {}",
        surfaces.dlc_error_debug
    );
    assert!(
        !surfaces.dlc_error_debug.contains("SECRETVAL"),
        "DLC error Debug leaked SECRETVAL: {}",
        surfaces.dlc_error_debug
    );
}
