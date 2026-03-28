use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;
use tower::ServiceExt;

use camel_api::endpoint_pipeline::CAMEL_SLIP_ENDPOINT;
use camel_api::{CamelError, EndpointPipelineConfig, Exchange, RoutingSlipConfig, Value};

use crate::endpoint_pipeline::EndpointPipelineService;

/// Routing Slip EIP implementation.
///
/// Evaluates an expression once to get a list of endpoint URIs, then routes
/// the message through them sequentially in a pipeline fashion.
#[derive(Clone)]
pub struct RoutingSlipService {
    config: RoutingSlipConfig,
    pipeline: EndpointPipelineService,
}

impl RoutingSlipService {
    pub fn new(config: RoutingSlipConfig, endpoint_resolver: camel_api::EndpointResolver) -> Self {
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

impl Service<Exchange> for RoutingSlipService {
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
            let slip = match (config.expression)(&exchange) {
                None => return Ok(exchange),
                Some(s) => s,
            };

            for uri in slip.split(&config.uri_delimiter) {
                let uri = uri.trim();
                if uri.is_empty() {
                    continue;
                }

                let endpoint = match pipeline.resolve(uri)? {
                    Some(e) => e,
                    None => continue,
                };

                exchange.set_property(CAMEL_SLIP_ENDPOINT, Value::String(uri.to_string()));

                let mut endpoint = endpoint;
                exchange = endpoint.ready().await?.call(exchange).await?;
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

    fn mock_resolver() -> camel_api::EndpointResolver {
        Arc::new(|uri: &str| {
            if uri.starts_with("mock:") {
                Some(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })))
            } else {
                None
            }
        })
    }

    #[tokio::test]
    async fn routing_slip_single_destination() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let count_clone = call_count.clone();

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

        let config = RoutingSlipConfig::new(Arc::new(|_ex: &Exchange| Some("mock:a".to_string())));

        let mut svc = RoutingSlipService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn routing_slip_multiple_destinations() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let count_clone = call_count.clone();

        let resolver = Arc::new(move |uri: &str| {
            if uri.starts_with("mock:") {
                let count = count_clone.clone();
                Some(BoxProcessor::from_fn(move |ex| {
                    count.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async move { Ok(ex) })
                }))
            } else {
                None
            }
        });

        let config = RoutingSlipConfig::new(Arc::new(|_ex: &Exchange| {
            Some("mock:a,mock:b,mock:c".to_string())
        }));

        let mut svc = RoutingSlipService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn routing_slip_empty_expression() {
        let config = RoutingSlipConfig::new(Arc::new(|_ex: &Exchange| None));

        let mut svc = RoutingSlipService::new(config, mock_resolver());
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn routing_slip_empty_string() {
        let config = RoutingSlipConfig::new(Arc::new(|_ex: &Exchange| Some(String::new())));

        let mut svc = RoutingSlipService::new(config, mock_resolver());
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn routing_slip_invalid_endpoint_error() {
        let config = RoutingSlipConfig::new(Arc::new(|_ex: &Exchange| {
            Some("invalid:endpoint".to_string())
        }))
        .ignore_invalid_endpoints(false);

        let mut svc = RoutingSlipService::new(config, mock_resolver());
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid endpoint"));
    }

    #[tokio::test]
    async fn routing_slip_ignore_invalid_endpoint() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let count_clone = call_count.clone();

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

        let config = RoutingSlipConfig::new(Arc::new(|_ex: &Exchange| {
            Some("invalid:endpoint,mock:valid".to_string())
        }))
        .ignore_invalid_endpoints(true);

        let mut svc = RoutingSlipService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn routing_slip_order_preserved() {
        use std::sync::Mutex;

        let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let order_clone = order.clone();

        let resolver = Arc::new(move |uri: &str| {
            let order = order_clone.clone();
            let uri = uri.to_string();
            Some(BoxProcessor::from_fn(move |ex| {
                order.lock().unwrap().push(uri.clone());
                Box::pin(async move { Ok(ex) })
            }))
        });

        let config = RoutingSlipConfig::new(Arc::new(|_ex: &Exchange| {
            Some("mock:first,mock:second,mock:third".to_string())
        }));

        let mut svc = RoutingSlipService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        svc.ready().await.unwrap().call(ex).await.unwrap();

        let order = order.lock().unwrap();
        assert_eq!(*order, vec!["mock:first", "mock:second", "mock:third"]);
    }

    #[tokio::test]
    async fn routing_slip_endpoint_property_set() {
        let last_uri: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
        let last_uri_clone = last_uri.clone();

        let resolver = Arc::new(move |uri: &str| {
            let last = last_uri_clone.clone();
            let _uri = uri.to_string();
            Some(BoxProcessor::from_fn(move |ex| {
                let prop = ex.property(CAMEL_SLIP_ENDPOINT).cloned();
                *last.lock().unwrap() = prop.and_then(|v| v.as_str().map(String::from));
                Box::pin(async move { Ok(ex) })
            }))
        });

        let config =
            RoutingSlipConfig::new(Arc::new(|_ex: &Exchange| Some("mock:a,mock:b".to_string())));

        let mut svc = RoutingSlipService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        svc.ready().await.unwrap().call(ex).await.unwrap();

        let last = last_uri.lock().unwrap();
        assert_eq!(last.as_deref(), Some("mock:b"));
    }

    #[tokio::test]
    async fn routing_slip_mutation_between_steps() {
        let resolver = Arc::new(|uri: &str| {
            if uri == "mock:mutate" {
                Some(BoxProcessor::from_fn(|mut ex| {
                    ex.input.body = camel_api::Body::Text("mutated".to_string());
                    Box::pin(async move { Ok(ex) })
                }))
            } else if uri == "mock:verify" {
                Some(BoxProcessor::from_fn(|ex| {
                    let body = ex.input.body.as_text().unwrap_or("").to_string();
                    assert_eq!(body, "mutated");
                    Box::pin(async move { Ok(ex) })
                }))
            } else {
                None
            }
        });

        let config = RoutingSlipConfig::new(Arc::new(|_ex: &Exchange| {
            Some("mock:mutate,mock:verify".to_string())
        }));

        let mut svc = RoutingSlipService::new(config, resolver);
        let ex = Exchange::new(Message::new("original"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn routing_slip_cache_hit() {
        let resolve_count = Arc::new(AtomicUsize::new(0));
        let resolve_clone = resolve_count.clone();

        let resolver = Arc::new(move |uri: &str| {
            if uri.starts_with("mock:") {
                resolve_clone.fetch_add(1, Ordering::SeqCst);
                Some(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })))
            } else {
                None
            }
        });

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_clone = call_count.clone();

        let config = RoutingSlipConfig::new(Arc::new(move |_ex: &Exchange| {
            let n = call_clone.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Some("mock:a,mock:b".to_string())
            } else {
                None
            }
        }));

        let mut svc = RoutingSlipService::new(config, resolver);
        let ex1 = Exchange::new(Message::new("test1"));
        svc.ready().await.unwrap().call(ex1).await.unwrap();
        let ex2 = Exchange::new(Message::new("test2"));
        svc.ready().await.unwrap().call(ex2).await.unwrap();

        assert_eq!(resolve_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn routing_slip_custom_delimiter() {
        let order: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let order_clone = order.clone();

        let resolver = Arc::new(move |uri: &str| {
            let order = order_clone.clone();
            let uri = uri.to_string();
            Some(BoxProcessor::from_fn(move |ex| {
                order.lock().unwrap().push(uri.clone());
                Box::pin(async move { Ok(ex) })
            }))
        });

        let config = RoutingSlipConfig::new(Arc::new(|_ex: &Exchange| {
            Some("mock:x|mock:y|mock:z".to_string())
        }))
        .uri_delimiter("|");

        let mut svc = RoutingSlipService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        svc.ready().await.unwrap().call(ex).await.unwrap();

        let order = order.lock().unwrap();
        assert_eq!(*order, vec!["mock:x", "mock:y", "mock:z"]);
    }

    #[tokio::test]
    async fn routing_slip_expression_evaluated_once() {
        let expr_count = Arc::new(AtomicUsize::new(0));
        let expr_count_clone = expr_count.clone();

        let resolver = Arc::new(|uri: &str| {
            if uri.starts_with("mock:") {
                Some(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })))
            } else {
                None
            }
        });

        let config = RoutingSlipConfig::new(Arc::new(move |_ex: &Exchange| {
            expr_count_clone.fetch_add(1, Ordering::SeqCst);
            Some("mock:a,mock:b".to_string())
        }));

        let mut svc = RoutingSlipService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        svc.ready().await.unwrap().call(ex).await.unwrap();

        assert_eq!(
            expr_count.load(Ordering::SeqCst),
            1,
            "Expression must be evaluated exactly once"
        );
    }
}
