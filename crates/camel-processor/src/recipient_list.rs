use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::task::JoinSet;
use tower::Service;
use tower::ServiceExt;

use camel_api::endpoint_pipeline::{CAMEL_SLIP_ENDPOINT, EndpointPipelineConfig};
use camel_api::recipient_list::RecipientListConfig;
use camel_api::{Body, CamelError, Exchange, Value};

use crate::endpoint_pipeline::EndpointPipelineService;

#[derive(Clone)]
pub struct RecipientListService {
    config: RecipientListConfig,
    pipeline: EndpointPipelineService,
}

impl RecipientListService {
    pub fn new(
        config: RecipientListConfig,
        endpoint_resolver: camel_api::EndpointResolver,
    ) -> Self {
        let pipeline_config = EndpointPipelineConfig {
            cache_size: EndpointPipelineConfig::from_signed(1000),
            ignore_invalid_endpoints: false,
        };
        Self {
            config,
            pipeline: EndpointPipelineService::new(endpoint_resolver, pipeline_config),
        }
    }
}

impl Service<Exchange> for RecipientListService {
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
            let uris_raw = (config.expression)(&exchange);
            if uris_raw.is_empty() {
                return Ok(exchange);
            }

            let uris: Vec<&str> = uris_raw
                .split(&config.delimiter)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if uris.is_empty() {
                return Ok(exchange);
            }

            if config.parallel {
                let original_for_aggregate = exchange.clone();
                let mut endpoints_to_call = Vec::with_capacity(uris.len());
                for uri in &uris {
                    if let Some(endpoint) = pipeline.resolve(uri)? {
                        endpoints_to_call.push((uri.to_string(), endpoint));
                    }
                }

                let mut results: Vec<Exchange> = Vec::with_capacity(endpoints_to_call.len());
                let mut join_set = JoinSet::new();
                let mut iter = endpoints_to_call.into_iter();
                let raw_limit = config.parallel_limit.unwrap_or(results.capacity());
                let limit = raw_limit.max(1).min(results.capacity().max(1));

                for _ in 0..limit {
                    if let Some((uri, mut endpoint)) = iter.next() {
                        let mut cloned = original_for_aggregate.clone();
                        cloned.set_property(CAMEL_SLIP_ENDPOINT, Value::String(uri));
                        join_set.spawn(async move { endpoint.ready().await?.call(cloned).await });
                    }
                }

                while let Some(result) = join_set.join_next().await {
                    match result {
                        Ok(Ok(ex)) => results.push(ex),
                        Ok(Err(e)) if config.stop_on_exception => {
                            join_set.abort_all();
                            return Err(e);
                        }
                        _ => {}
                    }

                    if let Some((uri, mut endpoint)) = iter.next() {
                        let mut cloned = original_for_aggregate.clone();
                        cloned.set_property(CAMEL_SLIP_ENDPOINT, Value::String(uri));
                        join_set.spawn(async move { endpoint.ready().await?.call(cloned).await });
                    }
                }

                exchange = aggregate_results(config.strategy, original_for_aggregate, results);
            } else {
                let mut results: Vec<Exchange> = Vec::new();
                let original_for_aggregate = exchange.clone();
                for uri in &uris {
                    let endpoint = match pipeline.resolve(uri)? {
                        Some(e) => e,
                        None => continue,
                    };
                    exchange.set_property(CAMEL_SLIP_ENDPOINT, Value::String(uri.to_string()));
                    let mut endpoint = endpoint;
                    let result = endpoint.ready().await?.call(exchange.clone()).await;
                    match result {
                        Ok(ex) => {
                            results.push(ex.clone());
                            exchange = ex;
                        }
                        Err(e) if config.stop_on_exception => return Err(e),
                        Err(_) => continue,
                    }
                }
                exchange = aggregate_results(config.strategy, original_for_aggregate, results);
            }

            Ok(exchange)
        })
    }
}

fn aggregate_results(
    strategy: camel_api::MulticastStrategy,
    original: Exchange,
    results: Vec<Exchange>,
) -> Exchange {
    match strategy {
        camel_api::MulticastStrategy::LastWins => results.into_iter().last().unwrap_or(original),
        camel_api::MulticastStrategy::Original => original,
        camel_api::MulticastStrategy::CollectAll => {
            let bodies: Vec<Value> = results
                .iter()
                .map(|ex| match &ex.input.body {
                    Body::Text(s) => Value::String(s.clone()),
                    Body::Json(v) => v.clone(),
                    Body::Xml(s) => Value::String(s.clone()),
                    Body::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
                    Body::Empty => Value::Null,
                    Body::Stream(s) => serde_json::json!({
                        "_stream": {
                            "origin": s.metadata.origin,
                            "placeholder": true,
                            "hint": "Materialize exchange body with .into_bytes() before recipient-list aggregation"
                        }
                    }),
                })
                .collect();
            let mut result = results.into_iter().last().unwrap_or(original);
            result.input.body = camel_api::Body::from(Value::Array(bodies));
            result
        }
        camel_api::MulticastStrategy::Custom(fn_) => {
            results.into_iter().fold(original, |acc, ex| fn_(acc, ex))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::MulticastStrategy;
    use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Message};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tokio::sync::Mutex;
    use tokio::time::sleep;

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
    async fn recipient_list_single_destination() {
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

        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| "mock:a".to_string()));

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn recipient_list_multiple_destinations() {
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

        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            "mock:a,mock:b,mock:c".to_string()
        }));

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn recipient_list_empty_expression() {
        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| String::new()));

        let mut svc = RecipientListService::new(config, mock_resolver());
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn recipient_list_invalid_endpoint_error() {
        let config =
            RecipientListConfig::new(Arc::new(|_ex: &Exchange| "invalid:endpoint".to_string()));

        let mut svc = RecipientListService::new(config, mock_resolver());
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid endpoint"));
    }

    #[tokio::test]
    async fn recipient_list_custom_delimiter() {
        use std::sync::Mutex;

        let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let resolver = {
            let order = order.clone();
            Arc::new(move |uri: &str| {
                let order = order.clone();
                let uri = uri.to_string();
                Some(BoxProcessor::from_fn(move |ex| {
                    order.lock().unwrap().push(uri.clone());
                    Box::pin(async move { Ok(ex) })
                }))
            })
        };

        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            "mock:x|mock:y|mock:z".to_string()
        }))
        .delimiter("|");

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        svc.ready().await.unwrap().call(ex).await.unwrap();

        let order = order.lock().unwrap();
        assert_eq!(*order, vec!["mock:x", "mock:y", "mock:z"]);
    }

    #[tokio::test]
    async fn recipient_list_expression_evaluated_once() {
        let expr_count = Arc::new(AtomicUsize::new(0));
        let expr_count_clone = expr_count.clone();

        let config = RecipientListConfig::new(Arc::new(move |_ex: &Exchange| {
            expr_count_clone.fetch_add(1, Ordering::SeqCst);
            "mock:a,mock:b".to_string()
        }));

        let mut svc = RecipientListService::new(config, mock_resolver());
        let ex = Exchange::new(Message::new("test"));
        svc.ready().await.unwrap().call(ex).await.unwrap();

        assert_eq!(
            expr_count.load(Ordering::SeqCst),
            1,
            "Expression must be evaluated exactly once"
        );
    }

    #[tokio::test]
    async fn recipient_list_ignores_empty_uri_tokens() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let resolver = Arc::new(move |uri: &str| {
            if uri.starts_with("mock:") {
                let count = call_count_clone.clone();
                Some(BoxProcessor::from_fn(move |ex| {
                    count.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async move { Ok(ex) })
                }))
            } else {
                None
            }
        });

        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            " ,mock:a, ,mock:b,, ".to_string()
        }));

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn recipient_list_mutation_between_steps() {
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

        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            "mock:mutate,mock:verify".to_string()
        }));

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("original"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn recipient_list_parallel_executes_concurrently() {
        let records: Arc<Mutex<Vec<(String, Instant, Instant)>>> = Arc::new(Mutex::new(Vec::new()));

        let resolver = {
            let records = records.clone();
            Arc::new(move |uri: &str| {
                if uri.starts_with("mock:") {
                    let records = records.clone();
                    let uri = uri.to_string();
                    Some(BoxProcessor::from_fn(move |ex| {
                        let records = records.clone();
                        let uri = uri.clone();
                        Box::pin(async move {
                            let start = Instant::now();
                            sleep(Duration::from_millis(100)).await;
                            let end = Instant::now();
                            records.lock().await.push((uri, start, end));
                            Ok(ex)
                        })
                    }))
                } else {
                    None
                }
            })
        };

        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            "mock:a,mock:b,mock:c".to_string()
        }))
        .parallel(true);

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        svc.ready().await.unwrap().call(ex).await.unwrap();

        let records = records.lock().await;
        assert_eq!(records.len(), 3);

        let mut overlap_found = false;
        for i in 0..records.len() {
            for j in (i + 1)..records.len() {
                let (_, a_start, a_end) = records[i];
                let (_, b_start, b_end) = records[j];
                if a_start < b_end && b_start < a_end {
                    overlap_found = true;
                    break;
                }
            }
            if overlap_found {
                break;
            }
        }

        assert!(overlap_found);
    }

    #[tokio::test]
    async fn recipient_list_parallel_stop_on_exception_returns_error() {
        let resolver = Arc::new(|uri: &str| {
            if uri == "mock:err" {
                Some(BoxProcessor::from_fn(|_ex| {
                    Box::pin(async { Err(CamelError::ProcessorError("boom".to_string())) })
                }))
            } else if uri.starts_with("mock:") {
                Some(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })))
            } else {
                None
            }
        });

        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            "mock:a,mock:err,mock:c".to_string()
        }))
        .parallel(true)
        .stop_on_exception(true);

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(matches!(result, Err(CamelError::ProcessorError(msg)) if msg == "boom"));
    }

    #[tokio::test]
    async fn recipient_list_parallel_limit_respects_limit() {
        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            "mock:a,mock:b,mock:c,mock:d".to_string()
        }))
        .parallel(true)
        .parallel_limit(2);

        let resolver = Arc::new(|uri: &str| {
            if uri.starts_with("mock:") {
                Some(BoxProcessor::from_fn(|ex| {
                    Box::pin(async move {
                        sleep(Duration::from_millis(100)).await;
                        Ok(ex)
                    })
                }))
            } else {
                None
            }
        });

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("test"));
        let start = Instant::now();
        svc.ready().await.unwrap().call(ex).await.unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed >= Duration::from_millis(180));
        assert!(elapsed < Duration::from_millis(350));
    }

    #[tokio::test]
    async fn recipient_list_collect_all_strategy() {
        let resolver = Arc::new(|uri: &str| {
            if uri == "mock:a" {
                Some(BoxProcessor::from_fn(|mut ex| {
                    ex.input.body = Body::Text("a".to_string());
                    Box::pin(async move { Ok(ex) })
                }))
            } else if uri == "mock:b" {
                Some(BoxProcessor::from_fn(|mut ex| {
                    ex.input.body = Body::Text("b".to_string());
                    Box::pin(async move { Ok(ex) })
                }))
            } else if uri == "mock:c" {
                Some(BoxProcessor::from_fn(|mut ex| {
                    ex.input.body = Body::Text("c".to_string());
                    Box::pin(async move { Ok(ex) })
                }))
            } else {
                None
            }
        });

        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            "mock:a,mock:b,mock:c".to_string()
        }))
        .strategy(MulticastStrategy::CollectAll);

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("seed"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();

        assert_eq!(
            result.input.body,
            Body::from(Value::Array(vec![
                Value::String("a".to_string()),
                Value::String("b".to_string()),
                Value::String("c".to_string()),
            ]))
        );
    }

    #[tokio::test]
    async fn recipient_list_original_strategy() {
        let resolver = Arc::new(|uri: &str| {
            if uri.starts_with("mock:") {
                let label = uri.to_string();
                Some(BoxProcessor::from_fn(move |mut ex| {
                    let label = label.clone();
                    ex.input.body = Body::Text(format!("mutated-{label}"));
                    Box::pin(async move { Ok(ex) })
                }))
            } else {
                None
            }
        });

        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            "mock:a,mock:b,mock:c".to_string()
        }))
        .strategy(MulticastStrategy::Original);

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("original"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("original"));
    }

    #[tokio::test]
    async fn recipient_list_last_wins_strategy() {
        let payloads: Arc<HashMap<String, String>> = Arc::new(HashMap::from([
            ("mock:a".to_string(), "first".to_string()),
            ("mock:b".to_string(), "second".to_string()),
            ("mock:c".to_string(), "third".to_string()),
        ]));

        let resolver = {
            let payloads = payloads.clone();
            Arc::new(move |uri: &str| {
                if let Some(payload) = payloads.get(uri) {
                    let payload = payload.clone();
                    Some(BoxProcessor::from_fn(move |mut ex| {
                        let payload = payload.clone();
                        ex.input.body = Body::Text(payload);
                        Box::pin(async move { Ok(ex) })
                    }))
                } else {
                    None
                }
            })
        };

        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            "mock:a,mock:b,mock:c".to_string()
        }))
        .strategy(MulticastStrategy::LastWins);

        let mut svc = RecipientListService::new(config, resolver);
        let ex = Exchange::new(Message::new("seed"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("third"));
    }
}
