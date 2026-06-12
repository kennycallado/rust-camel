use std::sync::Arc;

use camel_api::{BoxProcessor, EndpointResolver, ProducerContext};
use camel_component_api::{ComponentContext, RuntimeObservability};
use camel_endpoint::parse_uri;

/// Creates an `EndpointResolver` closure that resolves a URI string to a
/// `BoxProcessor` by:
///
/// 1. Parsing the URI via `camel_endpoint::parse_uri`
/// 2. Resolving the component by scheme through `ComponentContext`
/// 3. Creating an endpoint from the URI via the component
/// 4. Creating a producer from the endpoint
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
}
