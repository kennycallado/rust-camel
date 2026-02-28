use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;
use tower::ServiceExt;

use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{BoxProcessor, CamelError, Exchange, IdentityProcessor};

/// A Route defines a message flow: from a source endpoint, through a composed
/// Tower Service pipeline.
pub struct Route {
    /// The source endpoint URI.
    pub(crate) from_uri: String,
    /// The composed processor pipeline as a type-erased Tower Service.
    pub(crate) pipeline: BoxProcessor,
}

impl Route {
    /// Create a new route from the given source URI and processor pipeline.
    pub fn new(from_uri: impl Into<String>, pipeline: BoxProcessor) -> Self {
        Self {
            from_uri: from_uri.into(),
            pipeline,
        }
    }

    /// The source endpoint URI.
    pub fn from_uri(&self) -> &str {
        &self.from_uri
    }

    /// Consume the route and return its pipeline.
    pub fn into_pipeline(self) -> BoxProcessor {
        self.pipeline
    }
}

/// A step in an unresolved route definition.
pub enum BuilderStep {
    /// A pre-built Tower processor service.
    Processor(BoxProcessor),
    /// A destination URI — resolved at start time by CamelContext.
    To(String),
}

/// An unresolved route definition. "to" URIs have not been resolved to producers yet.
pub struct RouteDefinition {
    pub(crate) from_uri: String,
    pub(crate) steps: Vec<BuilderStep>,
    /// Optional per-route error handler config. Takes precedence over the global one.
    pub(crate) error_handler: Option<ErrorHandlerConfig>,
    /// Optional circuit breaker config. Applied between error handler and step pipeline.
    pub(crate) circuit_breaker: Option<CircuitBreakerConfig>,
}

impl RouteDefinition {
    /// Create a new route definition.
    pub fn new(from_uri: impl Into<String>, steps: Vec<BuilderStep>) -> Self {
        Self {
            from_uri: from_uri.into(),
            steps,
            error_handler: None,
            circuit_breaker: None,
        }
    }

    /// The source endpoint URI.
    pub fn from_uri(&self) -> &str {
        &self.from_uri
    }

    /// The steps in this route definition.
    pub fn steps(&self) -> &[BuilderStep] {
        &self.steps
    }

    /// Set a per-route error handler, overriding the global one.
    pub fn with_error_handler(mut self, config: ErrorHandlerConfig) -> Self {
        self.error_handler = Some(config);
        self
    }

    /// Set a circuit breaker for this route.
    pub fn with_circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker = Some(config);
        self
    }

    /// Get the circuit breaker config, if set.
    pub fn circuit_breaker_config(&self) -> Option<&CircuitBreakerConfig> {
        self.circuit_breaker.as_ref()
    }
}

/// Compose a list of BoxProcessors into a single pipeline that runs them sequentially.
pub fn compose_pipeline(processors: Vec<BoxProcessor>) -> BoxProcessor {
    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }
    BoxProcessor::new(SequentialPipeline { steps: processors })
}

/// A service that executes a sequence of BoxProcessors in order.
#[derive(Clone)]
struct SequentialPipeline {
    steps: Vec<BoxProcessor>,
}

impl Service<Exchange> for SequentialPipeline {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if let Some(first) = self.steps.first_mut() {
            first.poll_ready(cx)
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let mut steps = self.steps.clone();
        Box::pin(async move {
            let mut ex = exchange;
            for step in &mut steps {
                ex = step.ready().await?.call(ex).await?;
            }
            Ok(ex)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A service that returns `Pending` on the first `poll_ready`, then `Ready`.
    #[derive(Clone)]
    struct DelayedReadyService {
        ready: Arc<AtomicBool>,
    }

    impl Service<Exchange> for DelayedReadyService {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            if self.ready.fetch_or(true, Ordering::SeqCst) {
                // Already marked ready (second+ call) → Ready
                Poll::Ready(Ok(()))
            } else {
                // First call → Pending, schedule a wake
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }

        fn call(&mut self, ex: Exchange) -> Self::Future {
            Box::pin(async move { Ok(ex) })
        }
    }

    #[test]
    fn test_pipeline_poll_ready_delegates_to_first_step() {
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let inner = DelayedReadyService {
            ready: Arc::new(AtomicBool::new(false)),
        };
        let boxed = BoxProcessor::new(inner);
        let mut pipeline = SequentialPipeline {
            steps: vec![boxed],
        };

        // First poll_ready: inner returns Pending, so pipeline must too.
        let first = pipeline.poll_ready(&mut cx);
        assert!(first.is_pending(), "expected Pending on first poll_ready");

        // Second poll_ready: inner returns Ready, so pipeline must too.
        let second = pipeline.poll_ready(&mut cx);
        assert!(second.is_ready(), "expected Ready on second poll_ready");
    }

    #[test]
    fn test_pipeline_poll_ready_with_empty_steps() {
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut pipeline = SequentialPipeline {
            steps: vec![],
        };

        // Empty pipeline should be immediately ready.
        let result = pipeline.poll_ready(&mut cx);
        assert!(result.is_ready(), "expected Ready for empty pipeline");
    }
}
