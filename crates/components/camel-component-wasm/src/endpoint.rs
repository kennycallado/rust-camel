use std::path::PathBuf;
use std::sync::Arc;

use camel_api::CamelError;
use camel_component_api::{BoxProcessor, ComponentContext, Endpoint, ProducerContext};

use crate::config::WasmConfig;

pub struct WasmEndpoint {
    uri: String,
    module_path: PathBuf,
    registry: Arc<dyn ComponentContext>,
    config: WasmConfig,
}

impl WasmEndpoint {
    pub fn new(
        uri: String,
        module_path: PathBuf,
        registry: Arc<dyn ComponentContext>,
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

    fn create_consumer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
        let guest_config = crate::source_consumer::parse_guest_config(&self.uri);
        let consumer = crate::source_consumer::WasmSourceConsumer::new(
            self.module_path.clone(),
            self.uri.clone(),
            self.config.clone(),
            guest_config,
            self.registry.clone(),
        );
        Ok(Box::new(consumer))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let producer = crate::producer::WasmProducer::new(
            self.module_path.clone(),
            self.registry.clone(),
            self.config.clone(),
            rt,
        );
        Ok(BoxProcessor::new(producer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_endpoint_stores_config() {
        let config = WasmConfig {
            timeout_secs: 10,
            max_memory_bytes: 1024 * 1024,
            max_concurrent_calls: 4,
            ..WasmConfig::default()
        };
        let endpoint = WasmEndpoint::new(
            "wasm:test.wasm?timeout=10&max-memory=1048576".to_string(),
            PathBuf::from("test.wasm"),
            Arc::new(camel_component_api::NoOpComponentContext),
            config.clone(),
        );
        assert_eq!(endpoint.config().timeout_secs, 10);
        assert_eq!(endpoint.config().max_memory_bytes, 1024 * 1024);
    }
}
