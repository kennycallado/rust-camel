//! Audit regression corpus for Simple TEMPLATE mode (mission359 Task 1.2).
//!
//! `create_expression` now parses literal text + `${}` slots only; predicate
//! grammar stays on `create_predicate`. These tests exercise the public API
//! through `SimpleLanguage`.

use std::sync::Arc;

use camel_language_api::{Body, Exchange, Language, LanguageError, Message, Value};
use camel_language_simple::SimpleLanguage;
use serde_json::json;

fn exchange_with_header(key: &str, val: &str) -> Exchange {
    let mut msg = Message::default();
    msg.set_header(key, Value::String(val.to_string()));
    Exchange::new(msg)
}

fn empty_exchange() -> Exchange {
    Exchange::new(Message::default())
}

#[tokio::test]
async fn template_operator_text_is_literal() {
    let lang = SimpleLanguage::new();
    let expr = lang.create_expression("Got -> ${header.h}").unwrap();
    let val = expr
        .evaluate(&exchange_with_header("h", "B"))
        .await
        .unwrap();
    assert_eq!(val, Value::String("Got -> B".to_string()));
}

#[tokio::test]
async fn template_angle_brackets_do_not_parse() {
    let lang = SimpleLanguage::new();
    let expr = lang
        .create_expression("<order>${header.h}</order>")
        .unwrap();
    let val = expr
        .evaluate(&exchange_with_header("h", "B"))
        .await
        .unwrap();
    assert_eq!(val, Value::String("<order>B</order>".to_string()));
}

#[tokio::test]
async fn template_null_word_is_literal() {
    let lang = SimpleLanguage::new();
    let expr = lang
        .create_expression("value is null: ${header.h}")
        .unwrap();
    let val = expr
        .evaluate(&exchange_with_header("h", "B"))
        .await
        .unwrap();
    assert_eq!(val, Value::String("value is null: B".to_string()));
}

#[tokio::test]
async fn template_whitespace_and_number_text() {
    let lang = SimpleLanguage::new();
    let expr = lang.create_expression("${header.h} 3 items").unwrap();
    let val = expr
        .evaluate(&exchange_with_header("h", "B"))
        .await
        .unwrap();
    assert_eq!(val, Value::String("B 3 items".to_string()));
}

#[tokio::test]
async fn template_quotes_preserved() {
    let lang = SimpleLanguage::new();
    let expr = lang.create_expression("literal 'quoted' text").unwrap();
    let val = expr.evaluate(&empty_exchange()).await.unwrap();
    assert_eq!(val, Value::String("literal 'quoted' text".to_string()));
}

#[tokio::test]
async fn template_whole_slot_native_number() {
    let lang = SimpleLanguage::new();
    let expr = lang.create_expression("${header.n}").unwrap();
    let mut ex = Exchange::default();
    ex.input.set_header("n", json!(7));
    let val = expr.evaluate(&ex).await.unwrap();
    // A whole `${...}` slot returns the native value; here the header is a
    // JSON number, not a string.
    assert_eq!(val, json!(7));
}

#[tokio::test]
async fn template_whole_slot_native_array() {
    let lang = SimpleLanguage::new();
    let expr = lang.create_expression("${body.items}").unwrap();
    let mut ex = Exchange::default();
    ex.input.body = Body::Json(json!({"items": [1, 2]}));
    let val = expr.evaluate(&ex).await.unwrap();
    assert_eq!(val, json!([1, 2]));
}

#[tokio::test]
async fn template_whole_body_slot_serializes_json() {
    let lang = SimpleLanguage::new();
    let expr = lang.create_expression("${body}").unwrap();
    let mut ex = Exchange::default();
    ex.input.body = Body::Json(json!({"a": 1}));
    let val = expr.evaluate(&ex).await.unwrap();
    assert_eq!(val, Value::String("{\"a\":1}".to_string()));
}

#[tokio::test]
async fn template_unknown_language_fails_at_evaluation() {
    let lang = SimpleLanguage::new();
    // Creation succeeds: an unregistered language is an evaluation-time failure.
    let expr = lang.create_expression("${nosuchlang:x}").unwrap();
    let err = expr.evaluate(&empty_exchange()).await.unwrap_err();
    assert!(matches!(err, LanguageError::EvalError(_)), "got: {err:?}");
}

#[tokio::test]
async fn escaped_env_slot_error_text() {
    let lang =
        SimpleLanguage::with_resolver(Arc::new(|_: &str| -> Option<Arc<dyn Language>> { None }));
    let expr = lang.create_expression("$${env:NAME}").unwrap();
    let err = expr.evaluate(&empty_exchange()).await.unwrap_err();
    assert!(matches!(err, LanguageError::EvalError(_)), "got: {err:?}");
    assert!(
        err.to_string().contains("Language 'env' not found"),
        "unexpected error text: {err}"
    );
}

#[tokio::test]
async fn template_whole_slot_missing_is_null() {
    let lang = SimpleLanguage::new();
    let expr = lang.create_expression("${header.missing}").unwrap();
    let val = expr.evaluate(&empty_exchange()).await.unwrap();
    assert_eq!(val, Value::Null);
}

#[tokio::test]
async fn template_mixed_missing_is_empty() {
    let lang = SimpleLanguage::new();
    let expr = lang
        .create_expression("prefix-${header.missing}-suffix")
        .unwrap();
    let val = expr.evaluate(&empty_exchange()).await.unwrap();
    assert_eq!(val, Value::String("prefix--suffix".to_string()));
}

#[tokio::test]
async fn predicate_mode_unchanged() {
    let lang = SimpleLanguage::new();
    let pred = lang.create_predicate("${header.type} == 'order'").unwrap();
    assert!(
        pred.matches(&exchange_with_header("type", "order"))
            .await
            .unwrap()
    );

    // A bare non-boolean predicate is a strict type mismatch, not truthiness.
    let bare = lang.create_predicate("${header.h}").unwrap();
    assert!(matches!(
        bare.matches(&exchange_with_header("h", "B")).await,
        Err(LanguageError::TypeMismatch { .. })
    ));
}

#[tokio::test]
async fn expression_does_not_coerce_operators() {
    let lang = SimpleLanguage::new();
    let expr = lang.create_expression("a == b").unwrap();
    let val = expr.evaluate(&empty_exchange()).await.unwrap();
    assert_eq!(val, Value::String("a == b".to_string()));
}
