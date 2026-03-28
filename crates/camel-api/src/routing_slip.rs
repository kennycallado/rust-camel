use std::sync::Arc;

use crate::Exchange;

pub type RoutingSlipExpression = Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>;

#[derive(Clone)]
pub struct RoutingSlipConfig {
    pub expression: RoutingSlipExpression,
    pub uri_delimiter: String,
    pub cache_size: i32,
    pub ignore_invalid_endpoints: bool,
}

impl RoutingSlipConfig {
    pub fn new(expression: RoutingSlipExpression) -> Self {
        Self {
            expression,
            uri_delimiter: ",".to_string(),
            cache_size: 1000,
            ignore_invalid_endpoints: false,
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
}

impl std::fmt::Debug for RoutingSlipConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutingSlipConfig")
            .field("uri_delimiter", &self.uri_delimiter)
            .field("cache_size", &self.cache_size)
            .field("ignore_invalid_endpoints", &self.ignore_invalid_endpoints)
            .finish()
    }
}
