//! Pipeline-level Stop/reply-semantics tests for the HTTP consumer.
//!
//! These tests were moved from `src/lib_tests.rs` so that camel-core stays
//! out of the component src boundary (lint-component-deps). E2E dispatch
//! coverage of the full HTTP reply path lives in camel-test.

#[tokio::test]
async fn http_consumer_returns_body_and_code_on_stop() {
    use camel_api::{Body, BoxProcessor, BoxProcessorExt, Exchange, Message};
    use camel_core::route::{CompiledStep, PipelineRuntimeCtx, compose_pipeline_with_handler};
    use tower::ServiceExt;

    // Pipeline: set_body("nope") + set CamelHttpResponseCode=409 + Stop.
    let set_body_step = CompiledStep::Process {
        kind_hint: camel_api::SpanKindHint::Internal,
        processor: BoxProcessor::from_fn(|mut ex: Exchange| {
            ex.input.body = Body::Text("nope".into());
            Box::pin(async move { Ok(ex) })
        }),
        body_contract: None,
        lifecycle: None,
        label: None,
        to_uri: None,
    };
    let set_status_step = CompiledStep::Process {
        kind_hint: camel_api::SpanKindHint::Internal,
        processor: BoxProcessor::from_fn(|mut ex: Exchange| {
            ex.input.set_header(
                "CamelHttpResponseCode",
                serde_json::Value::Number(409.into()),
            );
            Box::pin(async move { Ok(ex) })
        }),
        body_contract: None,
        lifecycle: None,
        label: None,
        to_uri: None,
    };
    let pipeline = compose_pipeline_with_handler(
        vec![set_body_step, set_status_step, CompiledStep::Stop],
        None,
        PipelineRuntimeCtx::compile_time(),
    );

    let ex = Exchange::new(Message::default());
    let result = pipeline.oneshot(ex).await;
    assert!(result.is_ok(), "Stop must arrive as Ok (Bug B fix)");
    let returned = result.unwrap();
    assert_eq!(returned.input.body.as_text(), Some("nope"));
    assert_eq!(
        returned
            .input
            .header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64()),
        Some(409)
    );
}

#[tokio::test]
async fn http_consumer_returns_200_when_body_empty_on_stop() {
    // After ADR-0024: Stop with no body + no status header produces 200 (same as
    // a normal completion with no body). The 204 default is gone — users who
    // want 204 set CamelHttpResponseCode=204 explicitly.
    //
    // This test stays at the pipeline level (consistent with the test above).
    // E2E coverage of the full HTTP dispatch path is in
    // crates/camel-test/tests/integration_test.rs.
    use camel_api::{Exchange, Message};
    use camel_core::route::{CompiledStep, PipelineRuntimeCtx, compose_pipeline_with_handler};
    use tower::ServiceExt;

    let pipeline = compose_pipeline_with_handler(
        vec![CompiledStep::Stop],
        None,
        PipelineRuntimeCtx::compile_time(),
    );
    let ex = Exchange::new(Message::default());
    let result = pipeline.oneshot(ex).await;
    assert!(result.is_ok(), "Stop with empty body arrives as Ok");
    // Body is default (empty); no CamelHttpResponseCode header was set.
    // The HTTP reply finaliser (tested at E2E) maps this to status=200 + empty body.
}
