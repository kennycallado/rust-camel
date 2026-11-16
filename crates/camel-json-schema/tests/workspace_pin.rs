//! Workspace-wide pin: bare `jsonschema::validator_for` must fail closed.
//!
//! The workspace dependency disables upstream's `resolve-file` feature
//! (ADR-0100), so the default retriever cannot read `file:` references.
//! Only [`camel_json_schema::compile`] may resolve files, and it does so
//! through the confined retriever.

use camel_json_schema::FormatMode;
use camel_json_schema::SchemaSource;
use camel_json_schema::SchemaSyntax;
use camel_json_schema::compile;
use serde_json::json;
use std::path::Path;
use tempfile::TempDir;

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("writes a schema fixture");
}

/// The original vulnerability shape: a bare validator follows an absolute
/// `file:` reference to an existing schema on disk.
#[test]
fn bare_validator_for_fails_closed_without_resolve_file() {
    let dir: TempDir = tempfile::tempdir().expect("creates the fixture root");
    let defs = dir.path().join("defs.json");
    write(&defs, r#"{"$defs":{"x":{"type":"integer"}}}"#);

    let root = json!({
        "$ref": format!("file://{}#/$defs/x", defs.display()),
    });

    let outcome = jsonschema::validator_for(&root);
    assert!(
        outcome.is_err(),
        "bare validator_for must reject a file: ref without the resolve-file feature"
    );
}

/// The confined crate API keeps resolving sibling refs with no upstream
/// feature enabled.
#[test]
fn confined_crate_still_resolves_refs() {
    let dir: TempDir = tempfile::tempdir().expect("creates the fixture root");
    write(
        &dir.path().join("root.json"),
        r#"{"type":"object","properties":{"x":{"$ref":"defs.json#/$defs/x"}}}"#,
    );
    write(
        &dir.path().join("defs.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );

    let compiled = compile(
        &SchemaSource::File(dir.path().join("root.json")),
        SchemaSyntax::Auto,
        FormatMode::Enforce,
    )
    .expect("a sibling ref inside the schema root must resolve");

    assert!(
        compiled.violations(&json!({"x": 1})).is_empty(),
        "1 is an integer per the sibling definition"
    );
    assert!(
        !compiled.violations(&json!({"x": "s"})).is_empty(),
        "a string violates the sibling definition"
    );
}
