use crate::client::{StylesheetId, XsltBridgeClient};
use crate::component::XsltBridgeRuntime;
use crate::producer::XsltProducer;
use camel_component_api::{BoxProcessor, CamelError, Consumer, Endpoint, ProducerContext};
use std::sync::Arc;
use tokio::sync::OnceCell;

pub struct XsltEndpoint {
    uri: String,
    stylesheet_bytes: Vec<u8>,
    compiled: Arc<OnceCell<StylesheetId>>,
    params: Vec<(String, String)>,
    output_method: Option<String>,
    client: Arc<XsltBridgeClient>,
    runtime: Arc<XsltBridgeRuntime>,
}

impl XsltEndpoint {
    pub fn new(
        uri: String,
        stylesheet_bytes: Vec<u8>,
        params: Vec<(String, String)>,
        output_method: Option<String>,
        client: Arc<XsltBridgeClient>,
        runtime: Arc<XsltBridgeRuntime>,
    ) -> Self {
        Self {
            uri,
            stylesheet_bytes,
            compiled: Arc::new(OnceCell::new()),
            params,
            output_method,
            client,
            runtime,
        }
    }
}

impl Endpoint for XsltEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "xslt endpoint does not support consumers".to_string(),
        ))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(XsltProducer::new(
            self.stylesheet_bytes.clone(),
            Arc::clone(&self.compiled),
            self.params.clone(),
            self.output_method.clone(),
            Arc::clone(&self.client),
            Arc::clone(&self.runtime),
        )))
    }
}
