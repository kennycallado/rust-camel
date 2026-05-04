use std::sync::Arc;

use camel_component_api::{
    BoxProcessor, CamelError, Component, Consumer, Endpoint, ProducerContext,
};

use crate::config::CxfEndpointConfig;
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

impl Endpoint for CxfEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(CxfConsumer::new(
            Arc::clone(&self.pool),
            self.endpoint_config_service_config(),
        )))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        let operation = self.endpoint_config.operation.clone().unwrap_or_default();
        Ok(BoxProcessor::new(CxfProducer::new(
            Arc::clone(&self.pool),
            self.endpoint_config_service_config(),
            operation,
        )))
    }
}

impl CxfEndpoint {
    fn endpoint_config_service_config(&self) -> crate::config::CxfServiceConfig {
        let security = self.pool.find_security_config(
            &self.endpoint_config.wsdl_path,
            &self.endpoint_config.service_name,
            &self.endpoint_config.port_name,
        );

        crate::config::CxfServiceConfig {
            address: Some(self.endpoint_config.address.clone()),
            wsdl_path: self.endpoint_config.wsdl_path.clone(),
            service_name: self.endpoint_config.service_name.clone(),
            port_name: self.endpoint_config.port_name.clone(),
            security,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CxfPoolConfig;

    fn test_pool() -> Arc<CxfBridgePool> {
        Arc::new(
            CxfBridgePool::from_config(CxfPoolConfig {
                services: vec![],
                max_bridges: 1,
                bridge_start_timeout_ms: 5_000,
                health_check_interval_ms: 5_000,
                bridge_cache_dir: None,
                version: "0.1.0".to_string(),
            })
            .unwrap(),
        )
    }

    #[test]
    fn component_scheme_is_cxf() {
        let pool = test_pool();
        let component = CxfComponent::new(pool);
        assert_eq!(component.scheme(), "cxf");
    }

    #[test]
    fn create_endpoint_valid_uri() {
        let pool = test_pool();
        let component = CxfComponent::new(pool);
        let result = component.create_endpoint(
            "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort",
            &camel_component_api::NoOpComponentContext,
        );
        assert!(result.is_ok(), "got: {:?}", result.err());
    }

    #[test]
    fn create_endpoint_with_operation() {
        let pool = test_pool();
        let component = CxfComponent::new(pool);
        let endpoint = component
            .create_endpoint(
                "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort&operation=sayHello",
                &camel_component_api::NoOpComponentContext,
            )
            .unwrap();
        assert_eq!(
            endpoint.uri(),
            "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort&operation=sayHello"
        );
    }

    #[test]
    fn create_endpoint_rejects_wrong_scheme() {
        let pool = test_pool();
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
        let pool = test_pool();
        let component = CxfComponent::new(pool);
        let endpoint = component
            .create_endpoint(
                "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort",
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
        let pool = test_pool();
        let component = CxfComponent::new(pool);
        let endpoint = component
            .create_endpoint(
                "cxf://http://host:8080/service?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort",
                &camel_component_api::NoOpComponentContext,
            )
            .unwrap();
        let result = endpoint.create_consumer();
        assert!(result.is_ok(), "got: {:?}", result.err());
    }
}
