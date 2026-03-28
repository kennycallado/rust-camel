use camel_api::Value;
use camel_api::exchange::Exchange;
use camel_api::message::Message;
use camel_language_api::Language;
use camel_language_js::JsLanguage;
use camel_processor::script_mutator::ScriptMutator;
use tower::ServiceExt;

#[tokio::test]
async fn test_mutating_expression_through_script_mutator() {
    let lang = JsLanguage::new();
    let expr = lang
        .create_mutating_expression(
            "camel.headers.set('X-Processed', 'true'); camel.body = 'processed'; 'ok'",
        )
        .unwrap();

    let svc = ScriptMutator::new(expr);
    let mut exchange = Exchange::new(Message::new("original"));
    exchange
        .input
        .headers
        .insert("X-Original".to_string(), Value::String("keep".to_string()));

    let result = svc.oneshot(exchange).await;
    assert!(result.is_ok(), "ScriptMutator should succeed: {:?}", result);
    let exchange = result.unwrap();
    assert_eq!(
        exchange.input.headers.get("X-Processed"),
        Some(&Value::String("true".to_string())),
        "Header should be set by JS script"
    );
    assert_eq!(
        exchange.input.headers.get("X-Original"),
        Some(&Value::String("keep".to_string())),
        "Original header should be preserved"
    );
}

#[tokio::test]
async fn test_mutating_expression_error_propagated() {
    let lang = JsLanguage::new();
    let expr = lang
        .create_mutating_expression("camel.body = 'modified'; throw new Error('boom')")
        .unwrap();

    let svc = ScriptMutator::new(expr);
    let exchange = Exchange::new(Message::new("original"));

    let result = svc.oneshot(exchange).await;
    assert!(result.is_err(), "ScriptMutator should propagate error");
}

#[test]
fn test_js_language_aliases() {
    let lang = JsLanguage::new();
    assert_eq!(lang.name(), "js");
    let expr = lang.create_expression("1 + 1").unwrap();
    let val = expr.evaluate(&Exchange::new(Message::default())).unwrap();
    assert_eq!(val, serde_json::json!(2));
}
