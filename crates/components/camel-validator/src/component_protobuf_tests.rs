//! Component-level coverage for the protobuf schema type.
//!
//! These tests exercise endpoint creation (descriptor compile at route build)
//! and per-exchange validation through the producer surface.

use std::io::Write;

use camel_component_api::test_support::PanicRuntimeObservability;
use camel_component_api::{
    CamelError, Component, Endpoint, Exchange, Message, NoOpComponentContext, ProducerContext,
};
use tower::ServiceExt;

use crate::component::ValidatorComponent;

fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(PanicRuntimeObservability)
}

// ── protobuf schema type ────────────────────────────────────────────

const PROTO_PERSON: &[u8] = br#"
syntax = "proto3";
package test.v1;

enum Role {
  ROLE_UNSPECIFIED = 0;
  ROLE_ADMIN = 1;
  ROLE_USER = 2;
}

message Person {
  string name = 1;
  int32 age = 2;
  Role role = 3;
  oneof contact {
    string email = 4;
    string phone = 5;
  }
}
"#;

fn proto_file() -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new()
        .suffix(".proto")
        .tempfile()
        .unwrap();
    f.write_all(PROTO_PERSON).unwrap();
    f
}

fn protobuf_uri(path: &std::path::Path) -> String {
    format!("validator:{}?message=test.v1.Person", path.display())
}

fn protobuf_endpoint(path: &std::path::Path, extra_query: &str) -> Box<dyn Endpoint> {
    let uri = format!("{}{}", protobuf_uri(path), extra_query);
    ValidatorComponent::new()
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("protobuf endpoint creation should succeed")
}

#[tokio::test]
async fn protobuf_valid_json_body_accepted_and_body_unchanged() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let body = camel_component_api::Body::Json(serde_json::json!({
        "name": "Ann",
        "age": 30,
        "role": "ROLE_USER",
        "email": "a@b.c"
    }));
    let exchange = Exchange::new(Message::new(body.clone()));
    let returned = producer
        .oneshot(exchange)
        .await
        .expect("valid protobuf JSON must pass");
    assert!(
        matches!(returned.input.body, camel_component_api::Body::Json(_)),
        "body variant must be unchanged, got: {:?}",
        returned.input.body
    );
    assert_eq!(
        returned.input.body, body,
        "body value must be unchanged after validation"
    );
}

#[tokio::test]
async fn protobuf_valid_text_and_bytes_bodies_accepted() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");

    let text_exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
        r#"{"name":"B","age":1}"#.to_string(),
    )));
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    assert!(
        producer.oneshot(text_exchange).await.is_ok(),
        "valid protobuf JSON in Body::Text must pass"
    );

    let bytes_exchange = Exchange::new(Message::new(camel_component_api::Body::Bytes(
        br#"{"name":"C"}"#.to_vec().into(),
    )));
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    assert!(
        producer.oneshot(bytes_exchange).await.is_ok(),
        "valid protobuf JSON in Body::Bytes must pass"
    );
}

#[tokio::test]
async fn protobuf_body_byte_identical_no_normalization() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let input = r#"{"name": "Ann" , "age": 30}"#;
    let exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
        input.to_string(),
    )));
    let returned = producer
        .oneshot(exchange)
        .await
        .expect("valid protobuf JSON must pass");
    match returned.input.body {
        camel_component_api::Body::Text(s) => {
            assert_eq!(s, input, "validator must not normalize or rewrite the body")
        }
        other => panic!("expected Body::Text unchanged, got: {other:?}"),
    }
}

#[tokio::test]
async fn protobuf_wrong_scalar_type_rejected() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
        r#"{"name":123}"#.to_string(),
    )));
    let err = producer
        .oneshot(exchange)
        .await
        .expect_err("wrong scalar type must be rejected");
    assert!(
        matches!(&err, CamelError::ValidationError(_)),
        "expected CamelError::ValidationError, got: {err}"
    );
    assert!(
        err.to_string().contains("protobuf validation failed"),
        "got: {err}"
    );
}

#[tokio::test]
async fn protobuf_unknown_field_rejected() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
        r#"{"name":"x","nickname":"y"}"#.to_string(),
    )));
    let err = producer
        .oneshot(exchange)
        .await
        .expect_err("unknown field must be rejected");
    assert!(
        matches!(&err, CamelError::ValidationError(_)),
        "expected CamelError::ValidationError, got: {err}"
    );
    assert!(
        err.to_string().contains("protobuf validation failed"),
        "got: {err}"
    );
}

#[tokio::test]
async fn protobuf_invalid_enum_rejected() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
        r#"{"role":"ROLE_HACKER"}"#.to_string(),
    )));
    let err = producer
        .oneshot(exchange)
        .await
        .expect_err("unknown enum name must be rejected");
    assert!(
        matches!(&err, CamelError::ValidationError(_)),
        "expected CamelError::ValidationError, got: {err}"
    );
    assert!(
        err.to_string().contains("protobuf validation failed"),
        "got: {err}"
    );
}

#[tokio::test]
async fn protobuf_wrong_oneof_shape_rejected() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
        r#"{"email":5}"#.to_string(),
    )));
    let err = producer
        .oneshot(exchange)
        .await
        .expect_err("a number for a oneof string field must be rejected");
    assert!(
        matches!(&err, CamelError::ValidationError(_)),
        "expected CamelError::ValidationError, got: {err}"
    );
    assert!(
        err.to_string().contains("protobuf validation failed"),
        "got: {err}"
    );
}

#[tokio::test]
async fn protobuf_malformed_json_rejected() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
        "{oops".to_string(),
    )));
    let err = producer
        .oneshot(exchange)
        .await
        .expect_err("malformed JSON must be rejected");
    assert!(
        matches!(&err, CamelError::ValidationError(_)),
        "expected CamelError::ValidationError, got: {err}"
    );
    assert!(
        err.to_string().contains("protobuf validation failed"),
        "got: {err}"
    );
}

#[tokio::test]
async fn protobuf_trailing_data_rejected() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
        r#"{"name":"Trailing"} trailing-garbage"#.to_string(),
    )));
    let err = producer
        .oneshot(exchange)
        .await
        .expect_err("trailing data after the JSON object must be rejected");
    assert!(
        matches!(&err, CamelError::ValidationError(_)),
        "expected CamelError::ValidationError, got: {err}"
    );
    assert!(
        err.to_string().contains("protobuf validation failed"),
        "got: {err}"
    );
}

#[tokio::test]
async fn protobuf_non_utf8_bytes_rejected() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let exchange = Exchange::new(Message::new(camel_component_api::Body::Bytes(
        vec![0xff_u8, 0xfe, 0xfd].into(),
    )));
    let err = producer
        .oneshot(exchange)
        .await
        .expect_err("non-UTF-8 bytes must be rejected");
    assert!(
        matches!(&err, CamelError::ValidationError(_)),
        "expected CamelError::ValidationError, got: {err}"
    );
    assert!(err.to_string().contains("not valid UTF-8"), "got: {err}");
}

#[tokio::test]
async fn protobuf_null_and_missing_fields_follow_proto3_defaults() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");

    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let empty_exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
        "{}".to_string(),
    )));
    assert!(
        producer.oneshot(empty_exchange).await.is_ok(),
        "an empty object must satisfy proto3 defaults"
    );

    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let null_exchange = Exchange::new(Message::new(camel_component_api::Body::Json(
        serde_json::json!({"name": null}),
    )));
    assert!(
        producer.oneshot(null_exchange).await.is_ok(),
        "a JSON null field must follow proto3 default semantics"
    );
}

#[tokio::test]
async fn protobuf_unsupported_body_variant_rejected() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let exchange = Exchange::new(Message::new(camel_component_api::Body::Xml(
        "<a/>".to_string(),
    )));
    let err = producer
        .oneshot(exchange)
        .await
        .expect_err("XML body must be rejected by the protobuf validator");
    assert!(
        matches!(&err, CamelError::ValidationError(_)),
        "expected CamelError::ValidationError, got: {err}"
    );
    assert!(
        err.to_string()
            .contains("requires Body::Json, Body::Text, or Body::Bytes"),
        "got: {err}"
    );
}

#[tokio::test]
async fn protobuf_header_name_validates_header_value() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "&headerName=X-Person");

    let mut msg = Message::new(camel_component_api::Body::Empty);
    msg.set_header("X-Person", r#"{"name":"Ann"}"#);
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    assert!(
        producer.oneshot(Exchange::new(msg)).await.is_ok(),
        "a valid protobuf JSON header value must pass"
    );

    let mut bad = Message::new(camel_component_api::Body::Empty);
    bad.set_header("X-Person", "{oops");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    assert!(
        producer.oneshot(Exchange::new(bad)).await.is_err(),
        "a malformed protobuf JSON header value must be rejected"
    );
}

#[test]
fn protobuf_missing_message_fails_endpoint_creation() {
    let f = proto_file();
    let uri = format!("validator:{}", f.path().display());
    let err = ValidatorComponent::new()
        .create_endpoint(&uri, &NoOpComponentContext)
        .err()
        .expect("a protobuf endpoint without a message must fail");
    assert!(
        matches!(err, CamelError::Config(_)),
        "expected CamelError::Config, got: {err}"
    );
    assert!(
        err.to_string().contains("message"),
        "error must mention the required message parameter, got: {err}"
    );
}

#[test]
fn protobuf_empty_message_fails_endpoint_creation() {
    let f = proto_file();
    let uri = format!("validator:{}?message=", f.path().display());
    let err = ValidatorComponent::new()
        .create_endpoint(&uri, &NoOpComponentContext)
        .err()
        .expect("an empty message must fail endpoint creation");
    assert!(
        matches!(err, CamelError::Config(_)),
        "expected CamelError::Config, got: {err}"
    );
    assert!(
        err.to_string().contains("message"),
        "error must mention the required message parameter, got: {err}"
    );
}

#[test]
fn protobuf_missing_proto_file_fails_endpoint_creation() {
    let uri = "validator:/nonexistent/person.proto?message=test.v1.Person";
    let err = ValidatorComponent::new()
        .create_endpoint(uri, &NoOpComponentContext)
        .err()
        .expect("a missing proto file must fail endpoint creation");
    assert!(
        matches!(err, CamelError::EndpointCreationFailed(_)),
        "expected EndpointCreationFailed, got: {err}"
    );
    assert!(
        err.to_string().contains("failed to compile proto schema"),
        "got: {err}"
    );
}

#[test]
fn protobuf_compile_error_fails_endpoint_creation() {
    let mut f = tempfile::Builder::new()
        .suffix(".proto")
        .tempfile()
        .unwrap();
    f.write_all(b"syntax = \"bogus\";").unwrap();
    let uri = protobuf_uri(f.path());
    let err = ValidatorComponent::new()
        .create_endpoint(&uri, &NoOpComponentContext)
        .err()
        .expect("an uncompilable proto file must fail endpoint creation");
    assert!(
        matches!(err, CamelError::EndpointCreationFailed(_)),
        "expected EndpointCreationFailed, got: {err}"
    );
    assert!(
        err.to_string().contains("failed to compile proto schema"),
        "got: {err}"
    );
}

#[test]
fn protobuf_unknown_message_name_fails_endpoint_creation() {
    let f = proto_file();
    let uri = format!("validator:{}?message=test.v1.Nope", f.path().display());
    let err = ValidatorComponent::new()
        .create_endpoint(&uri, &NoOpComponentContext)
        .err()
        .expect("an unknown message name must fail endpoint creation");
    assert!(
        matches!(err, CamelError::Config(_)),
        "expected CamelError::Config, got: {err}"
    );
    assert!(
        err.to_string().contains("message descriptor not found"),
        "got: {err}"
    );
}

#[tokio::test]
async fn protobuf_max_payload_enforced() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "&maxPayloadBytes=10");
    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    let big = format!(r#"{{"name":"{}"}}"#, "x".repeat(100));
    let exchange = Exchange::new(Message::new(camel_component_api::Body::Text(big)));
    let err = producer
        .oneshot(exchange)
        .await
        .expect_err("an oversized payload must be rejected");
    assert!(err.to_string().contains("payload too large"), "got: {err}");
    assert!(
        matches!(&err, CamelError::ProcessorError(_)),
        "payload-too-large must stay on the processor-error path, got: {err}"
    );
}

/// The proto file is corrupted after endpoint creation to prove the compiled
/// descriptor is stored on the endpoint and not re-read or recompiled per exchange.
#[tokio::test]
async fn protobuf_descriptor_compiled_once_not_recompiled() {
    let f = proto_file();
    let ep = protobuf_endpoint(f.path(), "");
    std::fs::write(f.path(), b"syntax = \"bogus\";")
        .expect("corrupting the proto file after endpoint creation should succeed");
    let body = camel_component_api::Body::Text(r#"{"name":"Ann"}"#.to_string());

    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    assert!(
        producer
            .oneshot(Exchange::new(Message::new(body.clone())))
            .await
            .is_ok(),
        "first validation must succeed"
    );

    let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
    assert!(
        producer
            .oneshot(Exchange::new(Message::new(body)))
            .await
            .is_ok(),
        "second validation on the same endpoint must succeed without recompiling"
    );
}
