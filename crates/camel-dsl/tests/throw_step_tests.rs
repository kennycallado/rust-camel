//! `throw` step: load validation, declarative lowering, and runtime Business
//! error raising (business-error-model task 1.2).

use std::sync::Arc;

use camel_api::{Body, CamelError, Exchange, Message, Value, ValueSourceDef};
use camel_component_api::NoOpComponentContext;
use camel_component_direct::DirectComponent;
use camel_component_mock::MockComponent;
use camel_core::CamelContext;
use camel_core::route::RouteDefinition;
use camel_dsl::model::DeclarativeStep;
use camel_dsl::{parse_json, parse_json_to_declarative, parse_yaml, parse_yaml_to_declarative};
use serde_json::json;
use tower::ServiceExt;

fn yaml_route(step: &str) -> String {
    format!("routes:\n  - id: throw-test\n    from: direct:start\n    steps:\n{step}")
}

fn route_error(err: &CamelError, field: &str) {
    assert!(
        matches!(err, CamelError::RouteError(_)),
        "expected a typed RouteError, got {err:?}"
    );
    assert!(
        err.to_string().contains(field),
        "error must name `{field}`: {err}"
    );
}

fn business_fields(err: CamelError) -> (String, String, String, Option<serde_json::Value>) {
    match err {
        CamelError::Business {
            code,
            kind,
            message,
            data,
        } => (code, kind, message, data.map(|d| d.as_value().clone())),
        other => panic!("expected CamelError::Business, got {other:?}"),
    }
}

fn exchange_with(headers: &[(&str, &str)], body: &str) -> Exchange {
    let mut message = Message::new(Body::Text(body.to_string()));
    for (key, value) in headers {
        message.set_header(*key, Value::String((*value).to_string()));
    }
    Exchange::new(message)
}

async fn run_definitions(
    definitions: Vec<RouteDefinition>,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("test context builds");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(MockComponent::new());
    for definition in definitions {
        ctx.add_route_definition(definition)
            .await
            .expect("route definition adds");
    }
    ctx.start().await.expect("context starts");

    let producer_ctx = ctx.producer_context();
    let component = ctx
        .registry()
        .get("direct")
        .expect("direct component is registered");
    let endpoint = component
        .create_endpoint("direct:start", &ctx)
        .expect("direct endpoint resolves");
    let producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &producer_ctx)
        .expect("direct producer builds");
    producer.oneshot(exchange).await
}

async fn run_yaml(yaml: &str, exchange: Exchange) -> Result<Exchange, CamelError> {
    let definitions = parse_yaml(yaml).expect("route loads and compiles");
    run_definitions(definitions, exchange).await
}

#[test]
fn parse_throw_yaml() {
    let yaml = yaml_route(
        r#"      - throw:
          code: ORDER_NOT_FOUND
          kind: not-found
          message: "Order ${header.id} not found"
          data:
            orderId: "${header.id}"
"#,
    );

    let routes = parse_yaml_to_declarative(&yaml).expect("throw route must load");
    assert_eq!(routes.len(), 1);
    let DeclarativeStep::Throw(def) = &routes[0].steps[0] else {
        panic!("expected a Throw step, got {:?}", routes[0].steps[0]);
    };
    assert_eq!(def.code, "ORDER_NOT_FOUND");
    assert_eq!(def.kind.as_deref(), Some("not-found"));
    match def.message.as_ref() {
        Some(ValueSourceDef::Expression(expression)) => {
            assert_eq!(expression.language, "simple");
            assert_eq!(expression.source, "Order ${header.id} not found");
        }
        other => panic!("expected a simple expression message, got {other:?}"),
    }
    assert_eq!(def.data, Some(json!({"orderId": "${header.id}"})));
}

#[test]
fn blank_code_fails_load() {
    let yaml = yaml_route("      - throw: { code: \"  \" }\n");
    let err = parse_yaml_to_declarative(&yaml).expect_err("blank code must fail load");
    route_error(&err, "code");
}

#[test]
fn missing_code_fails_load() {
    let yaml = yaml_route("      - throw: { kind: not-found }\n");
    let err = parse_yaml_to_declarative(&yaml).expect_err("missing code must fail load");
    route_error(&err, "code");
}

#[test]
fn blank_kind_fails_load() {
    let yaml = yaml_route("      - throw: { code: STOCK_LOW, kind: \" \" }\n");
    let err = parse_yaml_to_declarative(&yaml).expect_err("blank kind must fail load");
    route_error(&err, "kind");
}

#[test]
fn non_object_data_fails_load() {
    let string_data = yaml_route("      - throw: { code: STOCK_LOW, data: \"nope\" }\n");
    let err = parse_yaml_to_declarative(&string_data).expect_err("string data must fail load");
    route_error(&err, "data");

    let array_data = yaml_route("      - throw: { code: STOCK_LOW, data: [1, 2] }\n");
    let err = parse_yaml_to_declarative(&array_data).expect_err("array data must fail load");
    route_error(&err, "data");
}

#[tokio::test]
async fn json_format_compiles_and_raises_throw() {
    let json_doc = r#"{
        "routes": [{
            "id": "throw-json",
            "from": "direct:start",
            "steps": [{ "throw": { "code": "ORDER_NOT_FOUND", "kind": "not-found" } }]
        }]
    }"#;

    let routes = parse_json_to_declarative(json_doc).expect("JSON throw route must load");
    let DeclarativeStep::Throw(def) = &routes[0].steps[0] else {
        panic!("expected a Throw step, got {:?}", routes[0].steps[0]);
    };
    assert_eq!(def.code, "ORDER_NOT_FOUND");
    assert_eq!(def.kind.as_deref(), Some("not-found"));

    let definitions = parse_json(json_doc).expect("JSON route compiles");
    let err = run_definitions(definitions, exchange_with(&[], "payload"))
        .await
        .expect_err("throw step must raise");
    let (code, kind, message, data) = business_fields(err);
    assert_eq!(code, "ORDER_NOT_FOUND");
    assert_eq!(kind, "not-found");
    assert_eq!(message, "ORDER_NOT_FOUND");
    assert_eq!(data, None);
}

#[tokio::test]
async fn throw_raises_business_with_templates() {
    let yaml = yaml_route(
        r#"      - throw:
          code: ORDER_NOT_FOUND
          kind: not-found
          message: "Order ${header.id} not found"
          data:
            orderId: "${header.id}"
"#,
    );

    let err = run_yaml(&yaml, exchange_with(&[("id", "A-1")], "payload"))
        .await
        .expect_err("throw step must raise");
    let (code, kind, message, data) = business_fields(err);
    assert_eq!(code, "ORDER_NOT_FOUND");
    assert_eq!(kind, "not-found");
    assert_eq!(message, "Order A-1 not found");
    assert_eq!(data, Some(json!({"orderId": "A-1"})));
}

#[tokio::test]
async fn throw_defaults_kind_and_message() {
    let yaml = yaml_route("      - throw: { code: STOCK_LOW }\n");

    let err = run_yaml(&yaml, exchange_with(&[], "payload"))
        .await
        .expect_err("throw step must raise");
    let (code, kind, message, data) = business_fields(err);
    assert_eq!(code, "STOCK_LOW");
    assert_eq!(kind, "failed");
    assert_eq!(message, "STOCK_LOW");
    assert_eq!(data, None);
}

#[tokio::test]
async fn throw_data_nested_quotes_and_nonstring() {
    let yaml = yaml_route(
        r#"      - throw:
          code: ORDER_NOT_FOUND
          data:
            order:
              id: "${header.id}"
              note: 'say "hi"'
            attempts: 3
            priority: true
"#,
    );

    let err = run_yaml(&yaml, exchange_with(&[("id", "A\"1")], "payload"))
        .await
        .expect_err("throw step must raise");
    let (_, _, _, data) = business_fields(err);
    assert_eq!(
        data,
        Some(json!({
            "order": {"id": "A\"1", "note": "say \"hi\""},
            "attempts": 3,
            "priority": true
        }))
    );
}

#[tokio::test]
async fn throw_int_coercion_failure_is_runtime_error() {
    let failing = yaml_route(
        r#"      - throw:
          code: STOCK_LOW
          data:
            bad: "${int(header.absent)}"
"#,
    );
    let err = run_yaml(&failing, exchange_with(&[], "payload"))
        .await
        .expect_err("failed data coercion must fail the step");
    assert!(
        !matches!(err, CamelError::Business { .. }),
        "coercion failure must not become a business error, got {err:?}"
    );
    assert!(
        matches!(err, CamelError::ExpressionFailed { .. }),
        "expected a typed evaluation error, got {err:?}"
    );

    let absent_header = yaml_route(
        r#"      - throw:
          code: STOCK_LOW
          data:
            ok: "${header.absent}"
"#,
    );
    let err = run_yaml(&absent_header, exchange_with(&[], "payload"))
        .await
        .expect_err("throw still raises after resolving the sibling leaf");
    let (_, _, _, data) = business_fields(err);
    assert_eq!(data, Some(json!({"ok": null})));
}

#[tokio::test]
async fn throw_writes_no_body_or_property() {
    let yaml = yaml_route(
        r#"      - throw:
          code: STOCK_LOW
          data:
            orderId: "${header.id}"
"#,
    );
    // A dead-letter channel with no explicit policies installs a catch-all
    // Handled policy, so the raised error is absorbed and the exchange comes
    // back for inspection instead of being dropped with the error.
    let yaml = yaml.replace(
        "    steps:\n",
        "    error_handler:\n      dead_letter_channel: mock:dead\n    steps:\n",
    );

    let mut message = Message::new(Body::Text("payload".to_string()));
    message.set_header("id", Value::String("A-1".to_string()));
    let mut exchange = Exchange::new(message);
    exchange.set_property("tenant", "acme");

    let completed = run_yaml(&yaml, exchange)
        .await
        .expect("handled throw must return the exchange");
    assert_eq!(completed.input.body, Body::Text("payload".to_string()));
    assert_eq!(
        completed.property("tenant"),
        Some(&Value::String("acme".into()))
    );
    assert_eq!(
        completed.properties,
        std::collections::HashMap::from([("tenant".to_string(), Value::String("acme".into()))]),
        "throw must not write data into exchange properties"
    );
}
