use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tower::{Service, ServiceExt};

use camel_api::{CamelError, Exchange};

/// Configuration for [`WireTapService`].
#[derive(Clone, Default)]
pub struct WireTapConfig {
    /// Maximum number of concurrent tap tasks. `None` means unlimited (backward-compatible).
    pub max_concurrent: Option<usize>,
}

impl WireTapConfig {
    /// Create a config with a bounded concurrency limit.
    pub fn bounded(max_concurrent: usize) -> Self {
        assert!(max_concurrent > 0, "max_concurrent must be > 0");
        Self {
            max_concurrent: Some(max_concurrent),
        }
    }
}

pub struct WireTapService {
    tap_endpoint: camel_api::BoxProcessor,
    semaphore: Option<std::sync::Arc<Semaphore>>,
    in_flight: Mutex<JoinSet<()>>,
}

// Manual Clone impl: `JoinSet` is not `Clone` (it has an owned background
// driver task), so the derive cannot be used. We hand out a brand-new
// JoinSet per clone — the original's spawned tasks are aborted on the
// original's Drop, not the clone's. This restores the pre-D-L1 Clone
// contract that `camel-core`'s step compiler relies on via
// `BoxProcessor::new`.
impl Clone for WireTapService {
    fn clone(&self) -> Self {
        Self {
            tap_endpoint: self.tap_endpoint.clone(),
            semaphore: self.semaphore.clone(),
            in_flight: Mutex::new(JoinSet::new()),
        }
    }
}

impl WireTapService {
    /// Create a new `WireTapService` with default (unbounded) concurrency.
    pub fn new(tap_endpoint: camel_api::BoxProcessor) -> Self {
        Self {
            tap_endpoint,
            semaphore: None,
            in_flight: Mutex::new(JoinSet::new()),
        }
    }

    /// Create a new `WireTapService` from a [`WireTapConfig`].
    pub fn with_config(tap_endpoint: camel_api::BoxProcessor, config: WireTapConfig) -> Self {
        let semaphore = config
            .max_concurrent
            .map(|limit| std::sync::Arc::new(Semaphore::new(limit)));
        Self {
            tap_endpoint,
            semaphore,
            in_flight: Mutex::new(JoinSet::new()),
        }
    }
}

impl Service<Exchange> for WireTapService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.tap_endpoint.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let mut tap_endpoint = self.tap_endpoint.clone();
        let tap_exchange = exchange.clone();
        let semaphore = self.semaphore.clone();

        // allow-unwrap: Mutex cannot be poisoned in normal operation
        self.in_flight
            .lock()
            .expect("in_flight mutex poisoned")
            .spawn(async move {
                // Acquire semaphore permit if concurrency is bounded.
                let _permit = match &semaphore {
                    Some(sem) => match sem.acquire().await {
                        Ok(p) => Some(p),
                        Err(_) => {
                            tracing::warn!("WireTap semaphore closed, dropping tap");
                            return;
                        }
                    },
                    None => None,
                };

                if let Err(e) = tap_endpoint.ready().await {
                    tracing::warn!("WireTap endpoint poll_ready failed: {}", e);
                    return;
                }
                if let Err(e) = tap_endpoint.call(tap_exchange).await {
                    // log-policy: handler-owned
                    tracing::warn!("WireTap processing error: {}", e);
                }
            });
        Box::pin(async move { Ok(exchange) })
    }
}

/// A Tower layer that produces `WireTapService` instances.
pub struct WireTapLayer {
    tap_endpoint: camel_api::BoxProcessor,
    config: WireTapConfig,
}

impl WireTapLayer {
    /// Create a new WireTapLayer with the given tap endpoint processor (unbounded).
    pub fn new(tap_endpoint: camel_api::BoxProcessor) -> Self {
        Self {
            tap_endpoint,
            config: WireTapConfig::default(),
        }
    }

    /// Create a new WireTapLayer with bounded concurrency.
    pub fn bounded(tap_endpoint: camel_api::BoxProcessor, max_concurrent: usize) -> Self {
        Self {
            tap_endpoint,
            config: WireTapConfig::bounded(max_concurrent),
        }
    }
}

impl<S> tower::Layer<S> for WireTapLayer {
    type Service = WireTapService;

    fn layer(&self, _inner: S) -> Self::Service {
        WireTapService::with_config(self.tap_endpoint.clone(), self.config.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{BoxProcessor, BoxProcessorExt, Message};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_wire_tap_returns_original_immediately() {
        let tap_processor = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));

        let mut wire_tap = WireTapService::new(tap_processor);
        let exchange = Exchange::new(Message::new("test message"));

        let result = wire_tap
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .unwrap();

        assert_eq!(result.input.body.as_text(), Some("test message"));
    }

    #[tokio::test]
    async fn test_wire_tap_endpoint_receives_clone() {
        let received_count = Arc::new(AtomicUsize::new(0));
        let count_clone = received_count.clone();

        let tap_processor = BoxProcessor::from_fn(move |ex| {
            let count = count_clone.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let mut wire_tap = WireTapService::new(tap_processor);
        let exchange = Exchange::new(Message::new("test"));

        let _result = wire_tap
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert_eq!(received_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_wire_tap_isolates_errors() {
        let tap_processor = BoxProcessor::from_fn(|_ex| {
            Box::pin(async move { Err(CamelError::ProcessorError("tap error".into())) })
        });

        let mut wire_tap = WireTapService::new(tap_processor);
        let exchange = Exchange::new(Message::new("test"));

        let result = wire_tap.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().input.body.as_text(), Some("test"));
    }

    #[tokio::test]
    async fn test_wire_tap_layer() {
        use tower::Layer;

        let tap_processor = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));

        let layer = super::WireTapLayer::new(tap_processor);
        let inner = camel_api::IdentityProcessor;
        let mut svc = layer.layer(inner);

        let exchange = Exchange::new(Message::new("test"));
        let result = svc.ready().await.unwrap().call(exchange).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("test"));
    }

    #[tokio::test]
    async fn test_wiretap_bounded_concurrency() {
        // WireTap with max_concurrent=2: sending 3 slow exchanges must not
        // spawn more than 2 concurrent tap tasks at any point.
        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        let c = Arc::clone(&concurrent);
        let mc = Arc::clone(&max_concurrent);
        let tap_processor = BoxProcessor::from_fn(move |ex| {
            let c = Arc::clone(&c);
            let mc = Arc::clone(&mc);
            Box::pin(async move {
                let current = c.fetch_add(1, Ordering::SeqCst) + 1;
                mc.fetch_max(current, Ordering::SeqCst);
                // Hold the task open so multiple taps overlap.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                c.fetch_sub(1, Ordering::SeqCst);
                Ok(ex)
            })
        });

        let config = super::WireTapConfig::bounded(2);
        let mut svc = super::WireTapService::with_config(tap_processor, config);

        // Send 3 exchanges rapidly — at most 2 tap tasks should run concurrently.
        for _ in 0..3 {
            let ex = Exchange::new(Message::new("test"));
            let _ = svc.ready().await.unwrap().call(ex).await.unwrap();
        }

        // Wait for all tap tasks to complete.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let observed_max = max_concurrent.load(Ordering::SeqCst);
        assert!(
            observed_max <= 2,
            "max concurrency was {observed_max}, expected <= 2"
        );
    }

    #[tokio::test]
    async fn test_wire_tap_drop_aborts_spawned_tasks() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let task_started = Arc::new(AtomicBool::new(false));
        let task_completed = Arc::new(AtomicBool::new(false));
        let started_clone = task_started.clone();
        let completed_clone = task_completed.clone();

        let tap_processor = BoxProcessor::from_fn(move |_ex| {
            let started = started_clone.clone();
            let completed = completed_clone.clone();
            Box::pin(async move {
                started.store(true, Ordering::SeqCst);
                // Simulate slow work that will be interrupted
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                completed.store(true, Ordering::SeqCst);
                Ok(Exchange::default())
            })
        });

        let mut service = WireTapService::new(tap_processor);
        let _ = service
            .ready()
            .await
            .unwrap()
            .call(Exchange::default())
            .await
            .unwrap();

        // Give the spawned task time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            task_started.load(Ordering::SeqCst),
            "tap task should be running"
        );
        assert!(
            !task_completed.load(Ordering::SeqCst),
            "task should not have completed yet"
        );

        // Drop the service — should abort spawned tasks
        drop(service);

        // Give abort time to propagate
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Task should NOT have completed (it was aborted before the 10s sleep finished)
        assert!(
            !task_completed.load(Ordering::SeqCst),
            "task should have been aborted, not completed"
        );
    }
}
