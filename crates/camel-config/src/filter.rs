use camel_api::CamelError;
use tracing_subscriber::EnvFilter;

pub(crate) fn is_raw_env_filter(s: &str) -> bool {
    s.contains(',') || s.contains('=') || s.contains('[') || s.contains("::")
}

pub(crate) fn expand_bare_level(level: &str) -> Option<&'static str> {
    match level.to_lowercase().as_str() {
        "trace" => Some("warn,camel=trace,camel_tracer=off"),
        "debug" => Some("warn,camel=debug,camel_tracer=off"),
        "info" => Some("warn,camel=info,camel_tracer=off"),
        "warn" | "warning" => Some("warn,camel=warn,camel_tracer=off"),
        "error" => Some("error,camel=error,camel_tracer=off"),
        "off" => Some("off"),
        _ => None,
    }
}

pub(crate) fn inject_camel_tracer_off(filter_str: &str) -> String {
    if filter_str.contains("camel_tracer") {
        filter_str.to_string()
    } else {
        format!("{filter_str},camel_tracer=off")
    }
}

pub(crate) fn build_env_filter(
    log_level: &str,
    rust_log: Option<&str>,
) -> Result<EnvFilter, CamelError> {
    if let Some(rust_log_val) = rust_log {
        let filter_str = inject_camel_tracer_off(rust_log_val);
        return EnvFilter::try_new(&filter_str).map_err(|e| {
            CamelError::Config(format!(
                "Invalid RUST_LOG environment variable '{}': {e}",
                rust_log_val
            ))
        });
    }

    let filter_str = if is_raw_env_filter(log_level) {
        inject_camel_tracer_off(log_level)
    } else {
        match expand_bare_level(log_level) {
            Some(expanded) => expanded.to_string(),
            None => {
                return Err(CamelError::Config(format!(
                    "Unknown log level '{}' in Camel.toml. Expected: trace, debug, info, warn, error, off, or a full EnvFilter directive.",
                    log_level
                )));
            }
        }
    };

    EnvFilter::try_new(&filter_str).map_err(|e| {
        CamelError::Config(format!(
            "Invalid log_level '{}' in Camel.toml: {e}",
            log_level
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_env_filter_bare_levels_expand_correctly() {
        let f = build_env_filter("trace", None).unwrap();
        let s = f.to_string().replace(' ', "");
        assert!(s.contains("camel=trace"), "trace: {s}");

        let f = build_env_filter("debug", None).unwrap();
        assert!(f.to_string().contains("camel=debug"));

        let f = build_env_filter("info", None).unwrap();
        assert!(f.to_string().contains("camel=info"));

        let f = build_env_filter("warn", None).unwrap();
        assert!(f.to_string().contains("camel=warn"));

        let f = build_env_filter("error", None).unwrap();
        assert!(f.to_string().contains("camel=error"));
    }

    #[test]
    fn build_env_filter_unknown_bare_level_returns_error() {
        let result = build_env_filter("infp", None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Unknown log level"), "msg: {msg}");
    }

    #[test]
    fn build_env_filter_raw_directive_parses_as_is() {
        let f = build_env_filter("warn,camel=info,sqlx=warn", None).unwrap();
        let s = f.to_string().replace(' ', "");
        assert!(s.contains("camel=info"));
        assert!(s.contains("sqlx=warn"));
        assert!(s.contains("camel_tracer=off"));
    }

    #[test]
    fn build_env_filter_raw_directive_preserves_user_camel_tracer() {
        let f = build_env_filter("info,camel_tracer=debug", None).unwrap();
        let s = f.to_string().replace(' ', "");
        assert!(s.contains("camel_tracer=debug"));
        assert!(!s.contains("camel_tracer=off"));
    }

    #[test]
    fn build_env_filter_invalid_returns_config_error() {
        let result = build_env_filter("info,bad===value", None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid log_level"));
    }

    #[test]
    fn build_env_filter_is_case_insensitive() {
        let f = build_env_filter("TRACE", None).unwrap();
        assert!(f.to_string().contains("camel=trace"));

        let f = build_env_filter("DeBuG", None).unwrap();
        assert!(f.to_string().contains("camel=debug"));
    }

    #[test]
    fn build_env_filter_empty_string_returns_error() {
        let result = build_env_filter("", None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Unknown log level"), "msg: {msg}");
    }

    #[test]
    fn build_env_filter_warning_alias_expands_to_warn() {
        let f = build_env_filter("warning", None).unwrap();
        assert!(f.to_string().contains("camel=warn"));
    }

    #[test]
    fn build_env_filter_rust_log_overrides_config() {
        let f = build_env_filter("error", Some("my_crate=debug")).unwrap();
        let s = f.to_string().replace(' ', "");
        assert!(s.contains("my_crate=debug"));
        assert!(s.contains("camel_tracer=off"));
    }

    #[test]
    fn build_env_filter_rust_log_invalid_returns_config_error() {
        let result = build_env_filter("info", Some("bad===value"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Invalid RUST_LOG"));
    }

    #[test]
    fn build_env_filter_rust_log_preserves_user_camel_tracer() {
        let f = build_env_filter("error", Some("debug,camel_tracer=info")).unwrap();
        let s = f.to_string().replace(' ', "");
        assert!(s.contains("camel_tracer=info"));
        assert!(!s.contains("camel_tracer=off"));
    }

    #[test]
    fn build_env_filter_bare_levels_include_warn_default_and_camel_tracer_off() {
        let f = build_env_filter("debug", None).unwrap();
        let s = f.to_string();
        assert!(s.contains("warn"), "expected warn global default: {s}");
        assert!(
            s.contains("camel_tracer=off"),
            "expected camel_tracer silenced: {s}"
        );
    }

    #[test]
    fn build_env_filter_off_bare_level() {
        let f = build_env_filter("off", None).unwrap();
        let s = f.to_string().replace(' ', "");
        assert_eq!(s, "off");
    }
}
