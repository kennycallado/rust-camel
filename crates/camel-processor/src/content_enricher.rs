use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use camel_api::{BoxProcessor, CamelError, Exchange};
use camel_component_api::endpoint::PollingConsumer;
use tower::Service;
use tower::ServiceExt;

use crate::EnrichmentStrategy;

/// EIP-7 enrich: synchronous content enrichment via a resolved producer.
#[derive(Clone)]
pub struct EnrichService {
    producer: BoxProcessor,
    strategy: Arc<dyn EnrichmentStrategy>,
}

impl EnrichService {
    pub fn new(producer: BoxProcessor, strategy: Arc<dyn EnrichmentStrategy>) -> Self {
        Self { producer, strategy }
    }
}

impl Service<Exchange> for EnrichService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Propagate producer backpressure (matches repo convention: see wire_tap.rs,
        // security_policy_layer.rs, throttler.rs). EnrichService must not claim
        // readiness while the wrapped producer still applies backpressure.
        self.producer.poll_ready(cx)
    }

    fn call(&mut self, original: Exchange) -> Self::Future {
        // BoxProcessor is BoxCloneService — clone for the async block.
        let producer = self.producer.clone();
        let strategy = Arc::clone(&self.strategy);
        Box::pin(async move {
            let enriched = producer.oneshot(original.clone()).await?;
            Ok(strategy.aggregate(original, enriched))
        })
    }
}

/// EIP-7 pollEnrich: blocking poll of a PollingConsumer with timeout.
///
/// Wraps the `Box<dyn PollingConsumer>` in `Arc<tokio::sync::Mutex<...>>`
/// because the trait requires `&mut self` for `receive` and `Box<dyn ...>`
/// is not Clone. The mutex is held only across the `receive().await` call,
/// not across the strategy aggregation.
#[derive(Clone)]
pub struct PollEnrichService {
    poller: Arc<tokio::sync::Mutex<Box<dyn PollingConsumer>>>,
    timeout: Duration,
    strategy: Arc<dyn EnrichmentStrategy>,
}

impl PollEnrichService {
    pub fn new(
        poller: Box<dyn PollingConsumer>,
        timeout: Duration,
        strategy: Arc<dyn EnrichmentStrategy>,
    ) -> Self {
        Self {
            poller: Arc::new(tokio::sync::Mutex::new(poller)),
            timeout,
            strategy,
        }
    }
}

impl Service<Exchange> for PollEnrichService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, original: Exchange) -> Self::Future {
        let poller = Arc::clone(&self.poller);
        let strategy = Arc::clone(&self.strategy);
        let timeout = self.timeout;
        Box::pin(async move {
            let mut guard = poller.lock().await;
            let enriched_opt = guard.receive(timeout).await?;
            drop(guard); // release before strategy to minimize critical section
            match enriched_opt {
                Some(enriched) => Ok(strategy.aggregate(original, enriched)),
                None => strategy.on_no_poll(original).await,
            }
        })
    }
}
