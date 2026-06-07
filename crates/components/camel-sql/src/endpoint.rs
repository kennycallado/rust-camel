use std::sync::Arc;
use tokio::sync::OnceCell;

use camel_component_api::{BodyType, BoxProcessor, CamelError, RuntimeObservability};
use camel_component_api::{Consumer, Endpoint, ProducerContext};
use sqlx::AnyPool;

use crate::config::SqlEndpointConfig;
use crate::consumer::SqlConsumer;
use crate::producer::SqlProducer;

pub(crate) struct SqlEndpoint {
    uri: String,
    pub(crate) config: SqlEndpointConfig,
    pub(crate) pool: Arc<OnceCell<AnyPool>>,
}

impl SqlEndpoint {
    #[cfg(test)]
    pub fn new(uri: String, config: SqlEndpointConfig) -> Self {
        Self::new_with_pool(uri, config, Arc::new(OnceCell::new()))
    }

    pub fn new_with_pool(
        uri: String,
        config: SqlEndpointConfig,
        pool: Arc<OnceCell<AnyPool>>,
    ) -> Self {
        Self { uri, config, pool }
    }
}

impl Endpoint for SqlEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let route_id = ctx.route_id().unwrap_or("unknown").to_string();
        Ok(BoxProcessor::new(SqlProducer::new(
            self.config.clone(),
            Arc::clone(&self.pool),
            rt,
            route_id,
        )))
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(SqlConsumer::new(
            self.config.clone(),
            Arc::clone(&self.pool),
            rt,
        )))
    }

    fn body_contract(&self) -> Option<BodyType> {
        if !self.config.use_message_body_for_sql {
            return None;
        }
        if self.config.batch {
            Some(BodyType::Json)
        } else {
            Some(BodyType::Text)
        }
    }
}

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
    use super::*;
    use crate::config::SqlEndpointConfig;
    use camel_component_api::Endpoint;
    use camel_component_api::UriConfig;

    fn make_endpoint(use_body: bool, batch: bool) -> SqlEndpoint {
        let mut config =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test")
                .expect("valid SQL endpoint config");
        config.use_message_body_for_sql = use_body;
        config.batch = batch;

        SqlEndpoint::new("sql:select 1".to_string(), config)
    }

    #[test]
    fn body_contract_returns_some_text_when_body_mode_enabled_no_batch() {
        let endpoint = make_endpoint(true, false);
        assert_eq!(endpoint.body_contract(), Some(BodyType::Text));
    }

    #[test]
    fn body_contract_returns_some_json_when_batch_mode_enabled() {
        let endpoint = make_endpoint(true, true);
        assert_eq!(endpoint.body_contract(), Some(BodyType::Json));
    }

    #[test]
    fn body_contract_returns_none_when_body_mode_disabled() {
        let endpoint = make_endpoint(false, false);
        assert_eq!(endpoint.body_contract(), None);
    }

    #[test]
    fn body_contract_returns_none_when_batch_without_body_mode() {
        let endpoint = make_endpoint(false, true);
        assert_eq!(endpoint.body_contract(), None);
    }

    #[test]
    fn new_stores_uri_and_config() {
        let config = SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test")
            .expect("valid config");
        let endpoint = SqlEndpoint::new("sql:select 1".to_string(), config.clone());
        assert_eq!(endpoint.uri(), "sql:select 1");
        assert_eq!(endpoint.config.db_url, "postgres://localhost/test");
    }

    #[test]
    fn uri_returns_stored_uri() {
        let endpoint = make_endpoint(false, false);
        assert_eq!(endpoint.uri(), "sql:select 1");
    }

    #[test]
    fn create_producer_returns_ok() {
        let endpoint = make_endpoint(false, false);
        let ctx = ProducerContext::new();
        let result = endpoint.create_producer(test_rt(), &ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn create_consumer_returns_ok() {
        let endpoint = make_endpoint(false, false);
        let result = endpoint.create_consumer(test_rt());
        assert!(result.is_ok());
    }

    #[test]
    fn pool_is_shared_via_arc() {
        let config = SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test")
            .expect("valid config");
        let endpoint = SqlEndpoint::new("sql:select 1".to_string(), config);
        let pool_ref1 = Arc::clone(&endpoint.pool);
        let pool_ref2 = Arc::clone(&endpoint.pool);
        assert!(Arc::ptr_eq(&pool_ref1, &pool_ref2));
    }
}
