//! Mission 515.2: exact diagnostic pins for the context/render branches
//! catalogued in
//! `openspec/changes/deps-majors-minijinja-ed25519/shape-matrix.md`.
//!
//! `evaluate` routes MiniJinja errors through `engine::map_render_error`,
//! which deliberately redacts upstream detail and keeps only the error class,
//! the template line, and the error kind. The pins below therefore assert the
//! exact public `LanguageError` display produced after that mapping — never
//! the upstream MiniJinja text (which is discarded).

use camel_api::body::{StreamBody, StreamMetadata};
use camel_api::error::CamelError;
use camel_language_api::{
    Body, Exchange, Language, LanguageError, Message, MinijinjaLimitsConfig, Value,
};
use camel_language_minijinja::MinijinjaLanguage;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Empty exchange: no headers, properties, or body.
fn empty_exchange() -> Exchange {
    Exchange::new(Message::default())
}

/// Exchange whose input body is the given body.
fn exchange_with_body(body: Body) -> Exchange {
    let mut ex = empty_exchange();
    ex.input.body = body;
    ex
}

/// Exchange with a single string header.
fn exchange_with_header(key: &str, value: &str) -> Exchange {
    let mut ex = empty_exchange();
    ex.input
        .headers
        .insert(key.into(), Value::String(value.into()));
    ex
}

/// Stream-bodied exchange, mirroring the construction used by
/// `body_stream_rejection_test` in `src/lib.rs`.
fn stream_exchange() -> Exchange {
    let stream = futures::stream::empty::<Result<bytes::Bytes, CamelError>>();
    exchange_with_body(Body::Stream(StreamBody {
        stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
        metadata: StreamMetadata {
            size_hint: None,
            content_type: None,
            origin: None,
        },
    }))
}

/// Compile with the default limits and evaluate, requiring a render error.
async fn evaluate_error(script: &str, exchange: &Exchange) -> LanguageError {
    let expr = MinijinjaLanguage::default()
        .create_expression(script)
        .expect("template must compile");
    expr.evaluate(exchange).await.expect_err("render must fail")
}

/// Compile with a context-size limit and evaluate, requiring a render error.
async fn evaluate_error_with_context_limit(
    script: &str,
    exchange: &Exchange,
    max_context_size: usize,
) -> LanguageError {
    let language = MinijinjaLanguage::with_limits(MinijinjaLimitsConfig {
        max_context_size: Some(max_context_size),
        ..Default::default()
    });
    let expr = language
        .create_expression(script)
        .expect("template must compile");
    expr.evaluate(exchange).await.expect_err("render must fail")
}

#[tokio::test]
async fn strict_undefined_interpolation_text_is_pinned() {
    let ex = empty_exchange();
    let err = evaluate_error(
        r#"{% autoescape "none" %}x{{ headers.missing }}x{% endautoescape %}"#,
        &ex,
    )
    .await;
    assert_eq!(
        err.to_string(),
        "evaluation failure: runtime at 1:0: undefined value"
    );
}

#[tokio::test]
async fn strict_undefined_conditional_text_is_pinned() {
    let ex = empty_exchange();
    let err = evaluate_error(
        r#"{% autoescape "none" %}{% if headers.missing %}y{% endif %}{% endautoescape %}"#,
        &ex,
    )
    .await;
    assert_eq!(
        err.to_string(),
        "evaluation failure: runtime at 1:0: undefined value"
    );
}

#[tokio::test]
async fn reader_zero_args_text_is_pinned() {
    let ex = exchange_with_header("a", "1");
    let err = evaluate_error(
        r#"{% autoescape "none" %}{{ header() }}{% endautoescape %}"#,
        &ex,
    )
    .await;
    assert_eq!(
        err.to_string(),
        "evaluation failure: type-mismatch at 1:0: invalid operation"
    );
}

#[tokio::test]
async fn reader_non_string_arg_text_is_pinned() {
    let ex = exchange_with_header("a", "1");
    let err = evaluate_error(
        r#"{% autoescape "none" %}{{ header(1) }}{% endautoescape %}"#,
        &ex,
    )
    .await;
    assert_eq!(
        err.to_string(),
        "evaluation failure: type-mismatch at 1:0: invalid operation"
    );
}

#[tokio::test]
async fn stream_body_refusal_text_is_pinned() {
    let ex = stream_exchange();
    let err = evaluate_error(
        r#"{% autoescape "none" %}{{ body }}{% endautoescape %}"#,
        &ex,
    )
    .await;
    assert_eq!(
        err.to_string(),
        "evaluation error: minijinja cannot render a Body::Stream directly; add `stream_cache` upstream (crates/camel-processor/src/stream_cache.rs:37-54)"
    );
}

/// S9 measurement failure (`engine.rs:408-412`). Uses a `Body::Text` payload
/// so the `Body::Bytes` precheck at `engine.rs:386-393` cannot fire first,
/// mirroring `oversize_context_trips_before_full_allocation`.
#[tokio::test]
async fn max_context_size_text_is_pinned() {
    let ex = exchange_with_body(Body::from("x".repeat(1024)));
    let err = evaluate_error_with_context_limit(
        r#"{% autoescape "none" %}{{ body }}{% endautoescape %}"#,
        &ex,
        8,
    )
    .await;
    assert_eq!(
        err.to_string(),
        "evaluation error: max-context-size 8 bytes exceeded"
    );
}

/// Early bytes precheck (`engine.rs:386-393`). A `Body::Bytes` payload larger
/// than the limit is rejected before the measurement pass can allocate a
/// body-sized lossy decoded copy.
#[tokio::test]
async fn bytes_precheck_refusal_text_is_pinned() {
    let ex = exchange_with_body(Body::Bytes(bytes::Bytes::from("x".repeat(1024))));
    let err = evaluate_error_with_context_limit(
        r#"{% autoescape "none" %}{{ body }}{% endautoescape %}"#,
        &ex,
        8,
    )
    .await;
    assert_eq!(
        err.to_string(),
        "evaluation error: max-context-size 8 bytes exceeded (body bytes alone: 1024)"
    );
}
