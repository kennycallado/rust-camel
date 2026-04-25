use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Service, ServiceExt};

use camel_api::loop_eip::{LoopConfig, LoopMode, MAX_LOOP_ITERATIONS};
use camel_api::{BoxProcessor, CamelError, Exchange, Value};

pub const CAMEL_LOOP_INDEX: &str = "CamelLoopIndex";
pub const CAMEL_LOOP_SIZE: &str = "CamelLoopSize";

#[derive(Clone)]
pub struct LoopService {
    config: LoopConfig,
    sub_pipeline: BoxProcessor,
}

impl LoopService {
    pub fn new(config: LoopConfig, sub_pipeline: BoxProcessor) -> Self {
        Self {
            config,
            sub_pipeline,
        }
    }
}

impl Service<Exchange> for LoopService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let mut pipeline = self.sub_pipeline.clone();

        Box::pin(async move {
            match config.mode {
                LoopMode::Count(n) => {
                    exchange.set_property(CAMEL_LOOP_SIZE, Value::from(n as u64));
                    for i in 0..n {
                        exchange.set_property(CAMEL_LOOP_INDEX, Value::from(i as u64));
                        exchange = pipeline.ready().await?.call(exchange).await?;
                    }
                }
                LoopMode::While(ref predicate) => {
                    exchange.set_property(CAMEL_LOOP_SIZE, Value::from(0u64));
                    for i in 0..MAX_LOOP_ITERATIONS {
                        if !predicate(&exchange) {
                            break;
                        }
                        exchange.set_property(CAMEL_LOOP_INDEX, Value::from(i as u64));
                        exchange = pipeline.ready().await?.call(exchange).await?;
                    }
                    if predicate(&exchange) {
                        tracing::warn!(
                            "Loop while-mode hit MAX_LOOP_ITERATIONS ({}) safety guard. Predicate still true.",
                            MAX_LOOP_ITERATIONS
                        );
                    }
                }
            }
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use camel_api::loop_eip::{LoopConfig, LoopMode, MAX_LOOP_ITERATIONS};
    use camel_api::{
        Body, BoxProcessor, BoxProcessorExt, CamelError, Exchange, IdentityProcessor, Message,
    };
    use tower::{Service, ServiceExt};

    use super::{CAMEL_LOOP_INDEX, CAMEL_LOOP_SIZE, LoopService};

    fn identity_pipeline() -> BoxProcessor {
        BoxProcessor::new(IdentityProcessor)
    }

    fn counter_pipeline(counter: Arc<AtomicUsize>) -> BoxProcessor {
        BoxProcessor::from_fn(move |exchange: Exchange| {
            let counter = Arc::clone(&counter);
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(exchange)
            })
        })
    }

    #[tokio::test]
    async fn test_loop_count_iterates_n_times() {
        let counter = Arc::new(AtomicUsize::new(0));
        let config = LoopConfig::new(LoopMode::Count(3));
        let mut service = LoopService::new(config, counter_pipeline(Arc::clone(&counter)));

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_loop_count_sets_properties() {
        let seen_indices = Arc::new(Mutex::new(Vec::<u64>::new()));
        let seen_indices_for_pipeline = Arc::clone(&seen_indices);

        let pipeline = BoxProcessor::from_fn(move |exchange: Exchange| {
            let seen_indices = Arc::clone(&seen_indices_for_pipeline);
            Box::pin(async move {
                if let Some(index) = exchange.property(CAMEL_LOOP_INDEX).and_then(|v| v.as_u64()) {
                    seen_indices.lock().unwrap().push(index);
                }
                Ok(exchange)
            })
        });

        let config = LoopConfig::new(LoopMode::Count(3));
        let mut service = LoopService::new(config, pipeline);

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(*seen_indices.lock().unwrap(), vec![0, 1, 2]);
        assert_eq!(result.property(CAMEL_LOOP_SIZE).and_then(|v| v.as_u64()), Some(3));
    }

    #[tokio::test]
    async fn test_loop_count_zero_is_noop() {
        let config = LoopConfig::new(LoopMode::Count(0));
        let mut service = LoopService::new(config, identity_pipeline());

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("test"));
        assert_eq!(result.property(CAMEL_LOOP_SIZE).and_then(|v| v.as_u64()), Some(0));
        assert!(result.property(CAMEL_LOOP_INDEX).is_none());
    }

    #[tokio::test]
    async fn test_loop_while_stops_when_predicate_false() {
        let counter = Arc::new(AtomicUsize::new(0));

        let predicate = Arc::new(|exchange: &Exchange| {
            exchange
                .property("iterations")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                < 2
        });

        let counter_for_pipeline = Arc::clone(&counter);
        let pipeline = BoxProcessor::from_fn(move |mut exchange: Exchange| {
            let counter = Arc::clone(&counter_for_pipeline);
            Box::pin(async move {
                let current = exchange
                    .property("iterations")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                exchange.set_property("iterations", current + 1);
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(exchange)
            })
        });

        let config = LoopConfig::new(LoopMode::While(predicate));
        let mut service = LoopService::new(config, pipeline);

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert_eq!(result.property("iterations").and_then(|v| v.as_u64()), Some(2));
        assert_eq!(result.property(CAMEL_LOOP_INDEX).and_then(|v| v.as_u64()), Some(1));
        assert_eq!(result.property(CAMEL_LOOP_SIZE).and_then(|v| v.as_u64()), Some(0));
    }

    #[tokio::test]
    async fn test_loop_while_respects_max_iterations() {
        let counter = Arc::new(AtomicUsize::new(0));
        let predicate = Arc::new(|_exchange: &Exchange| true);
        let config = LoopConfig::new(LoopMode::While(predicate));
        let mut service = LoopService::new(config, counter_pipeline(Arc::clone(&counter)));

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), MAX_LOOP_ITERATIONS);
    }

    #[tokio::test]
    async fn test_loop_error_propagation() {
        let pipeline = BoxProcessor::from_fn(|_exchange: Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        });

        let config = LoopConfig::new(LoopMode::Count(3));
        let mut service = LoopService::new(config, pipeline);

        let exchange = Exchange::new(Message::new("test"));
        let result = service.ready().await.unwrap().call(exchange).await;

        assert!(matches!(result, Err(CamelError::ProcessorError(msg)) if msg == "boom"));
    }

    #[tokio::test]
    async fn test_loop_pipeline_chaining() {
        let pipeline = BoxProcessor::from_fn(|mut exchange: Exchange| {
            Box::pin(async move {
                if let Body::Text(s) = &exchange.input.body {
                    exchange.input.body = Body::Text(format!("{s}x"));
                }
                Ok(exchange)
            })
        });

        let config = LoopConfig::new(LoopMode::Count(3));
        let mut service = LoopService::new(config, pipeline);

        let exchange = Exchange::new(Message::new("start"));
        let result = service.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("startxxx"));
    }

}
