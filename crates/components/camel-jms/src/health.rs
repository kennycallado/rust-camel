use crate::component::{BridgeState, JmsBridgePool, bridge_service_client};
use crate::proto::HealthRequest;
use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// One bridge health probe result: bridge liveness plus broker connectivity.
///
/// `status` is the raw numeric `HealthStatus` carried by the response
/// (future enum numbers are preserved, never rewritten), `age_ms` the
/// numeric stale age. Older bridges omit both; they decode as
/// `UNSPECIFIED`/`0` and keep the legacy boolean meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BridgeProbeResult {
    pub healthy: bool,
    pub broker_connected: bool,
    pub message: String,
    pub status: i32,
    pub age_ms: u64,
}

/// Crate-local explicit broker status carried by an observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerHealthStatus {
    Unspecified,
    Up,
    Stale,
    Unknown,
    Skipped,
    Down,
    /// Any numeric value outside the known range (future enum numbers).
    Future,
}

impl BrokerHealthStatus {
    pub(crate) fn from_raw(raw: i32) -> Self {
        match raw {
            0 => Self::Unspecified,
            1 => Self::Up,
            2 => Self::Stale,
            3 => Self::Unknown,
            4 => Self::Skipped,
            5 => Self::Down,
            _ => Self::Future,
        }
    }

    /// True for statuses that carry no broker verification: STALE, UNKNOWN,
    /// SKIPPED and future values are live-but-unverified and must never be
    /// read as fresh disconnection evidence.
    fn is_unverified(self) -> bool {
        matches!(
            self,
            Self::Stale | Self::Unknown | Self::Skipped | Self::Future
        )
    }
}

/// One raw broker-status observation: explicit status, legacy booleans, and
/// numeric age. Never interprets the message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BrokerHealthObservation {
    pub status: BrokerHealthStatus,
    pub healthy: bool,
    pub broker_connected: bool,
    /// Numeric stale age; meaningful only for STALE observations.
    pub age_ms: u64,
}

/// Readiness decision for one broker observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerReadiness {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifiedBroker {
    Up,
    Down,
}

/// Opaque identity of the Ready bridge channel backing a broker slot. Two
/// different values across consecutive readiness checks mean the bridge was
/// replaced (slot removal plus re-creation, or a replacement Ready channel)
/// and retained verification must reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BridgeIdentity(u64);

/// Retained broker verification for one bridge identity. UP and DOWN update
/// the verified state; unverified statuses preserve it. Retained
/// verification resets whenever the slot/Ready-channel identity changes.
#[derive(Debug, Default)]
pub(crate) struct BrokerHealthMemory {
    identity: Option<BridgeIdentity>,
    verified: Option<VerifiedBroker>,
    /// Last observed status kind; drives at-most-once stale-transition
    /// reporting in the health monitor.
    last_kind: Option<BrokerHealthStatus>,
}

impl BrokerHealthMemory {
    /// Drops retained verification and re-binds the memory to `identity`.
    pub(crate) fn reset(&mut self, identity: Option<BridgeIdentity>) {
        self.identity = identity;
        self.verified = None;
        self.last_kind = None;
    }

    /// Status kind of the last observation, `None` before any observation or
    /// after a reset. The monitor uses it to detect entering STALE.
    pub(crate) fn last_kind(&self) -> Option<BrokerHealthStatus> {
        self.last_kind
    }

    /// Test-only view of the retained verification: `Some(true)` = verified
    /// UP, `Some(false)` = verified DOWN, `None` = unverified.
    #[cfg(test)]
    pub(crate) fn verified(&self) -> Option<bool> {
        match self.verified {
            Some(VerifiedBroker::Up) => Some(true),
            Some(VerifiedBroker::Down) => Some(false),
            None => None,
        }
    }

    /// Applies one observation and decides readiness. UP and DOWN update
    /// verification; STALE, UNKNOWN, SKIPPED and future statuses preserve
    /// it; UNSPECIFIED keeps the legacy boolean mapping and never updates
    /// verification.
    pub(crate) fn observe(&mut self, observation: BrokerHealthObservation) -> BrokerReadiness {
        self.last_kind = Some(observation.status);
        match observation.status {
            BrokerHealthStatus::Up => {
                self.verified = Some(VerifiedBroker::Up);
                BrokerReadiness::Healthy
            }
            BrokerHealthStatus::Down => {
                self.verified = Some(VerifiedBroker::Down);
                BrokerReadiness::Unhealthy
            }
            BrokerHealthStatus::Unspecified => {
                match (observation.healthy, observation.broker_connected) {
                    (true, true) => BrokerReadiness::Healthy,
                    (true, false) => BrokerReadiness::Degraded,
                    _ => BrokerReadiness::Unhealthy,
                }
            }
            status if status.is_unverified() => match self.verified {
                Some(VerifiedBroker::Up) => BrokerReadiness::Healthy,
                Some(VerifiedBroker::Down) => BrokerReadiness::Unhealthy,
                None => BrokerReadiness::Degraded,
            },
            _ => BrokerReadiness::Degraded,
        }
    }
}

type HealthFuture = Pin<Box<dyn Future<Output = Result<BridgeProbeResult, CamelError>> + Send>>;

/// Trait abstracting a single gRPC health probe call (mockable for tests).
trait BridgeHealthProbe: Send + Sync {
    fn probe(&self) -> HealthFuture;

    /// Identity of the currently Ready bridge channel, `None` while the slot
    /// is absent or not Ready. Readiness resets retained verification
    /// whenever this differs from the previous check.
    fn ready_identity(&self) -> Option<BridgeIdentity>;

    /// True while no bridge slot exists for the broker yet (producer-only
    /// routes create it lazily). Readiness reports this as Degraded (200).
    fn not_started(&self) -> bool {
        false
    }
}

/// Tracks the slot watch for replacement detection: a new slot object, a new
/// watch channel, or a new watch value all mean the Ready bridge behind the
/// broker was replaced.
struct WatchMark {
    slot: usize,
    generation: u64,
    rx: watch::Receiver<BridgeState>,
}

/// Real probe that reaches into the bridge pool, extracts the channel from a
/// Ready slot, and issues a gRPC `HealthRequest` via `bridge_service_client`.
struct JmsBridgeHealthProbe {
    pool: Arc<JmsBridgePool>,
    broker_name: String,
    watch_mark: std::sync::Mutex<Option<WatchMark>>,
    /// Test-only trigger fired inside ready_identity's observation window,
    /// modeling a replacement published mid-observation.
    #[cfg(test)]
    mid_observation_hook: std::sync::Mutex<Option<Box<dyn FnMut() + Send + Sync>>>,
}

impl JmsBridgeHealthProbe {
    fn new(pool: Arc<JmsBridgePool>, broker_name: String) -> Self {
        Self {
            pool,
            broker_name,
            watch_mark: std::sync::Mutex::new(None),
            #[cfg(test)]
            mid_observation_hook: std::sync::Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn set_mid_observation_hook(&self, hook: Option<Box<dyn FnMut() + Send + Sync>>) {
        *self
            .mid_observation_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
    }

    #[cfg(test)]
    fn fire_mid_observation_hook(&self) {
        if let Some(hook) = self
            .mid_observation_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            hook();
        }
    }
}

impl BridgeHealthProbe for JmsBridgeHealthProbe {
    fn not_started(&self) -> bool {
        !self.pool.slots.contains_key(&self.broker_name)
    }

    fn ready_identity(&self) -> Option<BridgeIdentity> {
        // Short-lived DashMap guard: dropped before any await.
        let slot = self.pool.slots.get(&self.broker_name)?;
        let slot_ptr = Arc::as_ptr(&slot) as usize;
        let mut mark = self
            .watch_mark
            // Recovery, not propagation: the mark holds plain identity data,
            // still valid after a panicked checker.
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Take the tracking out so every branch hands back a fresh one
        // without holding a borrow across the rewrap.
        let previous = mark.take();
        let (generation, rx, ready) = match previous {
            Some(mut tracked)
                if tracked.slot == slot_ptr && tracked.rx.same_channel(&slot.state_rx) =>
            {
                // One acknowledged watch snapshot decides BOTH replacement
                // and readiness: has_changed() on the acknowledged ref is
                // atomic with the value we observe, so a replacement
                // published mid-observation can never be acknowledged while
                // reported as unchanged.
                #[cfg(test)]
                self.fire_mid_observation_hook();
                let snapshot = tracked.rx.borrow_and_update();
                let replaced = snapshot.has_changed();
                let ready = matches!(&*snapshot, BridgeState::Ready { .. });
                drop(snapshot);
                if replaced {
                    tracked.generation = tracked.generation.wrapping_add(1);
                }
                (tracked.generation, tracked.rx, ready)
            }
            previous => {
                // First sight, a replaced slot, or a replaced watch channel:
                // the old tracking cannot speak for the new bridge. Start
                // from one acknowledged snapshot of the slot's current watch.
                let generation = previous
                    .as_ref()
                    .map_or(0, |tracked| tracked.generation.wrapping_add(1));
                let mut rx = slot.state_rx.clone();
                let ready = matches!(&*rx.borrow_and_update(), BridgeState::Ready { .. });
                (generation, rx, ready)
            }
        };
        *mark = Some(WatchMark {
            slot: slot_ptr,
            generation,
            rx,
        });
        ready.then_some(BridgeIdentity(generation))
    }

    fn probe(&self) -> HealthFuture {
        let pool = Arc::clone(&self.pool);
        let broker_name = self.broker_name.clone();

        Box::pin(async move {
            let slot = pool.slots.get(&broker_name).ok_or_else(|| {
                CamelError::ProcessorError(format!(
                    "No bridge slot found for broker '{}'",
                    broker_name
                ))
            })?;

            let channel = match &*slot.state_rx.borrow() {
                BridgeState::Ready { channel } => channel.clone(),
                other => {
                    return Err(CamelError::ProcessorError(format!(
                        "Bridge not ready for broker '{}': {:?}",
                        broker_name, other
                    )));
                }
            };
            drop(slot);

            let mut client = bridge_service_client(channel);
            let resp = client.health(HealthRequest {}).await.map_err(|e| {
                CamelError::ProcessorError(format!(
                    "JMS bridge health RPC failed for broker '{}': {}",
                    broker_name, e
                ))
            })?;

            let resp = resp.into_inner();
            Ok(BridgeProbeResult {
                healthy: resp.healthy,
                broker_connected: resp.broker_connected,
                message: resp.message,
                status: resp.status,
                age_ms: resp.age_ms,
            })
        })
    }
}

/// Async health check for the JMS component that probes the bridge sidecar
/// via gRPC `HealthRequest`.
pub struct JmsHealthCheck {
    probe: Arc<dyn BridgeHealthProbe>,
    timeout: Duration,
    /// Retained broker verification. The mutex is held across the whole
    /// check so concurrent readiness checks serialize instead of
    /// interleaving observations.
    memory: tokio::sync::Mutex<BrokerHealthMemory>,
}

impl JmsHealthCheck {
    pub fn new(pool: Arc<JmsBridgePool>, broker_name: String) -> Self {
        Self {
            probe: Arc::new(JmsBridgeHealthProbe::new(pool, broker_name)),
            timeout: Duration::from_secs(2),
            memory: tokio::sync::Mutex::new(BrokerHealthMemory::default()),
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn BridgeHealthProbe>, timeout: Duration) -> Self {
        Self {
            probe,
            timeout,
            memory: tokio::sync::Mutex::new(BrokerHealthMemory::default()),
        }
    }
}

#[async_trait]
impl AsyncHealthCheck for JmsHealthCheck {
    fn name(&self) -> &str {
        "jms"
    }

    async fn check(&self) -> CheckResult {
        let mut memory = self.memory.lock().await;
        if self.probe.not_started() {
            memory.reset(None);
            return CheckResult::degraded(self.name(), "bridge not started (starts on first use)");
        }
        // Absent/not-ready slots (`None`) and replaced Ready channels (a
        // different identity) drop retained verification.
        let identity = self.probe.ready_identity();
        if memory.identity != identity {
            memory.reset(identity);
        }
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(result)) => {
                // Fence: the RPC was in flight, so the bridge may have been
                // replaced after the identity was sampled. Revalidate before
                // applying anything; a response from a replaced instance is
                // unbound and must not verify the new bridge. Identity and
                // RPC are fenced as one unit — UP/DOWN from a changed
                // instance is discarded as Degraded/unverified, while real
                // transport errors and timeouts stay Unhealthy below.
                let identity = self.probe.ready_identity();
                if memory.identity != identity {
                    memory.reset(identity);
                    return CheckResult::degraded(
                        self.name(),
                        "bridge replaced during health check; status unverified",
                    );
                }
                let observation = BrokerHealthObservation {
                    status: BrokerHealthStatus::from_raw(result.status),
                    healthy: result.healthy,
                    broker_connected: result.broker_connected,
                    age_ms: result.age_ms,
                };
                let readiness = memory.observe(observation);
                let message = result.message;
                match readiness {
                    BrokerReadiness::Healthy => CheckResult::healthy(self.name()),
                    BrokerReadiness::Degraded => CheckResult::degraded(
                        self.name(),
                        if message.is_empty() {
                            "broker status unverified"
                        } else {
                            &message
                        },
                    ),
                    BrokerReadiness::Unhealthy => CheckResult::unhealthy(
                        self.name(),
                        if message.is_empty() {
                            "bridge reported unhealthy"
                        } else {
                            &message
                        },
                    ),
                }
            }
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "health probe timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::HealthStatus;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};

    use crate::BrokerType;
    use crate::component::BridgeSlot;
    use crate::config::JmsPoolConfig;
    use crate::proto::{BrokerFailureClass, HealthResponse, HealthStatus as ProtoHealthStatus};

    /// Task 3.1: the additive wire fields exist on the generated type, round
    /// trip through prost, and default to "no capabilities / unspecified".
    #[test]
    fn health_response_new_fields_compile() {
        use prost::Message;

        let response = HealthResponse {
            capabilities: vec!["broker-tls-v1".to_string()],
            failure_class: BrokerFailureClass::TlsHostname as i32,
            ..Default::default()
        };
        let encoded = response.encode_to_vec();
        let decoded =
            HealthResponse::decode(encoded.as_slice()).expect("health response must decode");
        assert_eq!(decoded.capabilities, vec!["broker-tls-v1".to_string()]);
        assert_eq!(
            decoded.failure_class,
            BrokerFailureClass::TlsHostname as i32
        );

        let default = HealthResponse::default();
        assert!(default.capabilities.is_empty());
        assert_eq!(
            default.failure_class,
            BrokerFailureClass::Unspecified as i32
        );
    }

    struct MockHealthProbe {
        responder: Arc<dyn Fn() -> HealthFuture + Send + Sync>,
        identity: AtomicI64,
    }

    impl MockHealthProbe {
        fn new<F>(f: F) -> Self
        where
            F: Fn() -> HealthFuture + Send + Sync + 'static,
        {
            Self {
                responder: Arc::new(f),
                identity: AtomicI64::new(0),
            }
        }
    }

    impl BridgeHealthProbe for MockHealthProbe {
        fn probe(&self) -> HealthFuture {
            (self.responder)()
        }

        fn ready_identity(&self) -> Option<BridgeIdentity> {
            let raw = self.identity.load(Ordering::SeqCst);
            (raw >= 0).then_some(BridgeIdentity(raw as u64))
        }
    }

    struct NotStartedProbe;

    impl BridgeHealthProbe for NotStartedProbe {
        fn probe(&self) -> HealthFuture {
            Box::pin(async { Err(CamelError::ProcessorError("must not probe".to_string())) })
        }

        fn ready_identity(&self) -> Option<BridgeIdentity> {
            None
        }

        fn not_started(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn jms_health_check_degraded_when_bridge_not_started() {
        let check = JmsHealthCheck::with_probe_for_tests(
            Arc::new(NotStartedProbe),
            Duration::from_millis(50),
        );

        let result = check.check().await;

        assert_eq!(result.status, HealthStatus::Degraded);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("not started"))
        );
    }

    /// Legacy UNSPECIFIED-status response: older bridges send only the
    /// booleans, and the engine keeps the previous boolean mapping for them.
    fn probe_result(healthy: bool, broker_connected: bool, message: &str) -> HealthFuture {
        let message = message.to_string();
        Box::pin(async move {
            Ok(BridgeProbeResult {
                healthy,
                broker_connected,
                message,
                status: ProtoHealthStatus::Unspecified as i32,
                age_ms: 0,
            })
        })
    }

    fn probe_response(
        status: ProtoHealthStatus,
        age_ms: u64,
        healthy: bool,
        broker_connected: bool,
        message: &str,
    ) -> BridgeProbeResult {
        BridgeProbeResult {
            healthy,
            broker_connected,
            message: message.to_string(),
            status: status as i32,
            age_ms,
        }
    }

    /// One scripted probe outcome. `Error` models a transport failure and
    /// `Hang` a response that outlives the readiness timeout.
    #[derive(Clone)]
    enum ProbeStep {
        Response(BridgeProbeResult),
        Error(String),
        Hang(Duration),
    }

    /// Probe whose responses and bridge identity the test drives step by
    /// step. A negative identity means "slot present but not Ready": probes
    /// fail at the transport layer without consuming an observation.
    struct SequencedProbe {
        steps: Vec<ProbeStep>,
        calls: AtomicUsize,
        identity: AtomicI64,
        absent: AtomicBool,
        /// Test-only callback fired after the readiness check sampled the
        /// identity but before the response applies — a replacement landing
        /// while the probe is in flight.
        on_probe: std::sync::Mutex<Option<Box<dyn FnMut() + Send + Sync>>>,
    }

    impl SequencedProbe {
        fn new(steps: Vec<ProbeStep>) -> Self {
            Self {
                steps,
                calls: AtomicUsize::new(0),
                identity: AtomicI64::new(0),
                absent: AtomicBool::new(false),
                on_probe: std::sync::Mutex::new(None),
            }
        }

        fn set_identity(&self, identity: Option<u64>) {
            self.identity
                .store(identity.map_or(-1, |value| value as i64), Ordering::SeqCst);
        }

        fn set_absent(&self, absent: bool) {
            self.absent.store(absent, Ordering::SeqCst);
        }

        fn set_on_probe_hook(&self, hook: Option<Box<dyn FnMut() + Send + Sync>>) {
            *self
                .on_probe
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
        }

        fn fire_on_probe_hook(&self) {
            if let Some(hook) = self
                .on_probe
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_mut()
            {
                hook();
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    fn step_future(step: Option<ProbeStep>) -> HealthFuture {
        match step {
            Some(ProbeStep::Response(result)) => Box::pin(async move { Ok(result) }),
            Some(ProbeStep::Error(message)) => {
                Box::pin(async move { Err(CamelError::ProcessorError(message)) })
            }
            Some(ProbeStep::Hang(duration)) => Box::pin(async move {
                tokio::time::sleep(duration).await;
                Ok(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok"))
            }),
            None => Box::pin(async {
                Err(CamelError::ProcessorError(
                    "probe sequence exhausted".to_string(),
                ))
            }),
        }
    }

    impl BridgeHealthProbe for SequencedProbe {
        fn probe(&self) -> HealthFuture {
            if self.identity.load(Ordering::SeqCst) < 0 {
                return Box::pin(async {
                    Err(CamelError::ProcessorError(
                        "Bridge not ready for broker 'test'".to_string(),
                    ))
                });
            }
            let index = self.calls.fetch_add(1, Ordering::SeqCst);
            self.fire_on_probe_hook();
            step_future(self.steps.get(index).cloned())
        }

        fn ready_identity(&self) -> Option<BridgeIdentity> {
            if self.absent.load(Ordering::SeqCst) {
                return None;
            }
            let raw = self.identity.load(Ordering::SeqCst);
            (raw >= 0).then_some(BridgeIdentity(raw as u64))
        }

        fn not_started(&self) -> bool {
            self.absent.load(Ordering::SeqCst)
        }
    }

    /// Probe that pairs the real pool/watch `ready_identity` (lazy channel,
    /// no bridge process or network) with scripted observations.
    struct HybridProbe {
        real: JmsBridgeHealthProbe,
        steps: Vec<ProbeStep>,
        calls: AtomicUsize,
    }

    impl HybridProbe {
        fn new(real: JmsBridgeHealthProbe, steps: Vec<ProbeStep>) -> Self {
            Self {
                real,
                steps,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl BridgeHealthProbe for HybridProbe {
        fn probe(&self) -> HealthFuture {
            let index = self.calls.fetch_add(1, Ordering::SeqCst);
            step_future(self.steps.get(index).cloned())
        }

        fn ready_identity(&self) -> Option<BridgeIdentity> {
            self.real.ready_identity()
        }

        fn not_started(&self) -> bool {
            self.real.not_started()
        }
    }

    /// Pool with one broker slot parked in `Starting`, plus the slot handle
    /// so tests can publish Ready states on the real watch.
    fn watch_fixture(broker: &str) -> (Arc<JmsBridgePool>, Arc<BridgeSlot>) {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::default())
                .expect("default jms pool config must be valid"),
        );
        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = Arc::new(BridgeSlot {
            name: broker.to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
        });
        pool.slots.insert(broker.to_string(), Arc::clone(&slot));
        (pool, slot)
    }

    #[tokio::test]
    async fn jms_health_check_healthy_when_verified_up() {
        let probe = Arc::new(MockHealthProbe::new(|| probe_result(true, true, "ok")));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn jms_health_check_unhealthy_when_verified_down() {
        let probe = Arc::new(MockHealthProbe::new(|| {
            probe_result(false, false, "broker unreachable")
        }));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("broker unreachable"));
    }

    #[tokio::test]
    async fn jms_health_check_degraded_for_unknown_or_skipped() {
        let probe = Arc::new(MockHealthProbe::new(|| {
            probe_result(true, false, "broker probe skipped; connectivity unverified")
        }));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        for _ in 0..2 {
            let result = check.check().await;

            assert_eq!(result.name, "jms");
            assert_eq!(result.status, HealthStatus::Degraded);
            assert_eq!(
                result.message.as_deref(),
                Some("broker probe skipped; connectivity unverified")
            );
        }
    }

    #[tokio::test]
    async fn jms_health_check_degraded_for_stale() {
        let probe = Arc::new(MockHealthProbe::new(|| {
            probe_result(true, false, "broker status stale; age 10001 ms")
        }));
        let check = JmsHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Degraded);
        assert_eq!(
            result.message.as_deref(),
            Some("broker status stale; age 10001 ms")
        );
    }

    #[tokio::test]
    async fn jms_health_check_unhealthy_on_error_and_timeout() {
        let error_probe = Arc::new(MockHealthProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ProcessorError(
                    "simulated bridge error".to_string(),
                ))
            })
        }));
        let error_check =
            JmsHealthCheck::with_probe_for_tests(error_probe, Duration::from_millis(50));

        let result = error_check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated bridge error"))
        );

        let sleeping_probe = Arc::new(MockHealthProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(BridgeProbeResult {
                    healthy: true,
                    broker_connected: true,
                    message: String::new(),
                    status: ProtoHealthStatus::Unspecified as i32,
                    age_ms: 0,
                })
            })
        }));
        let timeout_check =
            JmsHealthCheck::with_probe_for_tests(sleeping_probe, Duration::from_millis(5));

        let result = timeout_check.check().await;

        assert_eq!(result.name, "jms");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("health probe timed out"));
    }

    /// Second-mission regression: unverified observations (STALE, UNKNOWN,
    /// SKIPPED) must retain the last verified broker state instead of
    /// degrading, and never read as fresh disconnection evidence.
    #[tokio::test]
    async fn jms_health_stale_keeps_last_verified_state() {
        let probe = Arc::new(SequencedProbe::new(vec![
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                60_000,
                true,
                false,
                "broker status stale; age 60000 ms",
            )),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                120_000,
                true,
                false,
                "broker status stale; age 120000 ms",
            )),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Unknown,
                0,
                true,
                false,
                "broker status not yet checked",
            )),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Skipped,
                0,
                true,
                false,
                "broker probe skipped; connectivity unverified",
            )),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Down,
                0,
                false,
                false,
                "broker unreachable",
            )),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                60_000,
                true,
                false,
                "broker status stale; age 60000 ms",
            )),
        ]));
        let check = JmsHealthCheck::with_probe_for_tests(
            Arc::clone(&probe) as Arc<dyn BridgeHealthProbe>,
            Duration::from_millis(50),
        );

        let expected = [
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Unhealthy,
            HealthStatus::Unhealthy,
        ];
        for (index, want) in expected.iter().enumerate() {
            let result = check.check().await;
            assert_eq!(result.status, *want, "after observation {}", index + 1);
        }
        assert_eq!(
            probe.calls(),
            expected.len(),
            "every observation must probe exactly once"
        );
    }

    /// Startup, absence, not-ready, and replacement paths each reset
    /// retained verification independently. The absence and not-ready proofs
    /// reuse ONE synthetic identity, so an identity rotation cannot mask the
    /// reset being proven.
    #[tokio::test]
    async fn jms_health_unverified_startup_and_replacement_are_degraded() {
        let probe = Arc::new(SequencedProbe::new(vec![
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Unknown,
                0,
                true,
                false,
                "broker status not yet checked",
            )),
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Unknown,
                0,
                true,
                false,
                "broker status not yet checked",
            )),
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Skipped,
                0,
                true,
                false,
                "broker probe skipped; connectivity unverified",
            )),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                60_000,
                true,
                false,
                "broker status stale; age 60000 ms",
            )),
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                60_000,
                true,
                false,
                "broker status stale; age 60000 ms",
            )),
        ]));
        let check = JmsHealthCheck::with_probe_for_tests(
            Arc::clone(&probe) as Arc<dyn BridgeHealthProbe>,
            Duration::from_millis(50),
        );

        // Absent slot: degraded without probing.
        probe.set_absent(true);
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Degraded, "absent slot");
        assert_eq!(probe.calls(), 0, "absent slot must not probe");

        // First observation of a Ready bridge: live but unverified.
        probe.set_absent(false);
        probe.set_identity(Some(1));
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Degraded, "initial unverified");

        // Verified UP.
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "verified up");

        // Not-ready event under the SAME identity: transport failure, and
        // the earlier UP must not survive it.
        probe.set_identity(None);
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Unhealthy, "not-ready slot");
        assert_eq!(
            probe.calls(),
            2,
            "a not-ready slot must not consume an observation"
        );

        // Same identity again, unverified: Degraded proves the not-ready
        // reset alone cleared the verification (no rotation involved).
        probe.set_identity(Some(1));
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "not-ready reset cleared UP"
        );

        // A second identity verifies UP.
        probe.set_identity(Some(2));
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "identity 2 up");

        // Absence under the SAME identity clears it.
        probe.set_absent(true);
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Degraded, "absence resets");

        // Same identity again: Degraded proves the absence reset alone
        // cleared the verification.
        probe.set_absent(false);
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "absence reset cleared UP"
        );

        // Replacement path: identity 3 verifies UP...
        probe.set_identity(Some(3));
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "identity 3 unverified"
        );
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "identity 3 up");

        // ...and a replaced identity reporting STALE starts unverified again,
        // never inheriting the previous bridge's UP.
        probe.set_identity(Some(4));
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "replacement bridge is unverified"
        );
        assert_eq!(probe.calls(), 8);
    }

    /// Real watch replacement tracking: one acknowledged Ready notification
    /// must yield a stable identity across later checks; a replacement Ready
    /// publication rotates it exactly once. Drives the real
    /// `JmsBridgeHealthProbe.ready_identity` against lazy channels, so no
    /// bridge process or network is involved.
    #[tokio::test]
    async fn jms_health_ready_identity_stable_until_replacement() {
        let (pool, slot) = watch_fixture("default");
        let real = JmsBridgeHealthProbe::new(pool, "default".to_string());
        // Unseen Ready notification on the slot watch before the first look.
        slot.state_tx.send_replace(BridgeState::Ready {
            channel: tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy(),
        });
        let stale = |age: u64| {
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                age,
                true,
                false,
                "broker status stale",
            ))
        };
        let probe = Arc::new(HybridProbe::new(
            real,
            vec![
                ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
                stale(60_000),
                stale(60_000),
                stale(60_000),
                stale(120_000),
                ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
                stale(60_000),
            ],
        ));
        let check = JmsHealthCheck::with_probe_for_tests(
            Arc::clone(&probe) as Arc<dyn BridgeHealthProbe>,
            Duration::from_millis(50),
        );

        // UP verifies the bridge the notification announced.
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "up");

        // Later STALE checks with no further state change stay Healthy: the
        // identity must be stable after the one acknowledged watch version.
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Healthy,
            "stale after up with no state change"
        );
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "stale still retained");
        let identity = probe.real.ready_identity().expect("ready identity");
        assert_eq!(
            probe.real.ready_identity(),
            Some(identity),
            "identity stable between checks"
        );

        // One replacement Ready publication rotates the identity exactly
        // once: the next STALE is Degraded and stays Degraded.
        slot.state_tx.send_replace(BridgeState::Ready {
            channel: tonic::transport::Channel::from_static("http://127.0.0.1:2").connect_lazy(),
        });
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Degraded, "replacement stale");
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "stale remains degraded until verification"
        );

        // Fresh verification of the replacement; a later STALE retains it.
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "replacement up");
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "next stale after up");
    }

    /// Regression: a replacement published inside ready_identity's
    /// observation window must still rotate the identity and reset retained
    /// verification. The acknowledged snapshot — not an earlier independent
    /// borrow — decides both the replacement and the slot's Ready state.
    #[tokio::test]
    async fn jms_health_replacement_during_identity_observation_resets_verification() {
        let (pool, slot) = watch_fixture("default");
        let real = JmsBridgeHealthProbe::new(pool, "default".to_string());
        slot.state_tx.send_replace(BridgeState::Ready {
            channel: tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy(),
        });
        let up = || ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok"));
        let stale_step = |age: u64| {
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                age,
                true,
                false,
                "broker status stale",
            ))
        };
        let probe = Arc::new(HybridProbe::new(
            real,
            vec![
                up(),
                stale_step(60_000),
                stale_step(60_000),
                ProbeStep::Error("bridge not ready mid-replacement".to_string()),
                up(),
                stale_step(60_000),
            ],
        ));
        let check = JmsHealthCheck::with_probe_for_tests(
            Arc::clone(&probe) as Arc<dyn BridgeHealthProbe>,
            Duration::from_millis(50),
        );

        // Verified UP against the original bridge instance.
        let identity_before = probe.real.ready_identity().expect("ready identity");
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "up");

        // Publish a replacement Ready inside the identity-observation window.
        probe.real.set_mid_observation_hook(Some(Box::new({
            let slot = Arc::clone(&slot);
            move || {
                slot.state_tx.send_replace(BridgeState::Ready {
                    channel: tonic::transport::Channel::from_static("http://127.0.0.1:2")
                        .connect_lazy(),
                });
            }
        })));
        let result = check.check().await;
        probe.real.set_mid_observation_hook(None);

        assert_ne!(
            probe.real.ready_identity(),
            Some(identity_before),
            "mid-observation replacement must rotate the identity"
        );
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "replacement must reset retained UP"
        );

        // The rotated identity is stable afterwards: another STALE stays
        // Degraded instead of inheriting anything.
        let identity_after = probe.real.ready_identity().expect("ready identity");
        let result = check.check().await;
        assert_eq!(
            probe.real.ready_identity(),
            Some(identity_after),
            "identity stable after rotation"
        );
        assert_eq!(result.status, HealthStatus::Degraded, "still unverified");

        // A replacement observed mid-flight to a NOT-Ready value: the
        // acknowledged snapshot is the watched value itself, so readiness
        // sees a not-ready slot (None identity, transport failure) instead
        // of an earlier Ready borrow.
        probe.real.set_mid_observation_hook(Some(Box::new({
            let slot = Arc::clone(&slot);
            move || {
                slot.state_tx
                    .send_replace(BridgeState::Degraded("replacement stopped".to_string()));
            }
        })));
        let result = check.check().await;
        probe.real.set_mid_observation_hook(None);
        assert_eq!(
            result.status,
            HealthStatus::Unhealthy,
            "not-ready mid-replacement"
        );
        assert!(
            probe.real.ready_identity().is_none(),
            "acknowledged snapshot is the not-ready watched value"
        );

        // The slot returns Ready on a third channel: fresh verification,
        // then STALE retains it.
        slot.state_tx.send_replace(BridgeState::Ready {
            channel: tonic::transport::Channel::from_static("http://127.0.0.1:3").connect_lazy(),
        });
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "fresh up");
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Healthy,
            "stale retains fresh up"
        );
    }

    /// Regression: a response that completed while its bridge was being
    /// replaced is unbound — it must never verify the new instance. The
    /// identity is revalidated after the awaited probe and unbound successful
    /// responses are discarded until a fresh observation arrives.
    #[tokio::test]
    async fn jms_health_replacement_during_probe_discards_obsolete_verification() {
        let probe = Arc::new(SequencedProbe::new(vec![
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                60_000,
                true,
                false,
                "broker status stale",
            )),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                60_000,
                true,
                false,
                "broker status stale",
            )),
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                60_000,
                true,
                false,
                "broker status stale",
            )),
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Down,
                0,
                false,
                false,
                "broker unreachable",
            )),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                60_000,
                true,
                false,
                "broker status stale",
            )),
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                60_000,
                true,
                false,
                "broker status stale",
            )),
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
        ]));
        let check = JmsHealthCheck::with_probe_for_tests(
            Arc::clone(&probe) as Arc<dyn BridgeHealthProbe>,
            Duration::from_millis(50),
        );

        // Identity 1 verifies UP.
        probe.set_identity(Some(1));
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "identity 1 up");

        // Replacement lands while the probe is in flight: the STALE answer
        // belongs to the old instance and must not verify identity 2.
        {
            let p = Arc::clone(&probe);
            probe.set_on_probe_hook(Some(Box::new(move || p.set_identity(Some(2)))));
        }
        let result = check.check().await;
        probe.set_on_probe_hook(None);
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "obsolete STALE must be discarded"
        );
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "identity 2 unverified"
        );
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "identity 2 up");

        // An old instance's UP completing after a replacement is equally
        // unbound: discarded, Degraded until the new instance verifies.
        {
            let p = Arc::clone(&probe);
            probe.set_on_probe_hook(Some(Box::new(move || p.set_identity(Some(3)))));
        }
        let result = check.check().await;
        probe.set_on_probe_hook(None);
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "obsolete UP must be discarded"
        );
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "identity 3 unverified"
        );
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "identity 3 up");

        // Retained DOWN must not leak into a replaced instance either.
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Unhealthy, "identity 3 down");
        {
            let p = Arc::clone(&probe);
            probe.set_on_probe_hook(Some(Box::new(move || p.set_identity(Some(4)))));
        }
        let result = check.check().await;
        probe.set_on_probe_hook(None);
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "retained DOWN must not report the new instance Unhealthy"
        );
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "identity 4 up");

        // Replacement to a not-ready slot mid-probe: the unbound response is
        // discarded and the None identity resets retention.
        {
            let p = Arc::clone(&probe);
            probe.set_on_probe_hook(Some(Box::new(move || p.set_identity(None))));
        }
        let result = check.check().await;
        probe.set_on_probe_hook(None);
        assert_eq!(result.status, HealthStatus::Degraded, "not-ready discard");

        // Absence on top keeps retention cleared.
        probe.set_absent(true);
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Degraded, "absent reset");
        probe.set_absent(false);

        // Nothing survived the None/absent chain: STALE is Degraded, then a
        // fresh UP verifies.
        probe.set_identity(Some(5));
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Degraded,
            "identity 5 unverified"
        );
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "identity 5 up");
    }

    /// Legacy UNSPECIFIED responses keep the boolean mapping, future numeric
    /// statuses are unverified-but-preserving, and transport failures stay
    /// Unhealthy without destroying retained verification.
    #[tokio::test]
    async fn jms_health_legacy_future_status_and_rpc_errors() {
        for (healthy, connected, expected) in [
            (true, true, HealthStatus::Healthy),
            (true, false, HealthStatus::Degraded),
            (false, false, HealthStatus::Unhealthy),
        ] {
            let probe = Arc::new(MockHealthProbe::new(move || {
                probe_result(healthy, connected, "legacy bridge")
            }));
            let check = JmsHealthCheck::with_probe_for_tests(
                Arc::clone(&probe) as Arc<dyn BridgeHealthProbe>,
                Duration::from_millis(50),
            );
            let result = check.check().await;
            assert_eq!(result.status, expected, "legacy {healthy}/{connected}");
        }

        // A future numeric status is unverified: Degraded before any
        // verification.
        let unverified = Arc::new(SequencedProbe::new(vec![ProbeStep::Response(
            BridgeProbeResult {
                healthy: true,
                broker_connected: false,
                message: "future status".to_string(),
                status: 42,
                age_ms: 0,
            },
        )]));
        let check = JmsHealthCheck::with_probe_for_tests(
            Arc::clone(&unverified) as Arc<dyn BridgeHealthProbe>,
            Duration::from_millis(50),
        );
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Degraded, "future unverified");

        // After UP, the same future status preserves the verification.
        let preserved = Arc::new(SequencedProbe::new(vec![
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Response(BridgeProbeResult {
                healthy: true,
                broker_connected: false,
                message: "future status".to_string(),
                status: 42,
                age_ms: 0,
            }),
        ]));
        let check = JmsHealthCheck::with_probe_for_tests(
            Arc::clone(&preserved) as Arc<dyn BridgeHealthProbe>,
            Duration::from_millis(50),
        );
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "up before future");
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Healthy,
            "future preserves UP verification"
        );

        // Transport failures stay Unhealthy and never manufacture broker
        // verification; a following STALE retains the earlier UP.
        let probe = Arc::new(SequencedProbe::new(vec![
            ProbeStep::Response(probe_response(ProtoHealthStatus::Up, 0, true, true, "ok")),
            ProbeStep::Error("simulated bridge error".to_string()),
            ProbeStep::Hang(Duration::from_millis(200)),
            ProbeStep::Response(probe_response(
                ProtoHealthStatus::Stale,
                60_000,
                true,
                false,
                "broker status stale; age 60000 ms",
            )),
        ]));
        let check = JmsHealthCheck::with_probe_for_tests(
            Arc::clone(&probe) as Arc<dyn BridgeHealthProbe>,
            Duration::from_millis(50),
        );
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "verified up");
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Unhealthy, "rpc error");
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Unhealthy, "timeout");
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy, "stale retains UP");
    }
}
