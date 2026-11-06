//! STAGE 1 compile probe for mission 510 (builderlayer).
//!
//! Verifies the tower Layer -> BoxProcessor assumptions BEFORE design:
//!   P1: Timeout<BoxProcessor> + CamelError adapter fits BoxProcessor (Clone+Send+Sync).
//!   P2: ConcurrencyLimit<BoxProcessor> same; cap holds under concurrent load.
//!   P3: RateLimit state sharing (direct wrap does NOT compile -> Buffer required).
//!   P4: Timeout error surfaces as tower::timeout::error::Elapsed via BoxError downcast.
//!   P5: OutcomeSegment -> Service adapter with stop re-marking compiles.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use camel_api::{BoxProcessor, CamelError, Exchange, Message};
use tower::{Layer, Service, ServiceExt};

type BoxError = std::boxed::Box<dyn std::error::Error + Send + Sync>;

/// Probe-local mapping trait (the real change adds `impl From<BoxError> for CamelError`
/// in camel-api; foreign-type orphan rules prevent it here).
trait MapCamelError {
    fn into_camel_error(self) -> CamelError;
}
impl MapCamelError for BoxError {
    fn into_camel_error(self) -> CamelError {
        // P4 sketch: downcast known tower errors; preserve the source chain.
        let mut src: Option<&(dyn std::error::Error + 'static)> = Some(self.as_ref());
        while let Some(s) = src {
            if s.downcast_ref::<tower::timeout::error::Elapsed>().is_some() {
                let source: Arc<dyn std::error::Error + Send + Sync> = Arc::from(self);
                return CamelError::ProcessorErrorWithSource(
                    "middleware timeout elapsed".into(),
                    source,
                );
            }
            src = s.source();
        }
        let source: Arc<dyn std::error::Error + Send + Sync> = Arc::from(self);
        CamelError::ProcessorErrorWithSource("middleware error".into(), source)
    }
}
fn to_camel<E: Into<BoxError>>(e: E) -> CamelError {
    e.into().into_camel_error()
}

#[derive(Clone)]
struct CamelErrorAdapter<S> {
    inner: S,
}

impl<S> Service<Exchange> for CamelErrorAdapter<S>
where
    S: Service<Exchange, Response = Exchange> + Clone + Send + Sync + 'static,
    S::Error: Into<BoxError>,
    S::Future: Send,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map(|r| r.map_err(to_camel))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        // tower readiness contract: poll_ready reserved capacity on THIS
        // handle; call must consume it on the same handle. Cloning here
        // (permit-style layers) deadlocks: the original handle keeps its
        // unconsumed permit while the clone waits for another one.
        let fut = self.inner.call(exchange);
        Box::pin(async move { fut.await.map_err(to_camel) })
    }
}

fn slow_processor(delay: Duration) -> BoxProcessor {
    BoxProcessor::new(camel_api::ProcessorFn::new(move |ex: Exchange| {
        async move {
            tokio::time::sleep(delay).await;
            Ok(ex)
        }
    }))
}

fn inflight_processor(
    in_flight: Arc<AtomicUsize>,
    max_seen: Arc<AtomicUsize>,
    delay: Duration,
) -> BoxProcessor {
    BoxProcessor::new(camel_api::ProcessorFn::new(move |ex: Exchange| {
        let in_flight = Arc::clone(&in_flight);
        let max_seen = Arc::clone(&max_seen);
        async move {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(ex)
        }
    }))
}

// ── P2a: concurrency cap with the layer ──
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2a_concurrency_cap_holds() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let raw = inflight_processor(
        Arc::clone(&in_flight),
        Arc::clone(&max_seen),
        Duration::from_millis(50),
    );
    let layered = tower::limit::ConcurrencyLimitLayer::new(2).layer(raw);
    let bp = BoxProcessor::new(CamelErrorAdapter { inner: layered });

    let mut handles = Vec::new();
    for i in 0..20 {
        let mut clone = bp.clone();
        handles.push(tokio::spawn(async move {
            eprintln!("task {i}: polling ready");
            let ex = Exchange::new(Message::new(format!("m{i}")));
            let ready = clone.ready().await.expect("ready");
            eprintln!("task {i}: ready ok, calling");
            let r = ready.call(ex).await;
            eprintln!("task {i}: done");
            r
        }));
    }
    for (i, h) in handles.into_iter().enumerate() {
        let r = tokio::time::timeout(Duration::from_secs(5), h).await
            .unwrap_or_else(|_| panic!("task {i} timed out after 5s"));
        r.expect("call ok");
    }
    let max = max_seen.load(Ordering::SeqCst);
    println!("P2a max in-flight {max} (cap 2, ratio {:.3})", max as f64 / 2.0);
    assert!(max <= 2, "concurrency cap violated: {max}");
}

// ── P2b: SAME flow WITHOUT any layer (baseline — must not hang) ──
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2b_baseline_no_layer() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let bp = BoxProcessor::new(CamelErrorAdapter {
        inner: inflight_processor(
            Arc::clone(&in_flight),
            Arc::clone(&max_seen),
            Duration::from_millis(20),
        ),
    });
    let mut handles = Vec::new();
    for i in 0..20 {
        let mut clone = bp.clone();
        handles.push(tokio::spawn(async move {
            let ex = Exchange::new(Message::new(format!("m{i}")));
            let ready = clone.ready().await.expect("ready");
            ready.call(ex).await
        }));
    }
    for h in handles {
        h.await.unwrap().expect("call ok");
    }
    println!("P2b baseline done, max in-flight {}", max_seen.load(Ordering::SeqCst));
}

// ── P2c: sequential calls through the limit (no spawn) ──
#[tokio::test]
async fn p2c_sequential_through_limit() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let raw = inflight_processor(
        Arc::clone(&in_flight),
        Arc::clone(&max_seen),
        Duration::from_millis(5),
    );
    let layered = tower::limit::ConcurrencyLimitLayer::new(2).layer(raw);
    let bp = BoxProcessor::new(CamelErrorAdapter { inner: layered });
    let started = std::time::Instant::now();
    for i in 0..6 {
        let mut clone = bp.clone();
        let ex = Exchange::new(Message::new(format!("m{i}")));
        let ready = clone.ready().await.expect("ready");
        ready.call(ex).await.expect("ok");
    }
    println!("P2c 6 sequential calls took {:?}", started.elapsed());
}

// ── P3c: RateLimit through Buffer shares state across clones ──
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p3c_rate_limit_buffered_shares_state() {
    let raw = slow_processor(Duration::from_millis(10));
    let rate = tower::limit::RateLimitLayer::new(2, Duration::from_secs(1)).layer(raw);
    let buffered = tower::buffer::Buffer::new(rate, 16);
    let bp = BoxProcessor::new(CamelErrorAdapter { inner: buffered });
    let mut handles = Vec::new();
    let started = std::time::Instant::now();
    for i in 0..5 {
        let mut clone = bp.clone();
        handles.push(tokio::spawn(async move {
            let ex = Exchange::new(Message::new(format!("m{i}")));
            let ready = clone.ready().await.expect("ready");
            ready.call(ex).await
        }));
    }
    for h in handles {
        h.await.unwrap().expect("buffered rate-limited call ok");
    }
    let elapsed = started.elapsed();
    println!("P3c 5 calls at rate 2/s took {elapsed:?} (shared state => >= ~1s)");
    assert!(
        elapsed >= Duration::from_millis(900),
        "rate limit state NOT shared across clones (elapsed {elapsed:?})"
    );
}

// ── P4: timeout error identity + mapping ──
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p4_timeout_error_is_elapsed() {
    let raw = slow_processor(Duration::from_secs(5));
    let layered = tower::timeout::TimeoutLayer::new(Duration::from_millis(50)).layer(raw);
    let mut bp = BoxProcessor::new(CamelErrorAdapter { inner: layered });
    let ex = Exchange::new(Message::new("hello"));
    let started = std::time::Instant::now();
    let res = {
        let ready = bp.ready().await.expect("ready");
        ready.call(ex).await
    };
    let elapsed = started.elapsed();
    let err = res.expect_err("slow producer must hit the 50ms timeout");
    println!("P4 error: {err} after {elapsed:?}");
    let source = std::error::Error::source(&err);
    assert!(
        source.is_some(),
        "ProcessorErrorWithSource must preserve the tower error as source"
    );
    let src_display = source.map(|s| s.to_string()).unwrap_or_default();
    println!("P4 source: {src_display}");
    assert!(
        src_display.contains("timed out") || src_display.contains("deadline"),
        "source must be the tower Elapsed error, got: {src_display}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "timeout must fire near 50ms, took {elapsed:?}"
    );
}

// ── P5: OutcomeSegment -> Service adapter compiles (stop re-marking sketch) ──
#[tokio::test]
async fn p5_segment_adapter_compiles() {
    use camel_api::{OutcomePipeline, OutcomeSegment, PipelineOutcome};
    struct EchoSeg;
    impl OutcomePipeline for EchoSeg {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(EchoSeg)
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineOutcome> + Send + 'a>>
        {
            Box::pin(async move { PipelineOutcome::Completed(exchange) })
        }
    }
    #[derive(Clone)]
    struct SegmentService {
        inner: OutcomeSegment,
    }
    impl Service<Exchange> for SegmentService {
        type Response = Exchange;
        type Error = CamelError;
        type Future = std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>,
        >;
        fn poll_ready(
            &mut self,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn call(&mut self, exchange: Exchange) -> Self::Future {
            let mut seg = self.inner.clone();
            Box::pin(async move {
                match seg.run(exchange).await {
                    PipelineOutcome::Completed(ex) => Ok(ex),
                    PipelineOutcome::Failed(e) => Err(e),
                    PipelineOutcome::Stopped(mut ex) => {
                        if !camel_api::is_camel_stop(&ex) {
                            ex.set_property(camel_api::CAMEL_STOP, serde_json::Value::Bool(true));
                        }
                        Ok(ex)
                    }
                }
            })
        }
    }
    let mut bp: BoxProcessor = BoxProcessor::new(CamelErrorAdapter { inner: SegmentService {
        inner: OutcomeSegment::new(Box::new(EchoSeg)),
    } });
    let ex = Exchange::new(Message::new("seg"));
    let ready = bp.ready().await.expect("ready");
    let out = ready.call(ex).await.expect("ok");
    assert_eq!(out.input.body.as_text(), Some("seg"));
}
