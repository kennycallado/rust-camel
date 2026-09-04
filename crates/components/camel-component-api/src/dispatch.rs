//! Inline route dispatch capability — the fast-path seam between a consumer
//! and its route pipeline.
//!
//! The camel-core runtime publishes an [`InlineRouteDispatcher`] on the
//! [`ConsumerContext`](crate::consumer::ConsumerContext) when a route's
//! concurrency model permits the inline fast path. Consumers (or the
//! producers bound to them) may then hand Exchanges straight into the
//! pipeline without a channel round-trip. The capability stays opaque:
//! implementors live in camel-core and component crates never see pipeline
//! internals (hexagonal boundary).

use std::future::Future;
use std::pin::Pin;

use camel_api::{CamelError, Exchange};

/// Opaque capability for dispatching an Exchange directly into a route
/// pipeline (request-reply), bypassing the consumer submission channel.
///
/// Set once by the camel-core runtime before the consumer starts, via
/// [`ConsumerContext::set_inline_dispatcher`](crate::consumer::ConsumerContext::set_inline_dispatcher).
/// The trait exposes ONLY `dispatch` — no pipeline or processor accessors —
/// so domain components stay framework-agnostic.
pub trait InlineRouteDispatcher: Send + Sync + 'static {
    /// Run `exchange` through the route pipeline and resolve with the
    /// processed exchange, or with the pipeline error if no error handler
    /// absorbed it.
    fn dispatch(
        &self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send + 'static>>;
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::Arc;

    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::InlineRouteDispatcher;
    use crate::consumer::{ConsumerContext, ExchangeEnvelope};
    use camel_api::{Exchange, Message};

    fn test_context() -> ConsumerContext {
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(1);
        ConsumerContext::new(tx, CancellationToken::new(), "test-route".to_string())
    }

    fn test_exchange() -> Exchange {
        Exchange::new(Message::new("payload"))
    }

    /// No-op fake: dispatch resolves with the exchange unchanged.
    struct IdentityDispatcher;

    impl InlineRouteDispatcher for IdentityDispatcher {
        fn dispatch(
            &self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = Result<Exchange, camel_api::CamelError>> + Send + 'static>>
        {
            Box::pin(async move { Ok(exchange) })
        }
    }

    /// Fake that tags the exchange with its name so tests can tell which
    /// dispatcher answered.
    struct TagDispatcher(&'static str);

    impl InlineRouteDispatcher for TagDispatcher {
        fn dispatch(
            &self,
            mut exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = Result<Exchange, camel_api::CamelError>> + Send + 'static>>
        {
            let tag = self.0;
            Box::pin(async move {
                exchange.set_property("dispatcher", tag);
                Ok(exchange)
            })
        }
    }

    #[test]
    fn inline_dispatcher_defaults_to_none() {
        let ctx = test_context();
        assert!(ctx.inline_dispatcher().is_none());
    }

    #[tokio::test]
    async fn inline_dispatcher_set_then_get_roundtrip() {
        let ctx = test_context();
        ctx.set_inline_dispatcher(Arc::new(IdentityDispatcher));

        let dispatcher = ctx.inline_dispatcher().expect("dispatcher must be set");
        // Clones observe the same capability slot.
        let clone = ctx.clone();
        assert!(clone.inline_dispatcher().is_some());

        let sent = test_exchange();
        let correlation_id = sent.correlation_id().to_string();
        let returned = dispatcher
            .dispatch(sent)
            .await
            .expect("dispatch must be Ok");
        assert_eq!(returned.correlation_id(), correlation_id);
        assert!(!returned.has_error());
    }

    #[tokio::test]
    async fn inline_dispatcher_second_set_keeps_first() {
        let ctx = test_context();
        ctx.set_inline_dispatcher(Arc::new(TagDispatcher("A")));
        ctx.set_inline_dispatcher(Arc::new(TagDispatcher("B")));

        let dispatcher = ctx.inline_dispatcher().expect("dispatcher must be set");
        let returned = dispatcher
            .dispatch(test_exchange())
            .await
            .expect("dispatch must be Ok");
        assert_eq!(
            returned.property("dispatcher").and_then(|v| v.as_str()),
            Some("A")
        );
    }
}
