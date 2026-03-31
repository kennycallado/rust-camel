// crates/camel-core/src/lifecycle/adapters/exchange_uow.rs
//! Exchange Unit of Work Tower layer.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tower::{Layer, Service, ServiceExt};

use camel_api::{BoxProcessor, CamelError, Exchange};

// ─── RAII Guard ──────────────────────────────────────────────────────────────

/// RAII guard that decrements the in-flight counter when dropped.
///
/// Uses `Ordering::Relaxed` intentionally: the counter is a best-effort
/// observability metric, not a synchronization primitive. Approximate
/// counts under concurrent load are acceptable — no happens-before
/// relationship with other memory is required.
pub struct InFlightGuard(Arc<AtomicU64>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

// ─── Layer ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ExchangeUoWLayer {
    counter: Arc<AtomicU64>,
    on_complete_producer: Option<BoxProcessor>,
    on_failure_producer: Option<BoxProcessor>,
}

impl ExchangeUoWLayer {
    pub fn new(
        counter: Arc<AtomicU64>,
        on_complete_producer: Option<BoxProcessor>,
        on_failure_producer: Option<BoxProcessor>,
    ) -> Self {
        Self {
            counter,
            on_complete_producer,
            on_failure_producer,
        }
    }
}

impl<S> Layer<S> for ExchangeUoWLayer
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Send + Clone + 'static,
    S::Future: Send + 'static,
{
    type Service = ExchangeUoW<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ExchangeUoW {
            inner,
            counter: Arc::clone(&self.counter),
            on_complete_producer: self.on_complete_producer.clone(),
            on_failure_producer: self.on_failure_producer.clone(),
        }
    }
}

// ─── Service ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ExchangeUoW<S> {
    inner: S,
    counter: Arc<AtomicU64>,
    on_complete_producer: Option<BoxProcessor>,
    on_failure_producer: Option<BoxProcessor>,
}

impl<S> Service<Exchange> for ExchangeUoW<S>
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Send + Clone + 'static,
    S::Future: Send + 'static,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let mut inner = self.inner.clone();
        let counter = Arc::clone(&self.counter);
        let on_complete = self.on_complete_producer.clone();
        let on_failure = self.on_failure_producer.clone();

        Box::pin(async move {
            // Relaxed: this counter is observability-only; no synchronization needed.
            counter.fetch_add(1, Ordering::Relaxed);
            let _guard = InFlightGuard(Arc::clone(&counter));
            let original = exchange.clone();

            let result = match inner.ready().await {
                Ok(svc) => svc.call(exchange).await,
                Err(err) => {
                    // Infrastructure failure: inner service not ready.
                    // Pass `original` (pre-call clone) since no post-processing occurred.
                    fire_hook(on_failure.clone(), Some(original)).await;
                    return Err(err);
                }
            };

            match &result {
                Err(_) => {
                    // Inner call returned Err: pass `original` (pre-call clone), no output exchange.
                    fire_hook(on_failure, Some(original)).await;
                }
                Ok(ex) if ex.has_error() => {
                    // Exchange-level error: pass the post-processing exchange so the hook
                    // can inspect the error flag and any headers/body set by the pipeline.
                    fire_hook(on_failure, Some(ex.clone())).await;
                }
                Ok(ex) => {
                    fire_hook(on_complete, Some(ex.clone())).await;
                }
            }

            result
        })
    }
}

async fn fire_hook(producer: Option<BoxProcessor>, exchange: Option<Exchange>) {
    let (Some(mut producer), Some(ex)) = (producer, exchange) else {
        return;
    };
    let fire = async move {
        if let Err(e) = producer.ready().await {
            tracing::warn!(error = %e, "UoW hook producer not ready");
            return;
        }
        if let Err(e) = producer.call(ex).await {
            tracing::warn!(error = %e, "UoW hook producer call failed");
        }
    };
    if tokio::time::timeout(Duration::from_secs(1), fire)
        .await
        .is_err()
    {
        tracing::warn!("UoW hook timed out after 1s (error swallowed)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessorExt, Message};
    use std::sync::atomic::Ordering;

    fn make_exchange() -> Exchange {
        Exchange::new(Message::new("test"))
    }

    fn identity() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    fn failing() -> BoxProcessor {
        BoxProcessor::from_fn(|_| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        })
    }

    fn error_exchange_proc() -> BoxProcessor {
        BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                ex.set_error(CamelError::ProcessorError("exchange error".into()));
                Ok(ex)
            })
        })
    }

    #[tokio::test]
    async fn counter_increments_then_decrements_on_success() {
        let counter = Arc::new(AtomicU64::new(0));
        let layer = ExchangeUoWLayer::new(Arc::clone(&counter), None, None);
        let svc = layer.layer(identity());
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        let _ = tower::ServiceExt::oneshot(svc, make_exchange())
            .await
            .unwrap();
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn counter_decrements_on_inner_error() {
        let counter = Arc::new(AtomicU64::new(0));
        let layer = ExchangeUoWLayer::new(Arc::clone(&counter), None, None);
        let svc = layer.layer(failing());
        let result = tower::ServiceExt::oneshot(svc, make_exchange()).await;
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn on_complete_fires_on_success() {
        let fired = Arc::new(AtomicU64::new(0));
        let fired_clone = Arc::clone(&fired);
        let hook = BoxProcessor::from_fn(move |ex| {
            fired_clone.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move { Ok(ex) })
        });
        let counter = Arc::new(AtomicU64::new(0));
        let layer = ExchangeUoWLayer::new(Arc::clone(&counter), Some(hook), None);
        let _ = tower::ServiceExt::oneshot(layer.layer(identity()), make_exchange())
            .await
            .unwrap();
        assert_eq!(fired.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn on_failure_fires_on_inner_error() {
        let fired = Arc::new(AtomicU64::new(0));
        let fired_clone = Arc::clone(&fired);
        let hook = BoxProcessor::from_fn(move |ex| {
            fired_clone.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move { Ok(ex) })
        });
        let counter = Arc::new(AtomicU64::new(0));
        let layer = ExchangeUoWLayer::new(Arc::clone(&counter), None, Some(hook));
        let _ = tower::ServiceExt::oneshot(layer.layer(failing()), make_exchange()).await;
        assert_eq!(fired.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn on_failure_fires_on_exchange_error() {
        let fired = Arc::new(AtomicU64::new(0));
        let fired_clone = Arc::clone(&fired);
        let hook = BoxProcessor::from_fn(move |ex| {
            fired_clone.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move { Ok(ex) })
        });
        let counter = Arc::new(AtomicU64::new(0));
        let layer = ExchangeUoWLayer::new(Arc::clone(&counter), None, Some(hook));
        let _ = tower::ServiceExt::oneshot(layer.layer(error_exchange_proc()), make_exchange())
            .await
            .unwrap();
        assert_eq!(fired.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn on_failure_fires_when_poll_ready_fails() {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Arc;
        use std::sync::atomic::AtomicU64;
        use std::task::{Context, Poll};
        use tower::Service;

        #[derive(Clone)]
        struct FailReadySvc {
            polls: Arc<AtomicU64>,
        }
        impl Service<Exchange> for FailReadySvc {
            type Response = Exchange;
            type Error = CamelError;
            type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;
            fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
                let n = self.polls.fetch_add(1, Ordering::Relaxed);
                if n == 0 {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Ready(Err(CamelError::ProcessorError("not ready".into())))
                }
            }
            fn call(&mut self, ex: Exchange) -> Self::Future {
                Box::pin(async move { Ok(ex) })
            }
        }

        let fired = Arc::new(AtomicU64::new(0));
        let fired_clone = Arc::clone(&fired);
        let hook = BoxProcessor::from_fn(move |ex| {
            fired_clone.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move { Ok(ex) })
        });
        let counter = Arc::new(AtomicU64::new(0));
        let layer = ExchangeUoWLayer::new(Arc::clone(&counter), None, Some(hook));
        let svc = layer.layer(FailReadySvc {
            polls: Arc::new(AtomicU64::new(0)),
        });
        let result = tower::ServiceExt::oneshot(svc, make_exchange()).await;
        assert!(result.is_err());
        assert_eq!(
            fired.load(Ordering::Relaxed),
            1,
            "on_failure must fire when poll_ready fails"
        );
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn on_complete_does_not_fire_on_exchange_error() {
        let fired = Arc::new(AtomicU64::new(0));
        let fired_clone = Arc::clone(&fired);
        let hook = BoxProcessor::from_fn(move |ex| {
            fired_clone.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move { Ok(ex) })
        });
        let counter = Arc::new(AtomicU64::new(0));
        let layer = ExchangeUoWLayer::new(Arc::clone(&counter), Some(hook), None);
        let _ = tower::ServiceExt::oneshot(layer.layer(error_exchange_proc()), make_exchange())
            .await
            .unwrap();
        assert_eq!(fired.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn hook_error_does_not_fail_exchange() {
        let bad_hook = BoxProcessor::from_fn(|_| {
            Box::pin(async { Err(CamelError::ProcessorError("hook failed".into())) })
        });
        let counter = Arc::new(AtomicU64::new(0));
        let layer = ExchangeUoWLayer::new(Arc::clone(&counter), Some(bad_hook), None);
        let result = tower::ServiceExt::oneshot(layer.layer(identity()), make_exchange()).await;
        assert!(
            result.is_ok(),
            "hook error must not fail exchange: {:?}",
            result
        );
    }

    #[test]
    fn in_flight_guard_decrements_on_drop() {
        let counter = Arc::new(AtomicU64::new(1));
        {
            let _guard = InFlightGuard(Arc::clone(&counter));
            assert_eq!(counter.load(Ordering::Relaxed), 1);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }
}
