use camel_api::CamelError;

use crate::UriComponents;

/// Trait for configuration types that can be parsed from Camel URIs.
///
/// This trait is typically implemented via the `#[derive(UriConfig)]` macro
/// from `camel-endpoint-macros`.
pub trait UriConfig: Sized {
    /// Returns the URI scheme this config handles (e.g., "timer", "http").
    fn scheme() -> &'static str;

    /// Parse a URI string into this configuration.
    fn from_uri(uri: &str) -> Result<Self, CamelError>;

    /// Parse already-extracted URI components into this configuration.
    fn from_components(parts: UriComponents) -> Result<Self, CamelError>;

    /// Override to add validation logic after parsing.
    fn validate(self) -> Result<Self, CamelError> {
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trait_exists() {
        // Just verify the trait is defined
        fn _uses_trait<T: UriConfig>() {}
    }
}

#[cfg(test)]
mod derive_tests {
    use super::*;
    use crate::UriConfig;

    // Allow the derive macro to reference `camel_endpoint::` when used within this crate
    extern crate self as camel_endpoint;

    // Simple config with just path
    #[derive(Debug, Clone, UriConfig)]
    #[uri_scheme = "test"]
    struct SimpleConfig {
        name: String,
    }

    #[test]
    fn test_simple_path_extraction() {
        let config = SimpleConfig::from_uri("test:hello").unwrap();
        assert_eq!(config.name, "hello");
    }

    #[test]
    fn test_simple_scheme() {
        assert_eq!(SimpleConfig::scheme(), "test");
    }

    // Config with parameters and defaults
    #[derive(Debug, Clone, UriConfig)]
    #[uri_scheme = "test"]
    struct ConfigWithParams {
        name: String,
        #[uri_param(default = "1000")]
        timeout: u64,
        #[uri_param(default = "true")]
        enabled: bool,
    }

    #[test]
    fn test_params_with_defaults() {
        let config = ConfigWithParams::from_uri("test:foo?timeout=500").unwrap();
        assert_eq!(config.name, "foo");
        assert_eq!(config.timeout, 500);
        assert!(config.enabled); // uses default
    }

    #[test]
    fn test_params_all_specified() {
        let config = ConfigWithParams::from_uri("test:bar?timeout=2000&enabled=false").unwrap();
        assert_eq!(config.name, "bar");
        assert_eq!(config.timeout, 2000);
        assert!(!config.enabled);
    }

    #[test]
    fn test_scheme_validation() {
        let result = SimpleConfig::from_uri("wrong:hello");
        assert!(result.is_err());
        if let Err(CamelError::InvalidUri(msg)) = result {
            assert!(msg.contains("expected scheme 'test'"));
            assert!(msg.contains("got 'wrong'"));
        } else {
            panic!("Expected InvalidUri error");
        }
    }

    // Config with Option fields
    #[derive(Debug, Clone, UriConfig)]
    #[uri_scheme = "timer"]
    struct TimerConfig {
        timer_name: String,
        #[uri_param]
        period: Option<u64>,
        #[uri_param]
        repeat: Option<bool>,
        #[uri_param]
        description: Option<String>,
    }

    #[test]
    fn test_option_params_present() {
        let config =
            TimerConfig::from_uri("timer:tick?period=1000&repeat=true&description=hello").unwrap();
        assert_eq!(config.timer_name, "tick");
        assert_eq!(config.period, Some(1000));
        assert_eq!(config.repeat, Some(true));
        assert_eq!(config.description, Some("hello".to_string()));
    }

    #[test]
    fn test_option_params_absent() {
        let config = TimerConfig::from_uri("timer:tick").unwrap();
        assert_eq!(config.timer_name, "tick");
        assert_eq!(config.period, None);
        assert_eq!(config.repeat, None);
        assert_eq!(config.description, None);
    }

    // Config with custom param names
    #[derive(Debug, Clone, UriConfig)]
    #[uri_scheme = "http"]
    struct HttpConfig {
        url: String,
        #[uri_param(name = "httpMethod")]
        method: Option<String>,
        #[uri_param(name = "connectTimeout", default = "5000")]
        timeout_ms: u64,
    }

    #[test]
    fn test_custom_param_names() {
        let config =
            HttpConfig::from_uri("http://example.com?httpMethod=POST&connectTimeout=10000")
                .unwrap();
        assert_eq!(config.url, "//example.com");
        assert_eq!(config.method, Some("POST".to_string()));
        assert_eq!(config.timeout_ms, 10000);
    }

    #[test]
    fn test_custom_param_name_default() {
        let config = HttpConfig::from_uri("http://example.com").unwrap();
        assert_eq!(config.url, "//example.com");
        assert_eq!(config.method, None);
        assert_eq!(config.timeout_ms, 5000); // default
    }

    // Config with multiple numeric types
    #[derive(Debug, Clone, UriConfig)]
    #[uri_scheme = "data"]
    struct NumericConfig {
        path: String,
        #[uri_param(default = "100")]
        count_u32: u32,
        #[uri_param(default = "1000")]
        count_u64: u64,
        #[uri_param(default = "10")]
        count_usize: usize,
        #[uri_param(default = "-5")]
        offset_i32: i32,
    }

    #[test]
    fn test_numeric_types() {
        let config = NumericConfig::from_uri(
            "data:test?count_u32=50&count_u64=500&count_usize=5&offset_i32=-10",
        )
        .unwrap();
        assert_eq!(config.path, "test");
        assert_eq!(config.count_u32, 50);
        assert_eq!(config.count_u64, 500);
        assert_eq!(config.count_usize, 5);
        assert_eq!(config.offset_i32, -10);
    }

    #[test]
    fn test_numeric_defaults() {
        let config = NumericConfig::from_uri("data:test").unwrap();
        assert_eq!(config.count_u32, 100);
        assert_eq!(config.count_u64, 1000);
        assert_eq!(config.count_usize, 10);
        assert_eq!(config.offset_i32, -5);
    }

    #[test]
    fn test_invalid_numeric_value() {
        let result = NumericConfig::from_uri("data:test?count_u32=abc");
        assert!(result.is_err());
    }

    // Test from_components directly
    #[test]
    fn test_from_components() {
        let components = UriComponents {
            scheme: "test".to_string(),
            path: "hello".to_string(),
            params: std::collections::HashMap::from([
                ("timeout".to_string(), "500".to_string()),
                ("enabled".to_string(), "false".to_string()),
            ]),
        };

        let config = ConfigWithParams::from_components(components).unwrap();
        assert_eq!(config.name, "hello");
        assert_eq!(config.timeout, 500);
        assert!(!config.enabled);
    }

    // Test with validate
    #[test]
    fn test_validate_passthrough() {
        let config = SimpleConfig::from_uri("test:hello")
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(config.name, "hello");
    }

    // Test: Issue 1 - Non-Option bool without default should error when missing
    #[derive(Debug, Clone, UriConfig)]
    #[uri_scheme = "feature"]
    struct FeatureConfig {
        feature_name: String,
        #[uri_param]
        enabled: bool, // No default - should require the parameter
    }

    #[test]
    fn test_bool_without_default_missing_errors() {
        // Should error because 'enabled' is required but not provided
        let result = FeatureConfig::from_uri("feature:test");
        assert!(result.is_err());
        if let Err(CamelError::InvalidUri(msg)) = result {
            assert!(
                msg.contains("missing required parameter"),
                "Error should mention missing parameter, got: {}",
                msg
            );
            assert!(msg.contains("enabled"), "Error should mention 'enabled'");
        } else {
            panic!("Expected InvalidUri error for missing bool parameter");
        }
    }

    #[test]
    fn test_bool_without_default_provided_works() {
        let config = FeatureConfig::from_uri("feature:test?enabled=true").unwrap();
        assert!(config.enabled);
        let config = FeatureConfig::from_uri("feature:test?enabled=false").unwrap();
        assert!(!config.enabled);
    }

    // Test: Issue 2 - Option<u64> with invalid value should return None, not Some(0)
    #[test]
    fn test_option_numeric_invalid_returns_none() {
        // Invalid numeric value should result in None, not Some(0)
        let config = TimerConfig::from_uri("timer:tick?period=invalid").unwrap();
        assert_eq!(
            config.period, None,
            "Invalid numeric value should return None, not Some(0)"
        );
    }

    // Test: Issue 3 - Generic type fallback should include parse error in message
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestEnum {
        Alpha,
        Beta,
    }

    impl std::str::FromStr for TestEnum {
        type Err = String;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            match s {
                "alpha" => Ok(TestEnum::Alpha),
                "beta" => Ok(TestEnum::Beta),
                _ => Err(format!("unknown variant: {}", s)),
            }
        }
    }

    #[derive(Debug, Clone, UriConfig)]
    #[uri_scheme = "enumtest"]
    struct EnumConfig {
        path: String,
        #[uri_param]
        mode: TestEnum,
    }

    #[test]
    fn test_enum_invalid_value_includes_error() {
        let result = EnumConfig::from_uri("enumtest:foo?mode=invalid");
        assert!(result.is_err());
        if let Err(CamelError::InvalidUri(msg)) = result {
            assert!(
                msg.contains("invalid value"),
                "Error should mention invalid value, got: {}",
                msg
            );
            // The error should include the actual parse error from FromStr
            assert!(
                msg.contains("unknown variant"),
                "Error should include parse error details, got: {}",
                msg
            );
            assert!(
                msg.contains("invalid"),
                "Error should include the invalid value, got: {}",
                msg
            );
        } else {
            panic!("Expected InvalidUri error for invalid enum value");
        }
    }

    #[test]
    fn test_enum_valid_value_works() {
        let config = EnumConfig::from_uri("enumtest:foo?mode=alpha").unwrap();
        assert_eq!(config.mode, TestEnum::Alpha);
        let config = EnumConfig::from_uri("enumtest:foo?mode=beta").unwrap();
        assert_eq!(config.mode, TestEnum::Beta);
    }

    // Duration type support tests
    #[derive(Debug, Clone, UriConfig)]
    #[uri_scheme = "timer"]
    struct TimerTestConfig {
        name: String,

        #[uri_param(default = "1000")]
        period_ms: u64,

        period: std::time::Duration,
    }

    #[test]
    fn test_duration_from_ms_field() {
        let config = TimerTestConfig::from_uri("timer:tick?period_ms=500").unwrap();
        assert_eq!(config.name, "tick");
        assert_eq!(config.period, std::time::Duration::from_millis(500));
    }

    #[test]
    fn test_duration_uses_default() {
        let config = TimerTestConfig::from_uri("timer:tick").unwrap();
        assert_eq!(config.name, "tick");
        assert_eq!(config.period_ms, 1000);
        assert_eq!(config.period, std::time::Duration::from_millis(1000));
    }

    // Test Duration with multiple Duration fields
    #[derive(Debug, Clone, UriConfig)]
    #[uri_scheme = "scheduler"]
    struct SchedulerConfig {
        task_name: String,

        #[uri_param(default = "5000")]
        initial_delay_ms: u64,

        #[uri_param(default = "10000")]
        interval_ms: u64,

        initial_delay: std::time::Duration,
        interval: std::time::Duration,
    }

    #[test]
    fn test_multiple_duration_fields() {
        let config =
            SchedulerConfig::from_uri("scheduler:cleanup?initial_delay_ms=2000&interval_ms=3000")
                .unwrap();
        assert_eq!(config.task_name, "cleanup");
        assert_eq!(config.initial_delay, std::time::Duration::from_millis(2000));
        assert_eq!(config.interval, std::time::Duration::from_millis(3000));
    }

    #[test]
    fn test_multiple_duration_defaults() {
        let config = SchedulerConfig::from_uri("scheduler:cleanup").unwrap();
        assert_eq!(config.task_name, "cleanup");
        assert_eq!(config.initial_delay, std::time::Duration::from_millis(5000));
        assert_eq!(config.interval, std::time::Duration::from_millis(10000));
    }
}
