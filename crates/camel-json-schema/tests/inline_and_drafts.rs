//! Inline-only compilation, draft selection, and format handling (ADR-0100).

use camel_json_schema::CompiledSchema;
use camel_json_schema::FormatMode;
use camel_json_schema::SchemaError;
use camel_json_schema::SchemaSource;
use camel_json_schema::SchemaSyntax;
use camel_json_schema::compile;
use serde_json::Value;
use serde_json::json;

fn compile_inline(schema: Value, format: FormatMode) -> Result<CompiledSchema, SchemaError> {
    compile(&SchemaSource::Inline(schema), SchemaSyntax::Auto, format)
}

#[test]
fn inline_internal_ref_resolves_and_validates() {
    let schema = json!({
        "$defs": {"pos": {"type": "integer", "minimum": 0}},
        "$ref": "#/$defs/pos",
    });
    let compiled =
        compile_inline(schema, FormatMode::Enforce).expect("inline internal ref should compile");

    assert!(
        !compiled.violations(&json!(-1)).is_empty(),
        "minimum: 0 must reject -1"
    );
    assert!(
        compiled.violations(&json!(3)).is_empty(),
        "3 satisfies minimum: 0"
    );
}

#[test]
fn inline_external_ref_rejected_with_named_error() {
    let error = compile_inline(json!({"$ref": "defs.json#/$defs/x"}), FormatMode::Enforce)
        .expect_err("external file ref must be rejected");

    assert!(
        matches!(&error, SchemaError::ExternalRefRejected { .. }),
        "unexpected error: {error:?}"
    );
    assert!(
        error.to_string().contains("defs.json"),
        "message must name the ref: {error}"
    );
}

#[test]
fn inline_external_ref_rejected_absolute_path() {
    let error = compile_inline(json!({"$ref": "file:///etc/hostname"}), FormatMode::Enforce)
        .expect_err("absolute file ref must be rejected");

    assert!(
        matches!(&error, SchemaError::ExternalRefRejected { .. }),
        "unexpected error: {error:?}"
    );
    assert!(
        error.to_string().contains("/etc/hostname"),
        "message must name the path: {error}"
    );
}

#[test]
fn inline_remote_ref_rejected_with_local_files_only_error() {
    let error = compile_inline(
        json!({"$ref": "https://example.com/s.json"}),
        FormatMode::Enforce,
    )
    .expect_err("remote ref must be rejected");

    assert!(
        matches!(&error, SchemaError::RemoteRefRejected { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        error.to_string(),
        "remote $ref 'https://example.com/s.json' rejected: schemas resolve from local files only (ADR-0100); vendor the file"
    );
}

#[test]
fn inline_remote_dollar_id_rebase_rejected_with_local_files_only_error() {
    let error = compile_inline(
        json!({"$id": "https://example.com/b.json", "$ref": "s.json"}),
        FormatMode::Enforce,
    )
    .expect_err("remote $id rebase must be rejected");

    assert!(
        matches!(&error, SchemaError::RemoteRefRejected { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        error.to_string(),
        "remote $ref 'https://example.com/s.json' rejected: schemas resolve from local files only (ADR-0100); vendor the file"
    );
}

#[test]
fn draft_2020_12_defs_used_when_absent() {
    let schema = json!({
        "$defs": {"x": {"type": "integer"}},
        "$ref": "#/$defs/x",
    });
    let compiled =
        compile_inline(schema, FormatMode::Enforce).expect("2020-12 is the default draft");

    assert!(
        !compiled.violations(&json!("not an integer")).is_empty(),
        "$defs must apply when $schema is absent"
    );
    assert!(compiled.violations(&json!(7)).is_empty());
}

#[test]
fn draft07_ignores_keywords_beside_ref() {
    let schema = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "integer",
        "$ref": "#/definitions/x",
        "definitions": {"x": {"type": "string"}},
    });
    let compiled = compile_inline(schema, FormatMode::Enforce).expect("draft-07 schema compiles");

    assert!(
        compiled.violations(&json!("hi")).is_empty(),
        "draft-07 ignores the sibling type keyword beside $ref"
    );
}

#[test]
fn draft202012_applies_keywords_beside_ref() {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "integer",
        "$ref": "#/$defs/x",
        "$defs": {"x": {"type": "string"}},
    });
    let compiled = compile_inline(schema, FormatMode::Enforce).expect("2020-12 schema compiles");

    assert!(
        !compiled.violations(&json!("hi")).is_empty(),
        "2020-12 applies the sibling type keyword beside $ref"
    );
}

#[test]
fn format_enforce_rejects_bad_email() {
    let schema = json!({"type": "string", "format": "email"});
    let compiled = compile_inline(schema, FormatMode::Enforce).expect("format schema compiles");

    assert!(
        !compiled.violations(&json!("not-an-email")).is_empty(),
        "Enforce validates the email format"
    );
}

#[test]
fn format_annotate_accepts_bad_email() {
    let schema = json!({"type": "string", "format": "email"});
    let compiled = compile_inline(schema, FormatMode::Annotate).expect("format schema compiles");

    assert!(
        compiled.violations(&json!("not-an-email")).is_empty(),
        "Annotate treats the email format as an annotation"
    );
}
