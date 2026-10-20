//! Executable contract between `schemas/jms-pool-health-config.json` and the
//! four health fields of [`JmsPoolConfig`].
//!
//! The schema is a hand-authored, scoped fragment: it must reject bad health
//! values while leaving every other pool property to runtime validation.

use std::path::PathBuf;

use camel_component_jms::{HealthBrokerProbeMode, JmsPoolConfig};
use serde_json::{Value, json};

fn workspace_root() -> PathBuf {
    // crates/components/camel-jms -> crates/components -> crates -> workspace.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn schema_path() -> PathBuf {
    workspace_root().join("schemas/jms-pool-health-config.json")
}

fn load_schema() -> Value {
    let path = schema_path();
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn property<'a>(schema: &'a Value, name: &str) -> &'a Value {
    &schema["properties"][name]
}

/// Compile the schema and assert the instance is rejected with at least one
/// error whose `instance_path` identifies `property` (not Display text).
fn assert_schema_rejects(schema: &Value, instance: &Value, property_name: &str) {
    let validator = jsonschema::validator_for(schema).expect("schema must compile");
    let paths: Vec<String> = validator
        .iter_errors(instance)
        .map(|e| e.instance_path().as_str().to_string())
        .collect();
    assert!(
        !paths.is_empty(),
        "instance must be rejected for `{property_name}`: {instance}"
    );
    assert!(
        paths
            .iter()
            .any(|path| path.split('/').any(|segment| segment == property_name)),
        "no validation error path names `{property_name}`; got {paths:?}"
    );
}

fn object_with(property_name: &str, value: Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(property_name.to_string(), value);
    Value::Object(map)
}

#[test]
fn schema_compiles_and_is_scoped_to_four_health_properties() {
    let schema = load_schema();
    jsonschema::validator_for(&schema).expect("schema must compile");

    let mut names: Vec<&str> = schema["properties"]
        .as_object()
        .expect("properties must be an object")
        .keys()
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "health_broker_probe_mode",
            "health_check_interval_ms",
            "health_failure_threshold",
            "health_timeout_ms",
        ]
    );
    assert_eq!(schema["additionalProperties"], Value::Bool(true));
}

#[test]
fn schema_defaults_minimums_enum_and_runtime_agree() {
    let schema = load_schema();
    let cfg = JmsPoolConfig::default();
    cfg.validate().expect("default pool config must be valid");

    let timeout = property(&schema, "health_timeout_ms");
    assert_eq!(timeout["default"].as_u64(), Some(cfg.health_timeout_ms));
    assert_eq!(timeout["default"].as_u64(), Some(3_000));
    assert_eq!(timeout["minimum"].as_u64(), Some(1));
    assert_eq!(timeout["maximum"].as_u64(), Some(u64::MAX));

    let threshold = property(&schema, "health_failure_threshold");
    assert_eq!(
        threshold["default"].as_u64(),
        Some(u64::from(cfg.health_failure_threshold))
    );
    assert_eq!(threshold["default"].as_u64(), Some(3));
    assert_eq!(threshold["minimum"].as_u64(), Some(1));
    assert_eq!(threshold["maximum"].as_u64(), Some(u64::from(u32::MAX)));

    let interval = property(&schema, "health_check_interval_ms");
    assert_eq!(
        interval["default"].as_u64(),
        Some(cfg.health_check_interval_ms)
    );
    assert_eq!(interval["default"].as_u64(), Some(5_000));
    assert_eq!(interval["minimum"].as_u64(), Some(1));
    assert!(
        interval.get("maximum").is_none(),
        "interval has no runtime upper bound"
    );

    let mode = property(&schema, "health_broker_probe_mode");
    let modes: Vec<&str> = mode["enum"]
        .as_array()
        .expect("enum must be an array")
        .iter()
        .map(|v| v.as_str().expect("enum entries must be strings"))
        .collect();
    assert_eq!(modes, vec!["reuse", "skip"]);
    assert_eq!(mode["default"], "reuse");
    assert_eq!(cfg.health_broker_probe_mode, HealthBrokerProbeMode::Reuse);
}

#[test]
fn zero_values_wrong_types_and_overflow_are_rejected() {
    let schema = load_schema();

    for property_name in [
        "health_timeout_ms",
        "health_failure_threshold",
        "health_check_interval_ms",
    ] {
        let instance = object_with(property_name, json!(0));
        assert_schema_rejects(&schema, &instance, property_name);

        let cfg: JmsPoolConfig = serde_json::from_value(instance)
            .expect("zero numeric value must still deserialize before runtime validation");
        let err = cfg
            .validate()
            .expect_err("zero value must fail runtime validation")
            .to_string();
        assert!(
            err.contains(property_name),
            "runtime error must name `{property_name}`: {err}"
        );
    }

    for property_name in [
        "health_timeout_ms",
        "health_failure_threshold",
        "health_check_interval_ms",
    ] {
        let instance = object_with(property_name, json!("3000"));
        assert_schema_rejects(&schema, &instance, property_name);
        assert!(
            serde_json::from_value::<JmsPoolConfig>(instance).is_err(),
            "string-valued `{property_name}` must fail serde deserialization"
        );
    }

    // Parse overflow JSON text instead of writing oversized Rust literals.
    for (property_name, raw) in [
        (
            "health_timeout_ms",
            r#"{"health_timeout_ms": 18446744073709551616}"#,
        ),
        (
            "health_failure_threshold",
            r#"{"health_failure_threshold": 4294967296}"#,
        ),
    ] {
        let instance: Value =
            serde_json::from_str(raw).expect("overflow JSON must parse as a Value");
        assert_schema_rejects(&schema, &instance, property_name);
        assert!(
            serde_json::from_str::<JmsPoolConfig>(raw).is_err(),
            "overflowing `{property_name}` must fail serde deserialization"
        );
    }
}

#[test]
fn omitted_health_keys_validate_and_deserialize_to_defaults() {
    let schema = load_schema();
    let instance = json!({ "brokers": {} });

    let validator = jsonschema::validator_for(&schema).expect("schema must compile");
    assert!(
        validator.validate(&instance).is_ok(),
        "omitted health keys must validate: {instance}"
    );

    let cfg: JmsPoolConfig =
        serde_json::from_value(instance).expect("brokers object must deserialize");
    assert_eq!(cfg.health_timeout_ms, 3_000);
    assert_eq!(cfg.health_failure_threshold, 3);
    assert_eq!(cfg.health_broker_probe_mode, HealthBrokerProbeMode::Reuse);
    assert_eq!(cfg.health_check_interval_ms, 5_000);
}

#[test]
fn unknown_probe_mode_is_rejected_by_schema_and_serde() {
    let schema = load_schema();
    let instance = json!({ "health_broker_probe_mode": "probe" });

    assert_schema_rejects(&schema, &instance, "health_broker_probe_mode");
    assert!(
        serde_json::from_value::<JmsPoolConfig>(instance).is_err(),
        "unknown probe mode must fail serde deserialization"
    );
}

#[test]
fn unknown_other_pool_key_is_permitted() {
    let schema = load_schema();
    let instance = json!({
        "max_bridges": 4,
        "brokers": {},
        "future_pool_key": true
    });

    let validator = jsonschema::validator_for(&schema).expect("schema must compile");
    assert!(
        validator.validate(&instance).is_ok(),
        "scoped fragment must permit other pool keys: {instance}"
    );
}
