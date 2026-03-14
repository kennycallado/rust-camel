use std::sync::Arc;
use tokio::sync::OnceCell;

use camel_api::{BoxProcessor, CamelError};
use camel_component::{Consumer, Endpoint, ProducerContext};
use sqlx::AnyPool;

use crate::config::SqlConfig;
use crate::consumer::SqlConsumer;
use crate::producer::SqlProducer;

pub(crate) struct SqlEndpoint {
    uri: String,
    pub(crate) config: SqlConfig,
    pub(crate) pool: Arc<OnceCell<AnyPool>>,
}

impl SqlEndpoint {
    pub fn new(uri: String, config: SqlConfig) -> Self {
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
}
