//! Tower `Layer` bridge for [`BoxProcessor`] pipelines.
//!
//! [`ProcessorLayer`] is an object-safe factory that wraps a [`BoxProcessor`]
//! in a Tower layer and re-erases the result behind [`CamelError`].
//! [`CamelErrorAdapter`] bridges a service whose error is `Into<BoxError>`
//! back to `CamelError`, and [`LayerFactory`] is the cloneable, redacted
//! handle that builder steps store. [`SegmentProcessor`] runs an
//! [`OutcomeSegment`] as a `Service<Exchange>`, so segment-mode pipelines can
//! be wrapped by the same layers.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::{Layer, Service};

use crate::error::{BoxError, CamelError};
use crate::exchange::Exchange;
use crate::outcome_segment::OutcomeSegment;
use crate::pipeline_outcome::PipelineOutcome;
use crate::processor::BoxProcessor;

/// Object-safe factory that wraps a [`BoxProcessor`] in a Tower layer.
///
/// Implementations are provided by the blanket impl below, which requires the
/// layered service to be `Clone + Send + Sync + 'static` with a `Send` future
/// and an error convertible into [`BoxError`].
///
/// That blanket impl also forecloses hand-written `ProcessorLayer` impls for
/// concrete tower layers in this crate — such an impl overlaps the blanket one
/// (`E0119`), so a manual impl needs a newtype wrapper around the tower layer.
///
/// # Compile-time rejection
///
/// A layer whose service does not satisfy those bounds fails to compile at the
/// `wrap` call site; there is no runtime fallback. A direct
/// `tower::limit::RateLimit` service is the canonical example: it is not
/// `Clone`, so wrapping it fails the `Clone` bound (`E0277`, compile-probe
/// evidence in `openspec/changes/builder-layer-middleware/probe/PROBE.md`,
/// finding 3). Stateful layers such as `RateLimit` must be shared through
/// `tower::buffer::Buffer` before they can be used here.
///
/// # Example
///
/// ```
/// use std::time::Duration;
///
/// use camel_api::{BoxProcessor, Exchange, LayerFactory, Message, ProcessorFn};
/// use tower::timeout::TimeoutLayer;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let inner: BoxProcessor =
///     BoxProcessor::new(ProcessorFn::new(|ex: Exchange| async move { Ok(ex) }));
///
/// let factory = LayerFactory::new(TimeoutLayer::new(Duration::from_millis(50)));
/// let wrapped = factory.wrap(inner).expect("timeout layer is Clone + Send");
///
/// let exchange = Exchange::new(Message::new("payload"));
/// let response = tower::ServiceExt::oneshot(wrapped, exchange)
///     .await
///     .expect("a fast call passes through the timeout layer");
/// assert_eq!(response.input.body.as_text(), Some("payload"));
/// # }
/// ```
pub trait ProcessorLayer: Send + Sync + 'static {
    /// Wrap `inner` and re-erase the layered service as a [`BoxProcessor`].
    fn wrap(&self, inner: BoxProcessor) -> Result<BoxProcessor, CamelError>;
}

/// Adapts a Tower service whose error is convertible into [`BoxError`] to the
/// pipeline's [`CamelError`] error type.
///
/// Readiness and invocation both operate on `self.inner`: the adapter never
/// clones inside `call`. Tower's readiness contract reserves capacity on the
/// handle that called `poll_ready`, so a clone-per-call adapter deadlocks
/// permit-style layers (compile-probe finding P4 in
/// `openspec/changes/builder-layer-middleware/probe/PROBE.md`).
#[derive(Clone)]
pub struct CamelErrorAdapter<S> {
    inner: S,
}

impl<S> CamelErrorAdapter<S> {
    /// Wrap `inner` so its errors surface as [`CamelError`].
    ///
    /// # Example
    ///
    /// A native [`CamelError`] passes through unchanged:
    ///
    /// ```
    /// use camel_api::{BoxProcessor, CamelError, CamelErrorAdapter, Exchange, Message, ProcessorFn};
    /// use tower::{Layer, ServiceExt};
    ///
    /// /// A pass-through layer: wraps nothing, changes no behavior.
    /// #[derive(Clone)]
    /// struct Passthrough;
    ///
    /// impl<S> Layer<S> for Passthrough {
    ///     type Service = S;
    ///
    ///     fn layer(&self, inner: S) -> S {
    ///         inner
    ///     }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let failing: BoxProcessor = BoxProcessor::new(ProcessorFn::new(|_ex: Exchange| async {
    ///     Err(CamelError::ProcessorError("x".to_string()))
    /// }));
    ///
    /// let adapter = CamelErrorAdapter::new(Passthrough.layer(failing));
    /// let err = ServiceExt::oneshot(adapter, Exchange::new(Message::new("x")))
    ///     .await
    ///     .expect_err("the inner service always fails");
    ///
    /// assert!(
    ///     matches!(err, CamelError::ProcessorError(ref msg) if msg == "x"),
    ///     "CamelError must pass through unchanged, got: {err:?}"
    /// );
    /// # }
    /// ```
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> Service<Exchange> for CamelErrorAdapter<S>
where
    S: Service<Exchange, Response = Exchange> + Clone + Send + Sync + 'static,
    S::Error: Into<BoxError>,
    S::Future: Send,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner
            .poll_ready(cx)
            .map(|r| r.map_err(|e| CamelError::from(e.into())))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        // tower readiness contract: poll_ready reserved capacity on THIS handle;
        // call must consume it on the same handle. Cloning here (permit-style
        // layers) deadlocks: the original handle keeps its unconsumed permit
        // while the clone waits for another one.
        let fut = self.inner.call(exchange);
        Box::pin(async move { fut.await.map_err(|e| CamelError::from(e.into())) })
    }
}

impl<L> ProcessorLayer for L
where
    L: Layer<BoxProcessor> + Send + Sync + 'static,
    L::Service: Service<Exchange, Response = Exchange> + Clone + Send + Sync + 'static,
    <L::Service as Service<Exchange>>::Error: Into<BoxError>,
    <L::Service as Service<Exchange>>::Future: Send,
{
    fn wrap(&self, inner: BoxProcessor) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(CamelErrorAdapter::new(self.layer(inner))))
    }
}

/// Cloneable handle to a [`ProcessorLayer`].
///
/// `Debug` is redacted to `LayerFactory(...)` so pipeline snapshots stay
/// printable regardless of the wrapped layer's `Debug` (same style as
/// [`OpaqueProcessor`](crate::processor::OpaqueProcessor)).
///
/// Equality is `Arc` pointer identity, not structural: two factories built
/// from equal-but-distinct layer values compare unequal. Builder tests rely on
/// this to assert factory ORDER in nesting assertions while the layer itself
/// stays private.
#[derive(Clone)]
pub struct LayerFactory(Arc<dyn ProcessorLayer>);

impl LayerFactory {
    /// Build a factory from any [`ProcessorLayer`] implementation.
    ///
    /// # Example
    ///
    /// A `TimeoutLayer` turns a slow producer into a timeout-classified error:
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use camel_api::{BoxProcessor, CamelError, Exchange, LayerFactory, Message, ProcessorFn};
    /// use tower::timeout::TimeoutLayer;
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let producer: BoxProcessor = BoxProcessor::new(ProcessorFn::new(|_ex: Exchange| async {
    ///     tokio::time::sleep(Duration::from_secs(5)).await;
    ///     Ok(Exchange::new(Message::new("late")))
    /// }));
    ///
    /// let factory = LayerFactory::new(TimeoutLayer::new(Duration::from_millis(50)));
    /// let wrapped = factory.wrap(producer).expect("timeout layer is Clone + Send");
    ///
    /// // The 50ms timeout must fire well inside this 1s guard.
    /// let result = tokio::time::timeout(
    ///     Duration::from_secs(1),
    ///     tower::ServiceExt::oneshot(wrapped, Exchange::new(Message::new("x"))),
    /// )
    /// .await
    /// .expect("the timeout must fire before the 1s guard");
    /// let err = result.expect_err("the 50ms timeout must fire before the 5s producer finishes");
    /// assert!(
    ///     err.to_string().contains("middleware timeout elapsed"),
    ///     "expected a timeout-classified CamelError, got: {err}"
    /// );
    ///
    /// // Walk the public `source()` chain: the opaque payload returns the
    /// // original error directly from `source()`, so the concrete `Elapsed` is
    /// // the first chain node (reading the payload directly would bypass the
    /// // consumer path).
    /// let mut current = std::error::Error::source(&err);
    /// let mut reached_elapsed = false;
    /// for _ in 0..3 {
    ///     let Some(node) = current else { break };
    ///     if node.downcast_ref::<tower::timeout::error::Elapsed>().is_some() {
    ///         reached_elapsed = true;
    ///         break;
    ///     }
    ///     current = std::error::Error::source(node);
    /// }
    /// assert!(
    ///     reached_elapsed,
    ///     "source chain must reach tower Elapsed: {err:?}"
    /// );
    /// # }
    /// ```
    pub fn new(layer: impl ProcessorLayer + 'static) -> Self {
        Self(Arc::new(layer))
    }

    /// Wrap `inner` with the layer held by this factory.
    pub fn wrap(&self, inner: BoxProcessor) -> Result<BoxProcessor, CamelError> {
        self.0.wrap(inner)
    }
}

impl std::fmt::Debug for LayerFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LayerFactory(...)")
    }
}

impl PartialEq for LayerFactory {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Runs an [`OutcomeSegment`] as a Tower `Service<Exchange>`.
///
/// Bridges segment-mode pipelines (which produce [`PipelineOutcome`]) into the
/// Tower-result world so they can be wrapped by [`ProcessorLayer`]
/// implementations. `Completed` and `Failed` map straight through;
/// `Stopped(ex)` maps to `Ok(ex)` after UNCONDITIONALLY setting the
/// `CamelStop` property to `true`, so `run_steps` re-derives the stop at the
/// next `Process` step. The unconditional overwrite is deliberate: an exchange
/// carrying `CamelStop = false` from an earlier step must not defeat the
/// re-derivation.
#[derive(Clone)]
pub struct SegmentProcessor {
    inner: OutcomeSegment,
}

impl SegmentProcessor {
    /// Wrap `inner` as a Tower service.
    ///
    /// # Example
    ///
    /// A `Stopped` outcome leaves the wrapper as `Ok(ex)` with `CamelStop`
    /// forced to `true`, even though the incoming exchange carried
    /// `CamelStop = false`:
    ///
    /// ```
    /// use std::future::Future;
    /// use std::pin::Pin;
    ///
    /// use camel_api::{
    ///     Exchange, Message, OutcomePipeline, OutcomeSegment, PipelineOutcome, SegmentProcessor,
    ///     Value, is_camel_stop,
    /// };
    /// use tower::ServiceExt;
    ///
    /// /// Always reports `Stopped`, leaving the exchange untouched.
    /// #[derive(Clone)]
    /// struct StoppedPipeline;
    ///
    /// impl OutcomePipeline for StoppedPipeline {
    ///     fn clone_box(&self) -> Box<dyn OutcomePipeline> {
    ///         Box::new(self.clone())
    ///     }
    ///
    ///     fn run<'a>(
    ///         &'a mut self,
    ///         exchange: Exchange,
    ///     ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
    ///         Box::pin(async move { PipelineOutcome::Stopped(exchange) })
    ///     }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let mut exchange = Exchange::new(Message::new("x"));
    /// exchange.set_property(camel_api::CAMEL_STOP, Value::Bool(false));
    ///
    /// let processor = SegmentProcessor::new(OutcomeSegment::new(Box::new(StoppedPipeline)));
    /// let response = ServiceExt::oneshot(processor, exchange)
    ///     .await
    ///     .expect("a stopped outcome is not an error");
    ///
    /// assert!(
    ///     is_camel_stop(&response),
    ///     "the wrapper must force CamelStop=true"
    /// );
    /// # }
    /// ```
    pub fn new(inner: OutcomeSegment) -> Self {
        Self { inner }
    }
}

impl Service<Exchange> for SegmentProcessor {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Segments are stateless w.r.t. capacity: `run_steps` also treats the
        // Segment arm as always ready.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        // Segments run through `&mut self`; clone the handle so the future owns
        // an independent segment. The clone walks the pipeline tree via
        // `clone_box`, so its cost is bounded by the compile-time tree — the
        // same cost `run_steps` already pays on every Segment step.
        let mut segment = self.inner.clone();
        Box::pin(async move {
            match segment.run(exchange).await {
                PipelineOutcome::Stopped(mut ex) => {
                    // Unconditional overwrite: an earlier `CamelStop=false`
                    // must not defeat re-derivation by `run_steps`.
                    ex.set_property(crate::exchange::CAMEL_STOP, serde_json::Value::Bool(true));
                    Ok(ex)
                }
                // `Completed`/`Failed` delegate to the single canonical
                // PipelineOutcome -> Result translation (ADR-0024 §3.5).
                outcome => outcome.into_tower_result(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use tokio::time::sleep;
    use tower::ServiceExt;
    use tower::timeout::TimeoutLayer;

    use crate::message::Message;
    use crate::outcome_pipeline::OutcomePipeline;
    use crate::outcome_segment::OutcomeSegment;
    use crate::pipeline_outcome::PipelineOutcome;
    use crate::processor::ProcessorFn;

    /// Outcome a [`StubPipeline`] reports, so `SegmentProcessor` mapping can be
    /// exercised without a real EIP.
    #[derive(Clone, Copy)]
    enum StubOutcome {
        Completed,
        Stopped,
        Failed,
    }

    /// Minimal [`OutcomePipeline`] stub: returns the configured outcome and
    /// leaves the exchange exactly as received.
    #[derive(Clone)]
    struct StubPipeline {
        outcome: StubOutcome,
    }

    impl StubPipeline {
        fn new(outcome: StubOutcome) -> Self {
            Self { outcome }
        }
    }

    impl OutcomePipeline for StubPipeline {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(self.clone())
        }

        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let outcome = self.outcome;
            Box::pin(async move {
                match outcome {
                    StubOutcome::Completed => PipelineOutcome::Completed(exchange),
                    StubOutcome::Stopped => PipelineOutcome::Stopped(exchange),
                    StubOutcome::Failed => {
                        PipelineOutcome::Failed(CamelError::ProcessorError("x".to_string()))
                    }
                }
            })
        }
    }

    fn sleeping_processor(delay: Duration) -> BoxProcessor {
        BoxProcessor::new(ProcessorFn::new(move |ex: Exchange| async move {
            sleep(delay).await;
            Ok(ex)
        }))
    }

    fn identity_processor() -> BoxProcessor {
        BoxProcessor::new(ProcessorFn::new(|ex: Exchange| async move { Ok(ex) }))
    }

    /// Walks the public error chain looking for the Tower timeout marker.
    ///
    /// The [`CamelError::ProcessorErrorWithSource`] payload is opaque, so the
    /// chain is traversed through [`std::error::Error::source`] exactly as a
    /// consumer would: the original error is the first node `source()` returns.
    fn source_chain_reaches_elapsed(err: &CamelError) -> bool {
        let mut current = std::error::Error::source(err);
        for _ in 0..3 {
            let Some(node) = current else { return false };
            if node
                .downcast_ref::<tower::timeout::error::Elapsed>()
                .is_some()
            {
                return true;
            }
            current = std::error::Error::source(node);
        }
        false
    }

    #[tokio::test]
    async fn layer_factory_wraps_timeout_layer() {
        let factory = LayerFactory::new(TimeoutLayer::new(Duration::from_millis(50)));
        let wrapped = factory
            .wrap(sleeping_processor(Duration::from_secs(5)))
            .expect("timeout layer must satisfy the ProcessorLayer bounds");

        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            wrapped.oneshot(Exchange::new(Message::new("x"))),
        )
        .await
        .expect("the 50ms timeout must return well inside the 1s guard");

        let err = result.expect_err("the sleeping producer must be timed out");
        assert!(
            source_chain_reaches_elapsed(&err),
            "source chain must reach tower::timeout::error::Elapsed: {err:?}"
        );
        assert!(
            err.to_string().contains("middleware timeout elapsed"),
            "unexpected error classification: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "timeout fired too late: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn layer_factory_is_clone_and_debug_redacted() {
        let factory = LayerFactory::new(TimeoutLayer::new(Duration::from_millis(50)));
        assert_eq!(format!("{factory:?}"), "LayerFactory(...)");

        let clone = factory.clone();
        let wrapped = clone
            .wrap(identity_processor())
            .expect("the clone must wrap the same layer");
        wrapped
            .oneshot(Exchange::new(Message::new("clone")))
            .await
            .expect("a fast call passes through the cloned factory");
    }

    #[test]
    fn layer_factory_partial_eq_ptr_identity() {
        let original = LayerFactory::new(TimeoutLayer::new(Duration::from_millis(50)));
        let cloned = original.clone();
        let different = LayerFactory::new(TimeoutLayer::new(Duration::from_millis(50)));

        assert_eq!(original, cloned, "a clone shares the same Arc");
        assert_ne!(
            original, different,
            "distinct layer values are distinct factories"
        );
    }

    #[tokio::test]
    async fn segment_processor_remarks_stopped_as_camel_stop_true() {
        // An earlier step explicitly cleared the stop marker; a Stopped outcome
        // must still leave the wrapper marked, unconditionally.
        let mut exchange = Exchange::new(Message::new("stopped"));
        exchange.set_property(crate::CAMEL_STOP, serde_json::Value::Bool(false));

        let processor = SegmentProcessor::new(OutcomeSegment::new(Box::new(StubPipeline::new(
            StubOutcome::Stopped,
        ))));

        let response = processor
            .oneshot(exchange)
            .await
            .expect("a stopped segment outcome is Ok at the Tower boundary");

        assert!(
            crate::is_camel_stop(&response),
            "SegmentProcessor must force CamelStop=true on the Stopped arm"
        );
    }

    #[tokio::test]
    async fn segment_processor_passes_completed_and_failed() {
        let completed = SegmentProcessor::new(OutcomeSegment::new(Box::new(StubPipeline::new(
            StubOutcome::Completed,
        ))));
        let response = completed
            .oneshot(Exchange::new(Message::new("done")))
            .await
            .expect("Completed maps to Ok");
        assert_eq!(response.input.body.as_text(), Some("done"));

        let failed = SegmentProcessor::new(OutcomeSegment::new(Box::new(StubPipeline::new(
            StubOutcome::Failed,
        ))));
        let err = failed
            .oneshot(Exchange::new(Message::new("boom")))
            .await
            .expect_err("Failed maps to Err");
        assert!(
            matches!(err, CamelError::ProcessorError(ref msg) if msg == "x"),
            "unexpected error: {err:?}"
        );
    }
}
