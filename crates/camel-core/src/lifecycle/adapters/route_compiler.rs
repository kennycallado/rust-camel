// adapters/route_compiler.rs
// Pipeline compilation functions: compose BuilderSteps into a Tower BoxProcessor.
// Tower types live here as this is the adapter layer responsible for
// translating declarative route definitions into executable pipelines.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;
use tower::ServiceExt;

use camel_api::metrics::MetricsCollector;
use camel_api::{BoxProcessor, CamelError, Exchange, IdentityProcessor};

use crate::shared::observability::adapters::TracingProcessor;
use crate::shared::observability::domain::DetailLevel;

/// Compose a list of BoxProcessors into a single pipeline that runs them sequentially.
pub fn compose_pipeline(processors: Vec<BoxProcessor>) -> BoxProcessor {
    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }
    BoxProcessor::new(SequentialPipeline { steps: processors })
}

/// Compose a list of BoxProcessors into a traced pipeline.
///
/// Each processor is wrapped with TracingProcessor to emit spans for observability.
/// When tracing is disabled, falls back to plain compose_pipeline with zero overhead.
pub fn compose_traced_pipeline(
    processors: Vec<BoxProcessor>,
    route_id: &str,
    trace_enabled: bool,
    detail_level: DetailLevel,
    metrics: Option<Arc<dyn MetricsCollector>>,
) -> BoxProcessor {
    if !trace_enabled {
        return compose_pipeline(processors);
    }

    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }

    let wrapped: Vec<BoxProcessor> = processors
        .into_iter()
        .enumerate()
        .map(|(idx, processor)| {
            BoxProcessor::new(TracingProcessor::new(
                processor,
                route_id.to_string(),
                idx,
                detail_level.clone(),
                metrics.clone(),
            ))
        })
        .collect();

    BoxProcessor::new(TracedPipeline { steps: wrapped })
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

/// A traced service pipeline for wrapped processors.
#[derive(Clone)]
struct TracedPipeline {
    steps: Vec<BoxProcessor>,
}

impl Service<Exchange> for TracedPipeline {
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
    use camel_api::BoxProcessorExt;
    use std::sync::atomic::{AtomicBool, Ordering};

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
                Poll::Ready(Ok(()))
            } else {
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
        let mut pipeline = SequentialPipeline { steps: vec![boxed] };

        let first = pipeline.poll_ready(&mut cx);
        assert!(first.is_pending(), "expected Pending on first poll_ready");

        let second = pipeline.poll_ready(&mut cx);
        assert!(second.is_ready(), "expected Ready on second poll_ready");
    }

    #[test]
    fn test_pipeline_poll_ready_with_empty_steps() {
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut pipeline = SequentialPipeline { steps: vec![] };
        let result = pipeline.poll_ready(&mut cx);
        assert!(result.is_ready(), "expected Ready for empty pipeline");
    }

    #[tokio::test]
    async fn test_pipeline_stops_gracefully_on_stopped_error() {
        let after_called = Arc::new(AtomicBool::new(false));
        let after_called_clone = after_called.clone();

        let stop_step = BoxProcessor::from_fn(|_ex| Box::pin(async { Err(CamelError::Stopped) }));
        let after_step = BoxProcessor::from_fn(move |ex| {
            after_called_clone.store(true, Ordering::SeqCst);
            Box::pin(async move { Ok(ex) })
        });

        let mut pipeline = SequentialPipeline {
            steps: vec![stop_step, after_step],
        };

        let ex = Exchange::new(camel_api::Message::new("hello"));
        let result = pipeline.call(ex).await;

        assert!(
            matches!(result, Err(CamelError::Stopped)),
            "expected Err(Stopped), got: {:?}",
            result
        );
        assert!(
            !after_called.load(Ordering::SeqCst),
            "step after stop should not be called"
        );
    }

    #[tokio::test]
    async fn test_compose_traced_pipeline_disabled() {
        let pipeline =
            compose_traced_pipeline(vec![], "test-route", false, DetailLevel::Minimal, None);
        let ex = Exchange::new(camel_api::Message::new("hello"));
        let result = tower::ServiceExt::oneshot(pipeline, ex).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_compose_traced_pipeline_enabled() {
        let step = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
        let pipeline =
            compose_traced_pipeline(vec![step], "test-route", true, DetailLevel::Minimal, None);
        let ex = Exchange::new(camel_api::Message::new("hello"));
        let result = tower::ServiceExt::oneshot(pipeline, ex).await;
        assert!(result.is_ok());
    }
}
