use std::sync::Arc;

use camel_component_api::{
    BoxProcessor, CamelError, Component, Consumer, Endpoint, ProducerContext,
};

use crate::config::{CxfEndpointConfig, CxfProfileConfig};
use crate::consumer::CxfConsumer;
use crate::pool::CxfBridgePool;
use crate::producer::CxfProducer;

// ── CxfComponent ─────────────────────────────────────────────────────────────

pub struct CxfComponent {
    pool: Arc<CxfBridgePool>,
}

impl CxfComponent {
    pub fn new(pool: Arc<CxfBridgePool>) -> Self {
        Self { pool }
    }
}

impl Component for CxfComponent {
    fn scheme(&self) -> &str {
        "cxf"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = CxfEndpointConfig::from_uri(uri)?;
        Ok(Box::new(CxfEndpoint {
            pool: Arc::clone(&self.pool),
            uri: uri.to_string(),
            endpoint_config,
        }))
    }
}

// ── CxfEndpoint ──────────────────────────────────────────────────────────────

struct CxfEndpoint {
    pool: Arc<CxfBridgePool>,
    uri: String,
    endpoint_config: CxfEndpointConfig,
}

impl CxfEndpoint {
    fn resolve_profile(&self) -> Result<CxfProfileConfig, CamelError> {
        let profile_name = self.endpoint_config.profile.as_ref().ok_or_else(|| {
            CamelError::ProcessorError("cxf URI requires 'profile' query parameter".to_string())
        })?;

        self.pool
            .configured_profiles
            .iter()
            .find(|p| p.name == *profile_name)
            .cloned()
            .ok_or_else(|| {
                CamelError::ProcessorError(format!("unknown profile '{}'", profile_name))
            })
    }
}

impl Endpoint for CxfEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        let profile = self.resolve_profile()?;
        Ok(Box::new(CxfConsumer::new(
            Arc::clone(&self.pool),
            profile.name.clone(),
        )))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        let profile = self.resolve_profile()?;
        let operation = self.endpoint_config.operation.clone().unwrap_or_default();
        // The URI's address is the SOAP target endpoint and overrides the
        // profile-level fallback. The parser guarantees endpoint_config.address
        // is non-empty.
        let address = if self.endpoint_config.address.is_empty() {
            profile.address.clone()
        } else {
            Some(self.endpoint_config.address.clone())
        };
        Ok(BoxProcessor::new(CxfProducer::new(
            Arc::clone(&self.pool),
            profile.name.clone(),
            profile.wsdl_path.clone(),
            profile.service_name.clone(),
            profile.port_name.clone(),
            address,
            operation,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CxfPoolConfig, CxfProfileConfig};

    fn test_pool_with_profiles() -> Arc<CxfBridgePool> {
        Arc::new(
            CxfBridgePool::from_config(CxfPoolConfig {
                profiles: vec![CxfProfileConfig {
                    name: "test".to_string(),
                    address: Some("http://host:8080/service".to_string()),
                    wsdl_path: "/wsdl/hello.wsdl".to_string(),
                    service_name: "HelloService".to_string(),
                    port_name: "HelloPort".to_string(),
                    security: Default::default(),
                }],
                max_bridges: 1,
                bridge_start_timeout_ms: 5_000,
                health_check_interval_ms: 5_000,
                bridge_cache_dir: None,
                version: "0.1.0".to_string(),
                bind_address: None,
            })
            .unwrap(),
        )
    }

    #[test]
    fn component_scheme_is_cxf() {
        let pool = test_pool_with_profiles();
        let component = CxfComponent::new(pool);
        assert_eq!(component.scheme(), "cxf");
    }

    #[test]
    fn create_endpoint_valid_uri() {
        let pool = test_pool_with_profiles();
        let component = CxfComponent::new(pool);
        let result = component.create_endpoint(
            "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort&profile=test",
            &camel_component_api::NoOpComponentContext,
        );
        assert!(result.is_ok(), "got: {:?}", result.err());
    }

    #[test]
    fn create_endpoint_with_operation() {
        let pool = test_pool_with_profiles();
        let component = CxfComponent::new(pool);
        let endpoint = component
            .create_endpoint(
                "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort&operation=sayHello&profile=test",
                &camel_component_api::NoOpComponentContext,
            )
            .unwrap();
        assert_eq!(
            endpoint.uri(),
            "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort&operation=sayHello&profile=test"
        );
    }

    #[test]
    fn create_endpoint_rejects_wrong_scheme() {
        let pool = test_pool_with_profiles();
        let component = CxfComponent::new(pool);
        let err = component
            .create_endpoint(
                "jms:queue:orders",
                &camel_component_api::NoOpComponentContext,
            )
            .err()
            .unwrap();
        assert!(
            err.to_string().contains("expected scheme 'cxf://'"),
            "got: {err}"
        );
    }

    #[test]
    fn create_producer_returns_processor() {
        let pool = test_pool_with_profiles();
        let component = CxfComponent::new(pool);
        let endpoint = component
            .create_endpoint(
                "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort&profile=test",
                &camel_component_api::NoOpComponentContext,
            )
            .unwrap();
        let result = endpoint.create_producer(&ProducerContext::default());
        assert!(
            result.is_ok(),
            "create_producer must return Ok(BoxProcessor)"
        );
    }

    #[test]
    fn create_consumer_returns_box_consumer() {
        let pool = test_pool_with_profiles();
        let component = CxfComponent::new(pool);
        let endpoint = component
            .create_endpoint(
                "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort&profile=test",
                &camel_component_api::NoOpComponentContext,
            )
            .unwrap();
        let result = endpoint.create_consumer();
        assert!(result.is_ok(), "got: {:?}", result.err());
    }

    #[test]
    fn create_producer_rejects_missing_profile() {
        let pool = test_pool_with_profiles();
        let component = CxfComponent::new(pool);
        let endpoint = component
            .create_endpoint(
                "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort",
                &camel_component_api::NoOpComponentContext,
            )
            .unwrap();
        let result = endpoint.create_producer(&ProducerContext::default());
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("profile"),
            "should mention profile"
        );
    }

    #[test]
    fn create_producer_rejects_unknown_profile() {
        let pool = test_pool_with_profiles();
        let component = CxfComponent::new(pool);
        let endpoint = component
            .create_endpoint(
                "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort&profile=nonexistent",
                &camel_component_api::NoOpComponentContext,
            )
            .unwrap();
        let result = endpoint.create_producer(&ProducerContext::default());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("unknown profile"),
            "should mention unknown profile, got: {err_msg}"
        );
    }
}
