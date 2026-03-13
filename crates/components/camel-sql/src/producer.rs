use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::OnceCell;
use tower::Service;
use sqlx::AnyPool;

use camel_api::{CamelError, Exchange};
use crate::config::SqlConfig;

#[derive(Clone)]
pub struct SqlProducer {
    pub(crate) config: SqlConfig,
    pub(crate) pool: Arc<OnceCell<AnyPool>>,
}

impl SqlProducer {
    pub fn new(config: SqlConfig, pool: Arc<OnceCell<AnyPool>>) -> Self {
        Self { config, pool }
    }
}

impl Service<Exchange> for SqlProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        Box::pin(async move {
            // TODO: Full implementation in Task 6
            Ok(exchange)
        })
    }
}
