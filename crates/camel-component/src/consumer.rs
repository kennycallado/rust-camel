use async_trait::async_trait;
use tokio::sync::mpsc;

use camel_api::{CamelError, Exchange};

/// Context provided to a Consumer, allowing it to send exchanges into the route.
#[derive(Clone)]
pub struct ConsumerContext {
    sender: mpsc::Sender<Exchange>,
}

impl ConsumerContext {
    /// Create a new consumer context wrapping the given channel sender.
    pub fn new(sender: mpsc::Sender<Exchange>) -> Self {
        Self { sender }
    }

    /// Send an exchange into the route pipeline.
    pub async fn send(&self, exchange: Exchange) -> Result<(), CamelError> {
        self.sender
            .send(exchange)
            .await
            .map_err(|_| CamelError::ChannelClosed)
    }
}

/// A Consumer receives messages from an external source and feeds them into the route.
#[async_trait]
pub trait Consumer: Send + Sync {
    /// Start consuming messages, sending them through the provided context.
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError>;

    /// Stop consuming messages.
    async fn stop(&mut self) -> Result<(), CamelError>;
}
