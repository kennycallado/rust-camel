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
    ) -> Result<Self, CamelError> {
        config.validate()?;
        let pipeline_config = EndpointPipelineConfig {
            cache_size: EndpointPipelineConfig::from_signed(1000),
            ignore_invalid_endpoints: false,
        };
        Ok(Self {
            config,
            pipeline: EndpointPipelineService::new(endpoint_resolver, pipeline_config),
        })
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

            // H13 Batch 1: cap the resolved-URI list BEFORE any endpoint
            // resolution. A malicious expression yielding millions of URIs
            // would otherwise allocate a Vec of millions of &str references
            // and resolve each one (multicast) or call each one (sequential).
            // The default cap is 1_000 (camel-api::recipient_list).
            let cap = config.max_recipients;
            let uris: Vec<&str> = uris_raw
                .split(&config.delimiter)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .take(cap)
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
                let mut last_parallel_error: Option<CamelError> = None;

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
                        Ok(Err(e)) => {
                            // stop_on_exception=false: track the representative
                            // error — the last failing task to complete via
                            // join_next order (ADR-0058). Pending tasks continue.
                            last_parallel_error = Some(e);
                        }
                        Err(join_err) if join_err.is_panic() => {
                            // A recipient task panicked. ADR-0058: a panic is
                            // zero-success attempted work and MUST NOT launder to
                            // Ok(original); convert to a representative error so
                            // the zero-success guard fires. (Cancellation is
                            // handled separately below — it is often self-induced
                            // by stop_on_exception's abort_all.)
                            last_parallel_error = Some(CamelError::ProcessorError(format!(
                                "recipient task panicked: {join_err}"
                            )));
                        }
                        Err(_) => {} // Cancellation (JoinSet abort); ignore.
                    }

                    if let Some((uri, mut endpoint)) = iter.next() {
                        let mut cloned = original_for_aggregate.clone();
                        cloned.set_property(CAMEL_SLIP_ENDPOINT, Value::String(uri));
                        join_set.spawn(async move { endpoint.ready().await?.call(cloned).await });
                    }
                }

                // ADR-0058: zero-success operational failure. At least one
                // recipient was called and zero returned Ok — report the
                // representative error instead of laundering to Ok(original).
                let zero_success_error = if results.is_empty() {
                    last_parallel_error
                } else {
                    None
                };
                if let Some(err) = zero_success_error {
                    return Err(err);
                }

                exchange = aggregate_results(config.strategy, original_for_aggregate, results);
            } else {
                let mut results: Vec<Exchange> = Vec::new();
                let mut last_error: Option<CamelError> = None;
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
                        Err(e) => {
                            // stop_on_exception=false: track the iteration-last
                            // error (ADR-0058) and continue to remaining recipients.
                            last_error = Some(e);
                            continue;
                        }
                    }
                }
                // ADR-0058: zero-success operational failure. At least one
                // recipient was called and zero returned Ok — report the
                // iteration-last error instead of laundering to Ok(original),
                // which would poison an outer cache write-back with the inbound body.
                let zero_success_error = if results.is_empty() { last_error } else { None };
                if let Some(err) = zero_success_error {
                    return Err(err);
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
        camel_api::MulticastStrategy::CollectAll => {
            let bodies: Vec<Value> = results
                .iter()
                .map(|ex| match &ex.input.body {
                    Body::Text(s) => Value::String(s.clone()),
                    Body::Json(v) => v.clone(),
                    Body::Xml(s) => Value::String(s.clone()),
                    Body::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
                    Body::Stream(s) => serde_json::json!({
                        "_stream": {
                            "origin": s.metadata.origin,
                            "placeholder": true,
                            "hint": "Materialize exchange body with .into_bytes() before recipient-list aggregation"
                        }
                    }),
                    // Empty and future variants contribute no extractable value.
                    _ => Value::Null,
                })
                .collect();
            let mut result = results.into_iter().last().unwrap_or(original);
            result.input.body = camel_api::Body::from(Value::Array(bodies));
            result
        }
        camel_api::MulticastStrategy::Custom(fn_) => {
            results.into_iter().fold(original, |acc, ex| fn_(acc, ex))
        }
        // Original and any future variant return the original exchange.
        _ => original,
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn recipient_list_empty_expression() {
        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| String::new()));

        let mut svc = RecipientListService::new(config, mock_resolver()).unwrap();
        let ex = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn recipient_list_invalid_endpoint_error() {
        let config =
            RecipientListConfig::new(Arc::new(|_ex: &Exchange| "invalid:endpoint".to_string()));

        let mut svc = RecipientListService::new(config, mock_resolver()).unwrap();
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
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

        let mut svc = RecipientListService::new(config, mock_resolver()).unwrap();
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
        let ex = Exchange::new(Message::new("original"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("original"));
    }

    // ── H13 Batch 1: cap resolved-URI count ──────────────────────────

    /// H13: an expression yielding millions of URIs is truncated to
    /// `max_recipients` before endpoint resolution. The test uses a
    /// cap of 4 to keep the test fast; the principle (cap the list) is
    /// what Batch 1 enforces. The default cap is 1_000 in camel-api.
    #[tokio::test]
    async fn test_huge_recipient_list_is_capped() {
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

        // Build the untrusted payload: 1_000_000 URIs as one string.
        let mut many = String::with_capacity(8 * 1_000_000);
        for i in 0..1_000_000 {
            if i > 0 {
                many.push(',');
            }
            many.push_str(&format!("mock:k{i}"));
        }

        // Disposition-5 pattern: the untrusted data flows FROM the exchange
        // (a header on the inbound message), NOT from a captured variable.
        // The expression reads it off the passed `&Exchange` — this is what
        // makes the cap an untrusted-data-validation control, not a local
        // limit.
        let config = RecipientListConfig::new(Arc::new(|ex: &Exchange| {
            ex.input
                .header("CamelRecipients")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default()
        }))
        .max_recipients(4);

        let mut svc = RecipientListService::new(config, resolver).unwrap();
        let mut ex = Exchange::new(Message::new("test"));
        ex.input.set_header("CamelRecipients", Value::String(many));
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(result.is_ok(), "capped execution should still succeed");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            4,
            "must resolve at most max_recipients (4) endpoints"
        );
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

        let mut svc = RecipientListService::new(config, resolver).unwrap();
        let ex = Exchange::new(Message::new("seed"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("third"));
    }

    // ── ADR-0058: zero-success operational failure must not launder to Ok(original) ─

    fn err_resolver(uri_to_err: Vec<(&'static str, CamelError)>) -> camel_api::EndpointResolver {
        Arc::new(move |uri: &str| {
            for (pattern, err) in &uri_to_err {
                if uri == *pattern {
                    let err = err.clone();
                    return Some(BoxProcessor::from_fn(move |_ex| {
                        let err = err.clone();
                        Box::pin(async move { Err(err) })
                    }));
                }
            }
            None
        })
    }

    #[tokio::test]
    async fn recipient_list_sequential_all_failed_returns_err() {
        // ADR-0058: zero-success sequential. One recipient errors; zero Ok.
        // MUST return Err, not Ok(original) (which would poison an outer cache).
        let resolver = err_resolver(vec![(
            "mock:a",
            CamelError::Config(String::from("seq-all-failed")),
        )]);
        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| "mock:a".to_string()))
            .strategy(MulticastStrategy::LastWins);

        let mut svc = RecipientListService::new(config, resolver).unwrap();
        let mut ex = Exchange::new(Message::new("timer:t tick #1"));
        ex.input.body = Body::Text(String::from("timer:t tick #1"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(
            result.is_err(),
            "zero-success recipient_list must return Err, not Ok(original)"
        );
        assert!(
            matches!(result, Err(CamelError::Config(m)) if m == "seq-all-failed"),
            "returned error must carry the iteration-last error"
        );
    }

    #[tokio::test]
    async fn recipient_list_parallel_all_failed_returns_err() {
        // ADR-0058: zero-success parallel. Two recipients error; zero Ok.
        // MUST return a representative Err, not Ok(original).
        let resolver = err_resolver(vec![
            ("mock:a", CamelError::Config(String::from("par-err-a"))),
            ("mock:b", CamelError::Config(String::from("par-err-b"))),
        ]);
        let config =
            RecipientListConfig::new(Arc::new(|_ex: &Exchange| "mock:a,mock:b".to_string()))
                .strategy(MulticastStrategy::LastWins)
                .parallel(true);

        let mut svc = RecipientListService::new(config, resolver).unwrap();
        let ex = Exchange::new(Message::new("inbound"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(
            result.is_err(),
            "zero-success parallel recipient_list must return Err, not Ok(original)"
        );
    }

    #[tokio::test]
    async fn recipient_list_parallel_last_error_is_join_next_order() {
        // ADR-0058 last-error determinism: the representative error is the one
        // from the task returned by the last `JoinSet::join_next` that completed
        // with an error. mock:a errors immediately; mock:b awaits a oneshot
        // signal then errors. The test sends the signal after a brief yield so
        // mock:a completes first → join_next order yields mock:b's error last.
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let rx = Arc::new(tokio::sync::Mutex::new(Some(rx)));
        let resolver: camel_api::EndpointResolver = Arc::new(move |uri: &str| {
            if uri == "mock:a" {
                Some(BoxProcessor::from_fn(|_ex| {
                    Box::pin(async move { Err(CamelError::Config(String::from("par-err-a"))) })
                }))
            } else if uri == "mock:b" {
                let rx = rx.clone();
                Some(BoxProcessor::from_fn(move |_ex| {
                    let rx = rx.clone();
                    Box::pin(async move {
                        // Wait for the test's signal before completing.
                        let mut lock = rx.lock().await;
                        if let Some(rx) = lock.take() {
                            let _ = rx.await;
                        }
                        Err(CamelError::Config(String::from("par-err-b")))
                    })
                }))
            } else {
                None
            }
        });
        let config =
            RecipientListConfig::new(Arc::new(|_ex: &Exchange| "mock:a,mock:b".to_string()))
                .strategy(MulticastStrategy::LastWins)
                .parallel(true);

        let mut svc = RecipientListService::new(config, resolver).unwrap();
        let ex = Exchange::new(Message::new("inbound"));

        // Drive the call concurrently; release mock:b after mock:a has had a
        // chance to error first.
        let join = tokio::spawn(async move { svc.ready().await.unwrap().call(ex).await });
        // Yield the runtime so mock:a (synchronous Err) completes before mock:b.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        let _ = tx.send(());
        let result = join.await.unwrap();

        assert!(
            matches!(result, Err(CamelError::Config(ref m)) if m == "par-err-b"),
            "representative error must be the last failing task to complete (mock:b), got: {result:?}"
        );
    }

    #[tokio::test]
    async fn recipient_list_partial_success_aggregates_and_returns_ok() {
        // ADR-0058: partial success (>=1 Ok) MUST aggregate over successes and
        // return Ok. The invariant fires only on ZERO successes.
        let call_count = Arc::new(AtomicUsize::new(0));
        let ok_count = call_count.clone();
        let resolver: camel_api::EndpointResolver = Arc::new(move |uri: &str| {
            if uri == "mock:ok" {
                let c = ok_count.clone();
                Some(BoxProcessor::from_fn(move |mut ex| {
                    c.fetch_add(1, Ordering::SeqCst);
                    ex.input.body = Body::Text(String::from("ok-body"));
                    Box::pin(async move { Ok(ex) })
                }))
            } else if uri == "mock:fail" {
                Some(BoxProcessor::from_fn(|_ex| {
                    Box::pin(async move { Err(CamelError::Config(String::from("partial-fail"))) })
                }))
            } else {
                None
            }
        });
        let config =
            RecipientListConfig::new(Arc::new(|_ex: &Exchange| "mock:fail,mock:ok".to_string()))
                .strategy(MulticastStrategy::LastWins);

        let mut svc = RecipientListService::new(config, resolver).unwrap();
        let ex = Exchange::new(Message::new("inbound"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(
            result.is_ok(),
            "partial success must return Ok, got: {result:?}"
        );
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert_eq!(result.unwrap().input.body.as_text(), Some("ok-body"));
    }

    #[tokio::test]
    async fn recipient_list_parallel_all_panic_returns_err() {
        // ADR-0058 (e_gpt review gap): a parallel recipient_list where every
        // spawned task PANICS produces only JoinError(panic) results. These
        // MUST NOT launder to Ok(original); convert to a representative error
        // so the zero-success guard fires. Cancels (self-induced abort) stay
        // ignored.
        let resolver: camel_api::EndpointResolver = Arc::new(|uri: &str| {
            if uri.starts_with("mock:panic") {
                Some(BoxProcessor::from_fn(|_ex| {
                    Box::pin(async move {
                        panic!("recipient panicked");
                    })
                }))
            } else {
                None
            }
        });
        let config = RecipientListConfig::new(Arc::new(|_ex: &Exchange| {
            "mock:panic1,mock:panic2".to_string()
        }))
        .strategy(MulticastStrategy::LastWins)
        .parallel(true);

        let mut svc = RecipientListService::new(config, resolver).unwrap();
        let ex = Exchange::new(Message::new("inbound"));
        let result = svc.ready().await.unwrap().call(ex).await;

        assert!(
            result.is_err(),
            "all-panic parallel recipient_list must return Err, not Ok(original); got: {result:?}"
        );
        assert!(
            matches!(result, Err(CamelError::ProcessorError(_))),
            "panic must surface as a ProcessorError representative"
        );
    }
}
