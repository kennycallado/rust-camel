//! Format-aware annotation for parse + lower error paths.
//!
//! Public parse entry points wrap their `Result` through [`annotate_format`] so
//! that errors name the user's input format. Every `CamelError::RouteError(msg)`
//! is prefixed with `"{fmt} DSL error: "` (uniformly — deserializer errors
//! become `"YAML DSL error: YAML parse error: ..."`). Idempotent guard prevents
//! double-prefixing when an error flows through multiple annotated boundaries.

use camel_api::CamelError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputFormat {
    Yaml,
    Json,
}

impl InputFormat {
    pub(crate) fn name(self) -> &'static str {
        match self {
            InputFormat::Yaml => "YAML",
            InputFormat::Json => "JSON",
        }
    }
}

/// Prefix `CamelError::RouteError(msg)` with `"{fmt} DSL error: "` unless the
/// message is already annotated. Idempotent across repeated calls.
pub(crate) fn annotate_format<T>(
    fmt: InputFormat,
    result: Result<T, CamelError>,
) -> Result<T, CamelError> {
    match result {
        Ok(v) => Ok(v),
        Err(err) => {
            let annotated = match &err {
                CamelError::RouteError(msg) => {
                    if msg.starts_with("YAML DSL error:") || msg.starts_with("JSON DSL error:") {
                        err
                    } else {
                        CamelError::RouteError(format!("{} DSL error: {}", fmt.name(), msg))
                    }
                }
                _ => err,
            };
            Err(annotated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_returns_canonical_strings() {
        assert_eq!(InputFormat::Yaml.name(), "YAML");
        assert_eq!(InputFormat::Json.name(), "JSON");
    }

    #[test]
    fn annotate_prefixes_route_error_yaml() {
        let err = CamelError::RouteError("route 'id' must not be empty".into());
        let result: Result<(), CamelError> = annotate_format(InputFormat::Yaml, Err(err));
        let annotated = result.unwrap_err().to_string();
        assert!(
            annotated.contains("YAML DSL error: route 'id' must not be empty"),
            "got: {annotated}"
        );
    }

    #[test]
    fn annotate_prefixes_route_error_json() {
        let err = CamelError::RouteError("to: URI must not be empty".into());
        let result: Result<(), CamelError> = annotate_format(InputFormat::Json, Err(err));
        let annotated = result.unwrap_err().to_string();
        assert!(
            annotated.contains("JSON DSL error: to: URI must not be empty"),
            "got: {annotated}"
        );
    }

    #[test]
    fn annotate_is_idempotent() {
        let err = CamelError::RouteError("YAML DSL error: existing prefix".into());
        let result: Result<(), CamelError> = annotate_format(InputFormat::Yaml, Err(err));
        let annotated = result.unwrap_err().to_string();
        // The CamelError display format prefixes with "Route error: "
        // so we check for the full expected prefix chain
        assert!(
            annotated.contains("Route error: YAML DSL error: existing prefix"),
            "got: {annotated}"
        );
        // No double prefix
        assert!(!annotated.contains("YAML DSL error: YAML DSL error:"));
    }

    #[test]
    fn annotate_skips_non_route_errors() {
        let err = CamelError::Io("disk full".into());
        let result: Result<(), CamelError> = annotate_format(InputFormat::Json, Err(err));
        let annotated = result.unwrap_err().to_string();
        // I/O error not transformed
        assert!(annotated.contains("disk full"));
        assert!(!annotated.contains("DSL error"));
    }

    #[test]
    fn annotate_passes_through_ok() {
        let result: Result<i32, CamelError> = annotate_format(InputFormat::Yaml, Ok(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn annotate_prefixes_parse_errors_uniformly() {
        // Deserializer errors ("YAML parse error:") are also prefixed.
        let err = CamelError::RouteError("YAML parse error: invalid at line 1".into());
        let result: Result<(), CamelError> = annotate_format(InputFormat::Yaml, Err(err));
        let annotated = result.unwrap_err().to_string();
        assert!(
            annotated.contains("YAML DSL error: YAML parse error: invalid at line 1"),
            "expected uniform double-prefix for parse errors, got: {annotated}"
        );
    }

    #[test]
    fn annotate_leaves_existing_opposite_format_untouched() {
        let err = CamelError::RouteError("JSON DSL error: route 'id' must not be empty".into());
        let result: Result<(), CamelError> = annotate_format(InputFormat::Yaml, Err(err));
        let annotated = result.unwrap_err().to_string();
        // Opposite format prefix is preserved (not re-prefixed with YAML).
        assert!(annotated.contains("JSON DSL error:"));
        assert!(!annotated.contains("YAML DSL error:"));
    }
}
