use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange};

/// Tower Service implementing the Stop EIP.
///
/// Always returns `Err(CamelError::Stopped)`, signalling the pipeline to halt
/// processing for this exchange. The pipeline captures `Stopped` and returns
/// the exchange as-is rather than propagating the error upstream.
#[derive(Clone)]
pub struct StopService;

impl Service<Exchange> for StopService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _exchange: Exchange) -> Self::Future {
        Box::pin(async { Err(CamelError::Stopped) })
    }
}

#[cfg(test)]
mod tests {
    use camel_api::{CamelError, Exchange, Message};
    use tower::{Service, ServiceExt};

    use super::StopService;

    // StopService always returns Err(CamelError::Stopped), regardless of exchange content.
    #[tokio::test]
    async fn test_stop_service_always_returns_stopped() {
        let mut svc = StopService;
        let ex = Exchange::new(Message::new("anything"));
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(matches!(result, Err(CamelError::Stopped)));
    }

    // StopService is Clone.
    #[tokio::test]
    async fn test_stop_service_is_clone() {
        let svc = StopService;
        let mut clone = svc.clone();
        let ex = Exchange::new(Message::new("x"));
        let result = clone.ready().await.unwrap().call(ex).await;
        assert!(matches!(result, Err(CamelError::Stopped)));
    }
}
