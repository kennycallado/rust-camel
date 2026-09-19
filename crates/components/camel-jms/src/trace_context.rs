//! W3C trace-context injection for bridge gRPC RPCs (`otel` feature).
//!
//! Two injection points feed the same rule set (never overwrite an existing
//! `traceparent`; skip entries that are not valid gRPC metadata):
//!
//! - [`TraceContextInterceptor`] attaches the *ambient* OpenTelemetry context
//!   to every RPC issued through
//!   [`bridge_service_client`](crate::component::bridge_service_client)
//!   (health checks, subscribe, and sends without an exchange).
//! - [`apply_exchange_trace_context`] attaches the context carried by an
//!   [`Exchange`] to an explicit outgoing request (producer send path), so an
//!   exchange-scoped trace wins over the ambient one.

use std::collections::HashMap;
use std::str::FromStr;

use camel_component_api::Exchange;
use tonic::metadata::{MetadataKey, MetadataMap, MetadataValue};
use tonic::service::Interceptor;
use tonic::{Request, Status};

use camel_otel::{Context, TRACE_PARENT_HEADER, inject_context, inject_from_exchange};

/// Injects the ambient OpenTelemetry context into every outgoing bridge RPC.
///
/// When the request already carries a `traceparent` header it is forwarded
/// untouched. Infallible by design: header entries that cannot be represented
/// as gRPC metadata are skipped and the request is always returned unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TraceContextInterceptor;

impl Interceptor for TraceContextInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        if request.metadata().get(TRACE_PARENT_HEADER).is_none() {
            let mut headers = HashMap::new();
            inject_context(&Context::current(), &mut headers);
            insert_trace_headers(&headers, request.metadata_mut());
        }
        Ok(request)
    }
}

/// Inject the trace context carried by `exchange` into outgoing gRPC metadata.
///
/// The exchange-scoped context takes precedence over the ambient one: when the
/// metadata already contains a `traceparent` (e.g. injected earlier from the
/// exchange), [`TraceContextInterceptor`] leaves it untouched. Entries that
/// fail to parse as gRPC metadata are skipped.
pub(crate) fn apply_exchange_trace_context(exchange: &Exchange, metadata: &mut MetadataMap) {
    if metadata.get(TRACE_PARENT_HEADER).is_some() {
        return;
    }
    let mut headers = HashMap::new();
    inject_from_exchange(exchange, &mut headers);
    insert_trace_headers(&headers, metadata);
}

/// Insert propagation headers into gRPC metadata, skipping entries whose key
/// or value is not valid gRPC metadata.
fn insert_trace_headers(headers: &HashMap<String, String>, metadata: &mut MetadataMap) {
    for (key, value) in headers {
        let (Ok(key), Ok(value)) = (MetadataKey::from_str(key), MetadataValue::from_str(value))
        else {
            continue;
        };
        metadata.insert(key, value);
    }
}

#[cfg(all(test, feature = "otel"))]
mod tests {
    use super::*;
    use camel_component_api::{Body, Message};

    /// Traceparent carried by the exchange in the injection tests.
    const EXCHANGE_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    /// Build an exchange whose `otel_context` holds the remote span context
    /// extracted from `traceparent` (the production extraction path).
    fn exchange_with_traceparent(traceparent: &str) -> Exchange {
        let mut exchange = Exchange::new(Message::new(Body::Empty));
        let mut headers = HashMap::new();
        headers.insert(TRACE_PARENT_HEADER.to_string(), traceparent.to_string());
        camel_otel::propagation::extract_into_exchange(&mut exchange, &headers);
        exchange
    }

    #[test]
    fn exchange_trace_context_sets_traceparent() {
        let exchange = exchange_with_traceparent(EXCHANGE_TRACEPARENT);
        let mut metadata = MetadataMap::new();

        apply_exchange_trace_context(&exchange, &mut metadata);

        let tp = metadata
            .get(TRACE_PARENT_HEADER)
            .expect("traceparent key present")
            .to_str()
            .expect("traceparent is ascii");
        assert_eq!(tp, EXCHANGE_TRACEPARENT);
    }

    #[test]
    fn preseeded_traceparent_is_not_overwritten() {
        let preseeded = "00-11111111111111111111111111111111-2222222222222222-01";
        let exchange = exchange_with_traceparent(EXCHANGE_TRACEPARENT);
        let mut metadata = MetadataMap::new();
        metadata.insert(
            MetadataKey::from_str(TRACE_PARENT_HEADER).expect("valid metadata key"),
            MetadataValue::from_str(preseeded).expect("valid metadata value"),
        );

        apply_exchange_trace_context(&exchange, &mut metadata);

        let tp = metadata
            .get(TRACE_PARENT_HEADER)
            .expect("traceparent key present")
            .to_str()
            .expect("traceparent is ascii");
        assert_eq!(tp, preseeded, "preseeded traceparent must win");
    }
}
