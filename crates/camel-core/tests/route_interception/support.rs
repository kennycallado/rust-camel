//! Divert-specific stubs and helpers for the route-interception suite.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll};

use async_trait::async_trait;
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, Value};
use camel_component_api::{Component, ComponentContext, Endpoint};
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;
use camel_core::intercept::{InterceptAction, InterceptRule, InterceptRules};
use tokio::sync::{Notify, Semaphore, mpsc};
use tower::Service;

use crate::common::TEST_TIMEOUT;

/// One divert rule: `source` sends are copied to `copy` (a `mock:` target).
pub(crate) fn divert_rules(source: &str, copy: &str) -> InterceptRules {
    InterceptRules::new(vec![InterceptRule {
        uri: source.into(),
        action: InterceptAction::DivertCopyTo { uri: copy.into() },
    }])
    .expect("valid mock targets")
}

/// Endpoint that vends a fixed producer cloned from the component.
pub(crate) struct StubEndpoint {
    uri: String,
    producer: BoxProcessor,
}

impl Endpoint for StubEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "stub has no consumer".into(),
        ))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &camel_component_api::ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(self.producer.clone())
    }
}

/// Component that resolves every URI under its scheme to a fixed stub
/// producer. Used to stand in for the `mock:` copy target (whose target URIs
/// must start with `mock:` per `InterceptRules` validation) and for real
/// targets with per-test recording/error behaviour.
pub(crate) struct StubComponent {
    scheme: String,
    producer: BoxProcessor,
}

impl StubComponent {
    pub(crate) fn new(scheme: &str, producer: BoxProcessor) -> Self {
        Self {
            scheme: scheme.into(),
            producer,
        }
    }
}

#[async_trait]
impl Component for StubComponent {
    fn scheme(&self) -> &str {
        &self.scheme
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(StubEndpoint {
            uri: uri.to_string(),
            producer: self.producer.clone(),
        }))
    }
}

/// Boot a context with interception rules, the direct component, and a set
/// of stub components — no MockComponent (a stub may occupy the `mock`
/// scheme instead).
pub(crate) async fn boot_stub_context(
    rules: InterceptRules,
    stubs: Vec<StubComponent>,
) -> CamelContext {
    let mut ctx = CamelContext::builder()
        .with_intercept_rules(rules)
        .build()
        .await
        .expect("build context");
    ctx.register_component(DirectComponent::new());
    for stub in stubs {
        ctx.register_component(stub);
    }
    ctx
}

/// Await a copy-completion signal within the test budget. Panics on timeout
/// instead of hanging the harness.
pub(crate) async fn await_signal(signal: &Notify, what: &str) {
    tokio::time::timeout(TEST_TIMEOUT, signal.notified())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

/// `MakeWriter` that appends formatted events to a shared sink, so tests can
/// assert that a `warn!` record was emitted (same pattern as the
/// camel-processor wiretap tests).
#[derive(Clone)]
pub(crate) struct CapturingWriter {
    sink: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for CapturingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sink.lock().expect("sink mutex").extend_from_slice(buf);
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

/// Install a capturing fmt subscriber as this thread's default. The guard
/// MUST be held for the whole test: determinism relies on the warn-emitting
/// tap task being polled on this thread (current-thread runtime) while the
/// guard is alive.
pub(crate) fn capture_tracing() -> (Arc<Mutex<Vec<u8>>>, tracing::subscriber::DefaultGuard) {
    let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(CapturingWriter {
            sink: Arc::clone(&sink),
        })
        .with_ansi(false)
        .finish();
    // Parallel tests race tracing's per-callsite interest cache against this
    // thread-local subscriber; force a rebuild so warn! callsites re-evaluate
    // against it (same pattern as the wiretap tests, bd rc-u9hs).
    let guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();
    (sink, guard)
}

/// Real-producer stub that records readiness and call events in order.
/// `poll_ready` pushes `"ready"`; `call` pushes `"call"` and returns the
/// exchange stamped with the `X-Sentinel` header.
#[derive(Clone)]
pub(crate) struct EventRealSvc {
    pub(crate) events: Arc<Mutex<Vec<&'static str>>>,
}

impl Service<Exchange> for EventRealSvc {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.events.lock().expect("events mutex").push("ready");
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut ex: Exchange) -> Self::Future {
        self.events.lock().expect("events mutex").push("call");
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
pub(crate) struct ReadyFailingRealSvc {
    pub(crate) events: Arc<Mutex<Vec<&'static str>>>,
}

impl Service<Exchange> for ReadyFailingRealSvc {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Err(CamelError::ProcessorError("sentinel-ready".into())))
    }

    fn call(&mut self, _ex: Exchange) -> Self::Future {
        self.events.lock().expect("events mutex").push("call");
        Box::pin(async move { Ok(Exchange::default()) })
    }
}

/// Real-producer stub that fails every call with a sentinel error.
pub(crate) fn real_boom_producer() -> BoxProcessor {
    BoxProcessor::from_fn(|_ex| {
        Box::pin(async { Err(CamelError::ProcessorError("real-boom".into())) })
    })
}

/// Copy-target stub (Task 5 test 2): each `call` takes an ordinal, signals
/// arrival on a channel FIRST, appends the ordinal to the shared order log,
/// then parks (calls 1-20, on a zero-permit semaphore) or returns
/// immediately (call 21+). No test-thread flag flip: a flippable flag races
/// copy 20 (it signals before parking, so the flip could land first).
#[derive(Clone)]
pub(crate) struct OrdinalParkSvc {
    pub(crate) ordinal: Arc<AtomicU32>,
    pub(crate) arrivals: mpsc::UnboundedSender<u32>,
    pub(crate) order_log: Arc<Mutex<Vec<u32>>>,
    pub(crate) park: Arc<Semaphore>,
}

impl Service<Exchange> for OrdinalParkSvc {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, ex: Exchange) -> Self::Future {
        let ordinal = Arc::clone(&self.ordinal);
        let arrivals = self.arrivals.clone();
        let order_log = Arc::clone(&self.order_log);
        let park = Arc::clone(&self.park);
        Box::pin(async move {
            let n = ordinal.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = arrivals.send(n);
            order_log.lock().expect("order log mutex").push(n);
            if n <= 20 {
                // Park unconditionally on the held (zero-permit) semaphore;
                // released only in the cleanup phase.
                let _ = park.acquire_owned().await;
            }
            Ok(ex)
        })
    }
}

/// Real-target stub (Task 5 test 2): appends `1000 + ordinal` to the shared
/// order log and returns the exchange stamped with the sentinel header.
#[derive(Clone)]
pub(crate) struct OrderLogRealSvc {
    pub(crate) ordinal: Arc<AtomicU32>,
    pub(crate) order_log: Arc<Mutex<Vec<u32>>>,
}

impl Service<Exchange> for OrderLogRealSvc {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut ex: Exchange) -> Self::Future {
        let n = self.ordinal.fetch_add(1, Ordering::SeqCst) + 1;
        self.order_log
            .lock()
            .expect("order log mutex")
            .push(1000 + n);
        ex.input.headers.insert(
            "X-Sentinel".to_string(),
            Value::String("real-ok".to_string()),
        );
        Box::pin(async move { Ok(ex) })
    }
}
