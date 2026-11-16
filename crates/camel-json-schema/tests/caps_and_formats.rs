//! Resource caps, bounded reads, and cross-format includes (ADR-0100).
//!
//! Every test builds its fixtures in its own temporary directory; the padded
//! files used by the cap tests are sized explicitly so the boundary under
//! test is exact.

use std::path::Path;

use camel_json_schema::CompiledSchema;
use camel_json_schema::FormatMode;
use camel_json_schema::SchemaError;
use camel_json_schema::SchemaSource;
use camel_json_schema::SchemaSyntax;
use camel_json_schema::compile;
use serde_json::json;
use tempfile::TempDir;

/// Per-file cap under test.
const FILE_CAP: u64 = 16 * 1024 * 1024;
/// Total-byte cap under test.
const TOTAL_CAP: u64 = 32 * 1024 * 1024;
/// Resource-count cap under test.
const RESOURCE_CAP: usize = 256;

fn compile_file(path: &Path) -> Result<CompiledSchema, SchemaError> {
    compile(
        &SchemaSource::File(path.to_path_buf()),
        SchemaSyntax::Auto,
        FormatMode::Enforce,
    )
}

fn compile_file_with(path: &Path, syntax: SchemaSyntax) -> Result<CompiledSchema, SchemaError> {
    compile(
        &SchemaSource::File(path.to_path_buf()),
        syntax,
        FormatMode::Enforce,
    )
}

fn fixture() -> TempDir {
    tempfile::tempdir().expect("creates the fixture root")
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("writes a schema fixture");
}

/// Write `json` followed by spaces so the file is exactly `size` bytes.
///
/// The padding stays inside the JSON document (trailing whitespace), so the
/// file remains a valid schema while hitting an exact byte boundary.
fn write_padded(path: &Path, json: &str, size: u64) {
    let size = usize::try_from(size).expect("fixture size fits in memory");
    assert!(json.len() <= size, "padding cannot shrink the JSON body");
    let mut contents = String::with_capacity(size);
    contents.push_str(json);
    contents.extend(std::iter::repeat_n(' ', size - json.len()));
    std::fs::write(path, contents).expect("writes a padded schema fixture");
}

/// Root `{"allOf":[{"$ref":"d1.json"},…]}` plus one `dN.json` per sibling.
fn write_ref_fanout(dir: &Path, siblings: usize) {
    let mut root = String::from(r#"{"allOf":["#);
    for index in 1..=siblings {
        if index > 1 {
            root.push(',');
        }
        root.push_str(&format!(r#"{{"$ref":"d{index}.json"}}"#));
    }
    root.push_str("]}");
    write(&dir.join("root.json"), &root);
    for index in 1..=siblings {
        write(&dir.join(format!("d{index}.json")), r#"{"type":"integer"}"#);
    }
}

#[test]
fn file_over_16_mib_rejected() {
    let dir = fixture();
    write(&dir.path().join("root.json"), r#"{"$ref":"big.json"}"#);
    write_padded(
        &dir.path().join("big.json"),
        r#"{"type":"integer"}"#,
        FILE_CAP + 1,
    );

    let error = compile_file(&dir.path().join("root.json"))
        .expect_err("a referenced file over the per-file cap must be rejected");

    let SchemaError::FileTooLarge { path, limit } = &error else {
        panic!("unexpected error: {error:?}");
    };
    assert!(
        path.contains("big.json"),
        "the rejection must name the oversized file: {path}"
    );
    assert_eq!(*limit, FILE_CAP, "the rejection must report the cap");
}

#[test]
fn file_at_16_mib_accepted() {
    let dir = fixture();
    write(&dir.path().join("root.json"), r#"{"$ref":"big.json"}"#);
    write_padded(
        &dir.path().join("big.json"),
        r#"{"type":"integer"}"#,
        FILE_CAP,
    );

    let compiled = compile_file(&dir.path().join("root.json"))
        .expect("a referenced file at exactly the per-file cap must compile");

    assert!(
        !compiled.violations(&json!("s")).is_empty(),
        "the padded sibling schema must still be enforced"
    );
}

#[test]
fn resource_budget_allows_256_rejects_257() {
    let allowed = fixture();
    write_ref_fanout(allowed.path(), RESOURCE_CAP - 1);
    compile_file(&allowed.path().join("root.json"))
        .expect("root plus 255 distinct siblings must fit the resource cap");

    let rejected = fixture();
    write_ref_fanout(rejected.path(), RESOURCE_CAP);
    let error = compile_file(&rejected.path().join("root.json"))
        .expect_err("root plus 256 distinct siblings must exceed the resource cap");

    assert!(
        matches!(
            &error,
            SchemaError::TooManyResources { limit } if *limit == RESOURCE_CAP
        ),
        "unexpected error: {error:?}"
    );
}

#[test]
fn total_budget_rejects_over_32_mib() {
    // Three 11 MiB siblings: each stays under the per-file cap, the total
    // crosses the 32 MiB budget.
    let over = fixture();
    for name in ["a.json", "b.json", "c.json"] {
        write_padded(
            &over.path().join(name),
            r#"{"type":"integer"}"#,
            11 * 1024 * 1024,
        );
    }
    write(
        &over.path().join("root.json"),
        r#"{"allOf":[{"$ref":"a.json"},{"$ref":"b.json"},{"$ref":"c.json"}]}"#,
    );
    let error = compile_file(&over.path().join("root.json"))
        .expect_err("combined resource size over the total budget must be rejected");
    assert!(
        matches!(
            &error,
            SchemaError::TotalBudgetExceeded { limit } if *limit == TOTAL_CAP
        ),
        "unexpected error: {error:?}"
    );

    // Two 11 MiB siblings stay under the total budget.
    let under = fixture();
    for name in ["a.json", "b.json"] {
        write_padded(
            &under.path().join(name),
            r#"{"type":"integer"}"#,
            11 * 1024 * 1024,
        );
    }
    write(
        &under.path().join("root.json"),
        r#"{"allOf":[{"$ref":"a.json"},{"$ref":"b.json"}]}"#,
    );
    compile_file(&under.path().join("root.json"))
        .expect("combined resource size under the total budget must compile");
}

#[test]
fn repeated_ref_counts_once() {
    let dir = fixture();
    let mut root = String::from(r#"{"allOf":["#);
    for index in 0..300 {
        if index > 0 {
            root.push(',');
        }
        root.push_str(r#"{"$ref":"d.json#/$defs/x"}"#);
    }
    root.push_str("]}");
    write(&dir.path().join("root.json"), &root);
    write(
        &dir.path().join("d.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );

    compile_file(&dir.path().join("root.json"))
        .expect("repeated refs to one canonical path must count as one resource");
}

#[test]
fn json_root_referencing_yaml_works() {
    let dir = fixture();
    write(
        &dir.path().join("root.json"),
        r#"{"properties":{"x":{"$ref":"defs.yaml#/$defs/x"}}}"#,
    );
    write(
        &dir.path().join("defs.yaml"),
        r#"$defs:
  x:
    type: integer
"#,
    );

    let compiled = compile_file(&dir.path().join("root.json"))
        .expect("a JSON root must resolve a YAML include");

    assert!(
        compiled.violations(&json!({"x": 1})).is_empty(),
        "1 is an integer per the YAML include"
    );
    assert!(
        !compiled.violations(&json!({"x": "s"})).is_empty(),
        "a string violates the YAML include"
    );
}

#[test]
fn yaml_root_referencing_json_works() {
    let dir = fixture();
    write(
        &dir.path().join("root.yaml"),
        r#"type: object
properties:
  x:
    $ref: "defs.json#/$defs/x"
"#,
    );
    write(
        &dir.path().join("defs.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );

    let compiled = compile_file(&dir.path().join("root.yaml"))
        .expect("a YAML root must resolve a JSON include");

    assert!(
        compiled.violations(&json!({"x": 1})).is_empty(),
        "1 is an integer per the JSON include"
    );
    assert!(
        !compiled.violations(&json!({"x": "s"})).is_empty(),
        "a string violates the JSON include"
    );
}

#[test]
fn explicit_syntax_override_keeps_sibling_resolution() {
    let dir = fixture();
    write(
        &dir.path().join("root.json"),
        r#"type: object
properties:
  x:
    $ref: "defs.json#/$defs/x"
"#,
    );
    write(
        &dir.path().join("defs.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );

    let compiled = compile_file_with(&dir.path().join("root.json"), SchemaSyntax::Yaml)
        .expect("a YAML root behind a .json extension must compile when overridden");

    assert!(
        compiled.violations(&json!({"x": 1})).is_empty(),
        "1 is an integer per the sibling definition"
    );
    assert!(
        !compiled.violations(&json!({"x": "s"})).is_empty(),
        "a string violates the sibling definition"
    );

    let error = compile_file_with(&dir.path().join("root.json"), SchemaSyntax::Json)
        .expect_err("YAML content parsed as JSON must fail");
    assert!(
        matches!(&error, SchemaError::Parse { .. }),
        "unexpected error: {error:?}"
    );
    assert!(
        error.to_string().contains("invalid JSON schema"),
        "the parse error must keep the JSON literal: {error}"
    );
}

#[test]
fn referenced_file_declaring_other_draft_compiles() {
    let dir = fixture();
    write(
        &dir.path().join("root.json"),
        r#"{"type":"object","properties":{"x":{"$ref":"d7.json#/definitions/x"}}}"#,
    );
    write(
        &dir.path().join("d7.json"),
        r#"{"$schema":"http://json-schema.org/draft-07/schema#","definitions":{"x":{"type":"integer"}}}"#,
    );

    let compiled = compile_file(&dir.path().join("root.json"))
        .expect("a referenced document may declare a different draft");

    assert!(
        compiled.violations(&json!({"x": 1})).is_empty(),
        "1 is an integer per the draft-07 definition"
    );
    assert!(
        !compiled.violations(&json!({"x": "s"})).is_empty(),
        "a string violates the draft-07 definition"
    );
}

#[test]
fn draft07_definitions_through_external_ref() {
    let dir = fixture();
    write(
        &dir.path().join("root.json"),
        r#"{"$schema":"http://json-schema.org/draft-07/schema#","$ref":"defs.json#/definitions/x"}"#,
    );
    write(
        &dir.path().join("defs.json"),
        r#"{"$schema":"http://json-schema.org/draft-07/schema#","definitions":{"x":{"type":"integer"}}}"#,
    );

    let compiled = compile_file(&dir.path().join("root.json"))
        .expect("a draft-07 root must resolve definitions through an external ref");

    assert!(
        compiled.violations(&json!(1)).is_empty(),
        "1 is an integer per the external definition"
    );
    assert!(
        !compiled.violations(&json!("s")).is_empty(),
        "a string violates the external definition"
    );
}
