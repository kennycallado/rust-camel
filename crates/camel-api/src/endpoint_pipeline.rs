use std::sync::Arc;

use crate::BoxProcessor;

pub const CAMEL_SLIP_ENDPOINT: &str = "CamelSlipEndpoint";

pub type EndpointResolver = Arc<dyn Fn(&str) -> Option<BoxProcessor> + Send + Sync>;

#[derive(Clone)]
pub struct EndpointPipelineConfig {
    pub cache_size: usize,
    pub ignore_invalid_endpoints: bool,
}

impl EndpointPipelineConfig {
    pub fn from_signed(n: i32) -> usize {
        if n <= 0 { 0 } else { n as usize }
    }
}

impl Default for EndpointPipelineConfig {
    fn default() -> Self {
        Self {
            cache_size: 1000,
            ignore_invalid_endpoints: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_signed_positive() {
        assert_eq!(EndpointPipelineConfig::from_signed(42), 42);
    }

    #[test]
    fn from_signed_zero() {
        assert_eq!(EndpointPipelineConfig::from_signed(0), 0);
    }

    #[test]
    fn from_signed_negative() {
        assert_eq!(EndpointPipelineConfig::from_signed(-5), 0);
    }

    #[test]
    fn default_values() {
        let cfg = EndpointPipelineConfig::default();
        assert_eq!(cfg.cache_size, 1000);
        assert!(!cfg.ignore_invalid_endpoints);
    }

    #[test]
    fn clone_preserves_values() {
        let cfg = EndpointPipelineConfig {
            cache_size: 50,
            ignore_invalid_endpoints: true,
        };
        let cloned = cfg.clone();
        assert_eq!(cloned.cache_size, 50);
        assert!(cloned.ignore_invalid_endpoints);
    }

    #[test]
    fn constant_value() {
        assert_eq!(CAMEL_SLIP_ENDPOINT, "CamelSlipEndpoint");
    }
}
