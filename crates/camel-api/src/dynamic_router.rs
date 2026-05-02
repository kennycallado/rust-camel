use std::sync::Arc;
use std::time::Duration;

use crate::Exchange;

pub type RouterExpression = Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>;

#[derive(Clone)]
pub struct DynamicRouterConfig {
    pub expression: RouterExpression,
    pub uri_delimiter: String,
    pub cache_size: i32,
    pub ignore_invalid_endpoints: bool,
    pub max_iterations: usize,
    pub timeout: Option<Duration>,
}

impl DynamicRouterConfig {
    pub fn new(expression: RouterExpression) -> Self {
        Self {
            expression,
            uri_delimiter: ",".to_string(),
            cache_size: 1000,
            ignore_invalid_endpoints: false,
            max_iterations: 1000,
            timeout: Some(Duration::from_secs(60)),
        }
    }

    pub fn uri_delimiter(mut self, d: impl Into<String>) -> Self {
        self.uri_delimiter = d.into();
        self
    }

    pub fn cache_size(mut self, n: i32) -> Self {
        self.cache_size = n;
        self
    }

    pub fn ignore_invalid_endpoints(mut self, ignore: bool) -> Self {
        self.ignore_invalid_endpoints = ignore;
        self
    }

    pub fn max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }

    pub fn no_timeout(mut self) -> Self {
        self.timeout = None;
        self
    }
}

impl std::fmt::Debug for DynamicRouterConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicRouterConfig")
            .field("uri_delimiter", &self.uri_delimiter)
            .field("cache_size", &self.cache_size)
            .field("ignore_invalid_endpoints", &self.ignore_invalid_endpoints)
            .field("max_iterations", &self.max_iterations)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    fn noop_expr() -> RouterExpression {
        Arc::new(|_| None)
    }

    #[test]
    fn new_has_defaults() {
        let cfg = DynamicRouterConfig::new(noop_expr());
        assert_eq!(cfg.uri_delimiter, ",");
        assert_eq!(cfg.cache_size, 1000);
        assert!(!cfg.ignore_invalid_endpoints);
        assert_eq!(cfg.max_iterations, 1000);
        assert_eq!(cfg.timeout, Some(Duration::from_secs(60)));
    }

    #[test]
    fn builder_chaining() {
        let cfg = DynamicRouterConfig::new(noop_expr())
            .uri_delimiter(";")
            .cache_size(100)
            .ignore_invalid_endpoints(true)
            .max_iterations(50)
            .timeout(Duration::from_secs(10));
        assert_eq!(cfg.uri_delimiter, ";");
        assert_eq!(cfg.cache_size, 100);
        assert!(cfg.ignore_invalid_endpoints);
        assert_eq!(cfg.max_iterations, 50);
        assert_eq!(cfg.timeout, Some(Duration::from_secs(10)));
    }

    #[test]
    fn no_timeout_clears() {
        let cfg = DynamicRouterConfig::new(noop_expr()).no_timeout();
        assert!(cfg.timeout.is_none());
    }

    #[test]
    fn debug_format_contains_fields() {
        let cfg = DynamicRouterConfig::new(noop_expr());
        let debug = format!("{cfg:?}");
        assert!(debug.contains("DynamicRouterConfig"));
        assert!(debug.contains("uri_delimiter"));
        assert!(debug.contains("cache_size"));
    }

    #[test]
    fn clone_preserves_values() {
        let cfg = DynamicRouterConfig::new(noop_expr())
            .uri_delimiter("|")
            .cache_size(42);
        let cloned = cfg.clone();
        assert_eq!(cloned.uri_delimiter, "|");
        assert_eq!(cloned.cache_size, 42);
    }
}
