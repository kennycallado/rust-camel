use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Layer, Service, ServiceExt};

use camel_api::error_handler::ExceptionPolicy;
use camel_api::{BoxProcessor, CamelError, Exchange};

/// Tower Layer that wraps a pipeline with error handling behaviour.
///
/// Constructed with already-resolved producers; URI resolution happens in `camel-core`.
pub struct ErrorHandlerLayer {
    /// Resolved DLC producer (None = log only).
    dlc_producer: Option<BoxProcessor>,
    /// Policies with their resolved `handled_by` producers.
    policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)>,
}

impl ErrorHandlerLayer {
    /// Create the layer with pre-resolved producers.
    pub fn new(
        dlc_producer: Option<BoxProcessor>,
        policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)>,
    ) -> Self {
        Self {
            dlc_producer,
            policies,
        }
    }
}

impl<S> Layer<S> for ErrorHandlerLayer
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Send + Clone + 'static,
    S::Future: Send + 'static,
{
    type Service = ErrorHandlerService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ErrorHandlerService {
            inner,
            dlc_producer: self.dlc_producer.clone(),
            policies: self
                .policies
                .iter()
                .map(|(p, prod)| (p.clone(), prod.clone()))
                .collect(),
        }
    }
}

/// Tower Service that absorbs pipeline errors by retrying and/or forwarding to a DLC.
///
/// `call` always returns `Ok` — errors are absorbed. The returned exchange will have
/// `has_error() == true` if the pipeline ultimately failed.
pub struct ErrorHandlerService<S> {
    inner: S,
    dlc_producer: Option<BoxProcessor>,
    policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)>,
}

impl<S: Clone> Clone for ErrorHandlerService<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            dlc_producer: self.dlc_producer.clone(),
            policies: self
                .policies
                .iter()
                .map(|(p, prod)| (p.clone(), prod.clone()))
                .collect(),
        }
    }
}

impl<S> ErrorHandlerService<S>
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Send + Clone + 'static,
    S::Future: Send + 'static,
{
    /// Create the service directly (used in unit tests; in production use the Layer).
    pub fn new(
        inner: S,
        dlc_producer: Option<BoxProcessor>,
        policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)>,
    ) -> Self {
        Self {
            inner,
            dlc_producer,
            policies,
        }
    }
}

impl<S> Service<Exchange> for ErrorHandlerService<S>
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
        let dlc = self.dlc_producer.clone();
        let policies: Vec<(ExceptionPolicy, Option<BoxProcessor>)> = self
            .policies
            .iter()
            .map(|(p, prod)| (p.clone(), prod.clone()))
            .collect();

        Box::pin(async move {
            let original = exchange.clone();
            let result = inner.ready().await?.call(exchange).await;

            let err = match result {
                Ok(ex) => return Ok(ex),
                Err(e) => e,
            };

            // Stop EIP is a control-flow sentinel — pass through without retry or DLC.
            if matches!(err, CamelError::Stopped) {
                return Err(err);
            }

            // Find the first matching policy.
            let matched = policies.into_iter().find(|(p, _)| (p.matches)(&err));

            if let Some((policy, policy_producer)) = matched {
                // Retry if configured.
                if let Some(ref backoff) = policy.retry {
                    for attempt in 0..backoff.max_attempts {
                        let delay = backoff.delay_for(attempt);
                        tokio::time::sleep(delay).await;
                        match inner.ready().await?.call(original.clone()).await {
                            Ok(ex) => return Ok(ex),
                            Err(_e) => {
                                if attempt + 1 == backoff.max_attempts {
                                    // Retries exhausted — send to handler.
                                    let mut ex = original.clone();
                                    ex.set_error(_e);
                                    let handler = policy_producer.or(dlc);
                                    return send_to_handler(ex, handler).await;
                                }
                            }
                        }
                    }
                }
                // No retry configured (or 0 attempts) — send to policy handler or DLC.
                let mut ex = original.clone();
                ex.set_error(err);
                let handler = policy_producer.or(dlc);
                send_to_handler(ex, handler).await
            } else {
                // No matching policy — forward directly to DLC.
                let mut ex = original;
                ex.set_error(err);
                send_to_handler(ex, dlc).await
            }
        })
    }
}

async fn send_to_handler(
    exchange: Exchange,
    producer: Option<BoxProcessor>,
) -> Result<Exchange, CamelError> {
    match producer {
        None => {
            tracing::error!(
                error = ?exchange.error,
                "Exchange failed with no error handler configured"
            );
            Ok(exchange)
        }
        Some(mut prod) => match prod.ready().await {
            Err(e) => {
                tracing::error!("DLC/handler not ready: {e}");
                Ok(exchange)
            }
            Ok(svc) => match svc.call(exchange.clone()).await {
                Ok(ex) => Ok(ex),
                Err(e) => {
                    tracing::error!("DLC/handler call failed: {e}");
                    // Return the original exchange with original error intact.
                    Ok(exchange)
                }
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{
        BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message,
        error_handler::ExponentialBackoff,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };
    use std::time::Duration;
    use tower::ServiceExt;

    fn make_exchange() -> Exchange {
        Exchange::new(Message::new("test"))
    }

    fn failing_processor() -> BoxProcessor {
        BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        })
    }

    fn ok_processor() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    fn fail_n_times(n: u32) -> BoxProcessor {
        let count = Arc::new(AtomicU32::new(0));
        BoxProcessor::from_fn(move |ex| {
            let count = Arc::clone(&count);
            Box::pin(async move {
                let c = count.fetch_add(1, Ordering::SeqCst);
                if c < n {
                    Err(CamelError::ProcessorError(format!("attempt {c}")))
                } else {
                    Ok(ex)
                }
            })
        })
    }

    #[tokio::test]
    async fn test_ok_passthrough() {
        let svc = ErrorHandlerService::new(ok_processor(), None, vec![]);
        let result = svc.oneshot(make_exchange()).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().has_error());
    }

    #[tokio::test]
    async fn test_error_goes_to_dlc() {
        let received = Arc::new(std::sync::Mutex::new(Vec::<Exchange>::new()));
        let received_clone = Arc::clone(&received);
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&received_clone);
            Box::pin(async move {
                r.lock().unwrap().push(ex.clone());
                Ok(ex)
            })
        });

        let svc = ErrorHandlerService::new(failing_processor(), Some(dlc), vec![]);
        let result = svc.oneshot(make_exchange()).await;
        assert!(result.is_ok());
        let ex = result.unwrap();
        assert!(ex.has_error());
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_retry_recovers() {
        let inner = fail_n_times(2);
        let policy = ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: Some(ExponentialBackoff {
                max_attempts: 3,
                initial_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_delay: Duration::from_millis(10),
            }),
            handled_by: None,
        };
        let svc = ErrorHandlerService::new(inner, None, vec![(policy, None)]);
        let result = svc.oneshot(make_exchange()).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().has_error());
    }

    #[tokio::test]
    async fn test_retry_exhausted_goes_to_dlc() {
        let inner = fail_n_times(10);
        let received = Arc::new(std::sync::Mutex::new(0u32));
        let received_clone = Arc::clone(&received);
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&received_clone);
            Box::pin(async move {
                *r.lock().unwrap() += 1;
                Ok(ex)
            })
        });
        let policy = ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: Some(ExponentialBackoff {
                max_attempts: 2,
                initial_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_delay: Duration::from_millis(10),
            }),
            handled_by: None,
        };
        let svc = ErrorHandlerService::new(inner, Some(dlc), vec![(policy, None)]);
        let result = svc.oneshot(make_exchange()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().has_error());
        assert_eq!(*received.lock().unwrap(), 1);
    }

    #[test]
    fn test_poll_ready_delegates_to_inner() {
        use std::sync::atomic::AtomicBool;

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

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let inner = DelayedReadyService {
            ready: Arc::new(AtomicBool::new(false)),
        };
        let mut svc = ErrorHandlerService::new(inner, None, vec![]);

        // First poll_ready: inner returns Pending, so ErrorHandlerService must too.
        let first = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(first.is_pending(), "expected Pending on first poll_ready");

        // Second poll_ready: inner returns Ready, so ErrorHandlerService must too.
        let second = Pin::new(&mut svc).poll_ready(&mut cx);
        assert!(second.is_ready(), "expected Ready on second poll_ready");
    }

    #[tokio::test]
    async fn test_no_matching_policy_uses_dlc() {
        let received = Arc::new(std::sync::Mutex::new(0u32));
        let received_clone = Arc::clone(&received);
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&received_clone);
            Box::pin(async move {
                *r.lock().unwrap() += 1;
                Ok(ex)
            })
        });
        let policy = ExceptionPolicy::new(|e| matches!(e, CamelError::Io(_)));
        let svc = ErrorHandlerService::new(failing_processor(), Some(dlc), vec![(policy, None)]);
        let result = svc.oneshot(make_exchange()).await;
        assert!(result.is_ok());
        assert_eq!(*received.lock().unwrap(), 1);
    }

    // Stopped is a control-flow sentinel, not a real error.
    // ErrorHandlerService must pass it through without retrying or forwarding to DLC.
    #[tokio::test]
    async fn test_stopped_bypasses_error_handler() {
        let stopped_inner = BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::Stopped) })
        });

        // DLC that tracks if it was ever called.
        let dlc_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dlc_called_clone = Arc::clone(&dlc_called);
        let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
            dlc_called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move { Ok(ex) })
        });

        let policy = ExceptionPolicy::new(|_| true); // matches everything
        let svc = ErrorHandlerService::new(stopped_inner, Some(dlc), vec![(policy, None)]);
        let result = svc.oneshot(make_exchange()).await;

        // Must propagate Err(Stopped) — not absorb it.
        assert!(matches!(result, Err(CamelError::Stopped)), "expected Err(Stopped), got: {:?}", result);
        // DLC must NOT have been called.
        assert!(!dlc_called.load(std::sync::atomic::Ordering::SeqCst), "DLC should not be called for Stopped");
    }
}
