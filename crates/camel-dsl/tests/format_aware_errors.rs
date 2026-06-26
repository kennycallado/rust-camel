//! Spec DoD for rc-iq7 Slice D1: format-aware errors.
//!
//! The same semantic malformation (empty route id) produces errors that
//! distinguish the user's input format. YAML and JSON entry points share the
//! same lower path (`route_dsl_to_declarative_route`) but the boundary
//! annotation tags the originating format.

use camel_dsl::{json::parse_json_to_declarative, yaml::parse_yaml_to_declarative};

const YAML_MALFORMED: &str = "routes:\n  - id: \"\"\n    from: timer:tick\n";
const JSON_MALFORMED: &str = r#"{"routes": [{"id": "", "from": "timer:tick"}]}"#;

#[test]
fn yaml_error_names_yaml_format() {
    let err = parse_yaml_to_declarative(YAML_MALFORMED)
        .err()
        .expect("YAML malformed route should error");
    let msg = err.to_string();
    assert!(
        msg.contains("YAML DSL error:"),
        "YAML error must name YAML format, got: {msg}"
    );
    assert!(
        !msg.contains("JSON DSL error:"),
        "YAML error must not mention JSON, got: {msg}"
    );
}

#[test]
fn json_error_names_json_format() {
    let err = parse_json_to_declarative(JSON_MALFORMED)
        .err()
        .expect("JSON malformed route should error");
    let msg = err.to_string();
    assert!(
        msg.contains("JSON DSL error:"),
        "JSON error must name JSON format, got: {msg}"
    );
    assert!(
        !msg.contains("YAML DSL error:"),
        "JSON error must not mention YAML, got: {msg}"
    );
}

#[test]
fn both_formats_share_underlying_semantic_message() {
    let yaml_err = parse_yaml_to_declarative(YAML_MALFORMED)
        .err()
        .expect("YAML malformed route should error");
    let json_err = parse_json_to_declarative(JSON_MALFORMED)
        .err()
        .expect("JSON malformed route should error");
    // Both must surface the same underlying semantic content.
    assert!(
        yaml_err
            .to_string()
            .contains("route 'id' must not be empty"),
        "YAML error lost semantic content: {}",
        yaml_err
    );
    assert!(
        json_err
            .to_string()
            .contains("route 'id' must not be empty"),
        "JSON error lost semantic content: {}",
        json_err
    );
}

#[test]
fn json_parse_error_distinguishes_from_yaml_parse_error() {
    // Use structurally broken YAML and JSON that the respective deserializers reject.
    let yaml_bad = "routes:\n  - id: [invalid\n    from: timer:tick\n";
    let yaml_result = parse_yaml_to_declarative(yaml_bad);
    assert!(
        yaml_result.is_err(),
        "YAML parse should error for: {yaml_bad:?}"
    );

    let json_bad = "{\"routes\":[{\"id\":[invalid]}]}";
    let json_result = parse_json_to_declarative(json_bad);
    assert!(
        json_result.is_err(),
        "JSON parse should error for: {json_bad:?}"
    );

    let yaml_msg = yaml_result.unwrap_err().to_string();
    let json_msg = json_result.unwrap_err().to_string();

    assert!(yaml_msg.contains("YAML DSL error: YAML parse error:"));
    assert!(json_msg.contains("JSON DSL error: JSON parse error:"));
}
