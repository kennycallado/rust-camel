//! Schema validation: every JSON example under examples/json-dsl/ must
//! conform to the committed schemas/dsl/route-schema.json.
//!
//! Negative test: a deliberately-malformed route is rejected.
//!
//! Uses jsonschema 0.46 API: `validator_for(&schema)` returns
//! `Result<Validator, ValidationError>`, and `Validator::validate(&instance)`
//! returns `Result<(), ValidationError>` (single error, not iterator). For
//! collecting ALL errors use `Validator::iter_errors(&instance)`.

use std::path::{Path, PathBuf};

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

fn load_raw(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_json(path: &Path) -> serde_json::Value {
    let raw = load_raw(path);
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
        let raw = load_raw(&path);
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
        // Schema conformance alone is not enough: the example must also
        // deserialize into the declarative model, or schema/field drift
        // lets an example pass here but fail at route load time.
        if let Err(e) = camel_dsl::parse_json_to_declarative(&raw) {
            panic!(
                "example {} failed serde deserialization into the DSL model: {e}",
                path.display()
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

#[test]
fn schema_validation_accepts_circuit_breaker_fallback() {
    let schema = load_json(&schema_path());
    let validator = validator_for(&schema).expect("schema compiles");

    let valid: serde_json::Value = serde_json::json!({
        "routes": [
            {
                "id": "cb-fallback-demo",
                "from": "timer:tick?period=8000&repeatCount=1",
                "circuit_breaker": {
                    "failure_threshold": 1,
                    "open_duration_ms": 60000,
                    "fallback": [
                        { "cache_peek_stale": { "key": "tile-xyz" } }
                    ]
                },
                "steps": [
                    { "log": "upstream fetch" }
                ]
            }
        ]
    });

    let errors: Vec<_> = validator.iter_errors(&valid).collect();
    assert!(
        errors.is_empty(),
        "route with circuit_breaker.fallback should validate, got: {errors:?}"
    );
}

#[test]
fn schema_validation_rejects_unknown_circuit_breaker_field() {
    let schema = load_json(&schema_path());
    let validator = validator_for(&schema).expect("schema compiles");

    let invalid: serde_json::Value = serde_json::json!({
        "routes": [
            {
                "id": "cb-unknown-field",
                "from": "timer:tick",
                "circuit_breaker": {
                    "failure_threshold": 1,
                    "unknown_key": 1
                }
            }
        ]
    });

    assert!(
        validator.validate(&invalid).is_err(),
        "unknown field under circuit_breaker must be rejected by schema, but validation passed"
    );
}

#[test]
fn schema_validation_accepts_cache_admin_route() {
    let schema = load_json(&schema_path());
    let validator = validator_for(&schema).expect("schema compiles");

    let valid: serde_json::Value = serde_json::json!({
        "routes": [
            {
                "id": "cache-admin",
                "from": "direct:start",
                "steps": [
                    { "cache_clear": { "repository": "memory" } },
                    { "cache_stats": {} }
                ]
            }
        ]
    });

    let errors: Vec<_> = validator.iter_errors(&valid).collect();
    assert!(
        errors.is_empty(),
        "route with cache_clear/cache_stats should validate, got: {errors:?}"
    );
}

#[test]
fn schema_validation_rejects_unknown_cache_clear_field() {
    let schema = load_json(&schema_path());
    let validator = validator_for(&schema).expect("schema compiles");

    let invalid: serde_json::Value = serde_json::json!({
        "routes": [
            {
                "id": "cache-clear-unknown",
                "from": "direct:start",
                "steps": [
                    { "cache_clear": { "repo": "memory" } }
                ]
            }
        ]
    });

    assert!(
        validator.validate(&invalid).is_err(),
        "unknown field under cache_clear must be rejected by schema, but validation passed"
    );
}

#[test]
fn schema_validation_accepts_cache_invalidate_key_prefix() {
    let schema = load_json(&schema_path());
    let validator = validator_for(&schema).expect("schema compiles");

    let valid: serde_json::Value = serde_json::json!({
        "routes": [
            {
                "id": "cache-invalidate-prefix",
                "from": "direct:start",
                "steps": [
                    { "cache_invalidate": { "key_prefix": "ns:" } }
                ]
            }
        ]
    });

    let errors: Vec<_> = validator.iter_errors(&valid).collect();
    assert!(
        errors.is_empty(),
        "route with cache_invalidate.key_prefix should validate, got: {errors:?}"
    );
}

#[test]
fn schema_validation_accepts_cache_coalesce_misses() {
    let schema = load_json(&schema_path());
    let validator = validator_for(&schema).expect("schema compiles");

    let valid: serde_json::Value = serde_json::json!({
        "routes": [
            {
                "id": "cache-coalesce",
                "from": "direct:start",
                "steps": [
                    {
                        "cache": {
                            "key": "k",
                            "coalesce_misses": true,
                            "on_miss": [ { "log": "miss" } ]
                        }
                    }
                ]
            }
        ]
    });

    let errors: Vec<_> = validator.iter_errors(&valid).collect();
    assert!(
        errors.is_empty(),
        "route with cache.coalesce_misses should validate, got: {errors:?}"
    );
}

#[test]
fn cache_invalidate_both_key_and_key_prefix_rejected_at_compile() {
    let yaml = r#"
routes:
  - id: cache-invalidate-both
    from: direct:start
    steps:
      - cache_invalidate:
          key: "k"
          key_prefix: "ns:"
"#;
    let routes = camel_dsl::parse_yaml_to_declarative(yaml).expect("YAML must parse");
    let step = routes[0].steps[0].clone();
    let err = camel_dsl::compile_declarative_step(step)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("exactly one of"),
        "both key and key_prefix must be rejected at compile, got: {err}"
    );
}
