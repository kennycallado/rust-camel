use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Service, ServiceExt};

use camel_api::{CamelError, Exchange};

#[derive(Clone)]
pub struct WireTapService {
    tap_endpoint: camel_api::BoxProcessor,
}

impl WireTapService {
    pub fn new(tap_endpoint: camel_api::BoxProcessor) -> Self {
        Self { tap_endpoint }
    }
}

impl Service<Exchange> for WireTapService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.tap_endpoint.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let mut tap_endpoint = self.tap_endpoint.clone();
        let tap_exchange = exchange.clone();

        tokio::spawn(async move {
            if let Err(e) = tap_endpoint.ready().await {
                tracing::warn!("WireTap endpoint poll_ready failed: {}", e);
                return;
            }
            if let Err(e) = tap_endpoint.call(tap_exchange).await {
                tracing::error!("WireTap processing error: {}", e);
            }
        });

        Box::pin(async move { Ok(exchange) })
    }
}

/// A Tower layer that produces `WireTapService` instances.
pub struct WireTapLayer {
    tap_endpoint: camel_api::BoxProcessor,
}

impl WireTapLayer {
    /// Create a new WireTapLayer with the given tap endpoint processor.
    pub fn new(tap_endpoint: camel_api::BoxProcessor) -> Self {
        Self { tap_endpoint }
    }
}

impl<S> tower::Layer<S> for WireTapLayer {
    type Service = WireTapService;

    fn layer(&self, _inner: S) -> Self::Service {
        WireTapService::new(self.tap_endpoint.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessor, BoxProcessorExt, Message};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_wire_tap_returns_original_immediately() {
        let tap_processor = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));

        let mut wire_tap = WireTapService::new(tap_processor);
        let exchange = Exchange::new(Message::new("test message"));

        let result = wire_tap
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .unwrap();

        assert_eq!(result.input.body.as_text(), Some("test message"));
    }

    #[tokio::test]
    async fn test_wire_tap_endpoint_receives_clone() {
        let received_count = Arc::new(AtomicUsize::new(0));
        let count_clone = received_count.clone();

        let tap_processor = BoxProcessor::from_fn(move |ex| {
            let count = count_clone.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let mut wire_tap = WireTapService::new(tap_processor);
        let exchange = Exchange::new(Message::new("test"));

        let _result = wire_tap
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert_eq!(received_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_wire_tap_isolates_errors() {
        let tap_processor = BoxProcessor::from_fn(|_ex| {
            Box::pin(async move { Err(CamelError::ProcessorError("tap error".into())) })
        });

        let mut wire_tap = WireTapService::new(tap_processor);
        let exchange = Exchange::new(Message::new("test"));

        let result = wire_tap.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().input.body.as_text(), Some("test"));
    }

    #[tokio::test]
    async fn test_wire_tap_layer() {
        use tower::Layer;

        let tap_processor = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));

        let layer = super::WireTapLayer::new(tap_processor);
        let inner = camel_api::IdentityProcessor;
        let mut svc = layer.layer(inner);

        let exchange = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("test"));
    }
}
