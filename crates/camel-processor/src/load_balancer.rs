use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use tower::Service;
use tower::ServiceExt;

use camel_api::{
    BoxProcessor, CamelError, Exchange, LoadBalanceStrategy, LoadBalancerConfig, Value,
};

use crate::multicast::{CAMEL_MULTICAST_COMPLETE, CAMEL_MULTICAST_INDEX};

#[derive(Clone)]
pub struct LoadBalancerService {
    endpoints: Vec<BoxProcessor>,
    config: LoadBalancerConfig,
    round_robin_index: Arc<AtomicUsize>,
    failover_index: Arc<AtomicUsize>,
}

impl LoadBalancerService {
    pub fn new(endpoints: Vec<BoxProcessor>, config: LoadBalancerConfig) -> Self {
        Self {
            endpoints,
            config,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
            failover_index: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Service<Exchange> for LoadBalancerService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        for endpoint in &mut self.endpoints {
            match endpoint.poll_ready(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {}
            }
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let endpoints = self.endpoints.clone();
        let config = self.config.clone();
        let round_robin_index = self.round_robin_index.clone();
        let failover_index = self.failover_index.clone();

        Box::pin(async move {
            if endpoints.is_empty() {
                return Ok(exchange);
            }

            if config.parallel {
                process_parallel(exchange, endpoints).await
            } else {
                match &config.strategy {
                    LoadBalanceStrategy::RoundRobin => {
                        process_round_robin(exchange, endpoints, round_robin_index).await
                    }
                    LoadBalanceStrategy::Random => process_random(exchange, endpoints).await,
                    LoadBalanceStrategy::Weighted(weights) => {
                        process_weighted(exchange, endpoints, weights).await
                    }
                    LoadBalanceStrategy::Failover => {
                        process_failover(exchange, endpoints, failover_index).await
                    }
                }
            }
        })
    }
}

async fn process_round_robin(
    exchange: Exchange,
    endpoints: Vec<BoxProcessor>,
    index: Arc<AtomicUsize>,
) -> Result<Exchange, CamelError> {
    let len = endpoints.len();
    let idx = index.fetch_add(1, Ordering::SeqCst) % len;
    let mut endpoint = endpoints[idx].clone();
    endpoint.ready().await?.call(exchange).await
}

async fn process_random(
    exchange: Exchange,
    endpoints: Vec<BoxProcessor>,
) -> Result<Exchange, CamelError> {
    let len = endpoints.len();
    let idx = rand::random::<usize>() % len;
    let mut endpoint = endpoints[idx].clone();
    endpoint.ready().await?.call(exchange).await
}

async fn process_weighted(
    exchange: Exchange,
    endpoints: Vec<BoxProcessor>,
    weights: &[(String, u32)],
) -> Result<Exchange, CamelError> {
    if endpoints.is_empty() || weights.is_empty() {
        return Ok(exchange);
    }

    let numeric_weights: Vec<u32> = weights.iter().map(|(_, w)| *w).collect();
    let total: u32 = numeric_weights.iter().sum();

    if total == 0 {
        return Err(CamelError::ProcessorError(
            "Weighted load balancer has zero total weight".to_string(),
        ));
    }

    let mut r = rand::random::<u32>() % total;
    let mut selected_idx = 0;
    for (i, w) in numeric_weights.iter().enumerate() {
        if r < *w {
            selected_idx = i.min(endpoints.len() - 1);
            break;
        }
        r -= w;
    }

    let mut endpoint = endpoints[selected_idx].clone();
    endpoint.ready().await?.call(exchange).await
}

async fn process_failover(
    mut exchange: Exchange,
    endpoints: Vec<BoxProcessor>,
    start_index: Arc<AtomicUsize>,
) -> Result<Exchange, CamelError> {
    let len = endpoints.len();
    let start = start_index.load(Ordering::SeqCst);
    let mut last_error = None;

    for i in 0..len {
        let idx = (start + i) % len;
        let mut endpoint = endpoints[idx].clone();
        match endpoint.ready().await?.call(exchange).await {
            Ok(ex) => {
                start_index.store((idx + 1) % len, Ordering::SeqCst);
                return Ok(ex);
            }
            Err(e) => {
                last_error = Some(e);
                exchange = Exchange::new(camel_api::Message::new(""));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        CamelError::ProcessorError("All endpoints failed in failover".to_string())
    }))
}

async fn process_parallel(
    exchange: Exchange,
    endpoints: Vec<BoxProcessor>,
) -> Result<Exchange, CamelError> {
    use futures::future::join_all;

    let total = endpoints.len();
    let futures: Vec<_> = endpoints
        .into_iter()
        .enumerate()
        .map(|(i, mut endpoint)| {
            let mut ex = exchange.clone();
            ex.set_property(CAMEL_MULTICAST_INDEX, Value::from(i as i64));
            ex.set_property(CAMEL_MULTICAST_COMPLETE, Value::Bool(i == total - 1));
            async move {
                tower::ServiceExt::ready(&mut endpoint).await?;
                endpoint.call(ex).await
            }
        })
        .collect();

    let results: Vec<Result<Exchange, CamelError>> = join_all(futures).await;

    for result in &results {
        if let Err(e) = result {
            return Err(e.clone());
        }
    }

    results.into_iter().last().unwrap_or(Ok(exchange))
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessorExt, Message};
    use tower::ServiceExt;

    fn counting_processor() -> (BoxProcessor, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();
        let processor = BoxProcessor::from_fn(move |ex| {
            count_clone.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(ex) })
        });
        (processor, count)
    }

    #[tokio::test]
    async fn test_round_robin_distribution() {
        let (p1, c1) = counting_processor();
        let (p2, c2) = counting_processor();
        let (p3, c3) = counting_processor();

        let config = LoadBalancerConfig::round_robin();
        let mut svc = LoadBalancerService::new(vec![p1, p2, p3], config);

        for _ in 0..6 {
            let ex = Exchange::new(Message::new("test"));
            svc.ready().await.unwrap().call(ex).await.unwrap();
        }

        assert_eq!(c1.load(Ordering::SeqCst), 2);
        assert_eq!(c2.load(Ordering::SeqCst), 2);
        assert_eq!(c3.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_random_distribution() {
        let (p1, c1) = counting_processor();
        let (p2, c2) = counting_processor();

        let config = LoadBalancerConfig::random();
        let mut svc = LoadBalancerService::new(vec![p1, p2], config);

        for _ in 0..100 {
            let ex = Exchange::new(Message::new("test"));
            svc.ready().await.unwrap().call(ex).await.unwrap();
        }

        let total = c1.load(Ordering::SeqCst) + c2.load(Ordering::SeqCst);
        assert_eq!(total, 100);
        assert!(c1.load(Ordering::SeqCst) > 20);
        assert!(c2.load(Ordering::SeqCst) > 20);
    }

    #[tokio::test]
    async fn test_failover_on_error() {
        let failing = BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("fail".into())) })
        });
        let (success, count) = counting_processor();

        let config = LoadBalancerConfig::failover();
        let mut svc = LoadBalancerService::new(vec![failing, success], config);

        let ex = Exchange::new(Message::new("test"));
        let _result = svc.ready().await.unwrap().call(ex).await.unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_failover_all_fail() {
        let failing = BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("fail".into())) })
        });

        let config = LoadBalancerConfig::failover();
        let mut svc = LoadBalancerService::new(vec![failing.clone(), failing], config);

        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_parallel_sends_to_all() {
        let (p1, c1) = counting_processor();
        let (p2, c2) = counting_processor();
        let (p3, c3) = counting_processor();

        let config = LoadBalancerConfig::round_robin().parallel(true);
        let mut svc = LoadBalancerService::new(vec![p1, p2, p3], config);

        let ex = Exchange::new(Message::new("test"));
        svc.ready().await.unwrap().call(ex).await.unwrap();

        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
        assert_eq!(c3.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_empty_endpoints() {
        let config = LoadBalancerConfig::round_robin();
        let mut svc = LoadBalancerService::new(vec![], config);

        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
    }
}
