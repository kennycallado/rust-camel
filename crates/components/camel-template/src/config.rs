//! Tunable resource limits for the external template component (ADR-0047
//! Stage 2). Mirrors the `MinijinjaLimitsConfig` shape in
//! `crates/languages/camel-language-api/src/language_limits.rs` — all fields
//! are `Option` with `kebab-case` rename + `deny_unknown_fields` so a typo in
//! `Camel.toml` is caught at deserialize time.

use serde::{Deserialize, Serialize};

use crate::error::TemplateReloadError;

/// Operator-facing limits block. `None` means "use the rust-camel default".
///
/// Surfaced in `Camel.toml` as:
///
/// ```toml
/// [components.template.limits]
/// max-total-source-bytes = 16777216
/// max-include-count = 64
/// max-include-depth = 16
/// max-template-size = 1048576
/// reload-timeout-ms = 5000
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExternalTemplateLimitsConfig {
    /// Maximum total source bytes across the dependency closure of a single
    /// template set. Defaults to 16 MiB when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_source_bytes: Option<usize>,

    /// Maximum number of included/imported templates per closure.
    /// Defaults to 64 when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_include_count: Option<u32>,

    /// Maximum include/extends nesting depth. Defaults to 16 when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_include_depth: Option<u32>,

    /// Maximum size of a single template file in bytes. Defaults to 1 MiB when
    /// unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_template_size: Option<usize>,

    /// Wall-clock budget for a full reload build, in milliseconds. Defaults to
    /// 5000 ms when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload_timeout_ms: Option<u64>,
}

/// Resolved, finite, non-zero limits. Built once at startup from
/// `ExternalTemplateLimitsConfig::resolve()`; the hot path reads this struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedExternalTemplateLimits {
    pub max_total_source_bytes: usize,
    pub max_include_count: u32,
    pub max_include_depth: u32,
    pub max_template_size: usize,
    pub reload_timeout_ms: u64,
}

impl ExternalTemplateLimitsConfig {
    /// Fold operator-supplied values to finite, non-zero defaults. Any
    /// explicitly-set `Some(0)` is rejected (zero would silently disable the
    /// bound); `None` substitutes the documented default.
    pub fn resolve(&self) -> Result<ResolvedExternalTemplateLimits, TemplateReloadError> {
        if let Some(0) = self.max_total_source_bytes {
            return Err(TemplateReloadError::BoundExceeded(
                "zero value not permitted",
            ));
        }
        if let Some(0) = self.max_include_count {
            return Err(TemplateReloadError::BoundExceeded(
                "zero value not permitted",
            ));
        }
        if let Some(0) = self.max_include_depth {
            return Err(TemplateReloadError::BoundExceeded(
                "zero value not permitted",
            ));
        }
        if let Some(0) = self.max_template_size {
            return Err(TemplateReloadError::BoundExceeded(
                "zero value not permitted",
            ));
        }
        if let Some(0) = self.reload_timeout_ms {
            return Err(TemplateReloadError::BoundExceeded(
                "zero value not permitted",
            ));
        }

        Ok(ResolvedExternalTemplateLimits {
            max_total_source_bytes: self.max_total_source_bytes.unwrap_or(16 * 1024 * 1024),
            max_include_count: self.max_include_count.unwrap_or(64),
            max_include_depth: self.max_include_depth.unwrap_or(16),
            max_template_size: self.max_template_size.unwrap_or(1024 * 1024),
            reload_timeout_ms: self.reload_timeout_ms.unwrap_or(5000),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_resolve_defaults() {
        let cfg = ExternalTemplateLimitsConfig::default();
        let resolved = cfg.resolve().expect("default config resolves");
        assert_eq!(resolved.max_total_source_bytes, 16 * 1024 * 1024);
        assert_eq!(resolved.max_include_count, 64);
        assert_eq!(resolved.max_include_depth, 16);
        assert_eq!(resolved.max_template_size, 1024 * 1024);
        assert_eq!(resolved.reload_timeout_ms, 5000);
    }

    #[test]
    fn limits_reject_zero() {
        let cfg = ExternalTemplateLimitsConfig {
            max_include_count: Some(0),
            ..Default::default()
        };
        let err = cfg.resolve().expect_err("zero bound must be rejected");
        assert!(matches!(err, TemplateReloadError::BoundExceeded(_)));
    }

    #[test]
    fn deny_unknown_fields() {
        let toml = "max-include-count = 3\nbogus = 1\n";
        let result: Result<ExternalTemplateLimitsConfig, _> = toml::from_str(toml);
        assert!(result.is_err(), "deny_unknown_fields must reject `bogus`");
    }
}
