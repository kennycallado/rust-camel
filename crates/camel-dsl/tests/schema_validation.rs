//! Schema validation: every JSON example under examples/json-dsl/ must
//! conform to the committed schemas/dsl/route-schema.json.
//!
//! Negative test: a deliberately-malformed route is rejected.
//!
//! Uses jsonschema 0.46 API: `validator_for(&schema)` returns
//! `Result<Validator, ValidationError>`, and `Validator::validate(&instance)`
//! returns `Result<(), ValidationError>` (single error, not iterator). For
//! collecting ALL errors use `Validator::iter_errors(&instance)`.

use std::path::PathBuf;

use jsonschema::validator_for;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn schema_path() -> PathBuf {
    workspace_root().join("schemas/dsl/route-schema.json")
}

fn examples_dir() -> PathBuf {
    workspace_root().join("examples/json-dsl/config")
}

fn load_json(path: &PathBuf) -> serde_json::Value {
    let raw =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("JSON must parse")
}

#[test]
fn schema_compiles() {
    let schema = load_json(&schema_path());
    validator_for(&schema).expect("DSL schema must compile");
}

#[test]
fn all_json_examples_validate() {
    let schema = load_json(&schema_path());
    let validator = validator_for(&schema).expect("schema compiles");

    let dir = examples_dir();
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("examples dir readable") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let example = load_json(&path);
        // Use iter_errors to collect all failures (not just the first).
        let errors: Vec<_> = validator.iter_errors(&example).collect();
        if !errors.is_empty() {
            let msgs: Vec<String> = errors.iter().map(|e| format!("  - {e}")).collect();
            panic!(
                "example {} failed schema validation:\n{}",
                path.display(),
                msgs.join("\n")
            );
        }
        checked += 1;
    }
    assert!(checked >= 1, "expected at least one example, found 0");
}

#[test]
fn negative_test_rejects_malformed_route() {
    let schema = load_json(&schema_path());
    let validator = validator_for(&schema).expect("schema compiles");

    let malformed: serde_json::Value = serde_json::json!({
        "routes": "this should be an array, not a string"
    });
    // validate() returns Result<(), ValidationError> in 0.46 — single error.
    assert!(
        validator.validate(&malformed).is_err(),
        "malformed route should be rejected by schema, but validation passed"
    );
}
