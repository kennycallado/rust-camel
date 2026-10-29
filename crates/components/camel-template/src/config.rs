//! Tunable resource limits for the external template component (ADR-0047
//! Stage 2). Mirrors the `MinijinjaLimitsConfig` shape in
//! `crates/languages/camel-language-api/src/language_limits.rs` — all fields
//! are `Option` with `kebab-case` rename + `deny_unknown_fields` so a typo in
//! `Camel.toml` is caught at deserialize time.

use serde::{Deserialize, Serialize};

use crate::error::TemplateReloadError;

/// Default autoescape mode for the external `template:` component.
///
/// TOML values are the exact lowercase strings `"html"`, `"json"`, `"none"`
/// (uppercase and mixed case are rejected). Precedence is: an explicit
/// top-level `{% autoescape %}` block in the entry template wins, then this
/// config default applies, and if neither is present compilation is rejected.
///
/// The default applies to the external `template:` component entry only.
/// Inline `language: minijinja` sources keep their mandatory explicit block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoescapeMode {
    /// Escape for an HTML text/attribute context.
    Html,
    /// Escape for a JSON string context.
    Json,
    /// Do not escape (explicit opt-out).
    None,
}

impl AutoescapeMode {
    /// Canonical lowercase TOML spelling of this mode.
    ///
    /// Consumed by the compile-path autoescape wrapper that injects the
    /// configured default around a block-less entry.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Json => "json",
            Self::None => "none",
        }
    }
}

/// Parse the `[components.template] default-autoescape` TOML value.
///
/// Only the exact lowercase strings `"html"`, `"json"`, and `"none"` are
/// accepted. Any other string or non-string value returns an error that names
/// the key and the allowed values, with the offending value rendered as TOML.
pub fn autoescape_mode_from_toml(value: &toml::Value) -> Result<AutoescapeMode, String> {
    match value {
        toml::Value::String(s) => match s.as_str() {
            "html" => Ok(AutoescapeMode::Html),
            "json" => Ok(AutoescapeMode::Json),
            "none" => Ok(AutoescapeMode::None),
            _ => Err(invalid_autoescape_message(value)),
        },
        _ => Err(invalid_autoescape_message(value)),
    }
}

fn invalid_autoescape_message(value: &toml::Value) -> String {
    format!(
        "`components.template.default-autoescape` must be one of \"html\", \"json\", \"none\"; got {value}"
    )
}

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

    #[test]
    fn autoescape_mode_parses_each_value() {
        assert_eq!(
            autoescape_mode_from_toml(&toml::Value::String("html".into()))
                .expect("`html` must parse"),
            AutoescapeMode::Html
        );
        assert_eq!(
            autoescape_mode_from_toml(&toml::Value::String("json".into()))
                .expect("`json` must parse"),
            AutoescapeMode::Json
        );
        assert_eq!(
            autoescape_mode_from_toml(&toml::Value::String("none".into()))
                .expect("`none` must parse"),
            AutoescapeMode::None
        );
    }

    #[test]
    fn autoescape_mode_rejects_unknown_string() {
        let err = autoescape_mode_from_toml(&toml::Value::String("yaml".into()))
            .expect_err("unknown string must be rejected");
        for needle in [
            "default-autoescape",
            "\"html\"",
            "\"json\"",
            "\"none\"",
            "yaml",
        ] {
            assert!(err.contains(needle), "error must contain {needle:?}: {err}");
        }
    }

    #[test]
    fn autoescape_mode_rejects_uppercase() {
        let err = autoescape_mode_from_toml(&toml::Value::String("HTML".into()))
            .expect_err("uppercase must be rejected (exact lowercase only)");
        assert!(
            err.contains("default-autoescape"),
            "error must name the key: {err}"
        );
        for needle in ["\"html\"", "\"json\"", "\"none\""] {
            assert!(err.contains(needle), "error must contain {needle:?}: {err}");
        }
    }

    #[test]
    fn autoescape_mode_rejects_non_string() {
        for value in [toml::Value::Boolean(true), toml::Value::Integer(1)] {
            let err =
                autoescape_mode_from_toml(&value).expect_err("non-string value must be rejected");
            assert!(
                err.contains("default-autoescape"),
                "error must name the key: {err}"
            );
            for needle in ["\"html\"", "\"json\"", "\"none\""] {
                assert!(err.contains(needle), "error must contain {needle:?}: {err}");
            }
        }
    }

    #[test]
    fn autoescape_mode_as_str_roundtrip() {
        for mode in [
            AutoescapeMode::Html,
            AutoescapeMode::Json,
            AutoescapeMode::None,
        ] {
            let rendered = mode.as_str();
            assert_eq!(
                autoescape_mode_from_toml(&toml::Value::String(rendered.to_owned()))
                    .expect("`as_str` output must re-parse"),
                mode
            );
        }
    }
}
