use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tower::Service;

use crate::error::CamelError;
use crate::exchange::Exchange;

/// A Processor is a Tower Service that transforms an Exchange.
///
/// Any type implementing `Service<Exchange, Response = Exchange, Error = CamelError>`
/// that is also `Clone + Send + Sync + 'static` automatically implements `Processor`.
pub trait Processor:
    Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + Sync + 'static
{
}

// Blanket implementation: anything satisfying the bounds is a Processor.
impl<P> Processor for P where
    P: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + Sync + 'static
{
}

/// An identity processor that passes the exchange through unchanged.
#[derive(Debug, Clone)]
pub struct IdentityProcessor;

impl Service<Exchange> for IdentityProcessor {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        Box::pin(async move { Ok(exchange) })
    }
}

/// A type-erased, cloneable processor. This is the main runtime representation
/// of a processor pipeline — a composed chain of Tower Services erased to a
/// single boxed type.
pub type BoxProcessor = tower::util::BoxCloneService<Exchange, Exchange, CamelError>;

/// Opaque newtype around [`BoxProcessor`] for `Debug` redaction.
///
/// `BoxProcessor` is a type alias for Tower's `BoxCloneService`, which doesn't
/// have a useful `Debug` impl. This newtype lets the rest of the codebase use
/// `#[derive(Debug)]` on data structures that hold processors while keeping
/// the `Debug` output bounded. Use `op.0` to get the inner `BoxProcessor`
/// for invocation or further wrapping.
///
/// Pre-v1.0: introduced to enable `#[derive(Debug)]` on `BuilderStep` (H2).
/// Once the data model is more stable, the inner type may grow a structured
/// `Debug` impl and this wrapper can be removed.
#[derive(Clone)]
pub struct OpaqueProcessor(pub BoxProcessor);

impl std::fmt::Debug for OpaqueProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BoxProcessor(...)")
    }
}

/// Thread-safe wrapper for [`BoxProcessor`].
///
/// `BoxProcessor` (`BoxCloneService`) is `Send` but not `Sync` because the
/// inner `Box<dyn CloneServiceInner>` lacks a `Sync` bound. This wrapper
/// stores the processor behind `Arc<Mutex<...>>`, providing safe `Send+Sync`
/// access. The Mutex is only held briefly during `clone()` — each caller
/// gets an independent `BoxProcessor` copy.
pub struct SyncBoxProcessor(Arc<Mutex<BoxProcessor>>);

impl SyncBoxProcessor {
    pub fn new(processor: BoxProcessor) -> Self {
        SyncBoxProcessor(Arc::new(Mutex::new(processor)))
    }

    pub fn clone_inner(&self) -> BoxProcessor {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Clone for SyncBoxProcessor {
    fn clone(&self) -> Self {
        SyncBoxProcessor(self.0.clone())
    }
}

/// Extension trait for [`BoxProcessor`] providing ergonomic constructors.
///
/// Since `BoxProcessor` is a type alias for Tower's `BoxCloneService`, we cannot
/// add inherent methods to it. This trait fills that gap.
///
/// # Example
///
/// ```ignore
/// use camel_api::{BoxProcessor, BoxProcessorExt};
///
/// let processor = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
/// ```
pub trait BoxProcessorExt {
    /// Create a [`BoxProcessor`] from an async closure.
    ///
    /// This is a convenience shorthand for `BoxProcessor::new(ProcessorFn::new(f))`.
    fn from_fn<F, Fut>(f: F) -> BoxProcessor
    where
        F: Fn(Exchange) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Exchange, CamelError>> + Send + 'static;
}

impl BoxProcessorExt for BoxProcessor {
    fn from_fn<F, Fut>(f: F) -> BoxProcessor
    where
        F: Fn(Exchange) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Exchange, CamelError>> + Send + 'static,
    {
        BoxProcessor::new(ProcessorFn::new(f))
    }
}

/// Adapts an `Fn(Exchange) -> Future<Result<Exchange>>` closure into a Tower Service.
/// This allows user-provided async closures (via `.process()`) to participate
/// in the Tower pipeline.
pub struct ProcessorFn<F> {
    f: Arc<F>,
}

// Manual Clone impl: Arc<F> is always Clone, regardless of F.
impl<F> Clone for ProcessorFn<F> {
    fn clone(&self) -> Self {
        Self {
            f: Arc::clone(&self.f),
        }
    }
}

impl<F> ProcessorFn<F> {
    pub fn new(f: F) -> Self {
        Self { f: Arc::new(f) }
    }
}

impl<F, Fut> Service<Exchange> for ProcessorFn<F>
where
    F: Fn(Exchange) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Exchange, CamelError>> + Send + 'static,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let f = Arc::clone(&self.f);
        Box::pin(async move { f(exchange).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_identity_processor_passes_through() {
        let exchange = Exchange::new(Message::new("hello"));
        let processor = IdentityProcessor;

        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("hello"));
    }

    #[tokio::test]
    async fn test_identity_processor_preserves_headers() {
        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .set_header("key", serde_json::Value::String("value".into()));

        let processor = IdentityProcessor;
        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("key"),
            Some(&serde_json::Value::String("value".into()))
        );
    }

    #[tokio::test]
    async fn test_identity_processor_preserves_properties() {
        let mut exchange = Exchange::new(Message::default());
        exchange.set_property("prop", serde_json::Value::Bool(true));

        let processor = IdentityProcessor;
        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property("prop"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[tokio::test]
    async fn test_processor_fn_transforms_exchange() {
        let processor = ProcessorFn::new(|mut ex: Exchange| async move {
            ex.input.body = crate::body::Body::Text("transformed".into());
            Ok(ex)
        });

        let exchange = Exchange::new(Message::new("original"));
        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("transformed"));
    }

    #[tokio::test]
    async fn test_processor_fn_can_return_error() {
        let processor = ProcessorFn::new(|_ex: Exchange| async move {
            Err(CamelError::ProcessorError("intentional error".into()))
        });

        let exchange = Exchange::new(Message::default());
        let result: Result<Exchange, CamelError> = processor.oneshot(exchange).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_processor_fn_is_cloneable() {
        let processor = ProcessorFn::new(|ex: Exchange| async move { Ok(ex) });
        let cloned = processor.clone();

        let exchange = Exchange::new(Message::new("test"));
        let result = cloned.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("test"));
    }

    #[tokio::test]
    async fn test_box_processor_from_identity() {
        let processor: BoxProcessor = tower::util::BoxCloneService::new(IdentityProcessor);

        let exchange = Exchange::new(Message::new("boxed"));
        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("boxed"));
    }

    #[tokio::test]
    async fn test_box_processor_from_processor_fn() {
        let processor: BoxProcessor =
            tower::util::BoxCloneService::new(ProcessorFn::new(|mut ex: Exchange| async move {
                ex.input.body = crate::body::Body::Text("via_box".into());
                Ok(ex)
            }));

        let exchange = Exchange::new(Message::new("original"));
        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("via_box"));
    }

    #[tokio::test]
    async fn test_box_processor_ext_from_fn() {
        let processor = BoxProcessor::from_fn(|mut ex: Exchange| async move {
            ex.input.body = crate::body::Body::Text("via_from_fn".into());
            Ok(ex)
        });

        let exchange = Exchange::new(Message::new("original"));
        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("via_from_fn"));
    }
}
