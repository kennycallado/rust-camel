use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;
use tracing::debug;

use crate::compiled::CompiledValidator;
use crate::config::ValidatorConfig;
use crate::xsd_bridge::{XsdBridge, XsdBridgeBackend};
use camel_component_api::ComponentContext;
use camel_component_api::{
    BoxProcessor, CamelError, Component, Consumer, Endpoint, Exchange, ProducerContext,
};

pub struct ValidatorComponent {
    xsd_bridge: Arc<dyn XsdBridge>,
}

impl ValidatorComponent {
    pub fn new() -> Self {
        Self {
            xsd_bridge: Arc::new(XsdBridgeBackend::new()),
        }
    }

    #[cfg(test)]
    fn with_xsd_bridge(xsd_bridge: Arc<dyn XsdBridge>) -> Self {
        Self { xsd_bridge }
    }
}

impl Default for ValidatorComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for ValidatorComponent {
    fn scheme(&self) -> &str {
        "validator"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = ValidatorConfig::from_uri(uri)?;
        let xsd_bridge = Arc::clone(&self.xsd_bridge);
        let compiled = CompiledValidator::compile(&config, xsd_bridge)?;
        Ok(Box::new(ValidatorEndpoint {
            uri: uri.to_string(),
            compiled: Arc::new(compiled),
        }))
    }
}

struct ValidatorEndpoint {
    uri: String,
    compiled: Arc<CompiledValidator>,
}

impl Endpoint for ValidatorEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "validator endpoint does not support consumers".to_string(),
        ))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(ValidatorProducer {
            uri: self.uri.clone(),
            compiled: Arc::clone(&self.compiled),
        }))
    }
}

#[derive(Clone)]
struct ValidatorProducer {
    uri: String,
    compiled: Arc<CompiledValidator>,
}

impl Service<Exchange> for ValidatorProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let compiled = Arc::clone(&self.compiled);
        let uri = self.uri.clone();
        Box::pin(async move {
            debug!(uri = uri, "validating exchange body");
            compiled.validate(&exchange.input.body).await?;
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ValidatorError;
    use async_trait::async_trait;
    use camel_component_api::{Message, NoOpComponentContext};
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    fn json_schema_file() -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
        f.write_all(br#"{"type":"object","required":["id"]}"#)
            .unwrap();
        f
    }

    fn xsd_file() -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new().suffix(".xsd").tempfile().unwrap();
        f.write_all(
            br#"<?xml version="1.0"?><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="order" type="xs:string"/></xs:schema>"#,
        ).unwrap();
        f
    }

    #[derive(Debug)]
    struct MockXsdBridge {
        register_calls: AtomicUsize,
        validate_calls: AtomicUsize,
        register_error: Option<ValidatorError>,
        validate_error: Option<ValidatorError>,
    }

    #[async_trait]
    impl XsdBridge for MockXsdBridge {
        async fn register(&self, _xsd_bytes: Vec<u8>) -> Result<String, ValidatorError> {
            self.register_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(err) = &self.register_error {
                return Err(err.clone());
            }
            Ok("xsd-mock-id".to_string())
        }

        async fn validate(
            &self,
            _schema_id: &str,
            _doc_bytes: Vec<u8>,
        ) -> Result<(), ValidatorError> {
            self.validate_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(err) = &self.validate_error {
                return Err(err.clone());
            }
            Ok(())
        }
    }

    #[test]
    fn scheme_is_validator() {
        assert_eq!(ValidatorComponent::new().scheme(), "validator");
    }

    #[test]
    fn consumer_not_supported() {
        let f = json_schema_file();
        let uri = format!("validator:{}", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        assert!(ep.create_consumer().is_err());
    }

    #[test]
    fn endpoint_creation_fails_for_missing_schema() {
        let result = ValidatorComponent::new()
            .create_endpoint("validator:/nonexistent/schema.json", &NoOpComponentContext);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn valid_json_body_passes_through() {
        let f = json_schema_file();
        let uri = format!("validator:{}", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(&ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Json(
            serde_json::json!({"id": "1"}),
        )));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn invalid_json_body_returns_err() {
        let f = json_schema_file();
        let uri = format!("validator:{}", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(&ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Json(
            serde_json::json!({"name": "x"}),
        )));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("validation failed"), "got: {msg}");
    }

    #[tokio::test]
    async fn valid_xml_body_passes() {
        let backend = Arc::new(MockXsdBridge {
            register_calls: AtomicUsize::new(0),
            validate_calls: AtomicUsize::new(0),
            register_error: None,
            validate_error: None,
        });
        let f = xsd_file();
        let uri = format!("validator:{}", f.path().display());
        let ep = ValidatorComponent::with_xsd_bridge(backend)
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(&ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Xml(
            "<order>hello</order>".to_string(),
        )));
        assert!(producer.oneshot(exchange).await.is_ok());
    }

    #[tokio::test]
    async fn xsd_bridge_register_and_validate_mock() {
        let backend = Arc::new(MockXsdBridge {
            register_calls: AtomicUsize::new(0),
            validate_calls: AtomicUsize::new(0),
            register_error: None,
            validate_error: None,
        });

        let f = xsd_file();
        let uri = format!("validator:{}", f.path().display());
        let ep = ValidatorComponent::with_xsd_bridge(Arc::clone(&backend) as Arc<dyn XsdBridge>)
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();

        let producer = ep.create_producer(&ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Xml(
            "<order>ok</order>".to_string(),
        )));
        assert!(producer.oneshot(exchange).await.is_ok());
        assert_eq!(backend.register_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.validate_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn xsd_bridge_register_error_propagates_on_validate() {
        let backend = Arc::new(MockXsdBridge {
            register_calls: AtomicUsize::new(0),
            validate_calls: AtomicUsize::new(0),
            register_error: Some(ValidatorError::CompilationFailed(
                "COMPILATION_FAILED".to_string(),
            )),
            validate_error: None,
        });
        let f = xsd_file();
        let uri = format!("validator:{}", f.path().display());
        // Endpoint creation now always succeeds for XSD (registration is deferred).
        let ep = ValidatorComponent::with_xsd_bridge(backend)
            .create_endpoint(&uri, &NoOpComponentContext)
            .expect("endpoint creation should succeed");
        // The error surfaces when the first message is processed (register is called).
        let producer = ep.create_producer(&ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Xml(
            "<order/>".to_string(),
        )));
        let err = producer
            .oneshot(exchange)
            .await
            .expect_err("expected validate to fail due to registration error");
        assert!(err.to_string().contains("COMPILATION_FAILED"));
    }
}
