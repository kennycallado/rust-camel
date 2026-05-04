use std::sync::Arc;
use tokio::sync::OnceCell;

use camel_component_api::{BodyType, BoxProcessor, CamelError};
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
    pub fn new(uri: String, config: SqlEndpointConfig) -> Self {
        Self {
            uri,
            config,
            pool: Arc::new(OnceCell::new()),
        }
    }
}

impl Endpoint for SqlEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(SqlProducer::new(
            self.config.clone(),
            Arc::clone(&self.pool),
        )))
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(SqlConsumer::new(
            self.config.clone(),
            Arc::clone(&self.pool),
        )))
    }

    fn body_contract(&self) -> Option<BodyType> {
        if self.config.use_message_body_for_sql {
            Some(BodyType::Text)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SqlEndpointConfig;
    use camel_component_api::Endpoint;
    use camel_component_api::UriConfig;

    fn make_endpoint(use_body: bool) -> SqlEndpoint {
        let mut config =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test")
                .expect("valid SQL endpoint config");
        config.use_message_body_for_sql = use_body;

        SqlEndpoint::new("sql:select 1".to_string(), config)
    }

    #[test]
    fn body_contract_returns_some_text_when_body_mode_enabled() {
        let endpoint = make_endpoint(true);
        assert_eq!(endpoint.body_contract(), Some(BodyType::Text));
    }

    #[test]
    fn body_contract_returns_none_when_body_mode_disabled() {
        let endpoint = make_endpoint(false);
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
        let endpoint = make_endpoint(false);
        assert_eq!(endpoint.uri(), "sql:select 1");
    }

    #[test]
    fn create_producer_returns_ok() {
        let endpoint = make_endpoint(false);
        let ctx = ProducerContext::new();
        let result = endpoint.create_producer(&ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn create_consumer_returns_ok() {
        let endpoint = make_endpoint(false);
        let result = endpoint.create_consumer();
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
