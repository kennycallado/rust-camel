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
