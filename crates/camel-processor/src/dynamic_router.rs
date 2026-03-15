use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Instant;

use tower::Service;
use tower::ServiceExt;

use camel_api::{BoxProcessor, CamelError, DynamicRouterConfig, Exchange, Value};

const CAMEL_SLIP_ENDPOINT: &str = "CamelSlipEndpoint";

pub type EndpointResolver = Arc<dyn Fn(&str) -> Option<BoxProcessor> + Send + Sync>;

#[derive(Clone)]
pub struct DynamicRouterService {
    config: DynamicRouterConfig,
    endpoint_resolver: EndpointResolver,
    endpoint_cache: Arc<Mutex<HashMap<String, BoxProcessor>>>,
}

impl DynamicRouterService {
    pub fn new(config: DynamicRouterConfig, endpoint_resolver: EndpointResolver) -> Self {
        Self {
            config,
            endpoint_resolver,
            endpoint_cache: Arc::new(Mutex::new(HashMap::new())),
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
        let resolver = self.endpoint_resolver.clone();
        let cache = self.endpoint_cache.clone();

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

                    let endpoint = {
                        let cache_guard = cache.lock().unwrap();
                        cache_guard.get(uri).cloned()
                    };

                    let endpoint = match endpoint {
                        Some(e) => e,
                        None => {
                            let e = match (resolver)(uri) {
                                Some(e) => e,
                                None => {
                                    if config.ignore_invalid_endpoints {
                                        continue;
                                    } else {
                                        return Err(CamelError::ProcessorError(format!(
                                            "Invalid endpoint: {}",
                                            uri
                                        )));
                                    }
                                }
                            };
                            if config.cache_size > 0 {
                                let mut cache_guard = cache.lock().unwrap();
                                cache_guard.insert(uri.to_string(), e.clone());
                            }
                            e
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
    use camel_api::{BoxProcessorExt, Message};
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
}
