use std::path::PathBuf;
use std::sync::Arc;

use camel_api::CamelError;
use camel_component_api::{BoxProcessor, Endpoint, ProducerContext};
use camel_core::Registry;

pub struct WasmEndpoint {
    uri: String,
    module_path: PathBuf,
    registry: Arc<std::sync::Mutex<Registry>>,
}

impl WasmEndpoint {
    pub fn new(
        uri: String,
        module_path: PathBuf,
        registry: Arc<std::sync::Mutex<Registry>>,
    ) -> Self {
        Self {
            uri,
            module_path,
            registry,
        }
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
        );
        Ok(BoxProcessor::new(producer))
    }
}
