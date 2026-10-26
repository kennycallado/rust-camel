//! End-to-end pin for the `parse_json(body)` binding (bd rc-2nx48).
//!
//! The downstream repro binds the exchange body with
//! `let data = parse_json(body); data.a`: an eager `Text` body parses, while
//! an eager `Bytes` body used to report function-not-found at 1:12 because no
//! guard resolved for a non-string argument. The guards added in
//! `camel-language-rhai` resolve the call and fail with a typed
//! `string`/`blob` mismatch at the same position.
//!
//! The binding happens in the language evaluation layer and is
//! consumer-independent, so a `direct:` consumer through the real
//! `camel-dsl`/`camel-test` pipeline (the
//! `gh62_language_boundary_regression_test.rs` harness) exercises the
//! identical path with a real `Exchange` body. No HTTP listener is faked.

use std::time::Duration;

use camel_api::{
    Body, BoxProcessor, CamelError, ErrorPosition, Exchange, ExpressionErrorClass, Message,
};
use camel_component_api::test_support::acquire_deadline;
use camel_core::LanguageRegistryError;
use camel_dsl::parse_yaml;
use camel_language_rhai::RhaiLanguage;
use camel_test::CamelTestContext;
use tower::ServiceExt;

/// The downstream repro shape: `parse_json(body)` starts at 1:12 and its
/// result is indexed and stringified into the body, proving the parse.
const PARSE_JSON_YAML: &str = r#"routes:
  - id: parse-json-probe
    from: "direct:parse-json-probe"
    steps:
      - set_body:
          rhai: |-
            let data = parse_json(body); data.a.to_string()
"#;

fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(camel_component_api::NoOpComponentContext)
}

fn ensure_rhai_registered(ctx: &mut camel_core::CamelContext) {
    match ctx.register_language("rhai", Box::new(RhaiLanguage::new())) {
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

/// Register rhai and start the shared repro route in a fresh harness.
async fn start_parse_json_route() -> CamelTestContext {
    let h = CamelTestContext::builder().with_direct().build().await;
    {
        let mut guard = acquire_deadline(
            h.ctx(),
            "camel context (register_rhai)",
            Duration::from_secs(10),
        )
        .await;
        ensure_rhai_registered(&mut guard);
    }

    for route in parse_yaml(PARSE_JSON_YAML).expect("parse_json repro YAML must parse") {
        h.add_route(route).await.expect("route must be accepted");
    }
    h.start().await;
    h
}

/// A `Text` body carrying JSON must parse end to end: the step succeeds and
/// the resulting body is the stringified `data.a` (`"1"`), proving the parse
/// and the downstream index both bound the real exchange body.
#[tokio::test(flavor = "multi_thread")]
async fn parse_json_text_body_succeeds_end_to_end() {
    let h = start_parse_json_route().await;

    let exchange = Exchange::new(Message::new(Body::Text(r#"{"a":1}"#.to_string())));
    let ex = send_when_registered(&h, "direct:parse-json-probe", exchange)
        .await
        .expect("a Text JSON body must parse through parse_json");

    assert_eq!(
        ex.input.body.as_text(),
        Some("1"),
        "set_body must observe the parsed `data.a`, got: {:?}",
        ex.input.body
    );

    h.stop().await;
}

/// A `Bytes` body carrying JSON must fail with the typed `string`/`blob`
/// mismatch at the parse_json call position (1:12) — never function-not-found.
/// The cause carries only the redacted type names, so no payload leaks.
#[tokio::test(flavor = "multi_thread")]
async fn parse_json_bytes_body_fails_with_typed_mismatch() {
    let h = start_parse_json_route().await;

    let exchange = Exchange::new(Message::new(Body::Bytes(b"{\"a\":1}".to_vec().into())));
    let err = send_when_registered(&h, "direct:parse-json-probe", exchange)
        .await
        .expect_err("a Bytes body must fail the parse_json guard");

    let rendered = format!("{err}");
    match &err {
        CamelError::ExpressionFailed {
            language,
            route_id,
            step_id,
            verb,
            class,
            position,
            cause,
            ..
        } => {
            assert_eq!(language, "rhai");
            assert_eq!(route_id, "parse-json-probe");
            assert_eq!(verb, "set_body");
            assert_eq!(step_id, "set_body#0");
            assert_eq!(
                *class,
                ExpressionErrorClass::TypeMismatch,
                "a blob body must classify as a typed mismatch: {rendered}"
            );
            assert_eq!(
                *position,
                Some(ErrorPosition {
                    line: 1,
                    column: 12
                }),
                "the mismatch must point at the parse_json call: {rendered}"
            );
            let cause = cause
                .as_ref()
                .expect("TypeMismatch must attach a redacted cause")
                .to_string();
            assert_eq!(
                cause, "Processor error: expected string, actual blob",
                "cause must carry the redacted type names only"
            );
        }
        other => panic!("expected CamelError::ExpressionFailed, got: {other:?}"),
    }

    assert!(
        rendered.contains("type-mismatch"),
        "the rendered error must name the class: {rendered}"
    );
    assert!(
        rendered.contains("1:12"),
        "the rendered error must carry the source position: {rendered}"
    );
    assert!(
        !rendered.contains("function-not-found"),
        "the guard must resolve the call, never report function-not-found: {rendered}"
    );

    h.stop().await;
}
