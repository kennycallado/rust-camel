//! camel-master test module root: shared mocks, metrics recording infra,
//! and helpers used across concern submodules. Split from the former
//! monolithic tests.rs by concern (bd rc-ubk1v).
use super::*;
use camel_api::{
    BoxProcessorExt, Exchange, LeadershipEvent, LeadershipHandle, LeadershipService, Message,
    NoOpMetrics, NoopPlatformService, NoopReadinessGate, PlatformError, PlatformIdentity,
    PlatformService, ReadinessGate,
};
use camel_component_api::NoOpComponentContext;
use camel_component_api::test_support::PanicRuntimeObservability;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use tokio::sync::{oneshot, watch};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

struct FakeLeadershipService {
    tx: Mutex<Option<watch::Sender<Option<LeadershipEvent>>>>,
    is_leader: Arc<AtomicBool>,
    /// Leader epoch published to the supervision loop (shared through
    /// `LeadershipHandle::new`). Bumped from tests to simulate a coalesced
    /// takeover flap while the delegate stays Active.
    leader_epoch: Arc<AtomicU64>,
    initial: Option<LeadershipEvent>,
}

impl FakeLeadershipService {
    fn new(initial: Option<LeadershipEvent>) -> Self {
        let starts_as_leader = matches!(initial, Some(LeadershipEvent::StartedLeading));
        Self {
            tx: Mutex::new(None),
            is_leader: Arc::new(AtomicBool::new(starts_as_leader)),
            leader_epoch: Arc::new(AtomicU64::new(1)),
            initial,
        }
    }

    /// Shared handle to the epoch counter for bump-from-test access.
    fn leader_epoch(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.leader_epoch)
    }

    async fn emit(&self, event: LeadershipEvent) {
        self.is_leader.store(
            matches!(event, LeadershipEvent::StartedLeading),
            Ordering::Release,
        );
        if let Some(tx) = self
            .tx
            .lock()
            .expect("mutex poisoned: fake elector sender")
            .as_ref()
        {
            let _ = tx.send(Some(event));
        }
    }
}

#[async_trait]
impl LeadershipService for FakeLeadershipService {
    async fn start(&self, _lock_name: &str) -> Result<LeadershipHandle, PlatformError> {
        let (tx, rx) = watch::channel(self.initial.clone());
        *self.tx.lock().expect("mutex poisoned: fake elector sender") = Some(tx);

        let cancel = CancellationToken::new();
        let cancel_wait = cancel.clone();
        let (term_tx, term_rx) = oneshot::channel();
        tokio::spawn(async move {
            cancel_wait.cancelled().await;
            let _ = term_tx.send(());
        });

        Ok(LeadershipHandle::new(
            rx,
            Arc::clone(&self.is_leader),
            Arc::clone(&self.leader_epoch),
            cancel,
            term_rx,
        ))
    }
}

struct FakePlatformService {
    identity: PlatformIdentity,
    readiness_gate: Arc<dyn ReadinessGate>,
    leadership: Arc<dyn LeadershipService>,
}

impl FakePlatformService {
    fn new(leadership: Arc<dyn LeadershipService>) -> Self {
        Self {
            identity: PlatformIdentity::local("master-tests"),
            readiness_gate: Arc::new(NoopReadinessGate),
            leadership,
        }
    }
}

impl PlatformService for FakePlatformService {
    fn identity(&self) -> PlatformIdentity {
        self.identity.clone()
    }

    fn readiness_gate(&self) -> Arc<dyn ReadinessGate> {
        Arc::clone(&self.readiness_gate)
    }

    fn leadership(&self) -> Arc<dyn LeadershipService> {
        Arc::clone(&self.leadership)
    }
}

struct FakeDelegateComponent {
    create_consumer_calls: Arc<AtomicUsize>,
    start_calls: Arc<AtomicUsize>,
}

impl Component for FakeDelegateComponent {
    fn scheme(&self) -> &str {
        "fake"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(FakeDelegateEndpoint {
            create_consumer_calls: Arc::clone(&self.create_consumer_calls),
            start_calls: Arc::clone(&self.start_calls),
        }))
    }
}

struct FakeDelegateEndpoint {
    create_consumer_calls: Arc<AtomicUsize>,
    start_calls: Arc<AtomicUsize>,
}

impl Endpoint for FakeDelegateEndpoint {
    fn uri(&self) -> &str {
        "fake:delegate"
    }

    fn create_consumer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        let epoch = self.create_consumer_calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Box::new(FakeDelegateConsumer {
            epoch,
            start_calls: Arc::clone(&self.start_calls),
        }))
    }

    fn create_producer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed("not used".to_string()))
    }
}

struct FakeDelegateConsumer {
    epoch: usize,
    start_calls: Arc<AtomicUsize>,
}

// ── rc-i1z test infrastructure ──────────────────────────────────────

/// Delegate component that returns errors from create_endpoint or create_consumer.
/// Configurable: which error to return, and after how many successful calls
/// to stop failing.
struct ErrorDelegateComponent {
    create_endpoint_calls: Arc<AtomicUsize>,
    create_consumer_calls: Arc<AtomicUsize>,
    endpoint_error: Option<CamelError>,
    consumer_error_after: usize, // fail start() this many times, then succeed
    consumer_error: Option<CamelError>,
    /// One-shot exit signal handed to the FIRST created consumer (create
    /// ordinal 1) only: the consumer exits on its own when the signal
    /// fires, so the supervision tick observes a finished handle without
    /// any teardown. `None` in every test that does not need the knob.
    first_exit_signal: Arc<Mutex<Option<watch::Receiver<()>>>>,
}

impl Component for ErrorDelegateComponent {
    fn scheme(&self) -> &str {
        "errdelegate"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        self.create_endpoint_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(ref err) = self.endpoint_error {
            return Err(err.clone());
        }
        Ok(Box::new(ErrorDelegateEndpoint {
            create_consumer_calls: Arc::clone(&self.create_consumer_calls),
            consumer_error_after: self.consumer_error_after,
            consumer_error: self.consumer_error.clone(),
            first_exit_signal: Arc::clone(&self.first_exit_signal),
        }))
    }
}

struct ErrorDelegateEndpoint {
    create_consumer_calls: Arc<AtomicUsize>,
    consumer_error_after: usize,
    consumer_error: Option<CamelError>,
    first_exit_signal: Arc<Mutex<Option<watch::Receiver<()>>>>,
}

impl Endpoint for ErrorDelegateEndpoint {
    fn uri(&self) -> &str {
        "errdelegate:delegate"
    }

    fn create_consumer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        let call_idx = self.create_consumer_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call_idx <= self.consumer_error_after {
            return Err(self
                .consumer_error
                .clone()
                .unwrap_or_else(|| CamelError::ProcessorError("default error".to_string())));
        }
        // The one-shot exit signal is scoped to create ordinal 1: the
        // first consumer self-exits on the signal, later consumers run
        // until cancelled.
        let exit_signal = if call_idx == 1 {
            self.first_exit_signal
                .lock()
                .expect("mutex poisoned: delegate exit signal")
                .take()
        } else {
            None
        };
        Ok(Box::new(SuccessDelegateConsumer { exit_signal }))
    }

    fn create_producer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed("not used".to_string()))
    }
}

/// A delegate consumer that starts, sends one message, then cancels.
/// With a one-shot `exit_signal` (dead-delegate test), it exits on its
/// own when the signal fires instead — the task handle finishes without
/// any teardown, so the supervision tick observes a dead Active delegate.
struct SuccessDelegateConsumer {
    exit_signal: Option<watch::Receiver<()>>,
}

#[async_trait]
impl Consumer for SuccessDelegateConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        context.send(Exchange::new(Message::new("ok"))).await?;
        match self.exit_signal.as_mut() {
            Some(exit) => {
                let _ = exit.changed().await;
            }
            None => context.cancelled().await,
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

fn build_error_delegate_master(
    platform_service: Arc<dyn PlatformService>,
    create_endpoint_calls: Arc<AtomicUsize>,
    create_consumer_calls: Arc<AtomicUsize>,
    endpoint_error: Option<CamelError>,
    consumer_error_after: usize,
    consumer_error: Option<CamelError>,
    max_attempts: u32,
) -> MasterConsumer {
    build_error_delegate_master_with_metrics(
        platform_service,
        create_endpoint_calls,
        create_consumer_calls,
        endpoint_error,
        consumer_error_after,
        consumer_error,
        max_attempts,
        Arc::new(NoOpMetrics),
    )
}

// ── MST-001 metrics wiring tests (master-metrics-wiring Task 1.2) ──

const METRICS_TEST_LOCK: &str = "lock-err";

const METRICS_TEST_ROUTE: &str = "master-test-route";

/// Exported family name of the leadership state gauge. The recording
/// collector logs gauge edges into the shared observation log under this
/// name so gauge and counter emissions share one global insertion order.
const LEADERSHIP_GAUGE_METRIC: &str = "camel_master_is_leader";

/// One recorded counter observation: (metric name, value, owned labels).
type RecordedCounter = (String, f64, Vec<(String, String)>);

/// Metrics collector that records every `record_counter` observation as an
/// owned `(name, value, labels)` tuple. The five classic methods are no-ops;
/// the master component emits counters plus the `camel_master_is_leader`
/// state gauge, whose edges are logged into the same observation log.
struct RecordingMetricsCollector {
    events: Mutex<Vec<RecordedCounter>>,
}

impl RecordingMetricsCollector {
    /// Filtered, order-preserving view of all observations for one metric.
    fn counters_named(&self, name: &str) -> Vec<(f64, Vec<(String, String)>)> {
        self.events
            .lock()
            .expect("mutex poisoned: recording metrics collector")
            .iter()
            .filter(|(recorded, _, _)| recorded == name)
            .map(|(_, value, labels)| (*value, labels.clone()))
            .collect()
    }

    /// Position of the nth (0-based) observation of `name` in the GLOBAL
    /// insertion order (across all metric names). `None` when fewer than
    /// n+1 observations exist. Used to assert cross-metric emission
    /// ordering; `Option` forces callers to handle absence explicitly.
    fn nth_global_index_of(&self, name: &str, n: usize) -> Option<usize> {
        self.events
            .lock()
            .expect("mutex poisoned: recording metrics collector")
            .iter()
            .enumerate()
            .filter(|(_, (recorded, _, _))| recorded == name)
            .nth(n)
            .map(|(idx, _)| idx)
    }

    /// Last observed value of the leadership state gauge for `lock`
    /// (gauge semantics: the most recent edge wins). `None` before the
    /// first edge.
    fn leadership_gauge(&self, lock: &str) -> Option<f64> {
        self.events
            .lock()
            .expect("mutex poisoned: recording metrics collector")
            .iter()
            .rev()
            .find(|(recorded, _, labels)| {
                recorded == LEADERSHIP_GAUGE_METRIC
                    && labels.iter().any(|(k, v)| k == "lock" && v == lock)
            })
            .map(|(_, value, _)| *value)
    }
}

impl MetricsCollector for RecordingMetricsCollector {
    fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}
    fn increment_errors(&self, _route_id: &str, _error_type: &str) {}
    fn increment_exchanges(&self, _route_id: &str) {}
    fn set_queue_depth(&self, _route_id: &str, _depth: usize) {}
    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}
    fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        self.events
            .lock()
            .expect("mutex poisoned: recording metrics collector")
            .push((
                name.to_string(),
                value,
                labels
                    .iter()
                    .map(|(key, label_value)| (key.to_string(), label_value.to_string()))
                    .collect(),
            ));
    }

    fn set_master_leadership(&self, lock: &str, leader: bool) {
        self.events
            .lock()
            .expect("mutex poisoned: recording metrics collector")
            .push((
                LEADERSHIP_GAUGE_METRIC.to_string(),
                if leader { 1.0 } else { 0.0 },
                vec![("lock".to_string(), lock.to_string())],
            ));
    }
}

/// Transient per `is_retryable_camel_error` (`Io(_)` is always retryable).
fn transient_io_error() -> CamelError {
    CamelError::Io("boom".to_string())
}

/// Expected complete label set of a `master_delegate_lifecycle_total`
/// observation for the lock/route pair used by the metrics tests.
fn expected_lifecycle_labels(event: &str, reason: &str) -> Vec<(String, String)> {
    vec![
        ("lock".to_string(), METRICS_TEST_LOCK.to_string()),
        ("route_id".to_string(), METRICS_TEST_ROUTE.to_string()),
        ("event".to_string(), event.to_string()),
        ("reason".to_string(), reason.to_string()),
    ]
}

/// Count of lifecycle observations whose labels match `event` with the
/// default reason (`"none"`) for the lock/route pair used by the metrics
/// tests. `create_error` observations never match (different reason).
fn lifecycle_events(metrics: &RecordingMetricsCollector, event: &str) -> usize {
    metrics
        .counters_named("master_delegate_lifecycle_total")
        .iter()
        .filter(|(_, labels)| *labels == expected_lifecycle_labels(event, "none"))
        .count()
}

/// Expected complete label set of a `master_leadership_transitions_total`
/// observation for the lock/route pair used by the metrics tests.
fn expected_transition_labels(event: &str) -> Vec<(String, String)> {
    vec![
        ("lock".to_string(), METRICS_TEST_LOCK.to_string()),
        ("route_id".to_string(), METRICS_TEST_ROUTE.to_string()),
        ("event".to_string(), event.to_string()),
    ]
}

/// Sibling of [`build_error_delegate_master`] that injects a metrics
/// collector so tests observe every counter the supervision loop emits.
/// Recording-collector access is always through this builder.
#[allow(clippy::too_many_arguments)] // harness signature mandated by Task 1.2
fn build_error_delegate_master_with_metrics(
    platform_service: Arc<dyn PlatformService>,
    create_endpoint_calls: Arc<AtomicUsize>,
    create_consumer_calls: Arc<AtomicUsize>,
    endpoint_error: Option<CamelError>,
    consumer_error_after: usize,
    consumer_error: Option<CamelError>,
    max_attempts: u32,
    metrics: Arc<dyn MetricsCollector>,
) -> MasterConsumer {
    let reconnect = NetworkRetryPolicy {
        max_attempts,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        multiplier: 1.0,
        ..NetworkRetryPolicy::default()
    };
    MasterConsumer::new(
        METRICS_TEST_LOCK.to_string(),
        "errdelegate:delegate".to_string(),
        Arc::new(ErrorDelegateComponent {
            create_endpoint_calls,
            create_consumer_calls,
            endpoint_error,
            consumer_error_after,
            consumer_error,
            first_exit_signal: Arc::new(Mutex::new(None)),
        }),
        metrics,
        platform_service,
        Duration::from_millis(500),
        reconnect,
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    )
}

/// Poll the recording collector until `name` has at least `count`
/// observations. Retries advance on the 200 ms `DELEGATE_RETRY_INTERVAL`
/// tick, so the 5 s bound covers every retry-consuming config below
/// (worst case 4 attempts x 200 ms = 800 ms).
async fn await_counter_observations(
    metrics: &RecordingMetricsCollector,
    name: &str,
    count: usize,
) -> bool {
    timeout(Duration::from_secs(5), async {
        loop {
            if metrics.counters_named(name).len() >= count {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}

/// Poll the recording collector until the leadership state gauge for the
/// metrics-test lock reads `leader` (1.0 held / 0.0 lost). Same 5 ms poll
/// pattern and 5 s bound as [`await_counter_observations`].
async fn await_leadership_gauge(metrics: &RecordingMetricsCollector, leader: bool) -> bool {
    let expected = if leader { 1.0 } else { 0.0 };
    timeout(Duration::from_secs(5), async {
        loop {
            if metrics.leadership_gauge(METRICS_TEST_LOCK) == Some(expected) {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}

/// Poll the leadership task for completion (task failure or
/// budget-exhaustion shutdown) using the existing 5 ms poll pattern.
async fn await_leadership_task_exit(master: &MasterConsumer) -> bool {
    timeout(Duration::from_secs(5), async {
        loop {
            if master
                .leadership_task
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
            {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}

struct FailingDelegateComponent {
    create_endpoint_calls: Arc<AtomicUsize>,
}

impl Component for FailingDelegateComponent {
    fn scheme(&self) -> &str {
        "failing"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        self.create_endpoint_calls.fetch_add(1, Ordering::SeqCst);
        Err(CamelError::EndpointCreationFailed(
            "delegate endpoint creation failed".to_string(),
        ))
    }
}

#[async_trait]
impl Consumer for FakeDelegateConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        self.start_calls.fetch_add(1, Ordering::SeqCst);
        context
            .send(Exchange::new(Message::new(format!("epoch-{}", self.epoch))))
            .await?;

        loop {
            tokio::select! {
                _ = context.cancelled() => {
                    break;
                }
                _ = sleep(Duration::from_millis(20)) => {
                    context
                        .send(Exchange::new(Message::new(format!("epoch-{}", self.epoch))))
                        .await?;
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

#[cfg(test)]
mod acquisition_budget;
#[cfg(test)]
mod delegate_errors;
#[cfg(test)]
mod leadership_lifecycle;
#[cfg(test)]
mod leadership_state_gauge;
#[cfg(test)]
mod leadership_transition_edge;
#[cfg(test)]
mod master_uri;
#[cfg(test)]
mod metrics_wiring;
#[cfg(test)]
mod producer_passthrough;
#[cfg(test)]
mod regressions;
#[cfg(test)]
mod stale_stamp;
