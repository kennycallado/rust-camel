use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::OnceCell;
use sqlx::AnyPool;

use camel_api::CamelError;
use camel_component::{ConcurrencyModel, Consumer, ConsumerContext};

use crate::config::SqlConfig;

pub struct SqlConsumer {
    pub(crate) config: SqlConfig,
    pub(crate) pool: Arc<OnceCell<AnyPool>>,
}

impl SqlConsumer {
    pub fn new(config: SqlConfig, pool: Arc<OnceCell<AnyPool>>) -> Self {
        Self { config, pool }
    }
}

#[async_trait]
impl Consumer for SqlConsumer {
    async fn start(&mut self, _context: ConsumerContext) -> Result<(), CamelError> {
        // TODO: Full implementation in Task 7
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}
