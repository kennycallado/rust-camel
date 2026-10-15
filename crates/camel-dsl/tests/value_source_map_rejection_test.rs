//! Negative coverage for object-literal value-source maps (Task 1.1 of
//! change 364-simpleset).
//!
//! A mapping directly under `set_body`/`transform` is value-source
//! configuration, never an implicit literal object. A mapping that fails
//! `SetBodyConfig` deserialization reaches the literal branch and is rejected
//! during shared lowering with a `RouteError`. Explicit `value: {...}` objects
//! stay literal. Scalars, arrays, null and bare `${...}` strings keep their
//! existing behavior.

use camel_dsl::{
    DeclarativeStep, LanguageExpressionDef, ValueSourceDef, parse_json_to_declarative,
    parse_yaml_to_declarative,
};
use serde_json::json;

const ALTERNATIVES: &str = "`value`, `language`+`source`, `simple`, `rhai`, `jsonpath`, `xpath`";

fn yaml_doc(steps: &str) -> String {
    format!("routes:\n  - id: \"r\"\n    from: \"direct:start\"\n    steps:\n{steps}")
}

fn json_doc(steps: &str) -> String {
    format!("{{\"routes\":[{{\"id\":\"r\",\"from\":\"direct:start\",\"steps\":[{steps}]}}]}}")
}

/// Parse `doc` and return the rendered `CamelError::Display` of the expected
/// rejection.
fn assert_rejected(doc: &str, is_json: bool) -> String {
    let err = if is_json {
        parse_json_to_declarative(doc).expect_err("JSON doc must be rejected")
    } else {
        parse_yaml_to_declarative(doc).expect_err("YAML doc must be rejected")
    };
    err.to_string()
}

/// The explicit four-cell matrix: YAML/JSON x set_body/transform. Each cell is
/// `(fmt, verb, doc, is_json)`. `yaml_body` and `json_body` are the same value
/// rendered in each format.
fn cells(yaml_body: &str, json_body: &str) -> Vec<(&'static str, &'static str, String, bool)> {
    let mut cells = Vec::new();
    for verb in ["set_body", "transform"] {
        cells.push((
            "YAML",
            verb,
            yaml_doc(&format!("      - {verb}: {yaml_body}\n")),
            false,
        ));
        cells.push((
            "JSON",
            verb,
            json_doc(&format!("{{\"{verb}\":{json_body}}}")),
            true,
        ));
    }
    cells
}

/// Parse `doc` (which must succeed) and return the lowered `SetBody` value.
fn parse_body_value(doc: &str, is_json: bool) -> ValueSourceDef {
    let mut routes = if is_json {
        parse_json_to_declarative(doc).expect("JSON doc must parse")
    } else {
        parse_yaml_to_declarative(doc).expect("YAML doc must parse")
    };
    assert_eq!(routes.len(), 1, "exactly one route");
    let route = routes.remove(0);
    match &route.steps[0] {
        DeclarativeStep::SetBody(step) => step.value.clone(),
        other => panic!("expected SetBody step, got {other:?}"),
    }
}

#[test]
fn unknown_key_rejected_both_formats_both_verbs() {
    for (fmt, verb, doc, is_json) in
        cells(r#"{constant: "LEAK-ONE"}"#, r#"{"constant":"LEAK-ONE"}"#)
    {
        let rendered = assert_rejected(&doc, is_json);
        assert!(
            rendered.contains(&format!(
                "{fmt} DSL error: {verb}: unknown key(s) `constant` in value-source mapping"
            )),
            "missing unknown-key prefix in {fmt} {verb}: {rendered}"
        );
        assert!(
            rendered.contains(ALTERNATIVES),
            "missing alternatives in {fmt} {verb}: {rendered}"
        );
        assert!(
            rendered.contains("value: {"),
            "missing wrapper hint in {fmt} {verb}: {rendered}"
        );
        assert!(
            !rendered.contains("LEAK-ONE"),
            "leaked value in {fmt} {verb}: {rendered}"
        );
    }
}

#[test]
fn mixed_unknown_key_wins_both_formats_both_verbs() {
    for (fmt, verb, doc, is_json) in cells(
        r#"{simple: "${body}", foo: "LEAK-TWO"}"#,
        r#"{"simple":"${body}","foo":"LEAK-TWO"}"#,
    ) {
        let rendered = assert_rejected(&doc, is_json);
        assert!(
            rendered.contains("unknown key(s) `foo` "),
            "unknown key wins in {fmt} {verb}: {rendered}"
        );
        assert!(
            !rendered.contains("unknown key(s) `simple`"),
            "`simple` must not be reported unknown in {fmt} {verb}: {rendered}"
        );
        assert!(
            !rendered.contains("LEAK-TWO"),
            "leaked value in {fmt} {verb}: {rendered}"
        );
        assert!(
            rendered.contains("`simple`"),
            "alternatives must still list `simple` in {fmt} {verb}: {rendered}"
        );
    }
}

#[test]
fn wrong_type_reported_both_formats_both_verbs() {
    for (fmt, verb, doc, is_json) in cells("{simple: 123}", r#"{"simple":123}"#) {
        let rendered = assert_rejected(&doc, is_json);
        assert!(
            rendered.contains("wrong type for key `simple`"),
            "missing wrong-type key in {fmt} {verb}: {rendered}"
        );
        assert!(
            rendered.contains(ALTERNATIVES),
            "missing alternatives in {fmt} {verb}: {rendered}"
        );
        assert!(
            rendered.contains("value: {"),
            "missing wrapper hint in {fmt} {verb}: {rendered}"
        );
        assert!(
            !rendered.contains("123"),
            "leaked value in {fmt} {verb}: {rendered}"
        );
    }
}

#[test]
fn wrong_type_declaration_order_both_formats_both_verbs() {
    for (fmt, verb, doc, is_json) in cells("{rhai: [1], simple: 2}", r#"{"rhai":[1],"simple":2}"#) {
        let rendered = assert_rejected(&doc, is_json);
        assert!(
            rendered.contains("wrong type for key `simple`"),
            "declaration order must report `simple` in {fmt} {verb}: {rendered}"
        );
        assert!(
            !rendered.contains("wrong type for key `rhai`"),
            "`rhai` must not be reported in {fmt} {verb}: {rendered}"
        );
    }

    for (fmt, verb, doc, is_json) in
        cells("{xpath: [1], language: 2}", r#"{"xpath":[1],"language":2}"#)
    {
        let rendered = assert_rejected(&doc, is_json);
        assert!(
            rendered.contains("wrong type for key `language`"),
            "declaration order must report `language` in {fmt} {verb}: {rendered}"
        );
        assert!(
            !rendered.contains("wrong type for key `xpath`"),
            "`xpath` must not be reported in {fmt} {verb}: {rendered}"
        );
    }
}

#[test]
fn multiple_unknown_keys_sorted_both_formats_both_verbs() {
    for (fmt, verb, doc, is_json) in cells(
        r#"{zeta: "LEAK-A", alpha: "LEAK-B", simple: "${body}"}"#,
        r#"{"zeta":"LEAK-A","alpha":"LEAK-B","simple":"${body}"}"#,
    ) {
        let rendered = assert_rejected(&doc, is_json);
        assert!(
            rendered.contains("unknown key(s) `alpha`, `zeta` in value-source mapping"),
            "unknown keys must be sorted in {fmt} {verb}: {rendered}"
        );
        assert!(
            !rendered.contains("LEAK-A"),
            "leaked value A in {fmt} {verb}: {rendered}"
        );
        assert!(
            !rendered.contains("LEAK-B"),
            "leaked value B in {fmt} {verb}: {rendered}"
        );
    }
}

#[test]
fn wrapped_object_is_literal_both_formats_both_verbs() {
    for (_, _, doc, is_json) in cells("{value: {constant: ok}}", r#"{"value":{"constant":"ok"}}"#) {
        let value = parse_body_value(&doc, is_json);
        assert_eq!(
            value,
            ValueSourceDef::Literal(json!({"constant": "ok"})),
            "wrapped object must stay literal"
        );
    }
}

#[test]
fn scalar_array_null_and_interpolation_unchanged_both_formats_both_verbs() {
    let literal_cases = [
        ("42", "42", json!(42)),
        (
            "[1, 2, {a: 1}]",
            r#"[1,2,{"a":1}]"#,
            json!([1, 2, {"a": 1}]),
        ),
        ("null", "null", json!(null)),
    ];
    for (yaml_body, json_body, expected) in literal_cases {
        for (_, _, doc, is_json) in cells(yaml_body, json_body) {
            let value = parse_body_value(&doc, is_json);
            assert_eq!(value, ValueSourceDef::Literal(expected.clone()));
        }
    }

    for (_, _, doc, is_json) in cells(r#""${body}""#, r#""${body}""#) {
        let value = parse_body_value(&doc, is_json);
        assert_eq!(
            value,
            ValueSourceDef::Expression(LanguageExpressionDef {
                language: "simple".to_string(),
                source: "${body}".to_string(),
            }),
            "bare ${{}} string must stay a Simple expression"
        );
    }
}

#[test]
fn empty_config_and_multiple_sources_fail_both_formats_both_verbs() {
    for (fmt, verb, doc, is_json) in cells("{}", "{}") {
        let rendered = assert_rejected(&doc, is_json);
        assert!(
            rendered.contains(&format!("{fmt} DSL error: {verb}")),
            "missing format/verb prefix in {fmt} {verb}: {rendered}"
        );
        assert!(
            rendered.contains("must define exactly one value source"),
            "missing exactly-one message in {fmt} {verb}: {rendered}"
        );
    }

    for (fmt, verb, doc, is_json) in cells(
        r#"{value: "x", simple: "y"}"#,
        r#"{"value":"x","simple":"y"}"#,
    ) {
        let rendered = assert_rejected(&doc, is_json);
        assert!(
            rendered.contains(&format!("{fmt} DSL error: {verb}")),
            "missing format/verb prefix in {fmt} {verb}: {rendered}"
        );
        assert!(
            rendered.contains("must define exactly one value source"),
            "missing exactly-one message in {fmt} {verb}: {rendered}"
        );
    }
}

#[test]
fn error_prefix_is_format_and_verb_exact() {
    let yaml = assert_rejected(&yaml_doc("      - set_body: {constant: ok}\n"), false);
    assert!(yaml.contains("YAML DSL error: set_body:"), "got: {yaml}");

    let json = assert_rejected(&json_doc(r#"{"transform":{"constant":"ok"}}"#), true);
    assert!(json.contains("JSON DSL error: transform:"), "got: {json}");
}
