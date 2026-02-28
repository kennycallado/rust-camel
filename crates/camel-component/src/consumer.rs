use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use camel_api::{CamelError, Exchange};

/// A message sent from a consumer to the route pipeline.
///
/// Fire-and-forget exchanges use `reply_tx = None`.
/// Request-reply exchanges (e.g. `direct:`) provide a `reply_tx` so the
/// pipeline result can be sent back to the consumer.
pub struct ExchangeEnvelope {
    pub exchange: Exchange,
    pub reply_tx: Option<oneshot::Sender<Result<Exchange, CamelError>>>,
}

/// Context provided to a Consumer, allowing it to send exchanges into the route.
#[derive(Clone)]
pub struct ConsumerContext {
    sender: mpsc::Sender<ExchangeEnvelope>,
    cancel_token: CancellationToken,
}

impl ConsumerContext {
    /// Create a new consumer context wrapping the given channel sender.
    pub fn new(sender: mpsc::Sender<ExchangeEnvelope>, cancel_token: CancellationToken) -> Self {
        Self {
            sender,
            cancel_token,
        }
    }

    /// Returns a future that resolves when shutdown is requested.
    /// Use in `tokio::select!` inside consumer loops.
    pub async fn cancelled(&self) {
        self.cancel_token.cancelled().await
    }

    /// Returns true if shutdown has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Send an exchange into the route pipeline (fire-and-forget).
    pub async fn send(&self, exchange: Exchange) -> Result<(), CamelError> {
        self.sender
            .send(ExchangeEnvelope {
                exchange,
                reply_tx: None,
            })
            .await
            .map_err(|_| CamelError::ChannelClosed)
    }

    /// Send an exchange and wait for the pipeline result (request-reply).
    ///
    /// Returns `Ok(exchange)` on success or `Err(e)` if the pipeline failed
    /// without an error handler absorbing the error.
    pub async fn send_and_wait(&self, exchange: Exchange) -> Result<Exchange, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender
            .send(ExchangeEnvelope {
                exchange,
                reply_tx: Some(reply_tx),
            })
            .await
            .map_err(|_| CamelError::ChannelClosed)?;
        reply_rx.await.map_err(|_| CamelError::ChannelClosed)?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_consumer_context_cancelled() {
        let (tx, _rx) = mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        assert!(!ctx.is_cancelled());
        token.cancel();
        ctx.cancelled().await;
        assert!(ctx.is_cancelled());
    }
}
