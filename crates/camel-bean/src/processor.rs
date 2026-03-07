use async_trait::async_trait;
use camel_api::{CamelError, Exchange};

#[async_trait]
pub trait BeanProcessor: Send + Sync {
    async fn call(&self, method: &str, exchange: &mut Exchange) -> Result<(), CamelError>;

    fn methods(&self) -> Vec<&'static str>;
}
