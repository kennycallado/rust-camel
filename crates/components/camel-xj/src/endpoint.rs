use crate::component::XjBridgeRuntime;
use crate::config::Direction;
use crate::producer::XjProducer;
use camel_component_api::{BoxProcessor, CamelError, Consumer, Endpoint, ProducerContext};
use camel_xslt::StylesheetId;
use std::sync::Arc;

pub struct XjEndpoint {
    uri: String,
    stylesheet_id: StylesheetId,
    params: Vec<(String, String)>,
    client: Arc<camel_xslt::XsltBridgeClient>,
    runtime: Arc<XjBridgeRuntime>,
    direction: Direction,
}

impl XjEndpoint {
    pub fn new(
        uri: String,
        stylesheet_id: StylesheetId,
        params: Vec<(String, String)>,
        client: Arc<camel_xslt::XsltBridgeClient>,
        runtime: Arc<XjBridgeRuntime>,
        direction: Direction,
    ) -> Self {
        Self {
            uri,
            stylesheet_id,
            params,
            client,
            runtime,
            direction,
        }
    }
}

impl Endpoint for XjEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "xj endpoint does not support consumers".to_string(),
        ))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(XjProducer::new(
            self.stylesheet_id.clone(),
            self.params.clone(),
            Arc::clone(&self.client),
            Arc::clone(&self.runtime),
            self.direction,
        )))
    }
}
