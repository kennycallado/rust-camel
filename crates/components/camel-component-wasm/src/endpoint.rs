use std::path::PathBuf;
use std::sync::Arc;

use camel_api::CamelError;
use camel_component_api::{BoxProcessor, Endpoint, ProducerContext};
use camel_core::Registry;

use crate::config::WasmConfig;

pub struct WasmEndpoint {
    uri: String,
    module_path: PathBuf,
    registry: Arc<std::sync::Mutex<Registry>>,
    config: WasmConfig,
}

impl WasmEndpoint {
    pub fn new(
        uri: String,
        module_path: PathBuf,
        registry: Arc<std::sync::Mutex<Registry>>,
        config: WasmConfig,
    ) -> Self {
        Self {
            uri,
            module_path,
            registry,
            config,
        }
    }

    pub fn config(&self) -> &WasmConfig {
        &self.config
    }
}

impl Endpoint for WasmEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "WASM consumer (from: wasm:...) is not supported in v1".to_string(),
        ))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        let producer = crate::producer::WasmProducer::new(
            self.module_path.clone(),
            self.registry.clone(),
            self.config.clone(),
        );
        Ok(BoxProcessor::new(producer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn test_wasm_endpoint_stores_config() {
        let config = WasmConfig {
            timeout_secs: 10,
            max_memory_bytes: 1024 * 1024,
            max_concurrent_calls: 4,
        };
        let endpoint = WasmEndpoint::new(
            "wasm:test.wasm?timeout=10&max-memory=1048576".to_string(),
            PathBuf::from("test.wasm"),
            Arc::new(Mutex::new(Registry::new())),
            config.clone(),
        );
        assert_eq!(endpoint.config().timeout_secs, 10);
        assert_eq!(endpoint.config().max_memory_bytes, 1024 * 1024);
    }
}
