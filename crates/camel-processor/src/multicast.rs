//! ## Stop semantics (ADR-0025)
//!
//! This segment implements `OutcomePipeline` and propagates `PipelineOutcome::Stopped(ex)` with the exchange state intact (including mutations made inside the segment body before Stop fired). See ADR-0025 §3 (stopped-exchange-state-preservation invariant).

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{
    Body, BoxProcessor, CamelError, Exchange, MulticastConfig, MulticastStrategy, Value,
};

// ── Metadata property keys ─────────────────────────────────────────────

/// Property key for the zero-based index of the endpoint being invoked.
pub const CAMEL_MULTICAST_INDEX: &str = "CamelMulticastIndex";
/// Property key indicating whether this is the last endpoint invocation.
pub const CAMEL_MULTICAST_COMPLETE: &str = "CamelMulticastComplete";

// ── MulticastService ───────────────────────────────────────────────────

/// Tower Service implementing the Multicast EIP.
///
/// Sends a message to multiple endpoints, processing each independently,
/// and then aggregating the results.
///
/// Supports both sequential and parallel processing modes, configurable
/// via [`MulticastConfig::parallel`]. When parallel mode is enabled,
/// all endpoints are invoked concurrently with optional concurrency
/// limiting via [`MulticastConfig::parallel_limit`].
#[derive(Clone)]
pub struct MulticastService {
    endpoints: Vec<BoxProcessor>,
    config: MulticastConfig,
}

impl MulticastService {
    /// Create a new `MulticastService` from a list of endpoints and a [`MulticastConfig`].
    pub fn new(endpoints: Vec<BoxProcessor>, config: MulticastConfig) -> Result<Self, CamelError> {
        config.validate()?;
        Ok(Self { endpoints, config })
    }
}

impl Service<Exchange> for MulticastService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Do NOT aggregate endpoint readiness here.
        // Each endpoint's readiness is checked per-endpoint inside
        // process_sequential / process_parallel where stop_on_exception
        // is respected. Fail-fast here would bypass that logic entirely.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let original = exchange.clone();
        let endpoints = self.endpoints.clone();
        let config = self.config.clone();

        Box::pin(async move {
            // If no endpoints, return original exchange unchanged
            if endpoints.is_empty() {
                return Ok(original);
            }

            let total = endpoints.len();

            let results = if config.parallel {
                // Process endpoints in parallel
                process_parallel(exchange, endpoints, config.parallel_limit, total).await
            } else {
                // Process each endpoint sequentially
                process_sequential(exchange, endpoints, config.stop_on_exception, total).await
            };

            // Aggregate results per strategy
            aggregate(results, original, config.aggregation)
        })
    }
}

// ── Sequential processing ──────────────────────────────────────────────

async fn process_sequential(
    exchange: Exchange,
    endpoints: Vec<BoxProcessor>,
    stop_on_exception: bool,
    total: usize,
) -> Vec<Result<Exchange, CamelError>> {
    let mut results = Vec::with_capacity(endpoints.len());

    for (i, endpoint) in endpoints.into_iter().enumerate() {
        // Clone the exchange for each endpoint
        let mut cloned_exchange = exchange.clone();

        // Set multicast metadata properties
        cloned_exchange.set_property(CAMEL_MULTICAST_INDEX, Value::from(i as i64));
        cloned_exchange.set_property(CAMEL_MULTICAST_COMPLETE, Value::Bool(i == total - 1));

        let mut endpoint = endpoint;
        match tower::ServiceExt::ready(&mut endpoint).await {
            Err(e) => {
                results.push(Err(e));
                if stop_on_exception {
                    break;
                }
            }
            Ok(svc) => {
                let result = svc.call(cloned_exchange).await;
                let is_err = result.is_err();
                results.push(result);
                if stop_on_exception && is_err {
                    break;
                }
            }
        }
    }

    results
}

// ── Parallel processing ────────────────────────────────────────────────

async fn process_parallel(
    exchange: Exchange,
    endpoints: Vec<BoxProcessor>,
    parallel_limit: Option<usize>,
    total: usize,
) -> Vec<Result<Exchange, CamelError>> {
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    let semaphore = parallel_limit.map(|limit| Arc::new(Semaphore::new(limit)));

    // Build futures for each endpoint
    let futures: Vec<_> = endpoints
        .into_iter()
        .enumerate()
        .map(|(i, mut endpoint)| {
            let mut ex = exchange.clone();
            ex.set_property(CAMEL_MULTICAST_INDEX, Value::from(i as i64));
            ex.set_property(CAMEL_MULTICAST_COMPLETE, Value::Bool(i == total - 1));
            let sem = semaphore.clone();
            async move {
                // Acquire semaphore permit if limit is set
                let _permit = match &sem {
                    Some(s) => match s.acquire().await {
                        Ok(p) => Some(p),
                        Err(_) => {
                            return Err(CamelError::ProcessorError("semaphore closed".to_string()));
                        }
                    },
                    None => None,
                };

                // Readiness errors propagate via `?` into the per-endpoint result;
                // join_all ensures all endpoints run independently (no early abort).
                tower::ServiceExt::ready(&mut endpoint).await?;
                endpoint.call(ex).await
            }
        })
        .collect();

    // Execute all futures concurrently and collect results
    futures::future::join_all(futures).await
}

// ── Aggregation ────────────────────────────────────────────────────────

fn aggregate(
    results: Vec<Result<Exchange, CamelError>>,
    original: Exchange,
    strategy: MulticastStrategy,
) -> Result<Exchange, CamelError> {
    match strategy {
        MulticastStrategy::LastWins => {
            // Return the last result (error or success).
            // If last result is Err and stop_on_exception=false, return that error.
            results.into_iter().last().unwrap_or_else(|| Ok(original))
        }
        MulticastStrategy::CollectAll => {
            // Collect all bodies into a JSON array. Errors propagate.
            let mut bodies = Vec::new();
            for result in results {
                let ex = result?;
                let value = match &ex.input.body {
                    Body::Text(s) => Value::String(s.clone()),
                    Body::Json(v) => v.clone(),
                    Body::Xml(s) => Value::String(s.clone()),
                    Body::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
                    Body::Empty => Value::Null,
                    Body::Stream(s) => serde_json::json!({
                        "_stream": {
                            "origin": s.metadata.origin,
                            "placeholder": true,
                            "hint": "Materialize exchange body with .into_bytes() before multicast aggregation"
                        }
                    }),
                };
                bodies.push(value);
            }
            let mut out = original;
            out.input.body = Body::Json(Value::Array(bodies));
            Ok(out)
        }
        MulticastStrategy::Original => Ok(original),
        MulticastStrategy::Custom(fold_fn) => {
            // Fold using the custom function, starting from the first result.
            let mut iter = results.into_iter();
            let first = iter.next().unwrap_or_else(|| Ok(original.clone()))?;
            iter.try_fold(first, |acc, next_result| {
                let next = next_result?;
                Ok(fold_fn(acc, next))
            })
        }
    }
}

#[cfg(test)]
#[path = "multicast_tests.rs"]
mod tests;
