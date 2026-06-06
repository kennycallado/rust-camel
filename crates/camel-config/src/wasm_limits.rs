//! Shared WASM runtime limits surfaced through `Camel.toml`.
//!
//! Embedded by [`BeanConfig`](crate::config::BeanConfig) and
//! [`PermissionProviderConfig`](crate::config::PermissionProviderConfig) so that
//! every WASM consumer (Processor endpoint URI, Bean, AuthorizationPolicy,
//! SecurityPolicy) accepts the same knobs. Processor continues to parse its
//! knobs from the `wasm:` URI query string; non-Processor types read this
//! struct from `Camel.toml`.
//!
//! All fields are `Option`; `None` means "use the runtime default" — there is
//! no silent default lie here (per ADR-0011).

use serde::{Deserialize, Serialize};

/// Tunable WASM runtime limits for a single plugin instance.
///
/// Surfaced in `Camel.toml` as:
///
/// ```toml
/// [default.beans.my-bean]
/// plugin = "my-plugin"
/// [default.beans.my-bean.limits]
/// timeout-secs = 600
/// max-memory = 4294967296   # 4 GiB
/// max-concurrent-calls = 1
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WasmLimitsConfig {
    /// Maximum execution time per guest call, in seconds.
    /// Maps to `WasmConfig::timeout_secs` when `Some`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,

    /// Maximum linear memory the guest can allocate, in bytes.
    /// Maps to `WasmConfig::max_memory_bytes` when `Some`.
    ///
    /// Note: when `None`, the runtime default (50 MiB) is used and IS enforced
    /// via `wasmtime::StoreLimitsBuilder::memory_size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory: Option<u64>,

    /// Maximum concurrent guest invocations against this plugin instance.
    /// Maps to `WasmConfig::max_concurrent_calls` when `Some`.
    /// Only meaningful for Processor plugins (Bean/AuthzPolicy/SecurityPolicy
    /// are not concurrent-safe across reentrant calls today).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_calls: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_all_none() {
        let cfg = WasmLimitsConfig::default();
        assert_eq!(cfg.timeout_secs, None);
        assert_eq!(cfg.max_memory, None);
        assert_eq!(cfg.max_concurrent_calls, None);
    }

    #[test]
    fn deserialises_full_block() {
        let toml = toml::toml! {
            timeout-secs = 600
            max-memory = 4294967296i64
            max-concurrent-calls = 2
        };
        let cfg: WasmLimitsConfig = toml.try_into().expect("deserialize");
        assert_eq!(cfg.timeout_secs, Some(600));
        assert_eq!(cfg.max_memory, Some(4_294_967_296));
        assert_eq!(cfg.max_concurrent_calls, Some(2));
    }

    #[test]
    fn deserialises_partial_block() {
        let toml = toml::toml! {
            timeout-secs = 120
        };
        let cfg: WasmLimitsConfig = toml.try_into().expect("deserialize");
        assert_eq!(cfg.timeout_secs, Some(120));
        assert_eq!(cfg.max_memory, None);
        assert_eq!(cfg.max_concurrent_calls, None);
    }

    #[test]
    fn rejects_unknown_field() {
        let toml = toml::toml! {
            timeout-secs = 10
            fuel = 1000
        };
        let result: Result<WasmLimitsConfig, _> = toml.try_into();
        assert!(result.is_err(), "deny_unknown_fields must reject `fuel`");
    }

    #[test]
    fn serde_round_trip_preserves_set_fields() {
        let original = WasmLimitsConfig {
            timeout_secs: Some(45),
            max_memory: Some(64 * 1024 * 1024),
            max_concurrent_calls: None,
        };
        let serialized = toml::to_string(&original).expect("serialize");
        let back: WasmLimitsConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn skip_serializing_none_fields() {
        let cfg = WasmLimitsConfig {
            timeout_secs: Some(30),
            max_memory: None,
            max_concurrent_calls: None,
        };
        let s = toml::to_string(&cfg).expect("serialize");
        assert!(s.contains("timeout-secs"));
        assert!(!s.contains("max-memory"));
        assert!(!s.contains("max-concurrent-calls"));
    }
}
