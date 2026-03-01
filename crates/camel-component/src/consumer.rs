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

    /// Returns a clone of the `CancellationToken`.
    ///
    /// Useful for consumers that spawn per-request tasks and need to propagate
    /// shutdown to each task. See `HttpConsumer` for an example.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Returns a clone of the channel sender for manual exchange submission.
    ///
    /// Useful for consumers that spawn per-request tasks (e.g., `HttpConsumer`)
    /// where each task independently sends exchanges into the pipeline.
    /// For simple consumers, prefer `send()` or `send_and_wait()` instead.
    pub fn sender(&self) -> mpsc::Sender<ExchangeEnvelope> {
        self.sender.clone()
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

/// How a consumer's exchanges should be processed by the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConcurrencyModel {
    /// Exchanges are processed one at a time, in order. Default for polling
    /// consumers (timer, file) and synchronous consumers (direct).
    Sequential,
    /// Exchanges are processed concurrently via `tokio::spawn`. Optional
    /// semaphore limit (`max`). `None` means unbounded (channel buffer is
    /// the only backpressure).
    Concurrent { max: Option<usize> },
}

/// A Consumer receives messages from an external source and feeds them into the route.
#[async_trait]
pub trait Consumer: Send + Sync {
    /// Start consuming messages, sending them through the provided context.
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError>;

    /// Stop consuming messages.
    async fn stop(&mut self) -> Result<(), CamelError>;

    /// Declares this consumer's natural concurrency model.
    ///
    /// The runtime uses this to decide whether to process exchanges
    /// sequentially or spawn per-exchange. Consumers that accept inbound
    /// connections (HTTP, WebSocket, Kafka) should override this to return
    /// `ConcurrencyModel::Concurrent`.
    ///
    /// Default: `Sequential`.
    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
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

    #[test]
    fn test_concurrency_model_default_is_sequential() {
        use super::ConcurrencyModel;

        struct DummyConsumer;

        #[async_trait::async_trait]
        impl super::Consumer for DummyConsumer {
            async fn start(&mut self, _ctx: super::ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let consumer = DummyConsumer;
        assert_eq!(consumer.concurrency_model(), ConcurrencyModel::Sequential);
    }

    #[test]
    fn test_concurrency_model_concurrent_override() {
        use super::ConcurrencyModel;

        struct ConcurrentConsumer;

        #[async_trait::async_trait]
        impl super::Consumer for ConcurrentConsumer {
            async fn start(&mut self, _ctx: super::ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn concurrency_model(&self) -> ConcurrencyModel {
                ConcurrencyModel::Concurrent { max: Some(16) }
            }
        }

        let consumer = ConcurrentConsumer;
        assert_eq!(
            consumer.concurrency_model(),
            ConcurrencyModel::Concurrent { max: Some(16) }
        );
    }
}
