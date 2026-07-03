use camel_api::body::Body;
use camel_api::{CamelError, Exchange, IdentityProcessor, Message};
use camel_processor::JsonSchemaValidateService;
use serde_json::json;
use tower::ServiceExt;

fn schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" }
        },
        "required": ["name"]
    })
}

#[tokio::test]
async fn passes_valid_body() {
    let svc = JsonSchemaValidateService::new(IdentityProcessor, &schema()).unwrap();
    let ex = Exchange::new(Message::new(Body::Json(json!({
        "name": "kenny",
        "age": 42
    }))));
    let out = svc.oneshot(ex).await.unwrap();
    assert!(matches!(out.input.body, Body::Json(_)));
}

#[tokio::test]
async fn rejects_missing_required_field() {
    let svc = JsonSchemaValidateService::new(IdentityProcessor, &schema()).unwrap();
    let ex = Exchange::new(Message::new(Body::Json(json!({
        "age": 42
    }))));
    let err = svc.oneshot(ex).await.unwrap_err();
    match err {
        CamelError::ValidationError(msg) => {
            assert!(msg.contains("validation failed"), "msg: {msg}");
        }
        other => panic!("expected ValidationError, got: {other:?}"),
    }
}

#[tokio::test]
async fn rejects_wrong_type() {
    let svc = JsonSchemaValidateService::new(IdentityProcessor, &schema()).unwrap();
    let ex = Exchange::new(Message::new(Body::Json(json!({
        "name": "kenny",
        "age": "not a number"
    }))));
    let err = svc.oneshot(ex).await.unwrap_err();
    assert!(matches!(err, CamelError::ValidationError(_)));
}

#[tokio::test]
async fn non_json_body_skips_validation() {
    // A non-JSON body is the caller's problem (unmarshal handles it); the
    // validator must not fail when the body type is not JSON.
    let svc = JsonSchemaValidateService::new(IdentityProcessor, &schema()).unwrap();
    let ex = Exchange::new(Message::new(Body::Text("plain text".to_string())));
    let out = svc.oneshot(ex).await.unwrap();
    assert!(matches!(out.input.body, Body::Text(_)));
}

#[tokio::test]
async fn invalid_schema_returns_error_at_build() {
    // Not a valid JSON Schema — `validator_for` rejects it.
    let bad = json!({ "type": "not_a_real_type" });
    let err = JsonSchemaValidateService::new(IdentityProcessor, &bad).unwrap_err();
    assert!(matches!(err, CamelError::RouteError(_)));
}

#[tokio::test]
async fn empty_body_skips_validation() {
    // Empty body — no body to validate against; the validator passes through.
    let svc = JsonSchemaValidateService::new(IdentityProcessor, &schema()).unwrap();
    let ex = Exchange::new(Message::new(Body::Empty));
    let out = svc.oneshot(ex).await.unwrap();
    assert!(matches!(out.input.body, Body::Empty));
}

#[tokio::test]
async fn can_be_chained_behind_inner_processor() {
    // Validator wraps an inner processor (IdentityProcessor). After validation
    // passes, the inner service runs and the body passes through unchanged.
    let svc = JsonSchemaValidateService::new(IdentityProcessor, &schema()).unwrap();
    let inner = IdentityProcessor;
    let ex = Exchange::new(Message::new(Body::Json(json!({ "name": "kenny" }))));
    let validated = svc.oneshot(ex).await.unwrap();
    let final_ex = inner.oneshot(validated).await.unwrap();
    assert!(matches!(final_ex.input.body, Body::Json(_)));
}
