//! Divert composition for route interception (`advice-route-interception`).
//!
//! A divert sends a copy of the exchange to an interception target and then
//! feeds the original exchange to the real producer. The copy stage is a
//! public [`WireTapService`]; the real stage is any [`BoxProcessor`].

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Service, ServiceExt};

use camel_api::{BoxProcessor, CamelError, Exchange};

use crate::wire_tap::WireTapService;

/// Composed divert service: wiretap copy stage, then real producer.
///
/// The copy stage runs detached (or inline under CallerRuns saturation) and
/// its failures are suppressed by the wiretap. The real stage's `Result` is
/// returned verbatim to the caller.
#[derive(Clone)]
struct DivertService {
    tap: WireTapService,
    real: BoxProcessor,
}

impl Service<Exchange> for DivertService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    /// Always ready (ADR-0019): real-producer readiness is driven inside
    /// `call`, so the divert never blocks pipeline admission.
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let mut tap = self.tap.clone();
        let mut real = self.real.clone();
        Box::pin(async move {
            // Copy stage: the wiretap admits or drops the tap and returns the
            // original exchange. Readiness is unconditional (ADR-0019).
            let original = tap.ready().await?.call(exchange).await?;
            // Real stage: drive readiness on this same instance, then call.
            // A readiness error is returned verbatim and `call` is skipped.
            real.ready().await?;
            real.call(original).await
        })
    }
}

/// Compose a divert from a copy stage and a real producer.
///
/// `tap` is moved into the composed processor; clone it before the call to
/// keep a handle for lifecycle wiring — clones share the admission gate.
pub fn compose_divert(tap: WireTapService, real: BoxProcessor) -> BoxProcessor {
    BoxProcessor::new(DivertService { tap, real })
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};

    use tokio::sync::Notify;
    use tower::{Service, ServiceExt};

    use crate::wire_tap::WireTapService;
    use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message, Value};

    use super::*;

    /// Real-producer stub that records readiness and call events in order.
    /// `poll_ready` pushes `"ready"`; `call` pushes `"call"` and returns the
    /// exchange stamped with the `X-Sentinel` header.
    #[derive(Clone)]
    struct EventRealSvc {
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Service<Exchange> for EventRealSvc {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.events.lock().unwrap().push("ready"); // allow-unwrap: test-only
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, mut ex: Exchange) -> Self::Future {
            self.events.lock().unwrap().push("call"); // allow-unwrap: test-only
            ex.input.headers.insert(
                "X-Sentinel".to_string(),
                Value::String("real-ok".to_string()),
            );
            Box::pin(async move { Ok(ex) })
        }
    }

    /// Real-producer stub whose `poll_ready` fails with a sentinel error.
    /// `call` pushes `"call"` — it must never run.
    #[derive(Clone)]
    struct ReadyFailingRealSvc {
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Service<Exchange> for ReadyFailingRealSvc {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Err(CamelError::ProcessorError("sentinel-ready".into())))
        }

        fn call(&mut self, _ex: Exchange) -> Self::Future {
            self.events.lock().unwrap().push("call"); // allow-unwrap: test-only
            Box::pin(async move { Ok(Exchange::default()) })
        }
    }

    #[tokio::test]
    async fn real_producer_readiness_is_driven_before_call_success_order() {
        let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let copy_stub = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
        let real_stub = BoxProcessor::new(EventRealSvc {
            events: events.clone(),
        });

        let tap = WireTapService::new(copy_stub);
        let svc = compose_divert(tap, real_stub);

        let result = svc
            .oneshot(Exchange::new(Message::new("main")))
            .await
            .unwrap();

        assert_eq!(
            *events.lock().unwrap(), // allow-unwrap: test-only
            vec!["ready", "call"],
            "real producer readiness must be driven before call"
        );
        assert_eq!(
            result.input.headers.get("X-Sentinel"),
            Some(&Value::String("real-ok".to_string())),
            "returned exchange must be the real producer's sentinel"
        );
    }

    #[tokio::test]
    async fn real_producer_readiness_failure_returns_verbatim_and_skips_call() {
        let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let copy_stub = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
        let real_stub = BoxProcessor::new(ReadyFailingRealSvc {
            events: events.clone(),
        });

        let tap = WireTapService::new(copy_stub);
        let svc = compose_divert(tap, real_stub);

        let err = svc
            .oneshot(Exchange::new(Message::new("main")))
            .await
            .unwrap_err();

        match err {
            CamelError::ProcessorError(msg) => assert_eq!(msg, "sentinel-ready"),
            other => panic!("expected ProcessorError(\"sentinel-ready\"), got {other:?}"),
        }
        assert!(
            events.lock().unwrap().is_empty(), // allow-unwrap: test-only
            "real producer call must be skipped on readiness failure"
        );
    }

    #[tokio::test]
    async fn wiretap_lifecycle_start_reopens_admission_with_fresh_token() {
        use camel_api::StepShutdownReason;

        let arrivals = Arc::new(AtomicUsize::new(0));
        let arrived = Arc::new(Notify::new());

        let counter = arrivals.clone();
        let notify = arrived.clone();
        let copy_stub = BoxProcessor::from_fn(move |ex| {
            let counter = counter.clone();
            let notify = notify.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                notify.notify_one();
                Ok(ex)
            })
        });

        let svc = WireTapService::new(copy_stub);
        let lifecycle = svc.lifecycle();

        // First shutdown: admission closes, no copy runs.
        lifecycle
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .unwrap();
        let _ = svc
            .clone()
            .oneshot(Exchange::new(Message::new("after-shutdown")))
            .await
            .unwrap();
        assert_eq!(
            arrivals.load(Ordering::SeqCst),
            0,
            "no copy must run while admission is closed"
        );

        // Restart: admission reopens with a fresh token and tracker.
        lifecycle.start().await.unwrap();
        let _ = svc
            .clone()
            .oneshot(Exchange::new(Message::new("after-restart")))
            .await
            .unwrap();
        arrived.notified().await;
        assert_eq!(
            arrivals.load(Ordering::SeqCst),
            1,
            "copy must arrive after restart reopens admission"
        );

        // Second shutdown after restart: must be effective, not a no-op.
        lifecycle
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .unwrap();
        let _ = svc
            .clone()
            .oneshot(Exchange::new(Message::new("after-second-shutdown")))
            .await
            .unwrap();
        assert_eq!(
            arrivals.load(Ordering::SeqCst),
            1,
            "second shutdown after restart must close admission again"
        );
    }

    /// `MakeWriter` that appends formatted events to a shared sink, so tests
    /// can assert that a `warn!` record was emitted.
    #[derive(Clone)]
    struct CapturingWriter {
        sink: Arc<Mutex<Vec<u8>>>,
    }

    impl std::io::Write for CapturingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.sink.lock().unwrap().extend_from_slice(buf); // allow-unwrap: test-only
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingWriter {
        type Writer = CapturingWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn copy_call_failure_is_suppressed_and_logged() {
        let copy_done = Arc::new(Notify::new());
        let notify = copy_done.clone();
        let copy_stub = BoxProcessor::from_fn(move |_ex| {
            let notify = notify.clone();
            Box::pin(async move {
                notify.notify_one();
                Err(CamelError::ProcessorError("copy-boom".into()))
            })
        });
        let real_stub = BoxProcessor::from_fn(|mut ex| {
            Box::pin(async move {
                ex.input.headers.insert(
                    "X-Sentinel".to_string(),
                    Value::String("real-ok".to_string()),
                );
                Ok(ex)
            })
        });

        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(CapturingWriter { sink: sink.clone() })
            .with_ansi(false)
            .finish();

        // set_default propagates to tasks spawned on this thread; force a
        // callsite interest rebuild so warn! re-evaluates against it
        // (same pattern as the wire_tap tests, bd rc-u9hs).
        let _guard = tracing::subscriber::set_default(subscriber);
        tracing::callsite::rebuild_interest_cache();

        let tap = WireTapService::new(copy_stub);
        let svc = compose_divert(tap, real_stub);

        // Keep `svc` alive: dropping the last divert clone cancels the
        // shared tap token, which would abort the detached copy before it
        // signals.
        let result = svc
            .clone()
            .oneshot(Exchange::new(Message::new("main")))
            .await
            .unwrap();
        assert_eq!(
            result.input.headers.get("X-Sentinel"),
            Some(&Value::String("real-ok".to_string())),
            "real producer result must be returned verbatim"
        );

        // The copy runs detached; await its completion signal before the
        // warn assertion (the warn fires when the tap task observes the
        // copy failure).
        copy_done.notified().await;
        // Deterministic only on a current-thread runtime (#[tokio::test] default): no await between notify_one and warn! in run_tap; multi_thread would make this flaky.
        let captured = String::from_utf8(sink.lock().unwrap().clone()).unwrap(); // allow-unwrap: test-only
        assert!(
            captured.contains("copy-boom"),
            "a warn record mentioning the copy failure should have been emitted; got: {captured}"
        );
    }
}
