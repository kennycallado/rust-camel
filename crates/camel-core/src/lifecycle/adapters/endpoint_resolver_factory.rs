use std::sync::Arc;

use camel_api::{BoxProcessor, EndpointResolver, ProducerContext};
use camel_component_api::{ComponentContext, RuntimeObservability};
use camel_endpoint::parse_uri;

/// Recorded family counting successful endpoint creations in the resolver
/// path, labeled `component=<URI scheme>`. Exports as
/// `camel_core_endpoints_created_total` (rc-haik: the dotted `camel.` form
/// is treated as already-prefixed; dots sanitize to underscores).
const ENDPOINTS_CREATED_COUNTER: &str = "camel.core.endpoints_created_total";

/// Creates an `EndpointResolver` closure that resolves a URI string to a
/// `BoxProcessor` by:
///
/// 1. Parsing the URI via `camel_endpoint::parse_uri`
/// 2. Resolving the component by scheme through `ComponentContext`
/// 3. Creating an endpoint from the URI via the component
/// 4. Creating a producer from the endpoint
///
/// Every successful endpoint creation is counted in the shared metrics
/// family `camel_core_endpoints_created_total{component=<scheme>}` so
/// per-endpoint state churn under dynamic URIs is visible without source
/// reading (bd rc-haik).
///
/// Returns `None` at any step that fails (parse error, unknown scheme,
/// endpoint creation failure, producer creation failure).
///
/// This replaces 6 identical inline closure definitions in `step_resolution.rs`
/// (DynamicRouter, DeclarativeDynamicRouter, RoutingSlip, DeclarativeRoutingSlip,
/// RecipientList, DeclarativeRecipientList).
pub fn make_endpoint_resolver(
    component_ctx: Arc<dyn ComponentContext>,
    rt: Arc<dyn RuntimeObservability>,
    producer_ctx: ProducerContext,
) -> EndpointResolver {
    Arc::new(move |uri: &str| {
        let parsed = match parse_uri(uri) {
            Ok(p) => p,
            Err(_) => return None,
        };
        let component = match component_ctx.resolve_component(&parsed.scheme) {
            Some(c) => c,
            None => return None,
        };
        let endpoint = match component.create_endpoint(uri, component_ctx.as_ref()) {
            Ok(e) => e,
            Err(_) => return None,
        };
        // allow-open-label rc-haik (component label: URI scheme, bounded by registered components)
        rt.metrics().record_counter(
            ENDPOINTS_CREATED_COUNTER,
            1.0,
            &[("component", parsed.scheme.as_str())],
        );
        let producer = match endpoint.create_producer(Arc::clone(&rt), &producer_ctx) {
            Ok(p) => p,
            Err(_) => return None,
        };
        Some(BoxProcessor::new(producer))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use camel_api::{BoxProcessorExt, CamelError, MetricsCollector};
    use camel_component_api::{Component, Consumer, Endpoint};
    use std::sync::Mutex;

    #[test]
    fn make_endpoint_resolver_unknown_scheme_returns_none() {
        let component_ctx: Arc<dyn ComponentContext> =
            Arc::new(camel_component_api::NoOpComponentContext);
        let rt: Arc<dyn RuntimeObservability> = Arc::new(camel_component_api::NoOpComponentContext);
        let producer_ctx = ProducerContext::new();

        let resolver = make_endpoint_resolver(component_ctx, rt, producer_ctx);
        let result = resolver("unknown://test");
        assert!(result.is_none());
    }

    /// One recorded counter: family name plus its label pairs.
    type RecordedCounter = (String, Vec<(String, String)>);

    /// Recording `MetricsCollector` capturing every `record_counter` call
    /// as `(family name, label pairs)` (pattern: `RecMetrics` in
    /// `context_tests.rs`).
    struct CountingMetrics {
        counters: Mutex<Vec<RecordedCounter>>,
    }

    impl CountingMetrics {
        fn new() -> Self {
            Self {
                counters: Mutex::new(Vec::new()),
            }
        }

        fn counter_calls(&self) -> Vec<RecordedCounter> {
            self.counters.lock().expect("counters lock").clone()
        }

        /// Number of counter records for `name` carrying
        /// `component=<scheme>`.
        fn count_for(&self, name: &str, scheme: &str) -> usize {
            self.counters
                .lock()
                .expect("counters lock")
                .iter()
                .filter(|(n, labels)| {
                    n == name && labels.iter().any(|(k, v)| k == "component" && v == scheme)
                })
                .count()
        }
    }

    impl MetricsCollector for CountingMetrics {
        fn record_exchange_duration(&self, _: &str, _: std::time::Duration) {}
        fn increment_errors(&self, _: &str, _: &str) {}
        fn increment_exchanges(&self, _: &str) {}
        fn set_queue_depth(&self, _: &str, _: usize) {}
        fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
        fn record_counter(&self, name: &str, _value: f64, labels: &[(&str, &str)]) {
            self.counters.lock().expect("counters lock").push((
                name.to_string(),
                labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ));
        }
    }

    /// Endpoint stub whose producer always succeeds (the resolver counts on
    /// endpoint creation, so the producer must not abort the path).
    struct StubEndpoint {
        uri: String,
    }

    impl Endpoint for StubEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }

        fn create_consumer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
        ) -> Result<Box<dyn Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed("not a consumer".into()))
        }

        fn create_producer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Ok(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })))
        }
    }

    /// Component stub vending `StubEndpoint` under a configurable scheme.
    struct StubComponent {
        scheme: String,
    }

    #[async_trait]
    impl Component for StubComponent {
        fn scheme(&self) -> &str {
            &self.scheme
        }

        fn create_endpoint(
            &self,
            uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(StubEndpoint {
                uri: uri.to_string(),
            }))
        }
    }

    /// `ComponentContext` resolving only the `direct` and `mock` schemes
    /// and exposing the shared recording collector through `metrics()`
    /// (the blanket impl makes it a `RuntimeObservability`, mirroring the
    /// `Arc<CamelContext>` pattern in `context_tests.rs`).
    struct TwoSchemeContext {
        metrics: Arc<CountingMetrics>,
    }

    impl ComponentContext for TwoSchemeContext {
        fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
            match scheme {
                "direct" | "mock" => Some(Arc::new(StubComponent {
                    scheme: scheme.to_string(),
                })),
                _ => None,
            }
        }
        fn resolve_language(&self, _name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
            None
        }
        fn metrics(&self) -> Arc<dyn MetricsCollector> {
            Arc::clone(&self.metrics) as Arc<dyn MetricsCollector>
        }
        fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
            Arc::new(camel_api::NoopPlatformService::default())
        }
        fn register_route_health_check(
            &self,
            _route_id: &str,
            _check: Arc<dyn camel_api::AsyncHealthCheck>,
        ) {
        }
        fn unregister_route_health_check(&self, _route_id: &str) {}
    }

    /// Build a resolver over a context that records into `metrics`.
    fn counting_resolver(metrics: &Arc<CountingMetrics>) -> EndpointResolver {
        let rt: Arc<dyn RuntimeObservability> = Arc::new(TwoSchemeContext {
            metrics: Arc::clone(metrics),
        });
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(TwoSchemeContext {
            metrics: Arc::clone(metrics),
        });
        make_endpoint_resolver(component_ctx, rt, ProducerContext::new())
    }

    /// rc-haik: every successful endpoint creation in the resolver path is
    /// counted once, labeled with the URI scheme of the created endpoint.
    #[test]
    fn endpoints_created_counter_increments_per_created_endpoint() {
        let metrics = Arc::new(CountingMetrics::new());
        let resolver = counting_resolver(&metrics);

        assert!(resolver("direct:one").is_some(), "direct endpoint resolves");
        assert!(resolver("mock:two").is_some(), "mock endpoint resolves");

        assert_eq!(
            metrics.count_for("camel.core.endpoints_created_total", "direct"),
            1,
            "exactly one increment under component=direct"
        );
        assert_eq!(
            metrics.count_for("camel.core.endpoints_created_total", "mock"),
            1,
            "exactly one increment under component=mock"
        );
        let calls = metrics.counter_calls();
        assert_eq!(
            calls.len(),
            2,
            "one counter record per created endpoint, no others: {calls:?}"
        );
    }

    /// rc-haik: repeated dynamic URIs for the SAME component accumulate on
    /// that component's label (spec scenario "repeated dynamic URIs
    /// accumulate") — N creations under `component=direct` record N
    /// increments, so per-endpoint state churn is visible as a total.
    #[test]
    fn endpoints_created_counter_accumulates_per_component() {
        let metrics = Arc::new(CountingMetrics::new());
        let resolver = counting_resolver(&metrics);

        for uri in ["direct:alpha", "direct:beta", "direct:gamma"] {
            assert!(resolver(uri).is_some(), "{uri} resolves");
        }

        assert_eq!(
            metrics.count_for("camel.core.endpoints_created_total", "direct"),
            3,
            "three creations via distinct dynamic URIs accumulate to 3 under component=direct"
        );
    }

    /// rc-haik: the recorded family name is exactly the dotted form that
    /// exports as `camel_core_endpoints_created_total` (rc-haik treats a
    /// leading `camel.` as already-prefixed; dots sanitize to underscores).
    /// No cheap registry-export harness exists at camel-core unit tier, so
    /// per the bd the name assertion runs against the recording collector.
    #[test]
    fn endpoints_created_counter_in_exported_registry() {
        let metrics = Arc::new(CountingMetrics::new());
        let resolver = counting_resolver(&metrics);
        assert!(resolver("direct:one").is_some());

        let calls = metrics.counter_calls();
        assert_eq!(calls.len(), 1, "exactly one record: {calls:?}");
        assert_eq!(
            calls[0].0, "camel.core.endpoints_created_total",
            "recorded family must be the rc-haik dotted form"
        );
    }
}
