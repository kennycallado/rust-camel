use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use tower::Service;
use tower::ServiceExt;

use camel_api::endpoint_pipeline::{CAMEL_SLIP_ENDPOINT, EndpointPipelineConfig, EndpointResolver};
use camel_api::{CamelError, DynamicRouterConfig, Exchange, Value};

use crate::endpoint_pipeline::EndpointPipelineService;

#[derive(Clone)]
pub struct DynamicRouterService {
    config: DynamicRouterConfig,
    pipeline: EndpointPipelineService,
}

impl DynamicRouterService {
    pub fn new(config: DynamicRouterConfig, endpoint_resolver: EndpointResolver) -> Self {
        let pipeline_config = EndpointPipelineConfig {
            cache_size: EndpointPipelineConfig::from_signed(config.cache_size),
            ignore_invalid_endpoints: config.ignore_invalid_endpoints,
        };

        Self {
            config,
            pipeline: EndpointPipelineService::new(endpoint_resolver, pipeline_config),
        }
    }
}

impl Service<Exchange> for DynamicRouterService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let pipeline = self.pipeline.clone();

        Box::pin(async move {
            let start = Instant::now();
            let mut iterations = 0;

            loop {
                iterations += 1;

                if iterations > config.max_iterations {
                    return Err(CamelError::ProcessorError(format!(
                        "Dynamic router exceeded max iterations ({})",
                        config.max_iterations
                    )));
                }

                if let Some(timeout) = config.timeout
                    && start.elapsed() > timeout
                {
                    return Err(CamelError::ProcessorError(format!(
                        "Dynamic router timed out after {:?}",
                        timeout
                    )));
                }

                let destinations = match (config.expression)(&exchange) {
                    None => break,
                    Some(uris) => uris,
                };

                for uri in destinations.split(&config.uri_delimiter) {
                    let uri = uri.trim();
                    if uri.is_empty() {
                        continue;
                    }

                    let endpoint = match pipeline.resolve(uri)? {
                        Some(e) => e,
                        None => {
                            continue;
                        }
                    };

                    exchange.set_property(CAMEL_SLIP_ENDPOINT, Value::String(uri.to_string()));

                    let mut endpoint = endpoint;
                    exchange = endpoint.ready().await?.call(exchange).await?;
                }
            }

            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessor, BoxProcessorExt, Message};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    fn make_config<F>(f: F) -> DynamicRouterConfig
    where
        F: Fn(&Exchange) -> Option<String> + Send + Sync + 'static,
    {
        DynamicRouterConfig::new(Arc::new(f))
    }

    fn mock_resolver() -> EndpointResolver {
        Arc::new(|uri: &str| {
            if uri.starts_with("mock:") {
                Some(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })))
            } else {
                None
            }
        })
    }

    #[tokio::test]
    async fn test_dynamic_router_single_destination() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let count_clone = call_count.clone();
        let expr_count = Arc::new(AtomicUsize::new(0));
        let expr_count_clone = expr_count.clone();

        let resolver = Arc::new(move |uri: &str| {
            if uri == "mock:a" {
                let count = count_clone.clone();
                Some(BoxProcessor::from_fn(move |ex| {
                    count.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async move { Ok(ex) })
                }))
            } else {
                None
            }
        });

        let config = DynamicRouterConfig::new(Arc::new(move |ex: &Exchange| {
            let count = expr_count_clone.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                ex.input
                    .header("dest")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
            } else {
                None
            }
        }));

        let mut svc = DynamicRouterService::new(config, resolver);

        let mut ex = Exchange::new(Message::new("test"));
        ex.input.set_header("dest", Value::String("mock:a".into()));

        let _result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_dynamic_router_loop_terminates_on_none() {
        let iterations = Arc::new(AtomicUsize::new(0));
        let iterations_clone = iterations.clone();

        let config = DynamicRouterConfig::new(Arc::new(move |_ex: &Exchange| {
            let count = iterations_clone.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                Some("mock:a".to_string())
            } else {
                None
            }
        }));

        let mut svc = DynamicRouterService::new(config, mock_resolver());

        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
        assert_eq!(iterations.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_dynamic_router_max_iterations() {
        let config = make_config(|_| Some("mock:a".to_string())).max_iterations(5);

        let mut svc = DynamicRouterService::new(config, mock_resolver());

        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("max iterations"));
    }

    #[tokio::test]
    async fn test_dynamic_router_invalid_endpoint_error() {
        let config =
            make_config(|_| Some("invalid:endpoint".to_string())).ignore_invalid_endpoints(false);

        let mut svc = DynamicRouterService::new(config, mock_resolver());

        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid endpoint"));
    }

    #[tokio::test]
    async fn test_dynamic_router_ignore_invalid_endpoint() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let count_clone = call_count.clone();
        let expr_count = Arc::new(AtomicUsize::new(0));
        let expr_count_clone = expr_count.clone();

        let resolver = Arc::new(move |uri: &str| {
            if uri == "mock:valid" {
                let count = count_clone.clone();
                Some(BoxProcessor::from_fn(move |ex| {
                    count.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async move { Ok(ex) })
                }))
            } else {
                None
            }
        });

        let config = DynamicRouterConfig::new(Arc::new(move |_ex: &Exchange| {
            let count = expr_count_clone.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                Some("invalid:endpoint,mock:valid".to_string())
            } else {
                None
            }
        }))
        .ignore_invalid_endpoints(true);

        let mut svc = DynamicRouterService::new(config, resolver);

        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_dynamic_router_cache_size_enforced() {
        // Verify that the cache never exceeds the configured capacity.
        let resolver_call_count = Arc::new(AtomicUsize::new(0));
        let count_clone = resolver_call_count.clone();

        let resolver: EndpointResolver = Arc::new(move |uri: &str| {
            if uri.starts_with("mock:") {
                count_clone.fetch_add(1, Ordering::SeqCst);
                Some(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })))
            } else {
                None
            }
        });

        // Cache capacity of 2; we will route through 3 distinct URIs.
        let expr_count = Arc::new(AtomicUsize::new(0));
        let expr_count_clone = expr_count.clone();
        let config = DynamicRouterConfig::new(Arc::new(move |_ex: &Exchange| {
            let n = expr_count_clone.fetch_add(1, Ordering::SeqCst);
            match n {
                0 => Some("mock:a".to_string()),
                1 => Some("mock:b".to_string()),
                2 => Some("mock:c".to_string()),
                _ => None,
            }
        }))
        .cache_size(2);

        let mut svc = DynamicRouterService::new(config, resolver);

        let ex = Exchange::new(Message::new("test"));
        svc.ready().await.unwrap().call(ex).await.unwrap();

        // The cache capacity is 2 so mock:a, mock:b, mock:c each required a resolver call
        // (mock:a is evicted before mock:c is inserted). All three resolver calls must have happened.
        assert_eq!(resolver_call_count.load(Ordering::SeqCst), 3);
    }
}
