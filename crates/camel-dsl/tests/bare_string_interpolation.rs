//! Regression corpus for the top-level `${}` value-source gate (Task 1.3 of
//! mission359-languagep1wave).
//!
//! A bare **top-level** string value source containing `${` compiles to a
//! Simple template expression whichever verb carries it (`log:`, `set_body:`,
//! `set_header.value`, `set_property.value`, `transform:`). A top-level string
//! without a slot stays a literal, and a string nested inside an object/array
//! literal is never gated. One shared helper serves the YAML and JSON route
//! compilers.

use camel_dsl::env_interpolation::interpolate_env_with;
use camel_dsl::{
    DeclarativeRoute, DeclarativeStep, LanguageExpressionDef, ValueSourceDef,
    parse_json_to_declarative, parse_routes_with_env, parse_yaml_to_declarative,
};
use serde_json::json;

fn yaml_doc(steps: &str) -> String {
    format!("routes:\n  - id: \"r\"\n    from: \"direct:start\"\n    steps:\n{steps}")
}

fn json_doc(steps: &str) -> String {
    format!("{{\"routes\":[{{\"id\":\"r\",\"from\":\"direct:start\",\"steps\":[{steps}]}}]}}")
}

fn parse_yaml(doc: &str) -> DeclarativeRoute {
    let mut routes = parse_yaml_to_declarative(doc).expect("YAML must parse");
    assert_eq!(routes.len(), 1, "exactly one route");
    routes.remove(0)
}

fn parse_json(doc: &str) -> DeclarativeRoute {
    let mut routes = parse_json_to_declarative(doc).expect("JSON must parse");
    assert_eq!(routes.len(), 1, "exactly one route");
    routes.remove(0)
}

fn simple(source: &str) -> ValueSourceDef {
    ValueSourceDef::Expression(LanguageExpressionDef {
        language: "simple".to_string(),
        source: source.to_string(),
    })
}

fn literal(value: serde_json::Value) -> ValueSourceDef {
    ValueSourceDef::Literal(value)
}

fn body_value(route: &DeclarativeRoute) -> &ValueSourceDef {
    match &route.steps[0] {
        DeclarativeStep::SetBody(def) => &def.value,
        other => panic!("expected SetBody step, got {other:?}"),
    }
}

fn header_value(route: &DeclarativeRoute) -> &ValueSourceDef {
    match &route.steps[0] {
        DeclarativeStep::SetHeader(def) => &def.value,
        other => panic!("expected SetHeader step, got {other:?}"),
    }
}

fn property_value(route: &DeclarativeRoute) -> &ValueSourceDef {
    match &route.steps[0] {
        DeclarativeStep::SetProperty(def) => &def.value,
        other => panic!("expected SetProperty step, got {other:?}"),
    }
}

fn log_message(route: &DeclarativeRoute) -> &ValueSourceDef {
    match &route.steps[0] {
        DeclarativeStep::Log(def) => &def.message,
        other => panic!("expected Log step, got {other:?}"),
    }
}

#[test]
fn bare_set_body_with_slot_is_simple_expression() {
    let route = parse_yaml(&yaml_doc("      - set_body: \"Got ${header.h}\"\n"));
    assert_eq!(*body_value(&route), simple("Got ${header.h}"));
}

#[test]
fn object_value_form_interpolates() {
    let body = parse_yaml(&yaml_doc(
        "      - set_body: {value: \"Got ${header.h}\"}\n",
    ));
    assert_eq!(*body_value(&body), simple("Got ${header.h}"));

    let header = parse_yaml(&yaml_doc(
        "      - set_header: {key: x, value: \"Got ${header.h}\"}\n",
    ));
    assert_eq!(*header_value(&header), simple("Got ${header.h}"));

    let log = parse_yaml(&yaml_doc(
        "      - log: {message: {value: \"Got ${header.h}\"}}\n",
    ));
    assert_eq!(*log_message(&log), simple("Got ${header.h}"));
}

#[test]
fn bare_set_header_with_slot_is_simple_expression() {
    let route = parse_yaml(&yaml_doc(
        "      - set_header: {key: x, value: \"Got ${header.h}\"}\n",
    ));
    assert_eq!(*header_value(&route), simple("Got ${header.h}"));
}

#[test]
fn bare_set_property_with_slot_is_simple_expression() {
    let route = parse_yaml(&yaml_doc(
        "      - set_property: {name: p, value: \"Hi ${header.h}\"}\n",
    ));
    assert_eq!(*property_value(&route), simple("Hi ${header.h}"));
}

#[test]
fn bare_transform_with_slot_is_simple_expression() {
    let route = parse_yaml(&yaml_doc("      - transform: \"Got ${header.h}\"\n"));
    assert_eq!(*body_value(&route), simple("Got ${header.h}"));
}

#[test]
fn plain_literal_stays_literal() {
    let route = parse_yaml(&yaml_doc("      - set_body: \"order\"\n"));
    assert_eq!(*body_value(&route), literal(json!("order")));
}

#[test]
fn nested_literal_is_not_gated() {
    let route = parse_yaml(&yaml_doc(
        "      - set_body: {value: {nested: \"${header.h}\"}}\n",
    ));
    assert_eq!(
        *body_value(&route),
        literal(json!({"nested": "${header.h}"}))
    );
}

#[test]
fn json_and_yaml_agree() {
    let yaml_route = parse_yaml(&yaml_doc("      - set_body: \"Got ${header.h}\"\n"));
    let json_route = parse_json(&json_doc("{\"set_body\":\"Got ${header.h}\"}"));
    assert_eq!(*body_value(&yaml_route), *body_value(&json_route));
    assert_eq!(*body_value(&yaml_route), simple("Got ${header.h}"));
}

#[test]
fn bare_log_slot_is_simple_expression() {
    let route = parse_yaml(&yaml_doc("      - log: \"Got ${header.h}\"\n"));
    assert_eq!(*log_message(&route), simple("Got ${header.h}"));
}

#[test]
fn substituted_env_slot_value() {
    let raw = yaml_doc("      - set_body: \"Hello ${env:NAME}\"\n");
    let interpolated = interpolate_env_with(&raw, &|name| {
        (name == "NAME").then(|| "${header.h}".to_string())
    })
    .expect("env resolves");
    let route = parse_yaml(&interpolated);
    assert_eq!(*body_value(&route), simple("Hello ${header.h}"));
}

#[test]
fn substituted_env_plain_stays_literal() {
    let raw = yaml_doc("      - set_body: \"Hello ${env:NAME}\"\n");
    let interpolated =
        interpolate_env_with(&raw, &|name| (name == "NAME").then(|| "B".to_string()))
            .expect("env resolves");
    let route = parse_yaml(&interpolated);
    assert_eq!(*body_value(&route), literal(json!("Hello B")));
}

#[test]
fn unescaped_route_arg_fails_load() {
    let raw = yaml_doc("      - set_body: \"Hello ${arg:NAME}\"\n");
    let result = parse_routes_with_env(&raw, &|_| None);
    assert!(result.is_err(), "unescaped ${{arg:NAME}} must fail to load");
}

#[test]
fn template_looking_literal_becomes_expression() {
    let route = parse_yaml(&yaml_doc("      - set_body: \"echo ${HOME}\"\n"));
    assert_eq!(*body_value(&route), simple("echo ${HOME}"));
}

#[test]
fn escape_yields_slot_documented() {
    let raw = yaml_doc("      - set_body: \"$${env:NAME}\"\n");
    let interpolated =
        interpolate_env_with(&raw, &|name| (name == "NAME").then(|| "B".to_string()))
            .expect("escape stays literal");
    assert!(
        interpolated.contains("${env:NAME}"),
        "escape must yield literal text, got: {interpolated}"
    );
    let route = parse_yaml(&interpolated);
    assert_eq!(*body_value(&route), simple("${env:NAME}"));
}

#[test]
fn log_object_plain_message_is_always_simple() {
    let route = parse_yaml(&yaml_doc("      - log: {message: \"plain\"}\n"));
    assert_eq!(*log_message(&route), simple("plain"));
}

#[test]
fn log_object_value_form_is_gated() {
    let route = parse_yaml(&yaml_doc(
        "      - log: {message: {value: \"Got ${header.h}\"}}\n",
    ));
    assert_eq!(*log_message(&route), simple("Got ${header.h}"));
}
