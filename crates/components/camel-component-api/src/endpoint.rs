use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{BodyType, BoxProcessor, CamelError, Exchange};

use crate::ProducerContext;
use crate::consumer::Consumer;
use crate::runtime_observability::RuntimeObservability;

/// A polling consumer receives messages on demand (pull model) rather than
/// being event-driven (push model).
///
/// Implement this trait on endpoints that support synchronous pull-based
/// consumption (e.g., file, FTP, JMS). Components that are purely
/// event-driven (e.g., HTTP server, Kafka) can leave the default
/// [`Endpoint::polling_consumer`] returning `None`.
#[async_trait]
pub trait PollingConsumer: Send + Sync {
    /// Receive the next available exchange within `timeout`, or `None` if none
    /// arrives. `timeout = Duration::ZERO` means non-blocking (return
    /// immediately if nothing pending).
    async fn receive(&mut self, timeout: Duration) -> Result<Option<Exchange>, CamelError>;
}

/// An Endpoint represents a source or destination in a route URI.
pub trait Endpoint: Send + Sync {
    /// The URI that identifies this endpoint.
    fn uri(&self) -> &str;

    /// Create a consumer that reads from this endpoint.
    ///
    /// `rt` provides narrow observability access (`metrics()` + `health()`)
    /// per ADR-0012 Phase A. Consumers store the Arc for later
    /// `rt.metrics().increment_errors(...)` / `rt.health().force_unhealthy_for_route(...)`
    /// calls (Phase B).
    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError>;

    /// Create a producer that writes to this endpoint.
    ///
    /// `rt` provides narrow observability access (`metrics()` + `health()`)
    /// per ADR-0012 Phase A. Producers store the Arc for later
    /// `rt.health().force_unhealthy_for_route(...)` calls on creation failure
    /// (Phase B category (g) sites).
    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError>;

    /// Optional body type contract for the producer.
    ///
    /// When `Some(t)`, the pipeline will coerce the body to `t` before calling
    /// the producer. Default: `None` (accept any body variant, zero overhead).
    fn body_contract(&self) -> Option<BodyType> {
        None
    }

    /// Return a polling consumer for this endpoint, if supported.
    ///
    /// Polling consumers use a pull model — callers invoke
    /// [`PollingConsumer::receive`] to retrieve the next message.
    /// Endpoints that only support push-based consumption should leave
    /// this default (returns `None`).
    fn polling_consumer(&self) -> Option<Box<dyn PollingConsumer>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ComponentContext;
    use crate::test_support::PanicRuntimeObservability;

    /// A minimal mock endpoint for testing default trait methods.
    struct MockEndpoint {
        uri: String,
    }

    impl MockEndpoint {
        fn new(uri: &str) -> Self {
            Self {
                uri: uri.to_string(),
            }
        }
    }

    impl Endpoint for MockEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }

        fn create_consumer(
            &self,
            _rt: Arc<dyn crate::RuntimeObservability>,
        ) -> Result<Box<dyn Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed("mock".into()))
        }

        fn create_producer(
            &self,
            _rt: Arc<dyn crate::RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Err(CamelError::ProcessorError("mock".into()))
        }
    }

    #[test]
    fn mock_endpoint_polling_consumer_returns_none() {
        let ep = MockEndpoint::new("mock://test");
        assert!(ep.polling_consumer().is_none());
    }

    #[test]
    fn mock_endpoint_body_contract_default_is_none() {
        let ep = MockEndpoint::new("mock://test");
        assert!(ep.body_contract().is_none());
    }

    #[test]
    fn mock_endpoint_uri() {
        let ep = MockEndpoint::new("mock://test");
        assert_eq!(ep.uri(), "mock://test");
    }

    #[test]
    fn mock_endpoint_create_consumer_errors() {
        let ep = MockEndpoint::new("mock://test");
        let rt: Arc<dyn crate::RuntimeObservability> = Arc::new(PanicRuntimeObservability);
        let result = ep.create_consumer(rt);
        assert!(result.is_err());
    }

    #[test]
    fn mock_endpoint_create_producer_errors() {
        let ep = MockEndpoint::new("mock://test");
        let ctx = ProducerContext::new();
        let rt: Arc<dyn crate::RuntimeObservability> = Arc::new(PanicRuntimeObservability);
        let result = ep.create_producer(rt, &ctx);
        assert!(result.is_err());
    }

    /// Verify ComponentContext can be constructed (via NoOpComponentContext).
    #[test]
    fn component_context_noop_can_be_constructed() {
        let _ctx = crate::NoOpComponentContext;
    }

    #[test]
    fn component_context_noop_resolve_returns_none() {
        let ctx = crate::NoOpComponentContext;
        assert!(ctx.resolve_component("anything").is_none());
        assert!(ctx.resolve_language("anything").is_none());
    }
}
