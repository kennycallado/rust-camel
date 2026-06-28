//! Tunable resource limits for in-process scripting engines (Rhai, Boa JS).
//!
//! These types live in `camel-language-api` (the lowest shared language contract
//! crate) to avoid a circular dependency: `camel-language-rhai` / `camel-language-js`
//! already depend on `camel-language-api`, so the limit types must be defined here
//! rather than in `camel-config` (which those crates cannot depend on).
//!
//! All fields are `Option`; `None` means "use the rust-camel runtime default" —
//! never the upstream engine's unlimited default (per ADR-0011). The resolve
//! functions in each language crate document the concrete defaults.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Rhai limits
// ---------------------------------------------------------------------------

/// Tunable resource limits for a single Rhai `Engine` instance.
///
/// Surfaced in `Camel.toml` as:
///
/// ```toml
/// [languages.rhai.limits]
/// max-operations = 500000
/// max-string-size = 10485760
/// max-array-size = 100000
/// max-map-size = 100000
/// max-expression-depth = 10
/// max-function-expression-depth = 5
/// execution-timeout-ms = 5000
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RhaiLimitsConfig {
    /// Maximum number of operations before Rhai terminates the script
    /// (rhai: `max_operations`). Counter resets each call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_operations: Option<u64>,

    /// Maximum string size in bytes (rhai: `max_string_size`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_string_size: Option<usize>,

    /// Maximum array size in elements (rhai: `max_array_size`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_array_size: Option<usize>,

    /// Maximum map size in key-value pairs (rhai: `max_map_size`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_map_size: Option<usize>,

    /// Maximum nesting depth for expressions (rhai: `max_expression_depth`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_expression_depth: Option<u32>,

    /// Maximum nesting depth for function call expressions
    /// (rhai: `max_function_expression_depth`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_function_expression_depth: Option<u32>,

    /// Maximum execution wall-clock time in milliseconds.
    /// Rhai has no built-in timeout; the consuming code enforces this via
    /// `Engine::on_progress` or a tokio timeout wrapper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_timeout_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// JS (Boa) limits
// ---------------------------------------------------------------------------

/// Tunable resource limits for a single Boa JS `Context` instance.
///
/// Surfaced in `Camel.toml` as:
///
/// ```toml
/// [languages.js.limits]
/// execution-timeout-ms = 5000
/// max-loop-iterations = 1000000
/// max-recursion-depth = 64
/// max-stack-size = 1048576
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct JsLimitsConfig {
    /// Maximum execution wall-clock time in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_timeout_ms: Option<u64>,

    /// Maximum number of loop iterations before Boa terminates execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_loop_iterations: Option<u64>,

    /// Maximum recursion depth for function calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_recursion_depth: Option<usize>,

    /// Maximum Boa VM stack size, in stack slots (not bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_stack_size: Option<usize>,
}

// ---------------------------------------------------------------------------
// Wrapper structs for Camel.toml sections
// ---------------------------------------------------------------------------

/// Rhai engine configuration block in `Camel.toml`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RhaiEngineConfig {
    /// Resource limits for the Rhai engine.
    #[serde(default)]
    pub limits: RhaiLimitsConfig,
}

/// JS (Boa) engine configuration block in `Camel.toml`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct JsEngineConfig {
    /// Resource limits for the Boa JS engine.
    #[serde(default)]
    pub limits: JsLimitsConfig,
}

/// Top-level `[languages]` section in `Camel.toml`.
///
/// ```toml
/// [languages.rhai.limits]
/// max-operations = 500000
///
/// [languages.js.limits]
/// execution-timeout-ms = 5000
/// ```
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LanguagesConfig {
    /// Rhai engine configuration.
    #[serde(default)]
    pub rhai: RhaiEngineConfig,

    /// JS (Boa) engine configuration.
    #[serde(default)]
    pub js: JsEngineConfig,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- RhaiLimitsConfig tests -------------------------------------------

    #[test]
    fn rhai_defaults_to_all_none() {
        let cfg = RhaiLimitsConfig::default();
        assert_eq!(cfg.max_operations, None);
        assert_eq!(cfg.max_string_size, None);
        assert_eq!(cfg.max_array_size, None);
        assert_eq!(cfg.max_map_size, None);
        assert_eq!(cfg.max_expression_depth, None);
        assert_eq!(cfg.max_function_expression_depth, None);
        assert_eq!(cfg.execution_timeout_ms, None);
    }

    #[test]
    fn rhai_deserialises_full_block() {
        let toml = toml::toml! {
            max-operations = 500000i64
            max-string-size = 10485760i64
            max-array-size = 100000i64
            max-map-size = 100000i64
            max-expression-depth = 10
            max-function-expression-depth = 5
            execution-timeout-ms = 5000i64
        };
        let cfg: RhaiLimitsConfig = toml.try_into().expect("deserialize");
        assert_eq!(cfg.max_operations, Some(500_000));
        assert_eq!(cfg.max_string_size, Some(10_485_760));
        assert_eq!(cfg.max_array_size, Some(100_000));
        assert_eq!(cfg.max_map_size, Some(100_000));
        assert_eq!(cfg.max_expression_depth, Some(10));
        assert_eq!(cfg.max_function_expression_depth, Some(5));
        assert_eq!(cfg.execution_timeout_ms, Some(5000));
    }

    #[test]
    fn rhai_deserialises_partial_block() {
        let toml = toml::toml! {
            max-operations = 100000i64
            execution-timeout-ms = 3000i64
        };
        let cfg: RhaiLimitsConfig = toml.try_into().expect("deserialize");
        assert_eq!(cfg.max_operations, Some(100_000));
        assert_eq!(cfg.execution_timeout_ms, Some(3000));
        // All other fields should be None
        assert_eq!(cfg.max_string_size, None);
        assert_eq!(cfg.max_expression_depth, None);
    }

    #[test]
    fn rhai_rejects_unknown_field() {
        let toml = toml::toml! {
            max-operations = 100000i64
            fuel = 1000i64
        };
        let result: Result<RhaiLimitsConfig, _> = toml.try_into();
        assert!(result.is_err(), "deny_unknown_fields must reject `fuel`");
    }

    #[test]
    fn rhai_serde_round_trip_preserves_set_fields() {
        let original = RhaiLimitsConfig {
            max_operations: Some(200_000),
            max_string_size: Some(5_242_880),
            execution_timeout_ms: Some(10_000),
            ..Default::default()
        };
        let serialized = toml::to_string(&original).expect("serialize");
        let back: RhaiLimitsConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn rhai_skip_serializing_none_fields() {
        let cfg = RhaiLimitsConfig {
            max_operations: Some(100_000),
            max_string_size: None,
            execution_timeout_ms: Some(5000),
            ..Default::default()
        };
        let s = toml::to_string(&cfg).expect("serialize");
        assert!(s.contains("max-operations"));
        assert!(s.contains("execution-timeout-ms"));
        assert!(!s.contains("max-string-size"));
        assert!(!s.contains("max-expression-depth"));
    }

    // -- JsLimitsConfig tests ---------------------------------------------

    #[test]
    fn js_defaults_to_all_none() {
        let cfg = JsLimitsConfig::default();
        assert_eq!(cfg.execution_timeout_ms, None);
        assert_eq!(cfg.max_loop_iterations, None);
        assert_eq!(cfg.max_recursion_depth, None);
        assert_eq!(cfg.max_stack_size, None);
    }

    #[test]
    fn js_deserialises_full_block() {
        let toml = toml::toml! {
            execution-timeout-ms = 5000i64
            max-loop-iterations = 1000000i64
            max-recursion-depth = 64i64
            max-stack-size = 1048576i64
        };
        let cfg: JsLimitsConfig = toml.try_into().expect("deserialize");
        assert_eq!(cfg.execution_timeout_ms, Some(5000));
        assert_eq!(cfg.max_loop_iterations, Some(1_000_000));
        assert_eq!(cfg.max_recursion_depth, Some(64));
        assert_eq!(cfg.max_stack_size, Some(1_048_576));
    }

    #[test]
    fn js_deserialises_partial_block() {
        let toml = toml::toml! {
            execution-timeout-ms = 3000i64
            max-recursion-depth = 32i64
        };
        let cfg: JsLimitsConfig = toml.try_into().expect("deserialize");
        assert_eq!(cfg.execution_timeout_ms, Some(3000));
        assert_eq!(cfg.max_recursion_depth, Some(32));
        assert_eq!(cfg.max_loop_iterations, None);
        assert_eq!(cfg.max_stack_size, None);
    }

    #[test]
    fn js_rejects_unknown_field() {
        let toml = toml::toml! {
            execution-timeout-ms = 5000i64
            fuel = 1000i64
        };
        let result: Result<JsLimitsConfig, _> = toml.try_into();
        assert!(result.is_err(), "deny_unknown_fields must reject `fuel`");
    }

    #[test]
    fn js_serde_round_trip_preserves_set_fields() {
        let original = JsLimitsConfig {
            execution_timeout_ms: Some(10_000),
            max_loop_iterations: Some(500_000),
            ..Default::default()
        };
        let serialized = toml::to_string(&original).expect("serialize");
        let back: JsLimitsConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn js_skip_serializing_none_fields() {
        let cfg = JsLimitsConfig {
            execution_timeout_ms: Some(5000),
            max_loop_iterations: Some(1_000_000),
            ..Default::default()
        };
        let s = toml::to_string(&cfg).expect("serialize");
        assert!(s.contains("execution-timeout-ms"));
        assert!(s.contains("max-loop-iterations"));
        assert!(!s.contains("max-recursion-depth"));
        assert!(!s.contains("max-stack-size"));
    }

    // -- Wrapper struct tests ---------------------------------------------

    #[test]
    fn rhai_engine_config_defaults() {
        let cfg = RhaiEngineConfig::default();
        assert_eq!(cfg.limits, RhaiLimitsConfig::default());
    }

    #[test]
    fn js_engine_config_defaults() {
        let cfg = JsEngineConfig::default();
        assert_eq!(cfg.limits, JsLimitsConfig::default());
    }

    #[test]
    fn languages_config_defaults() {
        let cfg = LanguagesConfig::default();
        assert_eq!(cfg.rhai.limits, RhaiLimitsConfig::default());
        assert_eq!(cfg.js.limits, JsLimitsConfig::default());
    }

    #[test]
    fn languages_deserialises_both_engines() {
        let toml_str = r#"
            [rhai.limits]
            max-operations = 500000
            execution-timeout-ms = 5000

            [js.limits]
            execution-timeout-ms = 3000
            max-loop-iterations = 1000000
        "#;
        let cfg: LanguagesConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(cfg.rhai.limits.max_operations, Some(500_000));
        assert_eq!(cfg.rhai.limits.execution_timeout_ms, Some(5000));
        assert_eq!(cfg.js.limits.execution_timeout_ms, Some(3000));
        assert_eq!(cfg.js.limits.max_loop_iterations, Some(1_000_000));
    }

    #[test]
    fn languages_serde_round_trip() {
        let original = LanguagesConfig {
            rhai: RhaiEngineConfig {
                limits: RhaiLimitsConfig {
                    max_operations: Some(100_000),
                    ..Default::default()
                },
            },
            js: JsEngineConfig::default(),
        };
        let serialized = toml::to_string(&original).expect("serialize");
        let back: LanguagesConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(original, back);
    }
}
