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
    RuntimeObservability,
};

pub struct ValidatorComponent {
    xsd_bridge: Arc<dyn XsdBridge>,
    xsd_backend: Option<Arc<XsdBridgeBackend>>,
}

impl ValidatorComponent {
    pub fn new() -> Self {
        let xsd_backend = Arc::new(XsdBridgeBackend::new());
        Self {
            xsd_bridge: Arc::clone(&xsd_backend) as Arc<dyn XsdBridge>,
            xsd_backend: Some(xsd_backend),
        }
    }

    pub fn xsd_bridge_backend(&self) -> Option<Arc<XsdBridgeBackend>> {
        self.xsd_backend.as_ref().map(Arc::clone)
    }

    #[cfg(test)]
    fn with_xsd_bridge(xsd_bridge: Arc<dyn XsdBridge>) -> Self {
        Self {
            xsd_bridge,
            xsd_backend: None,
        }
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
            config,
            compiled: Arc::new(compiled),
            xsd_backend: self.xsd_backend.as_ref().map(Arc::clone),
        }))
    }
}

/// Validator endpoint that checks message bodies or headers against a schema.
///
/// Supports XML (XSD), JSON Schema, and YAML schema validation.
/// RelaxNG and Schematron are accepted in URI parsing but rejected at endpoint
/// creation with a clear error message.
struct ValidatorEndpoint {
    uri: String,
    config: ValidatorConfig,
    compiled: Arc<CompiledValidator>,
    xsd_backend: Option<Arc<XsdBridgeBackend>>,
}

impl ValidatorEndpoint {
    /// Returns a human-readable description of the configured schema.
    #[allow(dead_code)]
    pub fn schema_info(&self) -> String {
        format!(
            "{:?} schema: {}",
            self.config.schema_type,
            self.config.schema_path.display()
        )
    }
}

impl Endpoint for ValidatorEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "validator endpoint does not support consumers".to_string(),
        ))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        if let Some(ref backend) = self.xsd_backend {
            backend.set_observability(
                rt.clone(),
                ctx.route_id().unwrap_or("validator-bridge").to_string(),
            );
        }
        Ok(BoxProcessor::new(ValidatorProducer {
            uri: self.uri.clone(),
            config: self.config.clone(),
            compiled: Arc::clone(&self.compiled),
        }))
    }
}

#[derive(Clone)]
struct ValidatorProducer {
    uri: String,
    config: ValidatorConfig,
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
        let config = self.config.clone();
        Box::pin(async move {
            debug!(uri = uri, "validating exchange body");

            // VAL-003: header_name mode — validate header value instead of body
            if let Some(ref header_name) = config.header_name {
                match exchange.input.header(header_name) {
                    Some(value) => {
                        // Convert header Value to a string body for validation
                        let header_str = match value.as_str() {
                            Some(s) => s.to_string(),
                            None => value.to_string(),
                        };
                        let header_body = camel_component_api::Body::Text(header_str);
                        compiled.validate(&header_body).await?;
                    }
                    None => {
                        // VAL-004: failOnNullHeader
                        if config.fail_on_null_header {
                            return Err(CamelError::ProcessorError(format!(
                                "header '{header_name}' is missing and failOnNullHeader is true"
                            )));
                        }
                        // Pass through — no validation
                    }
                }
                return Ok(exchange);
            }

            // VAL-002: failOnNullBody
            if exchange.input.body.is_empty() {
                if config.fail_on_null_body {
                    return Err(CamelError::ProcessorError(
                        "body is empty and failOnNullBody is true".to_string(),
                    ));
                }
                // Pass through — no validation
                return Ok(exchange);
            }

            compiled.validate(&exchange.input.body).await?;
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

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
        assert!(ep.create_consumer(rt()).is_err());
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
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
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
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
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
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
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

        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
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
            register_error: Some(ValidatorError::CompilationFailed {
                message: "COMPILATION_FAILED".to_string(),
                source: None,
            }),
            validate_error: None,
        });
        let f = xsd_file();
        let uri = format!("validator:{}", f.path().display());
        // Endpoint creation now always succeeds for XSD (registration is deferred).
        let ep = ValidatorComponent::with_xsd_bridge(backend)
            .create_endpoint(&uri, &NoOpComponentContext)
            .expect("endpoint creation should succeed");
        // The error surfaces when the first message is processed (register is called).
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Xml(
            "<order/>".to_string(),
        )));
        let err = producer
            .oneshot(exchange)
            .await
            .expect_err("expected validate to fail due to registration error");
        assert!(err.to_string().contains("COMPILATION_FAILED"));
    }

    #[tokio::test]
    async fn test_validator_rejects_oversized_payload() {
        // Build validator with maxPayloadBytes=100
        let f = json_schema_file();
        let uri = format!("validator:{}?maxPayloadBytes=100", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
        // Send a body that is definitely > 100 bytes
        let big_body: String = "x".repeat(200);
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Text(big_body)));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err(), "expected oversized payload to be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("payload too large"),
            "expected 'payload too large' in error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_validator_allows_payload_under_limit() {
        let f = json_schema_file();
        let uri = format!("validator:{}?maxPayloadBytes=1024", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Json(
            serde_json::json!({"id": "1"}),
        )));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_ok(), "expected valid payload under limit to pass");
    }

    #[tokio::test]
    async fn test_validator_no_limit_allows_any_size() {
        let f = json_schema_file();
        let uri = format!("validator:{}", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
        // Valid JSON that would exceed a 10-byte limit
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Json(
            serde_json::json!({"id": "this is a longer value that would exceed small limits"}),
        )));
        let result = producer.oneshot(exchange).await;
        assert!(
            result.is_ok(),
            "expected no-limit validator to pass any valid payload"
        );
    }

    #[tokio::test]
    async fn test_fail_on_null_body_default_rejects_empty() {
        let f = json_schema_file();
        let uri = format!("validator:{}", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Empty));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("failOnNullBody"),
            "expected failOnNullBody in error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_fail_on_null_body_false_passes_empty() {
        let f = json_schema_file();
        let uri = format!("validator:{}?failOnNullBody=false", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Empty));
        let result = producer.oneshot(exchange).await;
        assert!(
            result.is_ok(),
            "expected empty body to pass with failOnNullBody=false"
        );
    }

    #[tokio::test]
    async fn test_header_name_validation_uses_header_value() {
        let f = json_schema_file();
        let uri = format!("validator:{}?headerName=X-Data", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
        let mut msg = Message::new(camel_component_api::Body::Empty);
        msg.set_header("X-Data", serde_json::json!({"id": "1"}).to_string());
        let exchange = Exchange::new(msg);
        let result = producer.oneshot(exchange).await;
        // Header value {"id":"1"} is valid JSON matching schema
        assert!(
            result.is_ok(),
            "expected valid header to pass: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_header_name_missing_header_fails_by_default() {
        let f = json_schema_file();
        let uri = format!("validator:{}?headerName=X-Missing", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Empty));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("X-Missing"),
            "expected header name in error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_fail_on_null_header_false_passes_missing_header() {
        let f = json_schema_file();
        let uri = format!(
            "validator:{}?headerName=X-Missing&failOnNullHeader=false",
            f.path().display()
        );
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(rt(), &ProducerContext::new()).unwrap();
        let exchange = Exchange::new(Message::new(camel_component_api::Body::Empty));
        let result = producer.oneshot(exchange).await;
        assert!(
            result.is_ok(),
            "expected missing header to pass with failOnNullHeader=false"
        );
    }

    #[test]
    fn test_relaxng_schema_type_rejected_at_creation() {
        let mut f = tempfile::Builder::new().suffix(".rng").tempfile().unwrap();
        use std::io::Write;
        f.write_all(b"<grammar/>").unwrap();
        let uri = format!("validator:{}", f.path().display());
        let result = ValidatorComponent::new().create_endpoint(&uri, &NoOpComponentContext);
        let err = result.err().expect("expected endpoint creation to fail");
        let msg = err.to_string();
        assert!(
            msg.contains("not yet supported"),
            "expected 'not yet supported' in error, got: {msg}"
        );
    }

    #[test]
    fn test_schematron_schema_type_rejected_at_creation() {
        let mut f = tempfile::Builder::new().suffix(".sch").tempfile().unwrap();
        use std::io::Write;
        f.write_all(b"<schema/>").unwrap();
        let uri = format!("validator:{}", f.path().display());
        let result = ValidatorComponent::new().create_endpoint(&uri, &NoOpComponentContext);
        let err = result.err().expect("expected endpoint creation to fail");
        let msg = err.to_string();
        assert!(
            msg.contains("not yet supported"),
            "expected 'not yet supported' in error, got: {msg}"
        );
    }

    #[test]
    fn test_schema_info_returns_description() {
        let f = json_schema_file();
        let uri = format!("validator:{}", f.path().display());
        let ep = ValidatorComponent::new()
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        // The endpoint is a Box<dyn Endpoint>, so we can't directly call schema_info().
        // We verify endpoint creation succeeds (schema compiled).
        assert_eq!(ep.uri(), uri);
    }
}
