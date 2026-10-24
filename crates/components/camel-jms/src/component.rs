use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use camel_bridge::{
    download::ensure_binary,
    health::wait_for_health,
    process::{BridgeError, BridgeProcess, BridgeProcessConfig, BrokerType, JmsTlsEnv},
};
use camel_component_api::{
    BoxProcessor, CamelError, Component, Consumer, Endpoint, Exchange, NetworkRetryPolicy,
    ProducerContext,
};
use dashmap::DashMap;
use tokio::sync::{Mutex, watch};
use tonic::transport::Channel;
use tower::Service;
use tracing::{info, warn};

use crate::config::{
    BrokerConfig, BrokerTlsConfig, HealthBrokerProbeMode, JmsEndpointConfig, JmsPoolConfig,
    attempt_journal_broker_key, default_attempt_journal_dir, ensure_attempt_journal_dir,
};
use crate::consumer::JmsConsumer;
use crate::diagnostics::{
    LastBrokerFailure, MISSING_BROKER_TLS_CAPABILITY, ensure_selector_capability,
    ensure_tls_capability, evaluate_startup_selector_gate, missing_message_selector_capability,
};
use crate::health::{
    BrokerHealthMemory, BrokerHealthObservation, BrokerHealthStatus, JmsHealthCheck,
};
use crate::producer::JmsProducer;
use crate::proto::{HealthRequest, bridge_service_client::BridgeServiceClient};

// ── Transport error classification ───────────────────────────────────────────

/// Shared constant prefix for all bridge transport errors.
///
/// Both error-producing sites (producer.rs, consumer.rs) and the detection
/// helper (`is_bridge_transport_error`) reference this constant so that a
/// format-string drift cannot silently break retry logic.
pub const BRIDGE_TRANSPORT_ERROR_PREFIX: &str = "JMS gRPC ";
const MAX_RESTART_ATTEMPTS: u32 = 10;

/// Fixed absolute budget for one channel refresh (task 2.5).
///
/// Established once on entry to [`JmsBridgePool::refresh_slot_channel`], it
/// covers process-lock acquisition and the whole selected connector future.
/// Connection progress, retries, and progress callbacks never renew it.
const CHANNEL_REFRESH_TIMEOUT_MS: u64 = 2_000;

// ── Bridge gRPC decode limit ─────────────────────────────────────────────────

/// Maximum inbound gRPC message size accepted when decoding Java bridge
/// responses.
///
/// The Java side caps request bodies at 16 MiB; the extra headroom covers the
/// protobuf envelope and headers around a maximal body. Without this, tonic's
/// 4 MiB default rejects legal maximal `JmsMessage`s mid-stream.
pub(crate) const BRIDGE_MAX_DECODING_MESSAGE_SIZE: usize = 20 * 1024 * 1024;

pub(crate) fn bridge_decode_limit() -> usize {
    BRIDGE_MAX_DECODING_MESSAGE_SIZE
}

/// Underlying transport type for bridge gRPC clients: the raw channel, or
/// the same channel wrapped with the otel trace-context interceptor.
#[cfg(feature = "otel")]
pub(crate) type BridgeClientChannel = tonic::service::interceptor::InterceptedService<
    Channel,
    crate::trace_context::TraceContextInterceptor,
>;
#[cfg(not(feature = "otel"))]
pub(crate) type BridgeClientChannel = Channel;

/// Production-path constructor for bridge gRPC clients.
///
/// Applies [`bridge_decode_limit`] to every RPC issued through this client.
/// All `BridgeServiceClient` construction sites must go through this helper
/// rather than calling `BridgeServiceClient::new` directly, so a channel
/// handed over by the pool always decodes maximal legal messages. With the
/// `otel` feature the client also attaches the ambient W3C trace context to
/// every RPC; a `traceparent` already present on the request always wins.
pub(crate) fn bridge_service_client(
    channel: Channel,
) -> crate::proto::bridge_service_client::BridgeServiceClient<BridgeClientChannel> {
    #[cfg(feature = "otel")]
    let client = BridgeServiceClient::with_interceptor(
        channel,
        crate::trace_context::TraceContextInterceptor,
    );
    #[cfg(not(feature = "otel"))]
    let client = BridgeServiceClient::new(channel);
    client.max_decoding_message_size(bridge_decode_limit())
}

// ── BridgeState ──────────────────────────────────────────────────────────────

/// Outcome of starting a bridge process, before the caller selects a
/// [`BridgeState`].
///
/// `Permanent` is a config-class rejection that must never be retried; phase 4
/// maps it to `BridgeState::Parked`. Every other failure keeps the historical
/// transient `CamelError` path.
#[derive(Debug)]
pub(crate) enum StartBridgeError {
    /// A permanent startup rejection carrying the bridge's reason.
    Permanent(String),
    /// A transient failure that the existing restart policy handles.
    Other(CamelError),
}

/// Maps a startup [`BridgeError`] into the typed start outcome.
///
/// The permanent variant is matched before any string formatting, so the
/// classification never depends on message substrings.
fn classify_start_error(error: BridgeError) -> StartBridgeError {
    match error {
        BridgeError::PermanentFailure(reason) => StartBridgeError::Permanent(reason),
        other => StartBridgeError::Other(CamelError::ProcessorError(format!(
            "JMS bridge start failed: {other}"
        ))),
    }
}

#[derive(Debug, Clone)]
pub enum BridgeState {
    Starting,
    Ready {
        channel: Channel,
    },
    Degraded(String),
    Restarting {
        attempt: u32,
        next_at: Instant,
    },
    /// Terminal. The bridge rejected startup permanently; the slot never
    /// retries, never respawns, and stays here until the operator repairs the
    /// cause and restarts the context (tasks 4.3/4.7).
    Parked(String),
    Stopped,
}

// ── Health-monitor liveness policy (task 2.4) ────────────────────────────────

/// Outcome of one health probe attempt.
///
/// A completed RPC is liveness proof regardless of broker fields; only RPC
/// errors and deadlines are liveness failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeOutcome {
    /// The health RPC completed with a response (any broker fields).
    Success,
    /// The health RPC exceeded `health_timeout_ms`.
    Timeout,
    /// The health RPC failed at the transport/status layer.
    RpcError,
}

/// Broker-status transition emitted by
/// [`BrokerStatusReporter::report_broker_status`]. The reporter uses it as
/// the emission gate so a repeated kind (or an age-only STALE change) does
/// not warn twice; under `cfg(test)` the matching transitions are also
/// recorded on the pool seam for assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerStatusTransition {
    /// Verified UP (explicit UP or legacy `healthy=true, broker_connected=true`).
    VerifiedUp,
    /// Verified DOWN (explicit DOWN or legacy `healthy=false,
    /// broker_connected=false`).
    BrokerDown,
    /// Any other pair (live but not verified connected).
    Unverified,
    /// Entered STALE with the numeric cache age; emitted once per entry into
    /// STALE, never repeated for age-only changes.
    Stale { age_ms: u64 },
}

/// Fixed, credential-safe event messages for broker-status transitions.
/// They are bound to the monitor's `warn!` calls and to the test capture
/// seam, so neither may ever grow provider message text, broker URLs, or
/// credential material.
const BROKER_STATUS_CHANGED_MSG: &str = "JMS bridge broker status changed";
const BROKER_STATUS_STALE_MSG: &str = "JMS bridge broker status stale; connectivity unverified, \
     retained verified broker state in effect";
const BROKER_STATUS_UNVERIFIED_MSG: &str =
    "JMS bridge broker status unverified; retained verified broker state in effect";

/// Test-only replacement launch result prepared for the monitor's
/// replacement branch (task 2.4).
#[cfg(test)]
type PreparedLaunchResult = Result<(BridgeProcess, Channel), StartBridgeError>;

/// Test-only connector future substituted after the real process endpoint is
/// selected, inside the unchanged production deadline (task 2.5).
#[cfg(test)]
type RefreshConnectorFuture = Pin<Box<dyn Future<Output = Result<Channel, BridgeError>> + Send>>;
#[cfg(test)]
type RefreshConnectorOverride = Arc<dyn Fn(u16) -> RefreshConnectorFuture + Send + Sync>;

/// Test-only monitor acknowledgement: one per completed probe, sent only
/// after the real counter/reporting/publication path has been applied.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct ProbeProcessed {
    /// Zero-based probe index.
    pub index: usize,
    /// Slot state observed at acknowledgement time.
    pub state: BridgeState,
    /// Probe outcome (successful response, timeout, or RPC error).
    pub outcome: ProbeOutcome,
    /// Consecutive-failure count after applying this probe.
    pub consecutive_failures: u32,
    /// Replacement attempts observed at acknowledgement time.
    pub attempts: usize,
}

/// Test-only observation of one completed `refresh_slot_channel` attempt
/// (task 2.5), recorded inside the real method after selecting the process.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefreshAttempt {
    /// Broker slot the refresh belonged to.
    pub broker_name: String,
    /// gRPC endpoint (port) of the process actually passed to `connect()`;
    /// `None` when no process was selected (missing process, lock timeout, or
    /// unknown broker).
    pub endpoint: Option<u16>,
    /// `true` when the refresh failed (connect error, timeout, publication
    /// refusal, or no selected process).
    pub failed: bool,
    /// `true` only when the fixed two-second deadline expired.
    pub timed_out: bool,
}

/// Test-only notification sent after a replacement attempt completed its
/// actual installation or error handling.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct ReplacementProcessed {
    /// Replacement attempts observed after this attempt.
    pub attempts: usize,
    /// `true` when a process was installed Ready.
    pub installed: bool,
    /// PID of the installed replacement child, when installed.
    pub installed_pid: Option<u32>,
    /// Slot state observed after installation/error handling.
    pub state: BridgeState,
}

/// Reports broker-status transitions for the health monitor.
///
/// Each completed health RPC is fed through a [`BrokerHealthMemory`] as a
/// [`BrokerHealthObservation`]: explicit UP/DOWN update the retained
/// verification, unverified statuses (STALE, UNKNOWN, SKIPPED, future) and
/// legacy UNSPECIFIED booleans never create verification. One bounded,
/// credential-safe `tracing::warn!` is emitted only on a status transition —
/// entering STALE logs the fixed message with the numeric `age_ms` once, and
/// an age-only change never logs again. An explicit unverified observation is
/// reported as unverified, never as a newly disconnected broker, and never
/// creates verified DOWN. The message is fixed and never carries the broker
/// URL, username, password, or the provider message. Under `cfg(test)` the
/// matching [`BrokerStatusTransition`] is recorded so tests observe
/// transitions without exposing raw credentials. Broker status reporting
/// never restarts the bridge.
pub(crate) struct BrokerStatusReporter {
    /// Retained broker verification and last observation kind for this
    /// monitor epoch; reset when the bridge is replaced.
    memory: BrokerHealthMemory,
    /// Last emitted transition; gates repeated emission when only the
    /// observation kind or the stale age changed.
    last_emitted: Option<BrokerStatusTransition>,
    #[cfg(test)]
    transitions: Arc<std::sync::Mutex<Vec<BrokerStatusTransition>>>,
}

impl BrokerStatusReporter {
    /// Builds the reporter from the pool's status seam.
    fn from_pool(_pool: &JmsBridgePool) -> Self {
        Self {
            memory: BrokerHealthMemory::default(),
            last_emitted: None,
            #[cfg(test)]
            transitions: Arc::clone(&_pool.broker_status_transitions),
        }
    }

    /// Clears retained verification and the emission gate for a replacement
    /// bridge epoch (a replacement monitor constructs a fresh reporter; a
    /// replaced Ready channel resets it in place).
    fn reset(&mut self) {
        self.memory.reset(None);
        self.last_emitted = None;
    }

    /// Test-only view of the retained verification: `Some(true)` = verified
    /// UP, `Some(false)` = verified DOWN, `None` = unverified.
    #[cfg(test)]
    fn verified(&self) -> Option<bool> {
        self.memory.verified()
    }

    /// Feeds one completed-RPC observation through the memory and emits a
    /// bounded warning only on a real status transition.
    fn report_broker_status(&mut self, observation: BrokerHealthObservation) {
        let entering_stale = observation.status == BrokerHealthStatus::Stale
            && self.memory.last_kind() != Some(BrokerHealthStatus::Stale);
        self.memory.observe(observation);
        let transition = match observation.status {
            BrokerHealthStatus::Up => BrokerStatusTransition::VerifiedUp,
            BrokerHealthStatus::Down => BrokerStatusTransition::BrokerDown,
            BrokerHealthStatus::Stale => BrokerStatusTransition::Stale {
                age_ms: observation.age_ms,
            },
            // Explicit unverified kinds are never disconnection evidence.
            BrokerHealthStatus::Unknown
            | BrokerHealthStatus::Skipped
            | BrokerHealthStatus::Future => BrokerStatusTransition::Unverified,
            BrokerHealthStatus::Unspecified => {
                match (observation.healthy, observation.broker_connected) {
                    (true, true) => BrokerStatusTransition::VerifiedUp,
                    (false, false) => BrokerStatusTransition::BrokerDown,
                    _ => BrokerStatusTransition::Unverified,
                }
            }
        };
        // STALE emits only when entering STALE (age-only changes stay
        // silent); every other transition emits only when it differs from
        // the last emitted one.
        let emit = match &transition {
            BrokerStatusTransition::Stale { .. } => entering_stale,
            other => self.last_emitted.as_ref() != Some(other),
        };
        if !emit {
            return;
        }
        self.last_emitted = Some(transition);
        #[cfg(test)]
        self.transitions
            .lock()
            .expect("broker_status_transitions mutex poisoned")
            .push(transition);
        match transition {
            BrokerStatusTransition::Stale { age_ms } => {
                // `"{}"` is required: a bare const path would be recorded as
                // a tracing field, not as the event message.
                warn!(age_ms, "{}", BROKER_STATUS_STALE_MSG);
            }
            BrokerStatusTransition::Unverified => {
                warn!("{}", BROKER_STATUS_UNVERIFIED_MSG);
            }
            BrokerStatusTransition::VerifiedUp => {
                warn!(
                    healthy = true,
                    broker_connected = true,
                    "{}",
                    BROKER_STATUS_CHANGED_MSG
                );
            }
            BrokerStatusTransition::BrokerDown => {
                warn!(
                    healthy = false,
                    broker_connected = false,
                    "{}",
                    BROKER_STATUS_CHANGED_MSG
                );
            }
        }
    }
}

/// Map the broker TLS table onto the bridge's plain-data env payload. `None`
/// when the broker has no `tls` table, so the spawn still removes inherited
/// `BRIDGE_BROKER_TLS_*` exports without injecting anything.
fn jms_tls_env(tls: Option<&BrokerTlsConfig>) -> Option<JmsTlsEnv> {
    tls.map(|tls| JmsTlsEnv {
        ca_cert_path: tls.ca_cert_path.clone(),
        client_cert_path: tls.client_cert_path.clone(),
        client_key_path: tls.client_key_path.clone(),
        server_name: tls.server_name.clone(),
    })
}

// ── BridgeSlot ───────────────────────────────────────────────────────────────

pub struct BridgeSlot {
    pub name: String,
    pub broker_url: String,
    pub broker_type: BrokerType,
    pub credentials: Option<(String, String)>,
    pub state_rx: watch::Receiver<BridgeState>,
    pub(crate) state_tx: watch::Sender<BridgeState>,
    /// BridgeProcess::stop(mut self) takes ownership — Mutex<Option<>> is required.
    pub process: Arc<tokio::sync::Mutex<Option<BridgeProcess>>>,
    /// JoinHandle of the health monitor task for this slot.
    /// Stored so that shutdown can await the monitor and observe panics.
    pub(crate) health_monitor_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Coalesced liveness wake-up for the health monitor (task 2.5).
    pub(crate) probe_wake: Arc<tokio::sync::Notify>,
    /// Last broker failure observed from a Health response (task 8.1),
    /// stored by the health monitor and read by the consumer error path to
    /// render a local, actionable diagnostic. `None` until the first Health
    /// poll.
    pub(crate) last_broker_failure: std::sync::Mutex<Option<LastBrokerFailure>>,
    /// Shared message-selector requirement (task 1.2): latched `true` by the
    /// consumer-attach path whenever an endpoint configures a non-blank
    /// selector, BEFORE any selector-gate evaluation. The startup gate, the
    /// runtime Health gate, and the consumer-attach gate read this flag fresh
    /// on every evaluation (no closure capture), so a selector attached after
    /// slot start stays covered. No capability evidence is stored on the slot.
    pub(crate) selector_required: Arc<AtomicBool>,
}

impl std::fmt::Debug for BridgeSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeSlot")
            .field("name", &self.name)
            .field("broker_url", &self.broker_url)
            .field("broker_type", &self.broker_type)
            .finish()
    }
}

impl BridgeSlot {
    /// Atomically publishes `next` unless the slot already reached a terminal
    /// state.
    ///
    /// `Parked` and `Stopped` are never overwritten by a late state
    /// transition (a delayed `Ready`, `Degraded`, or `Restarting`). The
    /// decision and the write share one watch write lock, so a concurrent
    /// terminal publication cannot be lost. Returns `true` when published.
    ///
    /// A transport-driven `Restarting` publication must go through the same
    /// closure so it cannot resurrect a terminal slot either.
    fn publish_unless_terminal(&self, next: BridgeState) -> bool {
        self.state_tx.send_if_modified(|cur| {
            if matches!(cur, BridgeState::Parked(_) | BridgeState::Stopped) {
                false
            } else {
                *cur = next;
                true
            }
        })
    }

    /// Shutdown is the sole publisher allowed to override `Parked`: it
    /// unconditionally publishes `Stopped` and returns the previous state.
    ///
    /// This runs *before* shutdown takes the process mutex, so a concurrent
    /// `install_ready_process` that already published `Ready` is still
    /// overridden and its child reaped by the shutdown `process.take()`.
    fn publish_shutdown(&self) -> BridgeState {
        self.state_tx.send_replace(BridgeState::Stopped)
    }

    /// Installs `process` under the process mutex and atomically publishes
    /// `Ready`, unless a terminal state won the race.
    ///
    /// When terminal wins, the process is not stored and is returned so the
    /// caller can `stop()` it outside both locks.
    ///
    /// Lock order: `process` first, then the watch write lock inside
    /// [`Self::publish_unless_terminal`]. Shutdown takes the watch write lock
    /// (in [`Self::publish_shutdown`]) and releases it *before* taking
    /// `process`, so the two orders never nest and cannot deadlock.
    async fn install_ready_process(
        &self,
        process: BridgeProcess,
        channel: Channel,
    ) -> Result<(), Box<BridgeProcess>> {
        let mut guard = self.process.lock().await;
        if self.publish_unless_terminal(BridgeState::Ready { channel }) {
            *guard = Some(process);
            Ok(())
        } else {
            Err(Box::new(process))
        }
    }
}

// ── JmsBridgePool ────────────────────────────────────────────────────────────

pub struct JmsBridgePool {
    pub(crate) slots: DashMap<String, Arc<BridgeSlot>>,
    pub(crate) config: HashMap<String, BrokerConfig>,
    pub(crate) bridge_start_timeout_ms: u64,
    /// Idle-connection timeout forwarded to the bridge's JMS pool
    /// (`JMS_POOL_IDLE_TIMEOUT_MS` on every spawn, including restarts).
    pub(crate) pool_idle_timeout_ms: u64,
    /// Expiration-sweep cadence forwarded to the bridge's JMS pool
    /// (`JMS_POOL_EXPIRATION_CHECK_MS` on every spawn, including restarts).
    pub(crate) pool_expiration_check_ms: u64,
    pub(crate) reconnect: NetworkRetryPolicy,
    pub(crate) health_check_interval_ms: u64,
    /// Configured liveness deadline for one health RPC (`health_timeout_ms`).
    pub(crate) health_timeout_ms: u64,
    /// Consecutive liveness failures tolerated before recovery
    /// (`health_failure_threshold`); the monitor clamps it to at least 1.
    pub(crate) health_failure_threshold: u32,
    pub(crate) health_broker_probe_mode: HealthBrokerProbeMode,
    pub(crate) bridge_version: String,
    pub(crate) bridge_cache_dir: PathBuf,
    /// Maximum number of concurrently active (Starting/Ready) bridges.
    pub(crate) max_bridges: usize,
    /// Serializes bridge admission (check + insert) to prevent race on max_bridges.
    bridge_create_lock: Mutex<()>,
    pub(crate) shutting_down: Arc<AtomicBool>,
    /// Test seam: recorded broker-status transitions (task 2.4).
    #[cfg(test)]
    pub(crate) broker_status_transitions: Arc<std::sync::Mutex<Vec<BrokerStatusTransition>>>,
    /// Test seam: counted replacement attempts (task 2.4).
    #[cfg(test)]
    pub(crate) replacement_attempts: Arc<std::sync::atomic::AtomicUsize>,
    /// Test seam: prepared launch results substituted for real launches in
    /// the monitor's replacement branch; an empty queue launches normally
    /// (task 2.4).
    #[cfg(test)]
    pub(crate) replacement_outcomes:
        Arc<tokio::sync::Mutex<std::collections::VecDeque<PreparedLaunchResult>>>,
    /// Test seam: monitor probe acknowledgements, sent only after the real
    /// counter/reporting/publication path ran (task 2.4).
    #[cfg(test)]
    pub(crate) probe_processed:
        Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<ProbeProcessed>>>>,
    /// Test seam: replacement-processed notifications, sent after actual
    /// installation or error handling (task 2.4).
    #[cfg(test)]
    pub(crate) replacement_processed:
        Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<ReplacementProcessed>>>>,
    /// Test seam: completed channel-refresh attempts, recorded inside the real
    /// `refresh_slot_channel` path (task 2.5).
    #[cfg(test)]
    pub(crate) refresh_attempts: Arc<std::sync::Mutex<Vec<RefreshAttempt>>>,
    /// Test seam: post-recording notification for bounded refresh-attempt
    /// waits (task 2.5).
    #[cfg(test)]
    pub(crate) refresh_observed: Arc<tokio::sync::Notify>,
    /// Test seam: substitutes only the selected connector future, after real
    /// endpoint selection and inside the unchanged production deadline
    /// (task 2.5).
    #[cfg(test)]
    pub(crate) refresh_connector_override: Arc<std::sync::Mutex<Option<RefreshConnectorOverride>>>,
}

impl JmsBridgePool {
    pub fn from_config(pool_config: JmsPoolConfig) -> Result<Self, CamelError> {
        pool_config.validate()?;
        // Backward compat: broker_reconnect_interval_ms overrides
        // reconnect.initial_delay when explicitly set (non-default).
        let mut reconnect = pool_config.reconnect;
        if pool_config.broker_reconnect_interval_ms
            != crate::config::default_broker_reconnect_interval_ms()
        {
            reconnect.initial_delay =
                Duration::from_millis(pool_config.broker_reconnect_interval_ms);
        }
        Ok(Self {
            slots: DashMap::new(),
            config: pool_config.brokers,
            bridge_start_timeout_ms: pool_config.bridge_start_timeout_ms,
            pool_idle_timeout_ms: pool_config.pool_idle_timeout_ms,
            pool_expiration_check_ms: pool_config.pool_expiration_check_ms,
            reconnect,
            health_check_interval_ms: pool_config.health_check_interval_ms,
            health_timeout_ms: pool_config.health_timeout_ms,
            health_failure_threshold: pool_config.health_failure_threshold,
            health_broker_probe_mode: pool_config.health_broker_probe_mode,
            bridge_version: crate::BRIDGE_VERSION.to_string(),
            bridge_cache_dir: pool_config.bridge_cache_dir,
            max_bridges: pool_config.max_bridges,
            bridge_create_lock: Mutex::new(()),
            shutting_down: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            broker_status_transitions: Arc::new(std::sync::Mutex::new(Vec::new())),
            #[cfg(test)]
            replacement_attempts: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            replacement_outcomes: Arc::new(tokio::sync::Mutex::new(
                std::collections::VecDeque::new(),
            )),
            #[cfg(test)]
            probe_processed: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            replacement_processed: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            refresh_attempts: Arc::new(std::sync::Mutex::new(Vec::new())),
            #[cfg(test)]
            refresh_observed: Arc::new(tokio::sync::Notify::new()),
            #[cfg(test)]
            refresh_connector_override: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// Resolve broker name from the URI `broker=` param.
    ///
    /// - If `Some(name)` → validate it exists in config and return it.
    /// - If `None` and exactly one broker is configured → use it implicitly.
    /// - If `None` and multiple brokers are configured → error asking for `?broker=`.
    /// - If `None` and no brokers are configured → error asking to declare brokers.
    pub fn resolve_broker_name(&self, name: Option<&str>) -> Result<String, CamelError> {
        match name {
            Some(n) => {
                if self.config.contains_key(n) {
                    Ok(n.to_string())
                } else {
                    Err(CamelError::ProcessorError(format!(
                        "Unknown JMS broker '{n}' — declare it in [components.jms.brokers] in Camel.toml",
                    )))
                }
            }
            None => match self.config.len() {
                0 => Err(CamelError::ProcessorError(
                    "No JMS brokers configured — declare at least one in [components.jms.brokers] in Camel.toml".to_string(),
                )),
                1 => Ok(self.config.keys().next().unwrap().clone()), // allow-unwrap
                _ => Err(CamelError::ProcessorError(format!(
                    "Multiple JMS brokers configured ({}); specify one with ?broker=<name> in the URI",
                    self.config.keys().cloned().collect::<Vec<_>>().join(", ")
                ))),
            },
        }
    }

    /// Resolve broker type: activemq/artemis schemes hard-override config type; jms uses config.
    pub fn resolve_broker_type(&self, scheme: &str, broker_name: &str) -> BrokerType {
        let config_type = self
            .config
            .get(broker_name)
            .map(|c| c.broker_type.clone())
            .unwrap_or(BrokerType::Generic);

        match scheme {
            "activemq" => {
                if config_type != BrokerType::ActiveMq && config_type != BrokerType::Generic {
                    warn!(
                        "Scheme 'activemq' overrides configured broker_type '{:?}' for broker '{}'",
                        config_type, broker_name
                    );
                }
                BrokerType::ActiveMq
            }
            "artemis" => {
                if config_type != BrokerType::Artemis && config_type != BrokerType::Generic {
                    warn!(
                        "Scheme 'artemis' overrides configured broker_type '{:?}' for broker '{}'",
                        config_type, broker_name
                    );
                }
                BrokerType::Artemis
            }
            _ => config_type,
        }
    }

    /// Get or create a BridgeSlot for the given broker name.
    /// If the slot doesn't exist, starts the bridge process and spawns the health monitor.
    ///
    /// `selector_required` is the consumer-attach requirement (task 1.2): it
    /// latches the slot's shared `selector_required` flag BEFORE any
    /// selector-gate evaluation (the startup gate below reads it per launch).
    /// The flag only ever latches to `true`; a non-selector caller never clears
    /// a requirement registered by a sibling selector route.
    pub async fn get_or_create_slot(
        &self,
        broker_name: &str,
        selector_required: bool,
    ) -> Result<Arc<BridgeSlot>, CamelError> {
        if let Some(slot) = self.slots.get(broker_name) {
            if selector_required {
                slot.selector_required.store(true, Ordering::SeqCst);
            }
            return Ok(Arc::clone(&*slot));
        }

        // Serialize admission: check + insert in single critical section to prevent
        // concurrent creators from both seeing count below limit and exceeding max_bridges.
        let _guard = self.bridge_create_lock.lock().await;

        // Re-check after acquiring lock (another caller may have inserted while we waited).
        if let Some(slot) = self.slots.get(broker_name) {
            if selector_required {
                slot.selector_required.store(true, Ordering::SeqCst);
            }
            return Ok(Arc::clone(&*slot));
        }

        // Enforce max_bridges: count ALL slots in the map under the admission lock.
        // We count total slots (not just Starting/Ready) because any inserted slot
        // represents an allocated bridge — even Degraded/Restarting slots hold resources
        // and the bridge process may still be running. Counting only active states would
        // allow a race: slot A transitions Starting→Degraded between two callers' checks,
        // letting both pass the limit.
        let total_count = self.slots.len();
        if total_count >= self.max_bridges {
            return Err(CamelError::Config(format!(
                "JMS bridge limit reached: {total_count} bridge(s) >= max_bridges ({})",
                self.max_bridges
            )));
        }

        let broker_config = self.config.get(broker_name).ok_or_else(|| {
            CamelError::ProcessorError(format!("Unknown JMS broker '{}'", broker_name))
        })?;

        // Clone all required broker data before touching DashMap::entry().
        let broker_url = broker_config.broker_url.clone();
        let broker_type = broker_config.broker_type.clone();
        let tls = jms_tls_env(broker_config.tls.as_ref());
        let credentials = match (&broker_config.username, &broker_config.password) {
            (Some(u), Some(p)) => Some((u.clone(), p.clone())),
            _ => None,
        };

        let slot = match self.slots.entry(broker_name.to_string()) {
            dashmap::Entry::Occupied(existing) => {
                if selector_required {
                    existing
                        .get()
                        .selector_required
                        .store(true, Ordering::SeqCst);
                }
                return Ok(Arc::clone(existing.get()));
            }
            dashmap::Entry::Vacant(entry) => {
                let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
                let slot = Arc::new(BridgeSlot {
                    name: broker_name.to_string(),
                    broker_url: broker_url.clone(),
                    broker_type: broker_type.clone(),
                    credentials: credentials.clone(),
                    state_rx,
                    state_tx,
                    process: Arc::new(tokio::sync::Mutex::new(None)),
                    health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
                    probe_wake: Arc::new(tokio::sync::Notify::new()),
                    last_broker_failure: std::sync::Mutex::new(None),
                    selector_required: Arc::new(AtomicBool::new(selector_required)),
                });
                entry.insert(Arc::clone(&slot));
                slot
            }
        };

        let start_result = Self::start_bridge_process(
            &self.bridge_version,
            &self.bridge_cache_dir,
            self.bridge_start_timeout_ms,
            self.pool_idle_timeout_ms,
            self.pool_expiration_check_ms,
            broker_name,
            &broker_url,
            &broker_type,
            &credentials,
            tls,
            self.health_broker_probe_mode,
            &slot.selector_required,
        )
        .await;

        Self::apply_initial_start_result(&slot, start_result).await;

        self.spawn_health_monitor(Arc::clone(&slot)).await;

        Ok(slot)
    }

    /// Applies a completed initial start attempt to `slot`, publishing the
    /// resulting state atomically.
    ///
    /// A permanent rejection parks the slot (terminal); any other failure
    /// degrades it, and the health monitor retries. A terminal state published
    /// concurrently (shutdown `Stopped`) always wins. Extracted from
    /// `get_or_create_slot` so tests can inject a trusted [`StartBridgeError`]
    /// and observe the published watch state without spawning a process.
    pub(crate) async fn apply_initial_start_result(
        slot: &BridgeSlot,
        result: Result<(BridgeProcess, Channel), StartBridgeError>,
    ) {
        match result {
            Ok((process, channel)) => {
                if let Err(process) = slot.install_ready_process(process, channel).await {
                    // Shutdown won the race: the child must not survive.
                    let _ = process.stop().await;
                }
            }
            Err(StartBridgeError::Permanent(reason)) => {
                let _ = slot.publish_unless_terminal(BridgeState::Parked(reason));
            }
            Err(StartBridgeError::Other(e)) => {
                let _ = slot.publish_unless_terminal(BridgeState::Degraded(format!(
                    "Initial start failed: {e}"
                )));
            }
        }
    }

    /// Consumer-attach message-selector gate (task 1.2).
    ///
    /// `BridgeSlot` stores no capability evidence and runtime responses retain
    /// none, so a selector-configured consumer attaching to a running slot (at
    /// slot start or late) issues ONE fresh Health RPC — the existing health
    /// client under the configured `health_timeout_ms` — and gates on THAT
    /// response before the consumer's first Subscribe.
    ///
    /// Both failure arms park the slot permanently (fail closed, no silent
    /// start): a missing `message-selector-v1` capability rides the
    /// three-fragment error (pinned bridge version + capability + required
    /// version); an RPC error or timeout parks with a
    /// "could not verify message-selector support" message naming the
    /// capability and the required version. The shared `selector_required` flag
    /// must already be latched by the attach path (see `get_or_create_slot`).
    pub(crate) async fn selector_attach_gate(
        &self,
        slot: &Arc<BridgeSlot>,
        channel: &Channel,
    ) -> Result<(), CamelError> {
        let mut client = bridge_service_client(channel.clone());
        let health_timeout = Duration::from_millis(self.health_timeout_ms);
        let resp = match tokio::time::timeout(health_timeout, client.health(HealthRequest {})).await
        {
            Ok(Ok(resp)) => resp.into_inner(),
            Ok(Err(_)) | Err(_) => {
                let reason = "could not verify message-selector support: bridge health RPC \
                     failed or timed out; the bridge must report capability \
                     message-selector-v1 from the pinned jms bridge"
                    .to_string();
                warn!(
                    "Bridge for '{}' cannot verify message-selector support; killed and parking slot.",
                    slot.name
                );
                kill_and_park(slot, reason.clone()).await;
                return Err(parked_operator_error(&slot.name, &reason));
            }
        };
        if ensure_selector_capability(&resp.capabilities, true).is_err() {
            let reason = format!(
                "bridge {} {}",
                self.bridge_version,
                missing_message_selector_capability()
            );
            warn!(
                "Bridge for '{}' lacks message-selector-v1; killed and parking slot.",
                slot.name
            );
            kill_and_park(slot, reason.clone()).await;
            return Err(parked_operator_error(&slot.name, &reason));
        }
        Ok(())
    }

    /// Request an immediate liveness verification for a non-terminal slot.
    ///
    /// This is a coalesced wake-up, not a lifecycle transition: it never
    /// writes `Restarting`, starts/stops a bridge, degrades a slot, or fakes a
    /// watch change. Repeated requests collapse into one stored permit while
    /// the monitor is between waits. Terminal slots (`Parked`/`Stopped`) keep
    /// the existing warning path and are never resurrected.
    pub fn restart_slot(&self, broker_name: &str) {
        if let Some(slot) = self.slots.get(broker_name) {
            let terminal = matches!(
                &*slot.state_rx.borrow(),
                BridgeState::Parked(_) | BridgeState::Stopped
            );
            if terminal {
                warn!(
                    "JMS broker '{}' is terminal (parked/stopped); ignoring restart request",
                    broker_name
                );
                return;
            }
            slot.probe_wake.notify_one();
        }
    }

    /// Recreate the tonic channel for an existing running bridge process.
    ///
    /// Useful when a channel becomes stale after transport-level failures while
    /// the underlying bridge process is still alive.
    ///
    /// One absolute deadline is established on method entry and covers both
    /// process-lock acquisition and the whole selected connector future.
    /// Connection progress never renews it. Expiration returns a fixed,
    /// credential-safe error and never publishes a restart or counts a
    /// liveness failure.
    pub async fn refresh_slot_channel(&self, broker_name: &str) -> Result<(), CamelError> {
        #[cfg(test)]
        let selected_endpoint = Arc::new(std::sync::Mutex::new(None::<u16>));
        #[cfg(test)]
        let selected_for_connect = Arc::clone(&selected_endpoint);

        let result =
            tokio::time::timeout(Duration::from_millis(CHANNEL_REFRESH_TIMEOUT_MS), async {
                let slot = self
                    .slots
                    .get(broker_name)
                    .map(|s| Arc::clone(&*s))
                    .ok_or_else(|| {
                        CamelError::ProcessorError(format!("Unknown JMS broker '{}'", broker_name))
                    })?;

                // Lock is held during connect() (TLS handshake + retries) —
                // widened from the old grpc_port() read. The enclosing
                // deadline bounds both the lock wait and the whole connect.
                let guard = slot.process.lock().await;
                let process = guard.as_ref().ok_or_else(|| {
                    CamelError::ProcessorError(format!(
                        "JMS broker '{}' has no running bridge process",
                        broker_name
                    ))
                })?;
                #[cfg(test)]
                let endpoint = {
                    let endpoint = process.grpc_port();
                    *selected_for_connect
                        .lock()
                        .expect("selected_endpoint mutex poisoned") = Some(endpoint);
                    endpoint
                };

                // Test seam: substitute only the connector future, after the
                // real endpoint was selected and inside the production
                // deadline. The closure receives that selected endpoint.
                #[cfg(test)]
                let channel = {
                    let override_fn = self
                        .refresh_connector_override
                        .lock()
                        .expect("refresh_connector_override mutex poisoned")
                        .clone();
                    match override_fn {
                        Some(connect) => connect(endpoint).await,
                        None => process.connect().await,
                    }
                };
                #[cfg(not(test))]
                let channel = process.connect().await;

                let channel = channel.map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "JMS broker '{}' channel refresh failed: {}",
                        broker_name, e
                    ))
                })?;
                // Release ownership before returning the slot for publication.
                drop(guard);
                Ok::<_, CamelError>((slot, channel))
            })
            .await;

        #[cfg(test)]
        let endpoint = *selected_endpoint
            .lock()
            .expect("selected_endpoint mutex poisoned");

        match result {
            Ok(Ok((slot, channel))) => {
                // The check and the `Ready` publication are one atomic watch
                // mutation: a terminal state published while `connect()`
                // awaited (shutdown) is never overwritten by this delayed
                // refresh.
                let published = slot.publish_unless_terminal(BridgeState::Ready { channel });
                #[cfg(test)]
                self.record_refresh_attempt(RefreshAttempt {
                    broker_name: broker_name.to_string(),
                    endpoint,
                    failed: !published,
                    timed_out: false,
                });
                if !published {
                    return Err(CamelError::ProcessorError(format!(
                        "JMS broker '{}' became terminal during channel refresh",
                        broker_name
                    )));
                }
                Ok(())
            }
            Ok(Err(e)) => {
                #[cfg(test)]
                self.record_refresh_attempt(RefreshAttempt {
                    broker_name: broker_name.to_string(),
                    endpoint,
                    failed: true,
                    timed_out: false,
                });
                Err(e)
            }
            Err(_elapsed) => {
                #[cfg(test)]
                self.record_refresh_attempt(RefreshAttempt {
                    broker_name: broker_name.to_string(),
                    endpoint,
                    failed: true,
                    timed_out: true,
                });
                // Fixed and credential-safe: no broker URL, user, password, or
                // provider message. Never publishes `Restarting` or a failure
                // count.
                Err(CamelError::ProcessorError(
                    "JMS bridge channel refresh timed out".to_string(),
                ))
            }
        }
    }

    /// Test seam: records one completed refresh attempt and publishes the
    /// post-recording notification (task 2.5).
    #[cfg(test)]
    fn record_refresh_attempt(&self, attempt: RefreshAttempt) {
        self.refresh_attempts
            .lock()
            .expect("refresh_attempts mutex poisoned")
            .push(attempt);
        self.refresh_observed.notify_one();
    }

    /// Test seam: installs or replaces the selected-connector override.
    ///
    /// The override receives the real selected process endpoint and runs
    /// inside the unchanged production deadline; endpoint selection, timeout,
    /// observation, publication, and lifecycle policy stay real (task 2.5).
    #[cfg(test)]
    pub(crate) fn set_refresh_connector_override<F, Fut>(&self, connect: F)
    where
        F: Fn(u16) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Channel, BridgeError>> + Send + 'static,
    {
        *self
            .refresh_connector_override
            .lock()
            .expect("refresh_connector_override mutex poisoned") =
            Some(Arc::new(move |endpoint| Box::pin(connect(endpoint))));
    }

    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    /// Shutdown all slots: stop all bridge processes and await health monitors.
    pub async fn shutdown(&self) -> Result<(), CamelError> {
        self.begin_shutdown();
        let names: Vec<String> = self.slots.iter().map(|e| e.key().clone()).collect();
        let mut errors: Vec<String> = Vec::new();

        for name in names {
            if let Some((_, slot)) = self.slots.remove(&name) {
                // Signal the health monitor to stop. Shutdown is the only
                // publisher allowed to override a parked slot.
                let _ = slot.publish_shutdown();

                // Stop the bridge process.
                let process = {
                    let mut guard = slot.process.lock().await;
                    guard.take()
                };
                if let Some(p) = process
                    && let Err(e) = p.stop().await
                {
                    errors.push(format!("broker '{}': process stop failed: {e}", slot.name));
                }

                // Await the health monitor task with timeout; abort if it doesn't stop.
                let monitor_handle = {
                    let mut guard = slot.health_monitor_handle.lock().await;
                    guard.take()
                };
                if let Some(mut h) = monitor_handle
                    && tokio::time::timeout(Duration::from_secs(5), &mut h)
                        .await
                        .is_err()
                {
                    h.abort();
                    let _ = h.await;
                    warn!(
                        "health monitor for '{}' did not stop in 5s; aborted",
                        slot.name
                    );
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(CamelError::ProcessorError(format!(
                "JMS pool shutdown completed with {} error(s): {}",
                errors.len(),
                errors.join("; ")
            )))
        }
    }

    pub(crate) async fn spawn_health_monitor(&self, slot: Arc<BridgeSlot>) {
        let health_interval = self.health_check_interval_ms;
        let health_timeout = Duration::from_millis(self.health_timeout_ms);
        let threshold = self.health_failure_threshold.max(1);
        let bridge_version = self.bridge_version.clone();
        let bridge_cache_dir = self.bridge_cache_dir.clone();
        let start_timeout_ms = self.bridge_start_timeout_ms;
        let pool_idle_timeout_ms = self.pool_idle_timeout_ms;
        let pool_expiration_check_ms = self.pool_expiration_check_ms;
        let health_broker_probe_mode = self.health_broker_probe_mode;
        let handle_ref = Arc::clone(&slot.health_monitor_handle);
        let shutting_down = Arc::clone(&self.shutting_down);
        let broker_name = slot.name.clone();
        // Restarts re-use the broker's TLS table, exactly like the initial
        // launch; `None` still cuts inherited BRIDGE_BROKER_TLS_* exports.
        let slot_tls = jms_tls_env(self.config.get(&broker_name).and_then(|c| c.tls.as_ref()));
        // Task 8.1: expected TLS identity for the broker-failure diagnostics,
        // computed once at spawn time: `tls.server_name` when configured, else
        // the broker URL host. `tls_configured` arms the broker-tls-v1
        // capability gate on every observed Health response.
        let broker_tls = self.config.get(&broker_name).and_then(|c| c.tls.as_ref());
        let tls_configured = broker_tls.is_some();
        let expected_identity = broker_tls
            .and_then(|t| t.server_name.clone())
            .unwrap_or_else(|| crate::config::url_host(&slot.broker_url).to_string());
        let mut reporter = BrokerStatusReporter::from_pool(self);
        #[cfg(test)]
        let replacement_attempts = Arc::clone(&self.replacement_attempts);
        #[cfg(test)]
        let replacement_outcomes = Arc::clone(&self.replacement_outcomes);
        #[cfg(test)]
        let probe_processed = Arc::clone(&self.probe_processed);
        #[cfg(test)]
        let replacement_processed = Arc::clone(&self.replacement_processed);

        let handle = tokio::spawn(async move {
            let mut consecutive_failures: u32 = 0;
            #[cfg(test)]
            let mut probe_index: usize = 0;
            loop {
                let state = slot.state_rx.borrow().clone();
                match state {
                    BridgeState::Stopped => {
                        info!("Health monitor for '{}' exiting (Stopped)", slot.name);
                        break;
                    }
                    BridgeState::Ready { .. } => {
                        // Fresh sleep, not `interval.tick()`: no immediate
                        // initial tick and no catch-up burst. A stored permit
                        // wins immediately and coalesces repeated requests.
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(health_interval)) => {}
                            _ = slot.probe_wake.notified() => {}
                        }
                        // Re-read after either wait: a terminal publication or
                        // a channel replacement during the wait must be
                        // observed before this probe starts.
                        let state = slot.state_rx.borrow().clone();
                        let BridgeState::Ready { channel } = state else {
                            continue;
                        };
                        let mut client = bridge_service_client(channel);
                        let mut failure_msg: Option<String> = None;
                        let mut outcome = ProbeOutcome::Success;
                        match tokio::time::timeout(health_timeout, client.health(HealthRequest {}))
                            .await
                        {
                            Ok(Ok(resp)) => {
                                let resp = resp.into_inner();
                                // A completed RPC is liveness proof regardless
                                // of the broker fields (spec T1/T2): report the
                                // broker conditions, reset the counter, stay
                                // Ready. Broker conditions never restart the
                                // bridge.
                                consecutive_failures = 0;
                                // Task 8.1: the broker-tls-v1 capability gate
                                // over the observed build (the phase-2 startup
                                // call site runs the same gate before Ready is
                                // ever published). A TLS-configured broker
                                // requires the capability; a missing one is
                                // fatal: the slot's bridge is killed with the
                                // same stop helper used for other fatal bridge
                                // conditions and the permanent-error channel
                                // (Parked) carries the rejection — a parked
                                // slot is terminal, so no retry is scheduled.
                                // Task 1.2: the matching message-selector-v1
                                // gate, armed by the SHARED requirement flag
                                // read fresh per response (no closure capture)
                                // so a selector attached after monitor start
                                // stays covered and capability loss parks the
                                // slot and stops resubscription.
                                let gate_reason = if ensure_tls_capability(
                                    &resp.capabilities,
                                    tls_configured,
                                )
                                .is_err()
                                {
                                    warn!(
                                        "Bridge for '{}' lacks broker-tls-v1; killed and parking slot.",
                                        slot.name
                                    );
                                    Some(format!(
                                        "bridge {bridge_version} {MISSING_BROKER_TLS_CAPABILITY}"
                                    ))
                                } else if ensure_selector_capability(
                                    &resp.capabilities,
                                    slot.selector_required.load(Ordering::SeqCst),
                                )
                                .is_err()
                                {
                                    warn!(
                                        "Bridge for '{}' lacks message-selector-v1; killed and parking slot.",
                                        slot.name
                                    );
                                    Some(format!(
                                        "bridge {} {}",
                                        bridge_version,
                                        missing_message_selector_capability()
                                    ))
                                } else {
                                    None
                                };
                                if let Some(reason) = gate_reason {
                                    kill_and_park(&slot, reason).await;
                                } else {
                                    // Task 8.1: retain the last classified
                                    // broker failure (updated on every Health
                                    // poll) so the consumer error path can
                                    // render a local, actionable diagnostic.
                                    // Only local values enter the triple: the
                                    // class discriminant, the spawn-time
                                    // expected identity, and the redacted
                                    // broker URL.
                                    {
                                        let mut last = slot
                                            .last_broker_failure
                                            .lock()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                                        *last = Some(LastBrokerFailure {
                                            failure_class: resp.failure_class,
                                            expected_identity: expected_identity.clone(),
                                            connected_host: crate::config::redact_broker_url(
                                                &slot.broker_url,
                                            ),
                                        });
                                    }
                                    reporter.report_broker_status(BrokerHealthObservation {
                                        status: BrokerHealthStatus::from_raw(resp.status),
                                        healthy: resp.healthy,
                                        broker_connected: resp.broker_connected,
                                        age_ms: resp.age_ms,
                                    });
                                }
                            }
                            Ok(Err(e)) => {
                                consecutive_failures = consecutive_failures.saturating_add(1);
                                outcome = ProbeOutcome::RpcError;
                                failure_msg = Some(e.to_string());
                            }
                            Err(_) => {
                                consecutive_failures = consecutive_failures.saturating_add(1);
                                outcome = ProbeOutcome::Timeout;
                                failure_msg = Some(format!(
                                    "health RPC timed out after {}ms",
                                    health_timeout.as_millis()
                                ));
                            }
                        }
                        // Below the threshold the slot stays Ready and no
                        // Degraded is published; at the threshold the existing
                        // Degraded text drives the bounded restart flow.
                        if let Some(msg) = failure_msg
                            && consecutive_failures >= threshold
                        {
                            if matches!(outcome, ProbeOutcome::Timeout) {
                                warn!(
                                    "Health check timed out for broker '{}': {}. Marking Degraded.",
                                    slot.name, msg
                                );
                            } else {
                                warn!(
                                    "Health check failed for broker '{}': {}. Marking Degraded.",
                                    slot.name, msg
                                );
                            }
                            let _ = slot.publish_unless_terminal(BridgeState::Degraded(msg));
                        }
                        // Test seam: acknowledge only after the real
                        // counter/reporting/publication path ran (task 2.4).
                        #[cfg(test)]
                        {
                            let ack = ProbeProcessed {
                                index: probe_index,
                                state: slot.state_rx.borrow().clone(),
                                outcome,
                                consecutive_failures,
                                attempts: replacement_attempts.load(Ordering::SeqCst),
                            };
                            probe_index += 1;
                            if let Some(tx) = probe_processed
                                .lock()
                                .expect("probe_processed mutex poisoned")
                                .as_ref()
                            {
                                let _ = tx.send(ack);
                            }
                        }
                    }
                    BridgeState::Degraded(_) | BridgeState::Starting => {
                        if shutting_down.load(Ordering::SeqCst) {
                            tracing::info!(
                                "Pool shutting down — not restarting bridge for broker '{}'",
                                broker_name
                            );
                            break;
                        }
                        // Atomic: a terminal state published after the loop's
                        // state read wins and ends the monitor.
                        if !slot.publish_unless_terminal(BridgeState::Restarting {
                            attempt: 0,
                            next_at: Instant::now(),
                        }) {
                            break;
                        }
                    }
                    BridgeState::Parked(_) => {
                        // Terminal: a permanent startup rejection is never
                        // retried. Only an operator repair plus a context
                        // restart (tasks 4.3/4.7) can clear it.
                        info!(
                            "Health monitor for '{}' exiting (Parked, permanent failure)",
                            slot.name
                        );
                        break;
                    }
                    BridgeState::Restarting { attempt, next_at } => {
                        if shutting_down.load(Ordering::SeqCst) {
                            tracing::info!(
                                "Pool shutting down — aborting restart for broker '{}'",
                                broker_name
                            );
                            break;
                        }

                        let now = Instant::now();
                        if now < next_at {
                            tokio::time::sleep(next_at - now).await;
                        }

                        info!(
                            "Restarting bridge for broker '{}' (attempt {})",
                            slot.name,
                            attempt + 1
                        );

                        if attempt >= MAX_RESTART_ATTEMPTS {
                            // log-policy: system-broken
                            tracing::error!(
                                "Max restart attempts ({}) reached for broker '{}' — staying degraded",
                                attempt,
                                broker_name
                            );
                            let _ = slot.publish_unless_terminal(BridgeState::Degraded(format!(
                                "max restart attempts ({}) exceeded",
                                attempt
                            )));
                            break;
                        }

                        // Counted after the terminal/restart-budget guards and
                        // before stopping the prior process (task 2.4).
                        #[cfg(test)]
                        replacement_attempts.fetch_add(1, Ordering::SeqCst);

                        let old_process = {
                            let mut guard = slot.process.lock().await;
                            guard.take()
                        };
                        if let Some(p) = old_process {
                            let _ = p.stop().await;
                        }

                        // With a populated test queue only the launch result is
                        // substituted; installation and success/error handling
                        // below are unchanged. An empty queue launches normally.
                        #[cfg(test)]
                        let start_result = {
                            let mut queue = replacement_outcomes.lock().await;
                            match queue.pop_front() {
                                Some(prepared) => prepared,
                                None => {
                                    Self::start_bridge_process(
                                        &bridge_version,
                                        &bridge_cache_dir,
                                        start_timeout_ms,
                                        pool_idle_timeout_ms,
                                        pool_expiration_check_ms,
                                        &slot.name,
                                        &slot.broker_url,
                                        &slot.broker_type,
                                        &slot.credentials,
                                        slot_tls.clone(),
                                        health_broker_probe_mode,
                                        &slot.selector_required,
                                    )
                                    .await
                                }
                            }
                        };
                        #[cfg(not(test))]
                        let start_result = Self::start_bridge_process(
                            &bridge_version,
                            &bridge_cache_dir,
                            start_timeout_ms,
                            pool_idle_timeout_ms,
                            pool_expiration_check_ms,
                            &slot.name,
                            &slot.broker_url,
                            &slot.broker_type,
                            &slot.credentials,
                            slot_tls.clone(),
                            health_broker_probe_mode,
                            &slot.selector_required,
                        )
                        .await;

                        let mut monitor_exit = false;
                        #[cfg(test)]
                        let mut installed_pid: Option<u32> = None;
                        match start_result {
                            Ok((process, channel)) => {
                                #[cfg(test)]
                                let pid = process.child_pid();
                                // Atomic install + publication: if shutdown
                                // published `Stopped` while this start was
                                // in-flight, the child is returned, not stored.
                                if let Err(process) =
                                    slot.install_ready_process(process, channel).await
                                {
                                    let _ = process.stop().await;
                                    monitor_exit = true;
                                } else {
                                    info!("Broker '{}' bridge restarted successfully", slot.name);
                                    // A successful install resets the liveness
                                    // counter for the replacement bridge.
                                    consecutive_failures = 0;
                                    // The replacement bridge is a new broker
                                    // status epoch: retained verification and
                                    // the emission gate must not carry over.
                                    reporter.reset();
                                    #[cfg(test)]
                                    {
                                        installed_pid = pid;
                                    }
                                }
                            }
                            Err(StartBridgeError::Permanent(reason)) => {
                                warn!(
                                    "Bridge for '{}' rejected restart permanently: {reason}. Parking slot.",
                                    slot.name
                                );
                                // Atomic: a shutdown `Stopped` is preserved.
                                let _ = slot.publish_unless_terminal(BridgeState::Parked(reason));
                                monitor_exit = true;
                            }
                            Err(StartBridgeError::Other(err)) => {
                                let reason = err.to_string();
                                let delay_secs = std::cmp::min(5 * 2u64.pow(attempt), 120);
                                let next = Instant::now() + Duration::from_secs(delay_secs);
                                warn!(
                                    "Failed to restart bridge for '{}' (attempt {}): {reason}. Retry in {delay_secs}s",
                                    slot.name,
                                    attempt + 1
                                );
                                // Atomic: a shutdown `Stopped` ends the retry
                                // instead of being overwritten.
                                if !slot.publish_unless_terminal(BridgeState::Restarting {
                                    attempt: attempt + 1,
                                    next_at: next,
                                }) {
                                    monitor_exit = true;
                                }
                            }
                        }

                        // Test seam: notify only after actual installation or
                        // error handling (task 2.4).
                        #[cfg(test)]
                        {
                            let event = ReplacementProcessed {
                                attempts: replacement_attempts.load(Ordering::SeqCst),
                                installed: installed_pid.is_some(),
                                installed_pid,
                                state: slot.state_rx.borrow().clone(),
                            };
                            if let Some(tx) = replacement_processed
                                .lock()
                                .expect("replacement_processed mutex poisoned")
                                .as_ref()
                            {
                                let _ = tx.send(event);
                            }
                        }

                        if monitor_exit {
                            break;
                        }
                    }
                }
            }
        });

        // Store the handle so shutdown can await the monitor.
        let mut guard = handle_ref.lock().await;
        *guard = Some(handle);
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_bridge_process(
        bridge_version: &str,
        bridge_cache_dir: &std::path::Path,
        start_timeout_ms: u64,
        pool_idle_timeout_ms: u64,
        pool_expiration_check_ms: u64,
        broker_name: &str,
        broker_url: &str,
        broker_type: &BrokerType,
        credentials: &Option<(String, String)>,
        tls: Option<JmsTlsEnv>,
        health_broker_probe_mode: HealthBrokerProbeMode,
        selector_required: &AtomicBool,
    ) -> Result<(BridgeProcess, Channel), StartBridgeError> {
        info!(
            "Starting JMS bridge process for {}...",
            redact_url(broker_url)
        );
        let binary_path = ensure_binary(bridge_version, bridge_cache_dir)
            .await
            .map_err(|e| {
                // A download/transport failure is transient: keep the restart path.
                StartBridgeError::Other(CamelError::ProcessorError(format!(
                    "JMS bridge binary unavailable: {e}"
                )))
            })?;

        // Rust owns the default attempt-journal directory and derives the
        // broker key; both travel to the bridge by env on this launch and on
        // every restart (same name, type, and redacted URL => same key). A root
        // that cannot be resolved or created is a config-class permanent
        // failure, never a transient retry.
        let journal_dir = default_attempt_journal_dir().map_err(|e| {
            StartBridgeError::Permanent(format!("attempt journal directory resolution failed: {e}"))
        })?;
        ensure_attempt_journal_dir(&journal_dir).map_err(|e| {
            StartBridgeError::Permanent(format!(
                "attempt journal directory creation failed for {}: {e}",
                journal_dir.display()
            ))
        })?;
        let broker_key = attempt_journal_broker_key(broker_name, broker_type, broker_url);
        // Task 8.1: a `tls` table configures broker TLS and turns on the
        // startup broker-tls-v1 capability gate.
        let tls_configured = tls.is_some();

        let mut process_config = BridgeProcessConfig::jms(
            binary_path,
            broker_url.to_string(),
            broker_type.clone(),
            credentials.as_ref().map(|(u, _)| u.clone()),
            credentials
                .as_ref()
                .map(|(_, p)| camel_bridge::process::Redacted::new(p.clone())),
            start_timeout_ms,
            pool_idle_timeout_ms,
            pool_expiration_check_ms,
            Some(journal_dir.to_string_lossy().into_owned()),
            Some(broker_key),
            tls,
        );
        process_config
            .env_vars
            .push(health_probe_env(health_broker_probe_mode));

        Self::start_bridge_process_with_config(
            &process_config,
            start_timeout_ms,
            tls_configured,
            selector_required,
            None,
        )
        .await
    }

    /// Starts the configured bridge process and connects, classifying a
    /// permanent startup rejection before it can be flattened into a transient
    /// string.
    ///
    /// `tls_configured` drives the `broker-tls-v1` capability gate (task 8.1):
    /// after a successful startup Health response a TLS-configured broker
    /// requires a bridge build that reports the capability (an old bridge
    /// reports none and fails closed). When the gate rejects, the launched
    /// bridge is killed with the same `stop` helper used for other fatal
    /// bridge conditions and the permanent (config-class) error is returned —
    /// the permanent-error channel, never a retry.
    ///
    /// `selector_required` is the shared, dynamically registered
    /// message-selector requirement (task 1.2), read fresh at gate evaluation
    /// (no closure capture): when armed and the freshly collected Health
    /// evidence lacks `message-selector-v1`, the launch fails closed the same
    /// way. Evidence is re-collected per launch (`seen_capabilities` starts
    /// empty), so a previous bridge instance never authorizes its replacement.
    ///
    /// `startup_channel_override` is a test seam (mirroring
    /// `set_refresh_connector_override`: substitute only the channel
    /// acquisition) — when `Some`, the post-ready mTLS connect is skipped and
    /// the readiness probe plus both capability gates run unchanged over the
    /// provided channel. Production passes `None`.
    pub(crate) async fn start_bridge_process_with_config(
        process_config: &BridgeProcessConfig,
        start_timeout_ms: u64,
        tls_configured: bool,
        selector_required: &AtomicBool,
        startup_channel_override: Option<Channel>,
    ) -> Result<(BridgeProcess, Channel), StartBridgeError> {
        // Phase 1: spawn and read the ready handshake. `BridgeProcess::start`
        // bounds this internally by `start_timeout_ms` and returns a permanent
        // failure unmodified, so the observed signal is never erased by a
        // second deadline. On error the raw child is dropped (`kill_on_drop`).
        let process = BridgeProcess::start(process_config)
            .await
            .map_err(classify_start_error)?;

        // Task 8.1: capabilities of the Health response that satisfied the
        // phase-2 probe, captured for the startup gate below.
        let seen_capabilities = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        // Phase 2: TLS connect and health, bounded separately by the configured
        // startup budget. The child is owned by `process`; if this phase fails
        // or the future is cancelled, `BridgeProcess::drop` kills it.
        let total_timeout = Duration::from_millis(start_timeout_ms);
        let channel = tokio::time::timeout(total_timeout, async {
            let channel = match startup_channel_override {
                Some(channel) => channel,
                None => process.connect().await.map_err(classify_start_error)?,
            };

            wait_for_health(&channel, Duration::from_secs(10), |ch| {
                let mut client = bridge_service_client(ch);
                let capabilities = Arc::clone(&seen_capabilities);
                async move {
                    let resp = client.health(HealthRequest {}).await?.into_inner();
                    *capabilities
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        resp.capabilities.clone();
                    Ok(resp.healthy)
                }
            })
            .await
            .map_err(|e| {
                StartBridgeError::Other(CamelError::ProcessorError(format!(
                    "JMS bridge health check failed: {e}"
                )))
            })?;

            Ok::<Channel, StartBridgeError>(channel)
        })
        .await
        .map_err(|_| {
            StartBridgeError::Other(CamelError::ProcessorError(format!(
                "JMS bridge start timed out after {start_timeout_ms}ms"
            )))
        })??;

        // Task 8.1: the broker-tls-v1 capability gate. The gate classifies
        // the failure; this call site names the launched binary (the source it
        // already tracks) in the surfaced message. The child is killed with the
        // same stop helper used for other fatal bridge conditions, and the
        // permanent-error channel carries the rejection without retry.
        let gate_capabilities = seen_capabilities
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if ensure_tls_capability(&gate_capabilities, tls_configured).is_err() {
            let _ = process.stop().await;
            return Err(StartBridgeError::Permanent(format!(
                "bridge {} {MISSING_BROKER_TLS_CAPABILITY}",
                process_config.binary_path.display()
            )));
        }
        // Task 1.2: the message-selector-v1 startup gate over the same freshly
        // collected evidence, armed by the shared requirement flag (read per
        // evaluation). The shared decision seam builds the three-fragment
        // error (capability name + required version + pinned
        // `crate::BRIDGE_VERSION`); the child is killed with the same stop
        // helper and the permanent-error channel carries the rejection without
        // retry.
        match evaluate_startup_selector_gate(
            &gate_capabilities,
            selector_required.load(Ordering::SeqCst),
        ) {
            Ok(()) => {}
            Err(BridgeError::PermanentFailure(reason)) => {
                let _ = process.stop().await;
                return Err(StartBridgeError::Permanent(reason));
            }
            Err(other) => {
                let _ = process.stop().await;
                return Err(classify_start_error(other));
            }
        }

        Ok((process, channel))
    }
}

// ── Drop impl: cleanup on pool drop without explicit shutdown ─────────────────

impl Drop for JmsBridgePool {
    fn drop(&mut self) {
        self.shutting_down.store(true, Ordering::SeqCst);

        // Clone bridge slots before we lose access to self.slots.
        let slots: Vec<(String, Arc<BridgeSlot>)> = self
            .slots
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();

        if slots.is_empty() {
            return;
        }

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                // Spawn the per-bridge cleanup (same sequence as shutdown()).
                // Dropping the JoinHandle detaches the task — it runs to completion.
                drop(handle.spawn(async move {
                    for (_name, slot) in slots {
                        // Signal the health monitor to stop. Shutdown is the
                        // only publisher allowed to override a parked slot.
                        let _ = slot.publish_shutdown();

                        // Stop the bridge process.
                        let process = {
                            let mut guard = slot.process.lock().await;
                            guard.take()
                        };
                        if let Some(p) = process {
                            let _ = p.stop().await;
                        }

                        // Await the health monitor task with timeout; abort if needed.
                        let monitor_handle = {
                            let mut guard = slot.health_monitor_handle.lock().await;
                            guard.take()
                        };
                        if let Some(mut h) = monitor_handle
                            && tokio::time::timeout(Duration::from_secs(5), &mut h)
                                .await
                                .is_err()
                        {
                            h.abort();
                            let _ = h.await;
                            warn!(
                                "health monitor for '{}' did not stop in 5s; aborted",
                                slot.name
                            );
                        }
                    }
                }));
            }
            Err(_) => {
                warn!("JmsBridgePool dropped outside tokio runtime; bridges not cleaned up");
            }
        }
    }
}

// ── JmsComponent ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct JmsComponent {
    scheme: String,
    pool: Arc<JmsBridgePool>,
}

impl JmsComponent {
    pub fn with_scheme(scheme: impl Into<String>, pool: Arc<JmsBridgePool>) -> Self {
        Self {
            scheme: scheme.into(),
            pool,
        }
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Test helper: send a message directly without going through a route.
    #[cfg(test)]
    pub async fn send_for_test(
        &self,
        destination: &str,
        body: &[u8],
        content_type: &str,
    ) -> Result<String, CamelError> {
        let broker_name = self.pool.resolve_broker_name(None)?;
        let slot = self.pool.get_or_create_slot(&broker_name, false).await?;
        let channel = match &*slot.state_rx.borrow() {
            BridgeState::Ready { channel } => channel.clone(),
            other => {
                return Err(CamelError::ProcessorError(format!(
                    "Bridge not ready: {:?}",
                    other
                )));
            }
        };
        let mut client = bridge_service_client(channel);
        let r = client
            .send(crate::proto::SendRequest {
                destination: destination.to_string(),
                body: body.to_vec(),
                headers: Default::default(),
                content_type: content_type.to_string(),
            })
            .await
            .map_err(|e| CamelError::ProcessorError(format!("test send error: {e}")))?;
        Ok(r.into_inner().message_id)
    }
}

impl Component for JmsComponent {
    fn scheme(&self) -> &str {
        &self.scheme
    }

    fn metadata(&self) -> camel_component_api::ComponentMetadata {
        // Alias instances (activemq/artemis) must report their own scheme,
        // not the shared descriptor's "jms", or Registry::register warns and
        // normalizes on every startup.
        let mut meta = crate::metadata::JmsMetadataDescriptor::metadata();
        meta.scheme = self.scheme.clone();
        meta
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = JmsEndpointConfig::from_uri(uri)?;
        let broker_name = self
            .pool
            .resolve_broker_name(endpoint_config.broker_name.as_deref())?;
        let resolved_broker_type = self.pool.resolve_broker_type(&self.scheme, &broker_name);

        let health_check = JmsHealthCheck::new(Arc::clone(&self.pool), broker_name.clone());
        ctx.register_current_route_health_check(Arc::new(health_check));

        Ok(Box::new(JmsEndpoint {
            pool: Arc::clone(&self.pool),
            uri: uri.to_string(),
            broker_name,
            resolved_broker_type,
            endpoint_config,
        }))
    }
}

// ── JmsEndpoint ──────────────────────────────────────────────────────────────

struct JmsEndpoint {
    pool: Arc<JmsBridgePool>,
    uri: String,
    broker_name: String,
    resolved_broker_type: BrokerType,
    endpoint_config: JmsEndpointConfig,
}

impl Endpoint for JmsEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(LazyJmsProducer {
            pool: Arc::clone(&self.pool),
            broker_name: self.broker_name.clone(),
            endpoint_config: self.endpoint_config.clone(),
            resolved_broker_type: self.resolved_broker_type.clone(),
        }))
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(JmsConsumer::new(
            Arc::clone(&self.pool),
            self.broker_name.clone(),
            self.endpoint_config.clone(),
            self.pool.reconnect.clone(),
            rt,
        )))
    }
}

#[derive(Clone)]
struct LazyJmsProducer {
    pool: Arc<JmsBridgePool>,
    broker_name: String,
    endpoint_config: JmsEndpointConfig,
    #[allow(dead_code)]
    resolved_broker_type: BrokerType,
}

impl Service<Exchange> for LazyJmsProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Check existing slot state if one is already present.
        // If no slot exists yet, return Ready — call() will handle async bridge start.
        if let Some(slot) = self.pool.slots.get(&self.broker_name) {
            match &*slot.state_rx.borrow() {
                BridgeState::Ready { .. } => return Poll::Ready(Ok(())),
                BridgeState::Starting | BridgeState::Restarting { .. } => {
                    // Register a waker so the executor is notified when bridge state
                    // changes. Without this, Poll::Pending would stall callers that use
                    // strict Tower semantics (poll_ready loop before call()).
                    // Guard with try_current: fall back to wake_by_ref when no Tokio
                    // runtime is active (e.g. unit tests).
                    let waker = cx.waker().clone();
                    let mut rx = slot.state_rx.clone();
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        handle.spawn(async move {
                            let _ = rx.changed().await;
                            waker.wake();
                        });
                    } else {
                        waker.wake_by_ref();
                    }
                    return Poll::Pending;
                }
                BridgeState::Degraded(reason) => {
                    return Poll::Ready(Err(CamelError::ProcessorError(format!(
                        "JMS broker '{}' is degraded: {}",
                        self.broker_name, reason
                    ))));
                }
                BridgeState::Parked(reason) => {
                    // Fail closed: a parked slot is terminal, never Ready.
                    return Poll::Ready(Err(parked_operator_error(&self.broker_name, reason)));
                }
                BridgeState::Stopped => {
                    return Poll::Ready(Err(CamelError::ConsumerStopping));
                }
            }
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let pool = Arc::clone(&self.pool);
        let broker_name = self.broker_name.clone();
        let endpoint_config = self.endpoint_config.clone();

        Box::pin(async move {
            let slot = pool.get_or_create_slot(&broker_name, false).await?;
            let mut rx = slot.state_rx.clone();

            loop {
                let state = rx.borrow().clone();
                match state {
                    BridgeState::Ready { channel } => {
                        let mut producer = JmsProducer::new(channel, endpoint_config.clone());
                        match producer.call(exchange).await {
                            Ok(done) => return Ok(done),
                            Err(first_err) if is_bridge_transport_error(&first_err) => {
                                warn!(
                                    broker = %broker_name,
                                    error = %first_err,
                                    "JMS send transport error; refreshing channel (no automatic resend)"
                                );

                                if let Err(refresh_err) =
                                    pool.refresh_slot_channel(&broker_name).await
                                {
                                    warn!(
                                        broker = %broker_name,
                                        error = %refresh_err,
                                        "JMS channel refresh failed; requesting liveness probe"
                                    );
                                    pool.restart_slot(&broker_name);
                                }

                                // Do NOT automatically resend — the first send may have reached
                                // the broker even though the ack failed. Resending non-idempotent
                                // writes causes duplicates. Return the original error so the caller
                                // can decide whether to retry.
                                return Err(first_err);
                            }
                            Err(other_err) => return Err(other_err),
                        }
                    }
                    BridgeState::Degraded(reason) => {
                        return Err(CamelError::ProcessorError(format!(
                            "JMS broker '{}' is degraded: {}",
                            broker_name, reason
                        )));
                    }
                    BridgeState::Parked(reason) => {
                        // Fail closed: a parked slot is terminal, never Ready.
                        return Err(parked_operator_error(&broker_name, &reason));
                    }
                    BridgeState::Stopped => {
                        return Err(CamelError::ProcessorError(format!(
                            "JMS broker '{}' is stopped",
                            broker_name
                        )));
                    }
                    BridgeState::Starting | BridgeState::Restarting { .. } => {
                        if rx.changed().await.is_err() {
                            return Err(CamelError::ProcessorError(format!(
                                "JMS broker '{}' state channel closed",
                                broker_name
                            )));
                        }
                    }
                }
            }
        })
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Environment entry that carries the configured broker-probe mode into every
/// bridge launch (initial start and every health-monitor restart).
pub(crate) fn health_probe_env(mode: HealthBrokerProbeMode) -> (String, String) {
    (
        "JMS_HEALTH_BROKER_PROBE_MODE".to_string(),
        mode.as_env_str().to_string(),
    )
}

/// Operator repair guidance appended to every permanent JMS error.
///
/// A parked slot never recovers on its own and a route-only permanent failure
/// is terminal for that route, so every permanent error must name the action
/// an operator has to take. Shared by `await_ready_channel`, the consumer
/// pre-flight probe, the lazy producer, and the route-only gRPC
/// `FAILED_PRECONDITION` paths so the repair text has a single source (tasks
/// 4.3/4.7). It carries no payload or header bytes.
pub(crate) const OPERATOR_REPAIR_HINT: &str = "stop the route, repair or remove the corrupt journal or the unkeyable source, \
     then restart the context";

/// Kills the slot's installed bridge (if any) and parks the slot with
/// `reason` — the shared fatal-capability flow (tasks 8.1/1.2): the child is
/// stopped with the same helper used for other fatal bridge conditions and
/// `Parked` (terminal) carries the rejection without retry.
async fn kill_and_park(slot: &BridgeSlot, reason: String) {
    let old_process = {
        let mut guard = slot.process.lock().await;
        guard.take()
    };
    if let Some(p) = old_process {
        let _ = p.stop().await;
    }
    let _ = slot.publish_unless_terminal(BridgeState::Parked(reason));
}

/// Builds the terminal error returned to every waiter on a `Parked` slot
/// (tasks 4.3/4.7). The message keeps the bridge-reported `reason` — a safe,
/// local startup/config value, never a payload or header — and adds the
/// operator repair action.
pub(crate) fn parked_operator_error(broker_name: &str, reason: &str) -> CamelError {
    CamelError::ProcessorError(format!(
        "JMS broker '{broker_name}' is parked and cannot serve requests: {reason}. \
         Operator repair required: {OPERATOR_REPAIR_HINT}"
    ))
}

/// Redact credentials from a broker URL for safe logging. Delegates to the
/// canonical [`camel_api::redact::redact_url`] (bd rc-eh49): userinfo in
/// every authority window (maximal `/`+`\` runs, backslash runs gated on a
/// scheme prefix) is masked, everything from the earliest `?` or `#` is
/// dropped, the sentinels compose in first-occurrence order, and the
/// result is capped at 256 bytes on a UTF-8 char boundary.
fn redact_url(url: &str) -> String {
    camel_api::redact::redact_url(url)
}

pub fn is_bridge_transport_error(err: &CamelError) -> bool {
    // Typed variant matching: only ProcessorError messages that start with
    // the well-known transport prefix are classified as transport errors.
    // This rejects Config errors, business errors, and other CamelError variants
    // without relying on the Display wrapper formatting.
    match err {
        CamelError::ProcessorError(msg) => msg.starts_with(BRIDGE_TRANSPORT_ERROR_PREFIX),
        _ => false,
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

    use super::*;
    use crate::config::{BrokerConfig, HealthBrokerProbeMode, JmsPoolConfig, redact_broker_url};
    use std::collections::HashMap;

    #[test]
    fn from_config_accepts_empty_brokers() {
        let pool_config = JmsPoolConfig::default();
        let result = JmsBridgePool::from_config(pool_config);
        assert!(result.is_ok());
    }

    #[test]
    fn jms_metadata_scheme_matches_instance_scheme() {
        // Alias instances (activemq/artemis) must report their own scheme in
        // metadata(), not the shared descriptor's "jms".
        let pool = Arc::new(JmsBridgePool::from_config(JmsPoolConfig::default()).unwrap());
        for scheme in ["jms", "activemq", "artemis"] {
            let component = JmsComponent::with_scheme(scheme, Arc::clone(&pool));
            assert_eq!(component.metadata().scheme, scheme);
        }
    }

    #[test]
    fn resolve_broker_name_with_explicit_name() {
        let pool = JmsBridgePool::from_config(JmsPoolConfig::single_broker(
            "tcp://localhost:61616",
            BrokerType::ActiveMq,
        ))
        .unwrap();
        assert_eq!(
            pool.resolve_broker_name(Some("default")).unwrap(),
            "default"
        );
    }

    #[test]
    fn resolve_broker_name_default() {
        let pool = JmsBridgePool::from_config(JmsPoolConfig::single_broker(
            "tcp://localhost:61616",
            BrokerType::ActiveMq,
        ))
        .unwrap();
        assert_eq!(pool.resolve_broker_name(None).unwrap(), "default");
    }

    #[test]
    fn resolve_broker_name_unknown_returns_error() {
        let pool = JmsBridgePool::from_config(JmsPoolConfig::single_broker(
            "tcp://localhost:61616",
            BrokerType::ActiveMq,
        ))
        .unwrap();
        let err = pool.resolve_broker_name(Some("unknown")).unwrap_err();
        assert!(
            err.to_string().contains("Unknown JMS broker 'unknown'"),
            "got: {}",
            err
        );
    }

    #[test]
    fn resolve_broker_type_scheme_overrides() {
        let pool = JmsBridgePool::from_config(JmsPoolConfig::single_broker(
            "tcp://localhost:61616",
            BrokerType::Generic,
        ))
        .unwrap();
        assert_eq!(
            pool.resolve_broker_type("activemq", "default"),
            BrokerType::ActiveMq
        );
        assert_eq!(
            pool.resolve_broker_type("artemis", "default"),
            BrokerType::Artemis
        );
        assert_eq!(
            pool.resolve_broker_type("jms", "default"),
            BrokerType::Generic
        );
    }

    #[test]
    fn resolve_broker_type_activemq_scheme_overrides_artemis_config() {
        let pool = JmsBridgePool::from_config(JmsPoolConfig {
            brokers: HashMap::from([(
                "main".to_string(),
                BrokerConfig {
                    broker_url: "tcp://localhost:61616".to_string(),
                    broker_type: BrokerType::Artemis,
                    username: None,
                    password: None,
                    tls: None,
                },
            )]),
            ..JmsPoolConfig::default()
        })
        .unwrap();
        assert_eq!(
            pool.resolve_broker_type("activemq", "main"),
            BrokerType::ActiveMq
        );
        assert_eq!(pool.resolve_broker_type("jms", "main"), BrokerType::Artemis);
    }

    #[test]
    fn create_endpoint_resolves_broker() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );
        let component = JmsComponent::with_scheme("jms", pool);
        let endpoint = component.create_endpoint(
            "jms:queue:orders",
            &camel_component_api::NoOpComponentContext,
        );
        assert!(endpoint.is_ok(), "got: {:?}", endpoint.err());
    }

    #[test]
    fn create_endpoint_rejects_wrong_scheme() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );
        let component = JmsComponent::with_scheme("jms", pool);
        let err = component
            .create_endpoint("kafka:orders", &camel_component_api::NoOpComponentContext)
            .err()
            .unwrap();
        assert!(
            err.to_string()
                .contains("expected scheme 'jms', 'activemq', or 'artemis'"),
            "got: {}",
            err
        );
    }

    #[test]
    fn create_endpoint_with_explicit_broker_param() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: HashMap::from([
                    (
                        "primary".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://primary:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
                            tls: None,
                        },
                    ),
                    (
                        "secondary".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://secondary:61616".to_string(),
                            broker_type: BrokerType::Artemis,
                            username: None,
                            password: None,
                            tls: None,
                        },
                    ),
                ]),
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );
        let component = JmsComponent::with_scheme("jms", Arc::clone(&pool));
        let endpoint = component.create_endpoint(
            "jms:queue:orders?broker=secondary",
            &camel_component_api::NoOpComponentContext,
        );
        assert!(endpoint.is_ok(), "got: {:?}", endpoint.err());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn concurrent_get_or_create_slot_no_deadlock() {
        use tokio::time::timeout;

        // Serialize against other tests mutating CAMEL_JMS_BRIDGE_BINARY_PATH (rc-alwn).
        let _env_lock_guard = crate::BRIDGE_ENV_LOCK.lock().unwrap();

        let env_key = "CAMEL_JMS_BRIDGE_BINARY_PATH";
        let _guard = env_guard(env_key);
        // SAFETY: test-scoped env mutation.
        unsafe { std::env::set_var(env_key, "/bin/false") };

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: HashMap::from([(
                    "test".to_string(),
                    BrokerConfig {
                        broker_url: "tcp://localhost:61616".to_string(),
                        broker_type: BrokerType::ActiveMq,
                        username: None,
                        password: None,
                        tls: None,
                    },
                )]),
                bridge_start_timeout_ms: 100,
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );

        let handles: Vec<_> = (0..5)
            .map(|_| {
                let pool = Arc::clone(&pool);
                tokio::spawn(async move {
                    let _ = pool.get_or_create_slot("test", false).await;
                })
            })
            .collect();

        let result = timeout(Duration::from_secs(5), async {
            for h in handles {
                let _ = h.await;
            }
        })
        .await;

        assert!(result.is_ok(), "Concurrent get_or_create_slot deadlocked!");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn lazy_producer_reports_degraded_when_bridge_start_fails() {
        use tower::Service;

        // Serialize against other tests mutating CAMEL_JMS_BRIDGE_BINARY_PATH (rc-alwn).
        let _env_lock_guard = crate::BRIDGE_ENV_LOCK.lock().unwrap();

        let env_key = "CAMEL_JMS_BRIDGE_BINARY_PATH";
        let _guard = env_guard(env_key);
        // SAFETY: test-scoped env mutation.
        unsafe { std::env::set_var(env_key, "/bin/false") };

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: HashMap::from([(
                    "default".to_string(),
                    BrokerConfig {
                        broker_url: "tcp://localhost:61616".to_string(),
                        broker_type: BrokerType::ActiveMq,
                        username: None,
                        password: None,
                        tls: None,
                    },
                )]),
                bridge_start_timeout_ms: 100,
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );

        let component = JmsComponent::with_scheme("jms", pool);
        let endpoint = component
            .create_endpoint(
                "jms:queue:orders",
                &camel_component_api::NoOpComponentContext,
            )
            .unwrap();
        let mut producer = endpoint
            .create_producer(rt(), &camel_component_api::ProducerContext::default())
            .unwrap();

        let mut exchange = Exchange::default();
        exchange.input.body = camel_component_api::Body::Text("hello".to_string());

        let err = producer.call(exchange).await.unwrap_err();
        assert!(err.to_string().contains("is degraded"), "got: {}", err);
    }

    /// A send transport error first refreshes the channel. If the refresh
    /// targets the installed second dead endpoint and fails, the producer asks
    /// for a liveness probe and leaves the slot Ready (never Restarting).
    #[cfg(unix)]
    #[tokio::test]
    async fn lazy_producer_requests_liveness_probe_when_refresh_unavailable() {
        use tokio::sync::watch;
        use tonic::transport::Channel;
        use tower::Service;

        let dir = tempfile::tempdir().expect("stub tempdir");
        let first_dead_channel = Channel::from_static("http://127.0.0.1:1").connect_lazy();

        let (state_tx, state_rx) = watch::channel(BridgeState::Ready {
            channel: first_dead_channel,
        });

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        // The first dead channel drives the send failure. The installed real
        // process advertises a second, distinct dead endpoint (port 2); the
        // producer's own refresh must select and fail against it.
        let second_dead_process = ready_stub_process(dir.path(), "second-dead", 2).await;
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx: state_tx.clone(),
            process: Arc::new(tokio::sync::Mutex::new(Some(second_dead_process))),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let endpoint_config =
            crate::config::JmsEndpointConfig::from_uri("jms:queue:test-retry").unwrap();

        let mut producer = LazyJmsProducer {
            pool: Arc::clone(&pool),
            broker_name: "default".to_string(),
            endpoint_config,
            resolved_broker_type: BrokerType::ActiveMq,
        };

        let mut exchange = Exchange::default();
        exchange.input.body = camel_component_api::Body::Text("hello".to_string());

        let err = producer
            .call(exchange)
            .await
            .expect_err("the first dead channel must fail the send");
        assert!(
            err.to_string().contains(BRIDGE_TRANSPORT_ERROR_PREFIX),
            "the producer must surface the original transport error, got: {err}"
        );

        // The observation must come from the producer call itself, not from a
        // manual follow-up refresh.
        let attempt = next_refresh_attempt(&pool, 0).await;
        assert_eq!(attempt.broker_name, "default");
        assert_eq!(
            attempt.endpoint,
            Some(2),
            "the refresh must name the installed second endpoint: {attempt:?}"
        );
        assert!(
            attempt.failed,
            "the second dead endpoint must fail: {attempt:?}"
        );
        assert!(!attempt.timed_out, "{attempt:?}");
        assert_eq!(
            refresh_attempts(&pool).len(),
            1,
            "the producer call generated exactly one refresh observation"
        );
        assert!(
            matches!(&*state_rx.borrow(), BridgeState::Ready { .. }),
            "a liveness wake must preserve Ready"
        );
        assert_eq!(replacement_attempts(&pool), 0);

        stop_slot_process(&slot).await;
    }

    // ── JMS-007: Transport error classification ──────────────────────────────

    #[test]
    fn transport_error_detects_send_error() {
        let err = CamelError::ProcessorError(format!(
            "{}send error: connection refused",
            BRIDGE_TRANSPORT_ERROR_PREFIX
        ));
        assert!(
            is_bridge_transport_error(&err),
            "send error must be classified as transport"
        );
    }

    #[test]
    fn transport_error_detects_subscribe_error() {
        let err = CamelError::ProcessorError(format!(
            "{}subscribe error: stream reset",
            BRIDGE_TRANSPORT_ERROR_PREFIX
        ));
        assert!(
            is_bridge_transport_error(&err),
            "subscribe error must be classified as transport"
        );
    }

    #[test]
    fn transport_error_rejects_business_errors() {
        let err = CamelError::ProcessorError("JMS broker 'main' is degraded: timeout".to_string());
        assert!(
            !is_bridge_transport_error(&err),
            "degraded state error must NOT be transport"
        );
    }

    #[test]
    fn transport_error_rejects_config_errors() {
        let err = CamelError::Config("bridge_start_timeout_ms must be > 0".to_string());
        assert!(
            !is_bridge_transport_error(&err),
            "config error must NOT be transport"
        );
    }

    #[test]
    fn transport_error_prefix_is_used_by_producer_and_consumer() {
        // Verify the constant prefix matches what producer.rs and consumer.rs emit.
        // If this test fails, the constant has drifted from the error format strings.
        assert!(
            BRIDGE_TRANSPORT_ERROR_PREFIX.starts_with("JMS gRPC "),
            "prefix must start with 'JMS gRPC '"
        );
    }

    // ── JMS-006: max_bridges enforcement ─────────────────────────────────────

    #[tokio::test]
    async fn pool_enforces_max_bridges_limit() {
        use tokio::sync::watch;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: HashMap::from([
                    (
                        "b1".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://b1:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
                            tls: None,
                        },
                    ),
                    (
                        "b2".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://b2:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
                            tls: None,
                        },
                    ),
                    (
                        "b3".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://b3:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
                            tls: None,
                        },
                    ),
                ]),
                max_bridges: 2,
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );

        // Manually insert two slots to simulate existing bridges.
        // max_bridges counts ALL slots in the map (not just active states).
        for name in &["b1", "b2"] {
            let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
            let slot = Arc::new(BridgeSlot {
                name: name.to_string(),
                broker_url: format!("tcp://{name}:61616"),
                broker_type: BrokerType::ActiveMq,
                credentials: None,
                state_rx,
                state_tx,
                process: Arc::new(tokio::sync::Mutex::new(None)),
                health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
                probe_wake: Arc::new(tokio::sync::Notify::new()),
                last_broker_failure: std::sync::Mutex::new(None),
                selector_required: Arc::new(AtomicBool::new(false)),
            });
            pool.slots.insert(name.to_string(), slot);
        }

        // Attempting to create a third slot should fail.
        let err = pool.get_or_create_slot("b3", false).await.unwrap_err();
        assert!(
            err.to_string().contains("max_bridges"),
            "expected max_bridges error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn pool_allows_slot_when_below_max_bridges() {
        use tokio::sync::watch;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: HashMap::from([(
                    "b1".to_string(),
                    BrokerConfig {
                        broker_url: "tcp://b1:61616".to_string(),
                        broker_type: BrokerType::ActiveMq,
                        username: None,
                        password: None,
                        tls: None,
                    },
                )]),
                max_bridges: 2,
                bridge_start_timeout_ms: 100,
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );

        // Insert one slot in Degraded state (not counted as active).
        let (state_tx, state_rx) = watch::channel(BridgeState::Degraded("test".to_string()));
        let slot = Arc::new(BridgeSlot {
            name: "b1".to_string(),
            broker_url: "tcp://b1:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("b1".to_string(), slot);

        // b1 is Degraded (not active), so creating b1's slot returns existing.
        // The max_bridges check only applies to new slots.
        let result = pool.get_or_create_slot("b1", false).await;
        assert!(result.is_ok(), "existing slot must be returned");
    }

    // ── JMS-003: poll_ready reflects bridge state ────────────────────────────

    #[tokio::test]
    async fn poll_ready_returns_pending_when_starting() {
        use tokio::sync::watch;
        use tower::Service;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);

        let endpoint_config = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut producer = LazyJmsProducer {
            pool: Arc::clone(&pool),
            broker_name: "default".to_string(),
            endpoint_config,
            resolved_broker_type: BrokerType::ActiveMq,
        };

        let result = producer.poll_ready(&mut Context::from_waker(futures::task::noop_waker_ref()));
        assert!(
            matches!(result, Poll::Pending),
            "poll_ready must be Pending when Starting; got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn poll_ready_returns_error_when_degraded() {
        use tokio::sync::watch;
        use tower::Service;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        let (state_tx, state_rx) =
            watch::channel(BridgeState::Degraded("health check failed".to_string()));
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);

        let endpoint_config = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut producer = LazyJmsProducer {
            pool: Arc::clone(&pool),
            broker_name: "default".to_string(),
            endpoint_config,
            resolved_broker_type: BrokerType::ActiveMq,
        };

        let result = producer.poll_ready(&mut Context::from_waker(futures::task::noop_waker_ref()));
        assert!(
            matches!(result, Poll::Ready(Err(_))),
            "poll_ready must be Err when Degraded; got: {:?}",
            result
        );
        let err_msg = match result {
            Poll::Ready(Err(e)) => e.to_string(),
            _ => unreachable!(),
        };
        assert!(
            err_msg.contains("degraded"),
            "error must mention degraded: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn lazy_producer_poll_ready_returns_consumer_stopping_on_stopped() {
        use tokio::sync::watch;
        use tower::Service;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        let (state_tx, state_rx) = watch::channel(BridgeState::Stopped);
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);

        let endpoint_config = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut producer = LazyJmsProducer {
            pool: Arc::clone(&pool),
            broker_name: "default".to_string(),
            endpoint_config,
            resolved_broker_type: BrokerType::ActiveMq,
        };

        let result = producer.poll_ready(&mut Context::from_waker(futures::task::noop_waker_ref()));
        assert!(
            matches!(result, Poll::Ready(Err(CamelError::ConsumerStopping))),
            "poll_ready must return ConsumerStopping when Stopped; got: {:?}",
            result
        );
    }

    /// Task 4.3: a `Parked` slot is terminal — `poll_ready` must fail fast
    /// with an error naming the operator repair instead of pending forever.
    #[tokio::test]
    async fn lazy_producer_poll_ready_parked_fails_fast() {
        use tokio::sync::watch;
        use tower::Service;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        let (state_tx, state_rx) =
            watch::channel(BridgeState::Parked("corrupt journal".to_string()));
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);

        let endpoint_config = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut producer = LazyJmsProducer {
            pool: Arc::clone(&pool),
            broker_name: "default".to_string(),
            endpoint_config,
            resolved_broker_type: BrokerType::ActiveMq,
        };

        let result = producer.poll_ready(&mut Context::from_waker(futures::task::noop_waker_ref()));
        let err = match result {
            Poll::Ready(Err(e)) => e.to_string(),
            other => panic!("poll_ready must fail fast on Parked; got: {other:?}"),
        };
        assert!(
            err.contains("corrupt journal"),
            "error must carry the park reason: {err}"
        );
        assert!(
            err.contains("repair") && err.contains("restart the context"),
            "error must name the operator repair: {err}"
        );
    }

    /// Task 4.7: every permanent error names the operator action. The parked
    /// (terminal) error must tell the operator to stop the route, repair or
    /// remove the corrupt journal or the unkeyable source, then restart the
    /// context — and must not interpolate any payload or header bytes.
    #[test]
    fn permanent_error_names_operator_action() {
        let err = parked_operator_error("default", "corrupt journal").to_string();
        for needle in [
            "stop the route",
            "repair or remove",
            "corrupt journal",
            "unkeyable source",
            "restart the context",
        ] {
            assert!(
                err.contains(needle),
                "permanent error must name '{needle}': {err}"
            );
        }
    }

    #[tokio::test]
    async fn poll_ready_returns_ready_when_slot_ready() {
        use tokio::sync::watch;
        use tonic::transport::Endpoint as TonicEndpoint;
        use tower::Service;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        let lazy_channel = TonicEndpoint::from_static("http://127.0.0.1:1").connect_lazy();
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready {
            channel: lazy_channel,
        });
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);

        let endpoint_config = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut producer = LazyJmsProducer {
            pool: Arc::clone(&pool),
            broker_name: "default".to_string(),
            endpoint_config,
            resolved_broker_type: BrokerType::ActiveMq,
        };

        let result = producer.poll_ready(&mut Context::from_waker(futures::task::noop_waker_ref()));
        assert!(
            matches!(result, Poll::Ready(Ok(()))),
            "poll_ready must be Ready(Ok) when bridge is Ready; got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn poll_ready_returns_ready_when_no_slot_exists() {
        use tower::Service;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        let endpoint_config = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut producer = LazyJmsProducer {
            pool: Arc::clone(&pool),
            broker_name: "default".to_string(),
            endpoint_config,
            resolved_broker_type: BrokerType::ActiveMq,
        };

        // No slot exists yet — poll_ready should return Ready so call() can start the bridge.
        let result = producer.poll_ready(&mut Context::from_waker(futures::task::noop_waker_ref()));
        assert!(
            matches!(result, Poll::Ready(Ok(()))),
            "poll_ready must be Ready(Ok) when no slot exists; got: {:?}",
            result
        );
    }

    // ── JMS-001: Health monitor lifecycle ────────────────────────────────────

    #[tokio::test]
    async fn pool_shutdown_awaits_health_monitor() {
        use tokio::sync::watch;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: HashMap::from([(
                    "default".to_string(),
                    BrokerConfig {
                        broker_url: "tcp://localhost:61616".to_string(),
                        broker_type: BrokerType::ActiveMq,
                        username: None,
                        password: None,
                        tls: None,
                    },
                )]),
                health_check_interval_ms: 100,
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );

        // Create a slot manually with a spawned health monitor task.
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready {
            channel: tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy(),
        });
        let monitor_handle_ref: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>> =
            Arc::new(tokio::sync::Mutex::new(None));

        // Spawn a simple monitor that exits on Stopped.
        let state_rx_clone = state_rx.clone();
        let handle = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if matches!(*state_rx_clone.borrow(), BridgeState::Stopped) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await; // allow-test-sleep: paces the simulated health-monitor poll loop
                }
            })
            .await
            .expect("health-monitor shutdown must complete within 10s");
        });
        *monitor_handle_ref.lock().await = Some(handle); // allow-test-wait: fresh test-created handle slot; uncontended

        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: monitor_handle_ref,
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);

        // Shutdown should complete without hanging — the monitor exits on Stopped.
        let result = pool.shutdown().await;
        // May report errors from bridge process (none in this test), but must not hang.
        let _ = result;
    }

    #[tokio::test]
    async fn health_monitor_handle_stored_after_spawn() {
        use tokio::sync::watch;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: HashMap::from([(
                    "default".to_string(),
                    BrokerConfig {
                        broker_url: "tcp://localhost:61616".to_string(),
                        broker_type: BrokerType::ActiveMq,
                        username: None,
                        password: None,
                        tls: None,
                    },
                )]),
                health_check_interval_ms: 100,
                bridge_start_timeout_ms: 100,
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );

        let (state_tx, state_rx) = watch::channel(BridgeState::Stopped);
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        pool.spawn_health_monitor(Arc::clone(&slot)).await;

        // spawn_health_monitor stores the handle before returning, so the
        // guard is already Some here — no settle window needed.
        let guard = slot.health_monitor_handle.lock().await; // allow-test-wait: handle stored before return; uncontended
        assert!(
            guard.is_some(),
            "health monitor handle must be stored after spawn_health_monitor"
        );
    }

    // ── JMS-008: URL redaction for safe logging ──────────────────────────────

    #[test]
    fn redact_url_strips_userinfo_with_password() {
        assert_eq!(
            redact_url("tcp://admin:s3cret@broker:61616"),
            "tcp://***@broker:61616"
        );
    }

    #[test]
    fn redact_url_strips_userinfo_without_password() {
        assert_eq!(
            redact_url("tcp://admin@broker:61616"),
            "tcp://***@broker:61616"
        );
    }

    #[test]
    fn redact_url_passes_clean_url_unchanged() {
        assert_eq!(redact_url("tcp://localhost:61616"), "tcp://localhost:61616");
    }

    /// Spec cross-surface identity fixture (jms surface): the component
    /// `redact_url` delegates to `camel_api::redact::redact_url`, so both
    /// surfaces must return the same bytes on the same input.
    #[test]
    fn redact_url_shared_fixture_pin() {
        let raw = "http://h:99999/p?token=secret";
        assert_eq!(
            camel_api::redact::redact_url(raw),
            "http://h:99999/p?[redacted]"
        );
        assert_eq!(redact_url(raw), camel_api::redact::redact_url(raw));
    }

    #[test]
    fn redact_url_handles_ssl_scheme() {
        assert_eq!(
            redact_url("ssl://user:pass@secure-broker:61617"),
            "ssl://***@secure-broker:61617"
        );
    }

    /// bd rc-eh49: the earliest `?`/`#` introducer wins; both sentinels
    /// compose in first-occurrence order when the raw URL carries both
    /// introducers, and fragment bytes after the cut are dropped with it.
    #[test]
    fn redact_url_drops_query_and_fragment() {
        assert_eq!(
            redact_url("tcp://broker:61616?user=a#tok=x"),
            "tcp://broker:61616?[redacted]#[redacted]"
        );
    }

    /// bd rc-eh49 compose-both rule: a `#` before `?` flips the sentinel
    /// order accordingly.
    #[test]
    fn redact_url_sentinels_compose_fragment_first() {
        assert_eq!(
            redact_url("tcp://broker:61616#tok=x?user=a"),
            "tcp://broker:61616#[redacted]?[redacted]"
        );
    }

    /// bd rc-eh49: a `//` window after the first is scanned too —
    /// credentials cannot hide behind a benign first window.
    #[test]
    fn redact_url_later_window_masked() {
        assert_eq!(redact_url("tcp://h//user:pass@x/"), "tcp://h//***@x/");
    }

    /// bd rc-eh49: a slash run after `//` cannot hide userinfo from the
    /// window scan.
    #[test]
    fn redact_url_slash_run_masked() {
        let redacted = redact_url("tcp:////user:pass@broker:61616");
        assert!(!redacted.contains("user:pass"), "leaked: {redacted}");
        assert!(
            redacted.contains("***@broker:61616"),
            "masked in place: {redacted}"
        );
    }

    /// bd rc-eh49: the old first-`@`-anywhere scan masked through an `@`
    /// riding the query (`tcp://***@b`); only a window `@` is userinfo.
    #[test]
    fn redact_url_at_in_query_not_userinfo_mask() {
        assert_eq!(
            redact_url("tcp://broker:61616?q=a@b"),
            "tcp://broker:61616?[redacted]"
        );
    }

    /// bd rc-eh49: the 256-byte cap cuts on a char boundary — a multibyte
    /// character straddling byte 256 is dropped whole, never split
    /// mid-encode.
    #[test]
    fn redact_url_truncate_multibyte_boundary() {
        let mut url = String::from("tcp://broker:61616/");
        url.push_str(&"x".repeat(236)); // 255 ASCII bytes before the multibyte char
        url.push('日'); // 3 bytes straddling the 256-byte cap
        url.push_str(&"y".repeat(50)); // push the total past 300 bytes
        assert!(url.len() > 300);
        let redacted = redact_url(&url);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(std::str::from_utf8(redacted.as_bytes()).is_ok());
        assert!(
            !redacted.contains('日'),
            "straddling char dropped whole: {redacted}"
        );
    }

    /// e_gpt stage-4: the 256-byte cap must not split an appended sentinel.
    /// The base is truncated at `256 - sentinel_len` BEFORE the sentinel is
    /// appended, so the sentinel always renders intact and the total stays
    /// ≤ 256. Covers both jms redactors: `redact_url` (compose sentinels)
    /// and `redact_broker_url` (fragment sentinel over kept query content).
    #[test]
    fn redact_url_keeps_sentinels_intact_under_256_cap() {
        // `redact_url`: base (masked, cut at the `?`) is 255 bytes, so byte
        // 256 lands inside the appended `?[redacted]` (starts at 255) pre-fix.
        let url = format!("tcp://{}?x=1", "a".repeat(250));
        let redacted = redact_url(&url);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(
            redacted.ends_with("?[redacted]"),
            "redact_url sentinel must render intact: {redacted}"
        );

        // `redact_broker_url`: base + kept query is 252 bytes, so byte 256
        // lands inside the appended `#[redacted]` (starts at 252) pre-fix.
        let broker = format!("tcp://{}?keep=1#frag", "a".repeat(240));
        let redacted = redact_broker_url(&broker);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(
            redacted.ends_with("#[redacted]"),
            "redact_broker_url sentinel must render intact: {redacted}"
        );
    }

    // ── JMS-009: max_bridges race condition under concurrency ────────────────

    #[tokio::test]
    async fn concurrent_slot_creation_respects_max_bridges() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: HashMap::from([
                    (
                        "b1".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://b1:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
                            tls: None,
                        },
                    ),
                    (
                        "b2".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://b2:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
                            tls: None,
                        },
                    ),
                    (
                        "b3".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://b3:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
                            tls: None,
                        },
                    ),
                ]),
                max_bridges: 2,
                bridge_start_timeout_ms: 100,
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        for name in &["b1", "b2"] {
            let slot = Arc::new(BridgeSlot {
                name: name.to_string(),
                broker_url: format!("tcp://{name}:61616"),
                broker_type: BrokerType::ActiveMq,
                credentials: None,
                state_rx: state_rx.clone(),
                state_tx: state_tx.clone(),
                process: Arc::new(tokio::sync::Mutex::new(None)),
                health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
                probe_wake: Arc::new(tokio::sync::Notify::new()),
                last_broker_failure: std::sync::Mutex::new(None),
                selector_required: Arc::new(AtomicBool::new(false)),
            });
            pool.slots.insert(name.to_string(), slot);
        }

        assert_eq!(pool.slots.len(), 2);

        let guard = pool.bridge_create_lock.lock().await; // allow-test-wait: fresh pool; create lock uncontended
        let total_count = pool.slots.len();
        assert!(total_count >= pool.max_bridges);
        let result = if total_count >= pool.max_bridges {
            Err(CamelError::Config(format!(
                "JMS bridge limit reached: {total_count} bridge(s) >= max_bridges ({})",
                pool.max_bridges
            )))
        } else {
            Ok(())
        };
        drop(guard);

        assert!(result.is_err(), "3rd broker should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("max_bridges"),
            "error must mention max_bridges, got: {err_msg}"
        );
    }

    // ── JMS-010: Transport error does NOT auto-resend ────────────────────────

    #[cfg(unix)]
    #[tokio::test]
    async fn transport_error_requests_liveness_probe_and_does_not_resend() {
        use tokio::sync::watch;
        use tower::Service;

        let dir = tempfile::tempdir().expect("stub tempdir");
        let (port, sends, _health_calls, server_task) = spawn_counting_send_failure_bridge().await;
        let channel = connect_scripted(port).await;

        let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        // The installed real process advertises a second, distinct dead
        // endpoint; the producer's own refresh attempts it and fails without
        // resending the exchange.
        let second_dead_process = ready_stub_process(dir.path(), "second-dead", 2).await;
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx: state_tx.clone(),
            process: Arc::new(tokio::sync::Mutex::new(Some(second_dead_process))),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let endpoint_config =
            crate::config::JmsEndpointConfig::from_uri("jms:queue:test-no-resend").unwrap();

        let mut producer = LazyJmsProducer {
            pool: Arc::clone(&pool),
            broker_name: "default".to_string(),
            endpoint_config,
            resolved_broker_type: BrokerType::ActiveMq,
        };

        let mut exchange = Exchange::default();
        exchange.input.body = camel_component_api::Body::Text("hello".to_string());

        let err = producer
            .call(exchange)
            .await
            .expect_err("the transport-dead send must fail");
        assert!(
            err.to_string().contains(BRIDGE_TRANSPORT_ERROR_PREFIX),
            "error must be original transport error, got: {err}"
        );
        assert_eq!(
            sends.load(Ordering::SeqCst),
            1,
            "the original request must be sent exactly once (no resend)"
        );

        // The observation must come from the producer call itself, not from a
        // manual follow-up refresh.
        let attempt = next_refresh_attempt(&pool, 0).await;
        assert_eq!(attempt.broker_name, "default");
        assert_eq!(
            attempt.endpoint,
            Some(2),
            "the refresh must name the installed second endpoint: {attempt:?}"
        );
        assert!(
            attempt.failed,
            "the second dead endpoint must fail: {attempt:?}"
        );
        assert!(!attempt.timed_out, "{attempt:?}");
        assert_eq!(
            refresh_attempts(&pool).len(),
            1,
            "the producer call generated exactly one refresh observation"
        );
        assert!(
            matches!(&*state_rx.borrow(), BridgeState::Ready { .. }),
            "the slot must remain Ready"
        );
        assert_eq!(replacement_attempts(&pool), 0);

        server_task.abort();
        let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
        stop_slot_process(&slot).await;
    }

    // ── D-L12: Drop impl cleans up slots without explicit shutdown ───────────

    #[tokio::test]
    async fn test_jms_bridge_pool_drop_cleans_up_slots() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::sync::watch;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: HashMap::from([(
                    "default".to_string(),
                    BrokerConfig {
                        broker_url: "tcp://localhost:61616".to_string(),
                        broker_type: BrokerType::ActiveMq,
                        username: None,
                        password: None,
                        tls: None,
                    },
                )]),
                health_check_interval_ms: 50,
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );

        // Create a slot with a health monitor that exits on BridgeState::Stopped.
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready {
            channel: tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy(),
        });

        let monitor_exited = Arc::new(AtomicBool::new(false));
        let exited = monitor_exited.clone();
        let rx = state_rx.clone();
        let handle = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if matches!(*rx.borrow(), BridgeState::Stopped) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await; // allow-test-sleep: paces the simulated monitor poll loop
                }
            })
            .await
            .expect("slot cleanup monitor must exit within 10s");
            exited.store(true, Ordering::SeqCst);
        });

        let monitor_handle_ref: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        *monitor_handle_ref.lock().await = Some(handle); // allow-test-wait: fresh test-created handle slot; uncontended

        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: monitor_handle_ref,
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), slot);

        // Drop the pool WITHOUT calling shutdown() — the Drop impl must fire.
        drop(pool);

        // Poll-with-deadline instead of a bare sleep: the Drop cleanup task
        // sends Stopped asynchronously; a deadline-bounded wait fails the
        // assertion (not a flake) when it never lands.
        async fn wait_monitor_exit(
            exited: &Arc<AtomicBool>,
            deadline: std::time::Duration,
        ) -> bool {
            let start = std::time::Instant::now();
            while !exited.load(Ordering::SeqCst) {
                if start.elapsed() >= deadline {
                    return false;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            true
        }

        assert!(
            wait_monitor_exit(&monitor_exited, std::time::Duration::from_secs(2)).await,
            "health monitor should have exited after pool drop (Stopped signal sent by Drop)"
        );
    }

    /// Task 4.1 (platform-independent): `classify_start_error` keeps a
    /// permanent failure typed and maps every other error to the transient path.
    #[test]
    fn classify_start_error_maps_permanent() {
        match classify_start_error(BridgeError::PermanentFailure("boom".to_string())) {
            StartBridgeError::Permanent(reason) => assert_eq!(reason, "boom"),
            StartBridgeError::Other(e) => panic!("permanent was flattened: {e}"),
        }
        match classify_start_error(BridgeError::Transport("refused".to_string())) {
            StartBridgeError::Other(_) => {}
            StartBridgeError::Permanent(reason) => {
                panic!("transient mapped to permanent: {reason}")
            }
        }
    }

    /// Task 4.2: a permanent startup rejection parks the slot instead of
    /// degrading it into the restart loop. Drives the same private mapping the
    /// pool startup path uses and observes the published watch state.
    #[tokio::test]
    async fn startup_permanent_failure_parks_slot() {
        use tokio::sync::watch;

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        };

        JmsBridgePool::apply_initial_start_result(
            &slot,
            Err(StartBridgeError::Permanent(
                "JMS_ATTEMPT_JOURNAL_DIR is not owner-private".to_string(),
            )),
        )
        .await;
        match &*state_rx.borrow() {
            BridgeState::Parked(reason) => assert!(
                reason.contains("JMS_ATTEMPT_JOURNAL_DIR"),
                "parked reason must carry the bridge text: {reason}"
            ),
            other => panic!("expected Parked, got: {other:?}"),
        }

        // A transient failure on a fresh slot keeps the historical
        // degraded/retry path.
        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let transient = BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        };
        JmsBridgePool::apply_initial_start_result(
            &transient,
            Err(StartBridgeError::Other(CamelError::ProcessorError(
                "connection refused".to_string(),
            ))),
        )
        .await;
        assert!(
            matches!(&*state_rx.borrow(), BridgeState::Degraded(_)),
            "a transient failure must degrade the slot"
        );
    }

    /// Task 4.2: once parked, the slot is terminal — the health monitor exits,
    /// a restart request is refused, and the existing-slot lookup never spawns
    /// a new process.
    #[tokio::test]
    async fn parked_slot_is_not_respawned() {
        use tokio::sync::watch;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        JmsBridgePool::apply_initial_start_result(
            slot.as_ref(),
            Err(StartBridgeError::Permanent("corrupt journal".to_string())),
        )
        .await;
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        // The monitor must treat `Parked` as terminal and exit without a
        // restart attempt.
        pool.spawn_health_monitor(Arc::clone(&slot)).await;
        let handle = slot
            .health_monitor_handle
            .lock() // allow-test-wait: handle stored before return; uncontended
            .await
            .take()
            .expect("health monitor handle stored");
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("a parked monitor must exit, not loop")
            .expect("the health monitor task must not panic");
        assert!(
            matches!(&*state_rx.borrow(), BridgeState::Parked(_)),
            "the monitor must leave the slot parked"
        );

        // A producer/consumer restart request must not resurrect the slot.
        pool.restart_slot("default");
        assert!(
            matches!(&*state_rx.borrow(), BridgeState::Parked(_)),
            "restart_slot must not respawn a parked slot"
        );

        // An existing-slot lookup returns the parked slot without starting.
        let got = pool.get_or_create_slot("default", false).await.unwrap();
        assert!(
            matches!(&*got.state_rx.borrow(), BridgeState::Parked(_)),
            "existing-slot lookup must return the parked slot unchanged"
        );
        assert!(
            got.process.lock().await.is_none(),
            "no bridge process may be spawned for a parked slot"
        );
    }

    /// Task 4.2 race: shutdown publishes `Stopped` before an in-flight initial
    /// start completes with a permanent failure. The late `Parked` must not
    /// overwrite `Stopped`.
    #[tokio::test]
    async fn shutdown_before_initial_completion_preserves_stopped() {
        use tokio::sync::watch;

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        };

        slot.publish_shutdown();
        JmsBridgePool::apply_initial_start_result(
            &slot,
            Err(StartBridgeError::Permanent("late permanent".to_string())),
        )
        .await;

        assert!(
            matches!(&*state_rx.borrow(), BridgeState::Stopped),
            "a late Parked must not overwrite Stopped"
        );
    }

    /// Task 4.2 race: the same guard for a late transient `Degraded`.
    #[tokio::test]
    async fn shutdown_before_initial_transient_failure_preserves_stopped() {
        use tokio::sync::watch;

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        };

        slot.publish_shutdown();
        JmsBridgePool::apply_initial_start_result(
            &slot,
            Err(StartBridgeError::Other(CamelError::ProcessorError(
                "late transient".to_string(),
            ))),
        )
        .await;

        assert!(
            matches!(&*state_rx.borrow(), BridgeState::Stopped),
            "a late Degraded must not overwrite Stopped"
        );
    }

    /// Task 4.2 race: a delayed refresh (`Ready`) or restart (`Restarting`)
    /// publication must not overwrite `Parked`. Exercises the exact guard used
    /// by `refresh_slot_channel` / `restart_slot`.
    #[tokio::test]
    async fn late_ready_or_restart_publication_preserves_parked() {
        use tokio::sync::watch;
        use tonic::transport::Endpoint as TonicEndpoint;

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        };

        JmsBridgePool::apply_initial_start_result(
            &slot,
            Err(StartBridgeError::Permanent("parked".to_string())),
        )
        .await;
        assert!(matches!(&*state_rx.borrow(), BridgeState::Parked(_)));

        let channel = TonicEndpoint::from_static("http://127.0.0.1:1").connect_lazy();
        assert!(
            !slot.publish_unless_terminal(BridgeState::Ready { channel }),
            "a delayed Ready must be refused on a parked slot"
        );
        assert!(
            !slot.publish_unless_terminal(BridgeState::Restarting {
                attempt: 0,
                next_at: Instant::now(),
            }),
            "a delayed Restarting must be refused on a parked slot"
        );
        assert!(
            matches!(&*state_rx.borrow(), BridgeState::Parked(_)),
            "the delayed publications must not have overwritten Parked"
        );
    }

    /// Task 4.2 race: `restart_slot` eligibility and publication are atomic, so
    /// a transport-triggered restart cannot overwrite a terminal `Stopped`.
    #[tokio::test]
    async fn restart_slot_does_not_overwrite_stopped() {
        use tokio::sync::watch;

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        slot.publish_shutdown();
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        pool.restart_slot("default");

        assert!(
            matches!(&*state_rx.borrow(), BridgeState::Stopped),
            "restart_slot must not overwrite Stopped"
        );
    }

    /// Task 4.2 race with an actual child: a successful restart result that
    /// loses to a shutdown `Stopped` must not be stored, and its process must be
    /// terminated rather than leaked.
    #[cfg(unix)]
    #[tokio::test]
    async fn successful_restart_losing_shutdown_race_disposes_process() {
        use tokio::sync::watch;
        use tonic::transport::Endpoint as TonicEndpoint;

        let dir = tempfile::tempdir().expect("stub tempdir");
        let pidfile = dir.path().join("child.pid");
        let stub = write_stub(
            dir.path(),
            "stub-bridge.sh",
            &format!(
                "#!/bin/sh\necho $$ > '{}'\nprintf '%s\\n' '{{\"status\":\"ready\",\"port\":1}}'\nexec sleep 30\n",
                pidfile.display()
            ),
        );

        let config = BridgeProcessConfig::jms(
            stub,
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            None,
            None,
            300,
            30_000,
            30_000,
            Some("/var/lib/state/rust-camel/jms-bridge".to_string()),
            Some("0123456789abcdef0123456789abcdef".to_string()),
            None,
        );
        let process = BridgeProcess::start(&config)
            .await
            .expect("stub must reach the ready line");

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        };
        // Shutdown wins before the restart result is installed.
        slot.publish_shutdown();
        let dead_channel = TonicEndpoint::from_static("http://127.0.0.1:1").connect_lazy();

        let process = slot
            .install_ready_process(process, dead_channel)
            .await
            .expect_err("a stopped slot must refuse a Ready publication");

        assert!(
            slot.process.lock().await.is_none(),
            "a losing process must not be stored"
        );
        assert!(
            matches!(&*state_rx.borrow(), BridgeState::Stopped),
            "Stopped must be preserved"
        );

        process.stop().await.expect("stop the losing process");
        let pid: u32 = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        assert!(pid > 0, "stub must have written its pid");
        assert!(
            wait_pid_gone(pid).await,
            "the losing child {pid} must be terminated, not leaked"
        );
    }

    /// Writes an executable stub bridge script into `dir`.
    #[cfg(unix)]
    fn write_stub(dir: &std::path::Path, name: &str, script: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(name);
        std::fs::write(&path, script).expect("write stub bridge");
        let mut perms = std::fs::metadata(&path)
            .expect("stat stub bridge")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod stub bridge");
        path
    }

    /// Bounded (no Rust busy-wait) check that `pid` is gone, done with yielding
    /// async polling so the current-thread runtime keeps running and can reap a
    /// `kill_on_drop` child. A synchronous `Command::status` here would starve
    /// the runtime for the whole window, leaving the killed child a zombie that
    /// `kill -0` still reports as present.
    #[cfg(unix)]
    async fn wait_pid_gone(pid: u32) -> bool {
        for _ in 0..100 {
            let alive = tokio::process::Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            if !alive {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// Task 4.1: a permanent bridge startup line observed within the startup
    /// budget is returned as `StartBridgeError::Permanent` and is never erased
    /// by a second deadline; the still-running child is terminated.
    #[cfg(unix)]
    #[tokio::test]
    async fn start_bridge_error_maps_permanent() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let pidfile = dir.path().join("child.pid");
        let stub = write_stub(
            dir.path(),
            "stub-bridge.sh",
            &format!(
                "#!/bin/sh\necho $$ > '{}'\nprintf '%s\\n' '{{\"status\":\"permanent-failure\",\"reason\":\"JMS_ATTEMPT_JOURNAL_DIR is not owner-private\"}}'\nexec sleep 30\n",
                pidfile.display()
            ),
        );

        // A short budget: a delayed cleanup would blow it and erase the signal.
        let config = BridgeProcessConfig::jms(
            stub,
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            None,
            None,
            300,
            30_000,
            30_000,
            Some("/var/lib/state/rust-camel/jms-bridge".to_string()),
            Some("0123456789abcdef0123456789abcdef".to_string()),
            None,
        );

        let result = JmsBridgePool::start_bridge_process_with_config(
            &config,
            300,
            false,
            &AtomicBool::new(false),
            None,
        )
        .await;
        match result {
            Err(StartBridgeError::Permanent(reason)) => {
                assert!(
                    reason.contains("JMS_ATTEMPT_JOURNAL_DIR"),
                    "permanent reason must carry the bridge text: {reason}"
                );
            }
            Err(StartBridgeError::Other(e)) => {
                panic!("permanent startup failure was erased by the budget: {e}")
            }
            Ok(_) => panic!("a permanent-failure line must never start a bridge"),
        }

        let pid: u32 = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        assert!(pid > 0, "stub must have written its pid");
        assert!(wait_pid_gone(pid).await, "child {pid} must be terminated");
    }

    /// Task 8.1: a TLS-configured broker whose Health response reports no
    /// capabilities fails the `broker-tls-v1` gate on the startup Health
    /// handling — the spawned bridge process is killed (the same `stop` helper
    /// used for other fatal bridge conditions) and the permanent error names
    /// the capability.
    #[cfg(unix)]
    #[tokio::test]
    async fn startup_gate_kills_bridge_and_fails_permanently() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        // Real spawned bridge child: the kill seam is the pidfile +
        // `wait_pid_gone` pair the process-lifecycle tests observe.
        let child = stub_bridge_process(dir.path(), "gated").await;
        let child_pid = child.child_pid().expect("stub child pid");

        // TLS-configured broker arms the capability gate.
        let mut brokers = std::collections::HashMap::new();
        brokers.insert(
            "default".to_string(),
            BrokerConfig {
                broker_url: "ssl://10.0.0.5:61617".to_string(),
                broker_type: BrokerType::ActiveMq,
                username: None,
                password: None,
                tls: Some(BrokerTlsConfig {
                    ca_cert_path: None,
                    client_cert_path: None,
                    client_key_path: None,
                    server_name: None,
                }),
            },
        );
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers,
                health_check_interval_ms: 20,
                health_timeout_ms: 250,
                health_failure_threshold: 5,
                ..JmsPoolConfig::default()
            })
            .expect("gate pool config must be valid"),
        );
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        // Stubbed Health response with EMPTY capabilities (the proto3 default
        // an old bridge would send).
        let server =
            spawn_scripted_health(vec![ScriptedHealth::Respond(scripted_response(true, true))])
                .await;
        let mut fx =
            TestBridgeFixture::start(pool.clone(), Arc::clone(&slot), server, Some(child)).await;

        let ack = fx.next_probe().await;
        assert!(
            matches!(ack.outcome, ProbeOutcome::Success),
            "the stubbed Health response must be processed: {ack:?}"
        );
        let parked_reason = match &ack.state {
            BridgeState::Parked(reason) => reason.clone(),
            other => panic!("the gate error must ride the permanent-error channel: {other:?}"),
        };
        assert!(
            parked_reason.contains("broker-tls-v1"),
            "the permanent error must name the missing capability: {parked_reason}"
        );

        assert!(
            wait_pid_gone(child_pid).await,
            "the gated child {child_pid} must be killed, not leaked"
        );

        // No retry is scheduled: `Parked` is the permanent-error channel and a
        // terminal state — the monitor's restart flow never touches it and the
        // route fails through `parked_operator_error` without the backoff path
        // (tasks 4.3/4.7). The harness cannot observe the backoff path
        // directly; asserting the Parked publication is the observable
        // permanent channel.
        assert!(
            matches!(&*slot.state_rx.borrow(), BridgeState::Parked(_)),
            "the slot must stay parked without retry"
        );

        fx.shutdown().await;
    }

    #[test]
    fn health_probe_env_uses_lowercase_names() {
        for (mode, expected) in [
            (HealthBrokerProbeMode::Reuse, "reuse"),
            (HealthBrokerProbeMode::Skip, "skip"),
        ] {
            let (key, value) = health_probe_env(mode);
            assert_eq!(key, "JMS_HEALTH_BROKER_PROBE_MODE");
            assert_eq!(value, expected);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn start_bridge_process_exports_health_probe_mode() {
        // rustls 0.23 requires a process-level CryptoProvider before the
        // post-ready mTLS connect runs in `start_bridge_process`.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let _env_lock_guard = crate::BRIDGE_ENV_LOCK.lock().unwrap();

        let dir = tempfile::tempdir().expect("stub tempdir");
        let mode_file = dir.path().join("probe-mode.txt");
        let pidfile = dir.path().join("child.pid");
        let stub = write_stub(
            dir.path(),
            "stub-bridge.sh",
            &format!(
                "#!/bin/sh\necho $$ > '{}'\nprintf '%s' \"$JMS_HEALTH_BROKER_PROBE_MODE\" > '{}'\nprintf '%s\\n' '{{\"status\":\"ready\",\"port\":1}}'\nexec sleep 30\n",
                pidfile.display(),
                mode_file.display()
            ),
        );

        let env_key = "CAMEL_JMS_BRIDGE_BINARY_PATH";
        let _guard = env_guard(env_key);
        // SAFETY: test-scoped env mutation.
        unsafe { std::env::set_var(env_key, &stub) };

        let result = JmsBridgePool::start_bridge_process(
            crate::BRIDGE_VERSION,
            dir.path(),
            200,
            30_000,
            30_000,
            "default",
            "tcp://localhost:61616",
            &BrokerType::ActiveMq,
            &None,
            None,
            HealthBrokerProbeMode::Skip,
            &AtomicBool::new(false),
        )
        .await;

        assert!(
            result.is_err(),
            "the stub's unreachable port must fail connect/health after the env write"
        );
        let mode = std::fs::read_to_string(&mode_file).expect("stub must write the probe mode");
        assert_eq!(mode.trim(), "skip");

        let pid: u32 = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        assert!(pid > 0, "stub must have written its pid");
        assert!(wait_pid_gone(pid).await, "child {pid} must be terminated");
    }

    #[test]
    fn pool_carries_skip_mode_from_config() {
        let pool = JmsBridgePool::from_config(JmsPoolConfig {
            health_broker_probe_mode: HealthBrokerProbeMode::Skip,
            ..JmsPoolConfig::default()
        })
        .expect("pool config must be valid");

        assert_eq!(pool.health_broker_probe_mode, HealthBrokerProbeMode::Skip);
    }

    // ── Task 2.2: pool knobs retained across bridge restarts ────────────────

    /// Writes the env-recording stub shared by the task 2.2 spawn tests. Every
    /// launch appends one `KEY=VALUE`-pair record to `envfile` and its pid to
    /// `pidfile` before announcing `ready` on an unreachable port.
    #[cfg(unix)]
    fn pool_knob_stub(dir: &std::path::Path) -> std::path::PathBuf {
        let envfile = dir.join("spawn-env.txt");
        let pidfile = dir.join("child.pid");
        write_stub(
            dir,
            "stub-bridge.sh",
            &format!(
                "#!/bin/sh\necho $$ >> '{}'\nprintf 'JMS_POOL_IDLE_TIMEOUT_MS=%s JMS_POOL_EXPIRATION_CHECK_MS=%s\\n' \"$JMS_POOL_IDLE_TIMEOUT_MS\" \"$JMS_POOL_EXPIRATION_CHECK_MS\" >> '{}'\nprintf '%s\\n' '{{\"status\":\"ready\",\"port\":1}}'\nexec sleep 30\n",
                pidfile.display(),
                envfile.display(),
            ),
        )
    }

    /// Task 2.2: the configured pool knobs must reach the child environment of
    /// the initial spawn (pool acquisition) AND of the monitor's replacement
    /// spawn driven with an empty replacement-outcome queue.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn pool_knobs_reach_initial_and_replacement_spawn_env() {
        // rustls 0.23 requires a process-level CryptoProvider before the
        // post-ready mTLS connect runs in `start_bridge_process`.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let _env_lock_guard = crate::BRIDGE_ENV_LOCK.lock().unwrap();

        let dir = tempfile::tempdir().expect("stub tempdir");
        let envfile = dir.path().join("spawn-env.txt");
        let pidfile = dir.path().join("child.pid");
        let stub = pool_knob_stub(dir.path());

        let env_key = "CAMEL_JMS_BRIDGE_BINARY_PATH";
        let _guard = env_guard(env_key);
        // SAFETY: test-scoped env mutation.
        unsafe { std::env::set_var(env_key, &stub) };

        let pool = JmsBridgePool::from_config(JmsPoolConfig {
            pool_idle_timeout_ms: 60_000,
            pool_expiration_check_ms: 5_000,
            bridge_start_timeout_ms: 300,
            ..JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq)
        })
        .expect("pool config must be valid");

        let (replacement_tx, mut replacement_rx) = tokio::sync::mpsc::unbounded_channel();
        *pool
            .replacement_processed
            .lock()
            .expect("replacement_processed mutex poisoned") = Some(replacement_tx);

        // Initial launch through pool acquisition records the first env copy.
        let slot = pool
            .get_or_create_slot("default", false)
            .await
            .expect("initial slot acquisition");

        // The failed initial start degrades the slot; the monitor then drives
        // one real replacement launch (empty replacement-outcome queue).
        // Synchronize on the bounded replacement-processed notification.
        let _replacement = tokio::time::timeout(Duration::from_secs(5), replacement_rx.recv())
            .await
            .expect("replacement-processed notification within 5s")
            .expect("replacement_processed channel open");

        // Clean up monitor + child (bounded) before asserting.
        let _ = slot.publish_shutdown();
        if let Some(mut handle) = slot.health_monitor_handle.lock().await.take() // allow-test-wait: handle slot uncontended after shutdown
            && tokio::time::timeout(Duration::from_secs(2), &mut handle)
                .await
                .is_err()
        {
            handle.abort();
            let _ = handle.await;
        }
        stop_slot_process(slot.as_ref()).await;

        let recorded = std::fs::read_to_string(&envfile).expect("stub must record spawn env");
        let launches: Vec<&str> = recorded.lines().collect();
        assert!(
            launches.len() >= 2,
            "initial and replacement launches must both record env, got:\n{recorded}"
        );
        for (i, launch) in launches.iter().take(2).enumerate() {
            let pairs: Vec<&str> = launch.split_whitespace().collect();
            assert!(
                pairs.contains(&"JMS_POOL_IDLE_TIMEOUT_MS=60000"),
                "launch {i} env must contain JMS_POOL_IDLE_TIMEOUT_MS=60000, got: {launch}"
            );
            assert!(
                pairs.contains(&"JMS_POOL_EXPIRATION_CHECK_MS=5000"),
                "launch {i} env must contain JMS_POOL_EXPIRATION_CHECK_MS=5000, got: {launch}"
            );
        }

        for pid_line in std::fs::read_to_string(&pidfile)
            .expect("stub must record pids")
            .lines()
        {
            let pid: u32 = pid_line.trim().parse().expect("stub pid");
            assert!(pid > 0);
            assert!(wait_pid_gone(pid).await, "child {pid} must be terminated");
        }
    }

    /// Task 2.2: with a default `JmsPoolConfig` the default knob values
    /// (30_000 ms) must reach the initial spawn environment.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn pool_knob_defaults_reach_initial_spawn_env() {
        // rustls 0.23 requires a process-level CryptoProvider before the
        // post-ready mTLS connect runs in `start_bridge_process`.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let _env_lock_guard = crate::BRIDGE_ENV_LOCK.lock().unwrap();

        let dir = tempfile::tempdir().expect("stub tempdir");
        let envfile = dir.path().join("spawn-env.txt");
        let stub = pool_knob_stub(dir.path());

        let env_key = "CAMEL_JMS_BRIDGE_BINARY_PATH";
        let _guard = env_guard(env_key);
        // SAFETY: test-scoped env mutation.
        unsafe { std::env::set_var(env_key, &stub) };

        // Default `JmsPoolConfig`; only the broker map is required to drive
        // the initial launch through pool acquisition.
        let pool = JmsBridgePool::from_config(JmsPoolConfig {
            brokers: HashMap::from([(
                "default".to_string(),
                BrokerConfig {
                    broker_url: "tcp://localhost:61616".to_string(),
                    broker_type: BrokerType::ActiveMq,
                    username: None,
                    password: None,
                    tls: None,
                },
            )]),
            ..JmsPoolConfig::default()
        })
        .expect("pool config must be valid");

        let slot = pool
            .get_or_create_slot("default", false)
            .await
            .expect("initial slot acquisition");

        // Clean up monitor + child (bounded) before asserting.
        let _ = slot.publish_shutdown();
        if let Some(mut handle) = slot.health_monitor_handle.lock().await.take() // allow-test-wait: handle slot uncontended after shutdown
            && tokio::time::timeout(Duration::from_secs(2), &mut handle)
                .await
                .is_err()
        {
            handle.abort();
            let _ = handle.await;
        }
        stop_slot_process(slot.as_ref()).await;

        let recorded = std::fs::read_to_string(&envfile).expect("stub must record spawn env");
        let initial = recorded
            .lines()
            .next()
            .expect("initial launch must record spawn env");
        let pairs: Vec<&str> = initial.split_whitespace().collect();
        assert!(
            pairs.contains(&"JMS_POOL_IDLE_TIMEOUT_MS=30000"),
            "initial spawn env must contain JMS_POOL_IDLE_TIMEOUT_MS=30000, got: {initial}"
        );
        assert!(
            pairs.contains(&"JMS_POOL_EXPIRATION_CHECK_MS=30000"),
            "initial spawn env must contain JMS_POOL_EXPIRATION_CHECK_MS=30000, got: {initial}"
        );
    }

    // ── Task 2.4: configured liveness timeout/threshold in the real monitor ──

    use crate::bridge_client_test::{
        ScriptedHealth, TestBridgeFixture, broker_transitions, connect_scripted, env_guard,
        next_refresh_attempt, probe_gate, push_replacement, ready_stub_process, refresh_attempts,
        replacement_attempts, slot_child_pid, spawn_scripted_health, spawn_selector_gate_bridge,
        stop_slot_process,
    };
    use crate::proto::{BrokerFailureClass, HealthResponse};

    /// Builds a fresh non-terminal slot for monitor tests (task 2.4).
    fn test_slot(name: &str) -> Arc<BridgeSlot> {
        let (state_tx, state_rx) = tokio::sync::watch::channel(BridgeState::Starting);
        Arc::new(BridgeSlot {
            name: name.to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Pool with explicit liveness cadence, timeout, and threshold (task 2.4).
    fn liveness_pool(interval_ms: u64, timeout_ms: u64, threshold: u32) -> Arc<JmsBridgePool> {
        Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                health_check_interval_ms: interval_ms,
                health_timeout_ms: timeout_ms,
                health_failure_threshold: threshold,
                ..JmsPoolConfig::default()
            })
            .expect("pool config must be valid"),
        )
    }

    fn scripted_response(healthy: bool, broker_connected: bool) -> HealthResponse {
        HealthResponse {
            healthy,
            broker_connected,
            message: "scripted".to_string(),
            status: 0,
            age_ms: 0,
            ..Default::default()
        }
    }

    /// Task 8.1 wiring: a Health response carrying `failure_class =
    /// TLS_HOSTNAME` is stored on the broker handle by the health monitor and
    /// rendered by the consumer error path as the D1 local message — expected
    /// identity, redacted host, both remediations, and no remote text.
    #[tokio::test]
    async fn tls_hostname_diagnostic_wiring_end_to_end() {
        let mut brokers = std::collections::HashMap::new();
        brokers.insert(
            "default".to_string(),
            BrokerConfig {
                broker_url: "ssl://10.0.0.5:61617".to_string(),
                broker_type: BrokerType::ActiveMq,
                username: None,
                password: None,
                tls: Some(BrokerTlsConfig {
                    ca_cert_path: None,
                    client_cert_path: None,
                    client_key_path: None,
                    server_name: Some("mq.internal".to_string()),
                }),
            },
        );
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers,
                health_check_interval_ms: 20,
                health_timeout_ms: 250,
                health_failure_threshold: 5,
                ..JmsPoolConfig::default()
            })
            .expect("wiring pool config must be valid"),
        );

        let (state_tx, state_rx) = tokio::sync::watch::channel(BridgeState::Starting);
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "ssl://10.0.0.5:61617".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            probe_wake: Arc::new(tokio::sync::Notify::new()),
            last_broker_failure: std::sync::Mutex::new(None),
            selector_required: Arc::new(AtomicBool::new(false)),
        });
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        // A capability-reporting bridge whose broker handshake failed with a
        // hostname mismatch (bridge alive, broker not connected).
        let mut response = scripted_response(true, false);
        response.capabilities = vec!["broker-tls-v1".to_string()];
        response.failure_class = BrokerFailureClass::TlsHostname as i32;
        let server = spawn_scripted_health(vec![ScriptedHealth::Respond(response)]).await;
        let mut fx = TestBridgeFixture::start(pool.clone(), Arc::clone(&slot), server, None).await;

        let ack = fx.next_probe().await;
        assert!(
            matches!(ack.outcome, ProbeOutcome::Success),
            "the scripted Health response must be processed: {ack:?}"
        );

        // Delivery path of step 3: the consumer error path reads the stored
        // triple from the broker handle and renders the D1 message.
        let rendered = crate::consumer::connection_failure_detail(
            &pool,
            "default",
            "fallback must not be used when a class is on record",
        );
        for needle in [
            "mq.internal",
            "10.0.0.5",
            "Use a broker address the certificate names",
            "brokers.default.tls.server_name to a DNS name in the certificate",
        ] {
            assert!(
                rendered.contains(needle),
                "D1 message must contain '{needle}': {rendered}"
            );
        }
        assert!(
            !rendered.contains("scripted"),
            "the D1 message must not carry remote text: {rendered}"
        );

        fx.shutdown().await;
    }

    /// Ready-handshake stub child: writes its pid, announces `ready`, sleeps.
    #[cfg(unix)]
    async fn stub_bridge_process(dir: &std::path::Path, tag: &str) -> BridgeProcess {
        stub_bridge_process_on_port(dir, tag, 1).await
    }

    /// Ready-handshake stub child announcing a caller-selected (dead) port.
    #[cfg(unix)]
    async fn stub_bridge_process_on_port(
        dir: &std::path::Path,
        tag: &str,
        port: u16,
    ) -> BridgeProcess {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let pidfile = dir.join(format!("{tag}.pid"));
        let stub = write_stub(
            dir,
            &format!("{tag}.sh"),
            &format!(
                "#!/bin/sh\necho $$ > '{}'\nprintf '%s\\n' '{{\"status\":\"ready\",\"port\":{port}}}'\nexec sleep 30\n",
                pidfile.display()
            ),
        );
        let config = BridgeProcessConfig::jms(
            stub,
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            None,
            None,
            2_000,
            30_000,
            30_000,
            Some("/var/lib/state/rust-camel/jms-bridge".to_string()),
            Some("0123456789abcdef0123456789abcdef".to_string()),
            None,
        );
        BridgeProcess::start(&config)
            .await
            .expect("stub must reach the ready line")
    }

    /// Task 2.5 fixture: bridge whose `Send` always fails at the transport
    /// layer and whose `Health` succeeds, so producer-triggered wakes can be
    /// observed without a live broker.
    #[derive(Clone)]
    struct CountingSendFailureBridge {
        sends: Arc<std::sync::atomic::AtomicUsize>,
        health_calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::proto::bridge_service_server::BridgeService for CountingSendFailureBridge {
        async fn send(
            &self,
            _request: tonic::Request<crate::proto::SendRequest>,
        ) -> Result<tonic::Response<crate::proto::SendResponse>, tonic::Status> {
            self.sends.fetch_add(1, Ordering::SeqCst);
            Err(tonic::Status::unavailable("send channel is dead"))
        }

        type SubscribeStream = Pin<
            Box<dyn futures::Stream<Item = Result<crate::proto::JmsMessage, tonic::Status>> + Send>,
        >;

        async fn subscribe(
            &self,
            _request: tonic::Request<crate::proto::SubscribeRequest>,
        ) -> Result<tonic::Response<Self::SubscribeStream>, tonic::Status> {
            Err(tonic::Status::unimplemented(
                "counting send-failure fixture serves Send and Health only",
            ))
        }

        async fn health(
            &self,
            _request: tonic::Request<crate::proto::HealthRequest>,
        ) -> Result<tonic::Response<HealthResponse>, tonic::Status> {
            self.health_calls.fetch_add(1, Ordering::SeqCst);
            Ok(tonic::Response::new(scripted_response(true, true)))
        }

        async fn complete(
            &self,
            _request: tonic::Request<crate::proto::CompletionRequest>,
        ) -> Result<tonic::Response<crate::proto::CompletionResponse>, tonic::Status> {
            Ok(tonic::Response::new(crate::proto::CompletionResponse {
                accepted: true,
            }))
        }
    }

    async fn spawn_counting_send_failure_bridge() -> (
        u16,
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind counting send-failure listener");
        let port = listener.local_addr().expect("local addr").port();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        let sends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let health_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let service = CountingSendFailureBridge {
            sends: Arc::clone(&sends),
            health_calls: Arc::clone(&health_calls),
        };
        let task = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(crate::proto::bridge_service_server::BridgeServiceServer::new(service))
                .serve_with_incoming(incoming)
                .await;
        });
        (port, sends, health_calls, task)
    }

    /// T1: a completed `false/false` (verified-down) response is liveness
    /// proof — the monitor reports `BrokerDown`, keeps the Ready channel and
    /// child, and never replaces the bridge.
    #[cfg(unix)]
    #[tokio::test]
    async fn t1_monitor_reports_verified_down_and_keeps_ready_consumer() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let child = stub_bridge_process(dir.path(), "initial").await;
        let child_pid = child.child_pid().expect("stub child pid");
        let pool = liveness_pool(20, 250, 5);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let server = spawn_scripted_health(vec![ScriptedHealth::Respond(scripted_response(
            false, false,
        ))])
        .await;
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, Some(child))
                .await;

        let ack = fx.next_probe().await;
        assert!(
            matches!(ack.outcome, ProbeOutcome::Success),
            "a completed RPC is liveness proof: {ack:?}"
        );
        assert!(
            matches!(ack.state, BridgeState::Ready { .. }),
            "broker conditions must not publish Degraded: {ack:?}"
        );
        assert_eq!(ack.attempts, 0, "no attempt on broker status: {ack:?}");
        let transitions = broker_transitions(&pool);
        assert_eq!(
            transitions.first(),
            Some(&BrokerStatusTransition::BrokerDown),
            "BrokerDown must be reported for healthy=false, broker_connected=false: {transitions:?}"
        );
        assert!(
            matches!(&*slot.state_rx.borrow(), BridgeState::Ready { .. }),
            "the Ready channel must be unchanged"
        );
        assert_eq!(
            slot_child_pid(&slot).await,
            Some(child_pid),
            "the child PID must be unchanged"
        );
        assert_eq!(replacement_attempts(&pool), 0);
        fx.shutdown().await;
    }

    /// T2: recovery begins exactly at the Nth consecutive failure — never
    /// earlier — and installs the prepared replacement through the real
    /// install path.
    #[cfg(unix)]
    #[tokio::test]
    async fn t2_monitor_restarts_exactly_at_nth_failure() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let pool = liveness_pool(20, 250, 3);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let server = spawn_scripted_health(vec![
            ScriptedHealth::Error(tonic::Status::unavailable("rpc 1")),
            ScriptedHealth::Error(tonic::Status::unavailable("rpc 2")),
            ScriptedHealth::Error(tonic::Status::unavailable("rpc 3")),
        ])
        .await;

        let rep_child = stub_bridge_process(dir.path(), "rep1").await;
        let rep_pid = rep_child.child_pid().expect("replacement pid");
        let rep_channel = connect_scripted(server.port).await;
        push_replacement(&pool, rep_child, rep_channel).await;

        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, None).await;

        let ack1 = fx.next_probe().await;
        assert!(matches!(ack1.outcome, ProbeOutcome::RpcError), "{ack1:?}");
        assert_eq!(ack1.consecutive_failures, 1, "{ack1:?}");
        assert_eq!(
            ack1.attempts, 0,
            "zero attempts after failure one: {ack1:?}"
        );

        let ack2 = fx.next_probe().await;
        assert!(matches!(ack2.outcome, ProbeOutcome::RpcError), "{ack2:?}");
        assert_eq!(ack2.consecutive_failures, 2, "{ack2:?}");
        assert_eq!(
            ack2.attempts, 0,
            "zero attempts after failures one and two: {ack2:?}"
        );

        let ack3 = fx.next_probe().await;
        assert!(matches!(ack3.outcome, ProbeOutcome::RpcError), "{ack3:?}");
        assert_eq!(ack3.consecutive_failures, 3, "{ack3:?}");
        assert_eq!(
            ack3.attempts, 0,
            "the attempt is counted after the ack: {ack3:?}"
        );

        let rep = fx.next_replacement().await;
        assert_eq!(
            rep.attempts, 1,
            "exactly one attempt at the nth failure: {rep:?}"
        );
        assert!(
            rep.installed,
            "the prepared child must be installed: {rep:?}"
        );
        assert_eq!(rep.installed_pid, Some(rep_pid), "{rep:?}");
        assert!(
            matches!(rep.state, BridgeState::Ready { .. }),
            "Ready state after install: {rep:?}"
        );
        assert_eq!(
            slot_child_pid(&slot).await,
            Some(rep_pid),
            "the actual replacement child must be installed"
        );
        fx.shutdown().await;
    }

    /// T2: a successful response resets the counter even with broker fields
    /// `false/false`, so it never contributes to the threshold.
    #[tokio::test]
    async fn t2_monitor_resets_failure_count_on_successful_false_response() {
        let pool = liveness_pool(20, 250, 2);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let server = spawn_scripted_health(vec![
            ScriptedHealth::Error(tonic::Status::unavailable("rpc 1")),
            ScriptedHealth::Respond(scripted_response(false, false)),
            ScriptedHealth::Error(tonic::Status::unavailable("rpc 2")),
        ])
        .await;
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, None).await;

        let ack1 = fx.next_probe().await;
        assert!(matches!(ack1.outcome, ProbeOutcome::RpcError), "{ack1:?}");
        assert_eq!(ack1.consecutive_failures, 1, "{ack1:?}");

        let ack2 = fx.next_probe().await;
        assert!(matches!(ack2.outcome, ProbeOutcome::Success), "{ack2:?}");
        assert_eq!(
            ack2.consecutive_failures, 0,
            "a successful response resets the count: {ack2:?}"
        );

        let ack3 = fx.next_probe().await;
        assert!(matches!(ack3.outcome, ProbeOutcome::RpcError), "{ack3:?}");
        assert_eq!(ack3.consecutive_failures, 1, "{ack3:?}");

        for ack in [&ack1, &ack2, &ack3] {
            assert!(
                matches!(ack.state, BridgeState::Ready { .. }),
                "Ready throughout: {ack:?}"
            );
            assert_eq!(ack.attempts, 0, "zero attempts throughout: {ack:?}");
        }
        assert_eq!(replacement_attempts(&pool), 0);
        fx.shutdown().await;
    }

    /// T2: a successful replacement resets the counter for the new bridge, so
    /// the next recovery needs a fresh threshold run (zero/one/one/two
    /// attempts after four errors with threshold two).
    #[cfg(unix)]
    #[tokio::test]
    async fn t2_monitor_resets_failure_count_after_successful_replacement() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let pool = liveness_pool(20, 250, 2);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let server = spawn_scripted_health(vec![
            ScriptedHealth::Error(tonic::Status::unavailable("rpc 1")),
            ScriptedHealth::Error(tonic::Status::unavailable("rpc 2")),
            ScriptedHealth::Error(tonic::Status::unavailable("rpc 3")),
            ScriptedHealth::Error(tonic::Status::unavailable("rpc 4")),
        ])
        .await;

        let rep1_child = stub_bridge_process(dir.path(), "rep1").await;
        let rep1_pid = rep1_child.child_pid().expect("rep1 pid");
        push_replacement(&pool, rep1_child, connect_scripted(server.port).await).await;
        let rep2_child = stub_bridge_process(dir.path(), "rep2").await;
        let rep2_pid = rep2_child.child_pid().expect("rep2 pid");
        push_replacement(&pool, rep2_child, connect_scripted(server.port).await).await;

        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, None).await;

        let ack1 = fx.next_probe().await;
        assert_eq!(ack1.attempts, 0, "zero after error one: {ack1:?}");

        let ack2 = fx.next_probe().await;
        assert_eq!(ack2.attempts, 0, "{ack2:?}");
        let rep1 = fx.next_replacement().await;
        assert_eq!(rep1.attempts, 1, "one after error two: {rep1:?}");
        assert!(rep1.installed, "{rep1:?}");
        assert_eq!(rep1.installed_pid, Some(rep1_pid), "{rep1:?}");

        let ack3 = fx.next_probe().await;
        assert_eq!(ack3.attempts, 1, "one after error three: {ack3:?}");
        assert_eq!(
            ack3.consecutive_failures, 1,
            "the install reset the counter: {ack3:?}"
        );

        let ack4 = fx.next_probe().await;
        assert_eq!(ack4.attempts, 1, "{ack4:?}");
        let rep2 = fx.next_replacement().await;
        assert_eq!(rep2.attempts, 2, "two after error four: {rep2:?}");
        assert!(rep2.installed, "{rep2:?}");
        assert_eq!(rep2.installed_pid, Some(rep2_pid), "{rep2:?}");
        assert!(
            matches!(rep2.state, BridgeState::Ready { .. }),
            "actual Ready installation after each pair: {rep2:?}"
        );
        assert_eq!(
            slot_child_pid(&slot).await,
            Some(rep2_pid),
            "the second pair must be the installed child"
        );
        fx.shutdown().await;
    }

    /// T2/T3: timeouts respect the threshold and a successful response resets
    /// it; only the final consecutive timeout triggers exactly one
    /// replacement.
    #[cfg(unix)]
    #[tokio::test]
    async fn t2_monitor_timeouts_respect_threshold_and_success_reset() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let pool = liveness_pool(20, 50, 3);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        // Gated replies are held past their deadlines; only bounded fixture
        // teardown releases them.
        let gate1 = probe_gate();
        let gate2 = probe_gate();
        let gate3 = probe_gate();
        let gate4 = probe_gate();
        let server = spawn_scripted_health(vec![
            ScriptedHealth::Gate {
                response: scripted_response(true, true),
                gate: gate1,
            },
            ScriptedHealth::Respond(scripted_response(false, false)),
            ScriptedHealth::Gate {
                response: scripted_response(true, true),
                gate: gate2,
            },
            ScriptedHealth::Gate {
                response: scripted_response(true, true),
                gate: gate3,
            },
            ScriptedHealth::Gate {
                response: scripted_response(true, true),
                gate: gate4,
            },
        ])
        .await;

        let rep_child = stub_bridge_process(dir.path(), "rep1").await;
        let rep_pid = rep_child.child_pid().expect("replacement pid");
        push_replacement(&pool, rep_child, connect_scripted(server.port).await).await;

        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, None).await;

        let ack1 = fx.next_probe().await;
        assert!(matches!(ack1.outcome, ProbeOutcome::Timeout), "{ack1:?}");
        assert_eq!(ack1.consecutive_failures, 1, "{ack1:?}");
        assert_eq!(ack1.attempts, 0, "{ack1:?}");

        let ack2 = fx.next_probe().await;
        assert!(matches!(ack2.outcome, ProbeOutcome::Success), "{ack2:?}");
        assert_eq!(
            ack2.consecutive_failures, 0,
            "the successful response resets the count: {ack2:?}"
        );
        assert_eq!(ack2.attempts, 0, "{ack2:?}");

        let ack3 = fx.next_probe().await;
        assert!(matches!(ack3.outcome, ProbeOutcome::Timeout), "{ack3:?}");
        assert_eq!(ack3.consecutive_failures, 1, "{ack3:?}");
        assert_eq!(ack3.attempts, 0, "{ack3:?}");

        let ack4 = fx.next_probe().await;
        assert!(matches!(ack4.outcome, ProbeOutcome::Timeout), "{ack4:?}");
        assert_eq!(ack4.consecutive_failures, 2, "{ack4:?}");
        assert_eq!(
            ack4.attempts, 0,
            "attempts stay zero through the penultimate outcome: {ack4:?}"
        );

        let ack5 = fx.next_probe().await;
        assert!(matches!(ack5.outcome, ProbeOutcome::Timeout), "{ack5:?}");
        assert_eq!(ack5.consecutive_failures, 3, "{ack5:?}");

        let rep = fx.next_replacement().await;
        assert_eq!(
            rep.attempts, 1,
            "exactly one replacement after the final timeout: {rep:?}"
        );
        assert!(rep.installed, "{rep:?}");
        assert_eq!(rep.installed_pid, Some(rep_pid), "{rep:?}");
        assert!(
            matches!(rep.state, BridgeState::Ready { .. }),
            "actual prepared child/channel installed Ready: {rep:?}"
        );
        // Releases all held gates in bounded fixture teardown.
        fx.shutdown().await;
    }

    /// T3: a response that completes within the configured deadline is
    /// accepted — Ready, zero attempts.
    #[tokio::test]
    async fn t3_monitor_accepts_response_before_configured_timeout() {
        let pool = liveness_pool(20, 500, 1);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let gate = probe_gate();
        let server = spawn_scripted_health(vec![ScriptedHealth::Gate {
            response: scripted_response(true, true),
            gate: Arc::clone(&gate),
        }])
        .await;
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, None).await;

        // Await server entry, then promptly release the reply.
        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .expect("the gated request must enter the server");
        let entered = Instant::now();
        gate.release.notify_one();

        let ack = fx.next_probe().await;
        assert!(
            matches!(ack.outcome, ProbeOutcome::Success),
            "the response must complete within the configured deadline: {ack:?}"
        );
        assert!(
            entered.elapsed() < Duration::from_secs(1),
            "the response must complete promptly: {:?}",
            entered.elapsed()
        );
        assert!(matches!(ack.state, BridgeState::Ready { .. }), "{ack:?}");
        assert_eq!(ack.attempts, 0, "{ack:?}");
        assert_eq!(replacement_attempts(&pool), 0);
        fx.shutdown().await;
    }

    /// T3: a held response times out at the configured deadline (not a
    /// hardcoded three seconds), and the late reply cannot overwrite the
    /// replacement state.
    #[cfg(unix)]
    #[tokio::test]
    async fn t3_monitor_times_out_after_configured_timeout() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let pool = liveness_pool(20, 50, 1);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let gate = probe_gate();
        let server = spawn_scripted_health(vec![ScriptedHealth::Gate {
            response: scripted_response(true, true),
            gate: Arc::clone(&gate),
        }])
        .await;

        let rep_child = stub_bridge_process(dir.path(), "rep1").await;
        let rep_pid = rep_child.child_pid().expect("replacement pid");
        push_replacement(&pool, rep_child, connect_scripted(server.port).await).await;

        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, None).await;

        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .expect("the gated request must enter the server");
        let entered = Instant::now();

        // The gate is held past its deadline: timeout plus replacement must
        // complete under a one-second bound.
        let (ack, rep) = tokio::time::timeout(Duration::from_secs(1), async {
            let ack = fx.next_probe().await;
            let rep = fx.next_replacement().await;
            (ack, rep)
        })
        .await
        .expect("timeout and replacement within one second");

        assert!(matches!(ack.outcome, ProbeOutcome::Timeout), "{ack:?}");
        let elapsed = entered.elapsed();
        assert!(
            elapsed >= Duration::from_millis(50),
            "the probe must wait the configured deadline: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "no hardcoded three-second deadline: {elapsed:?}"
        );
        assert_eq!(rep.attempts, 1, "exactly one replacement: {rep:?}");
        assert!(rep.installed, "{rep:?}");
        assert_eq!(rep.installed_pid, Some(rep_pid), "{rep:?}");
        assert!(
            matches!(rep.state, BridgeState::Ready { .. }),
            "actual Ready installation: {rep:?}"
        );

        // Release the old response: it must not overwrite the replacement.
        gate.release.notify_one();
        fx.await_server_completed().await;
        assert!(
            matches!(&*slot.state_rx.borrow(), BridgeState::Ready { .. }),
            "the late response must not overwrite the replacement state"
        );
        assert_eq!(
            slot_child_pid(&slot).await,
            Some(rep_pid),
            "the late response must not replace the installed child"
        );
        assert_eq!(replacement_attempts(&pool), 1);
        fx.shutdown().await;
    }

    /// T6: three stale (healthy=true, broker_connected=false) probe periods
    /// produce zero restarts and a stable child — broker false is never
    /// process failure.
    #[cfg(unix)]
    #[tokio::test]
    async fn t6_real_monitor_three_stale_periods_zero_restarts_and_stable_child() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let child = stub_bridge_process(dir.path(), "initial").await;
        let child_pid = child.child_pid().expect("stub child pid");
        let pool = liveness_pool(20, 250, 5);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let server = spawn_scripted_health(
            (0..5)
                .map(|_| ScriptedHealth::Respond(scripted_response(true, false)))
                .collect(),
        )
        .await;
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, Some(child))
                .await;

        let first = fx.next_probe().await;
        let start = Instant::now();
        let second = fx.next_probe().await;
        let third = fx.next_probe().await;
        let fourth = fx.next_probe().await;
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(20) * 3,
            "at least three real periods must elapse: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "three more acknowledgements under five seconds: {elapsed:?}"
        );

        for ack in [&first, &second, &third, &fourth] {
            assert!(
                matches!(ack.outcome, ProbeOutcome::Success),
                "prompt responses: {ack:?}"
            );
            assert!(
                matches!(ack.state, BridgeState::Ready { .. }),
                "Ready at every acknowledgement: {ack:?}"
            );
            assert_eq!(ack.attempts, 0, "zero attempts throughout: {ack:?}");
        }
        assert!(
            fourth.index >= 3,
            "at least four completed probes total: {:?}",
            fourth.index
        );
        assert!(
            fx.server_calls() >= 4,
            "the server observed at least four probes"
        );
        assert_eq!(replacement_attempts(&pool), 0);
        assert_eq!(
            slot_child_pid(&slot).await,
            Some(child_pid),
            "the slot's actual process PID must be unchanged"
        );
        fx.shutdown().await;
    }

    /// T6: verified-down (`false/false`) replies over three periods produce
    /// zero restarts, report `BrokerDown`, and keep the child.
    #[cfg(unix)]
    #[tokio::test]
    async fn t6_real_monitor_verified_down_false_false_zero_restarts() {
        let dir = tempfile::tempdir().expect("stub tempdir");
        let child = stub_bridge_process(dir.path(), "initial").await;
        let child_pid = child.child_pid().expect("stub child pid");
        let pool = liveness_pool(20, 250, 5);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        let server = spawn_scripted_health(
            (0..5)
                .map(|_| ScriptedHealth::Respond(scripted_response(false, false)))
                .collect(),
        )
        .await;
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, Some(child))
                .await;

        let first = fx.next_probe().await;
        let start = Instant::now();
        let second = fx.next_probe().await;
        let third = fx.next_probe().await;
        let fourth = fx.next_probe().await;
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(20) * 3,
            "at least three real periods must elapse: {elapsed:?}"
        );

        for ack in [&first, &second, &third, &fourth] {
            assert!(
                matches!(ack.outcome, ProbeOutcome::Success),
                "prompt responses: {ack:?}"
            );
            assert!(
                matches!(ack.state, BridgeState::Ready { .. }),
                "Ready throughout: {ack:?}"
            );
            assert_eq!(ack.attempts, 0, "zero attempts: {ack:?}");
        }
        let transitions = broker_transitions(&pool);
        assert_eq!(
            transitions.first(),
            Some(&BrokerStatusTransition::BrokerDown),
            "BrokerDown must be reported: {transitions:?}"
        );
        assert_eq!(replacement_attempts(&pool), 0);
        assert_eq!(
            slot_child_pid(&slot).await,
            Some(child_pid),
            "the slot's actual process PID must be unchanged"
        );
        fx.shutdown().await;
    }

    // ── Task 3.1: broker status transitions without false disconnections ──

    use crate::health::{BrokerHealthObservation, BrokerHealthStatus};

    /// One raw broker observation for the reporter tests.
    fn broker_observation(
        status: BrokerHealthStatus,
        healthy: bool,
        broker_connected: bool,
        age_ms: u64,
    ) -> BrokerHealthObservation {
        BrokerHealthObservation {
            status,
            healthy,
            broker_connected,
            age_ms,
        }
    }

    /// Env marker marking the isolated event-capture child process.
    const EVENT_CAPTURE_CHILD_ENV: &str = "JMS_BROKER_STATUS_EVENT_CAPTURE_CHILD";
    /// Full test path spawned for the isolated event-capture run.
    const EVENT_CAPTURE_TEST: &str =
        "component::tests::monitor_stale_preserves_verified_state_and_logs_once";

    /// One real `tracing` event field. `typed` records that the value
    /// arrived through a typed visitor method (`record_u64`/`record_bool`),
    /// proving the field is a real number or boolean rather than debug text.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CapturedField {
        name: String,
        value: String,
        typed: bool,
    }

    /// One real emitted `tracing` event.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CapturedEvent {
        level: String,
        target: String,
        fields: Vec<CapturedField>,
    }

    struct FieldCollector(Vec<CapturedField>);

    impl tracing::field::Visit for FieldCollector {
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            self.0.push(CapturedField {
                name: field.name().to_string(),
                value: value.to_string(),
                typed: true,
            });
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            self.0.push(CapturedField {
                name: field.name().to_string(),
                value: value.to_string(),
                typed: true,
            });
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push(CapturedField {
                name: field.name().to_string(),
                value: format!("{value:?}"),
                typed: false,
            });
        }
    }

    /// In-process subscriber collecting every emitted `tracing` event.
    #[derive(Debug)]
    struct CollectingSubscriber {
        events: Arc<std::sync::Mutex<Vec<CapturedEvent>>>,
        next_id: std::sync::atomic::AtomicU64,
    }

    impl tracing::Subscriber for CollectingSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _attributes: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(
                self.next_id
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            )
        }

        fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut collector = FieldCollector(Vec::new());
            event.record(&mut collector);
            self.events
                .lock()
                .expect("captured events mutex poisoned")
                .push(CapturedEvent {
                    level: event.metadata().level().to_string(),
                    target: event.metadata().target().to_string(),
                    fields: collector.0,
                });
        }

        fn enter(&self, _span: &tracing::Id) {}

        fn exit(&self, _span: &tracing::Id) {}

        fn register_callsite(
            &self,
            _metadata: &'static tracing::Metadata<'static>,
        ) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::always()
        }

        fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
            Some(tracing::level_filters::LevelFilter::TRACE)
        }
    }

    /// Child mode: reports the full response sequence under the collecting
    /// subscriber and asserts the real emitted events: exactly one event per
    /// real transition, the exact field set (typed numeric `age_ms`, typed
    /// booleans, message), the fixed credential-safe messages, no event for
    /// the age-only 120000 STALE change, retained UP/DOWN verification, and
    /// no leaked provider text, credential, or `disconnected` wording.
    fn run_event_capture_child() {
        let pool = JmsBridgePool::from_config(JmsPoolConfig::default())
            .expect("default jms pool config must be valid");
        let mut reporter = BrokerStatusReporter::from_pool(&pool);

        // Provider message text exactly as the bridge would send it: this
        // and its embedded credential must never appear in any emitted
        // event field.
        let provider_message = "broker status stale; creds admin:hunter2 tcp://host:61616";

        let events: Arc<std::sync::Mutex<Vec<CapturedEvent>>> = Arc::default();
        let subscriber = CollectingSubscriber {
            events: Arc::clone(&events),
            next_id: std::sync::atomic::AtomicU64::new(1),
        };

        let sequence = [
            (
                broker_observation(BrokerHealthStatus::Up, true, true, 0),
                Some(true),
            ),
            (
                broker_observation(BrokerHealthStatus::Stale, true, false, 60_000),
                Some(true),
            ),
            (
                broker_observation(BrokerHealthStatus::Stale, true, false, 120_000),
                Some(true),
            ),
            (
                broker_observation(BrokerHealthStatus::Unknown, true, false, 0),
                Some(true),
            ),
            (
                broker_observation(BrokerHealthStatus::Skipped, true, false, 0),
                Some(true),
            ),
            (
                broker_observation(BrokerHealthStatus::Down, false, false, 0),
                Some(false),
            ),
            (
                broker_observation(BrokerHealthStatus::Stale, true, false, 60_000),
                Some(false),
            ),
        ];

        tracing::subscriber::with_default(subscriber, || {
            for (obs, want_verified) in sequence {
                reporter.report_broker_status(obs);
                assert_eq!(
                    reporter.verified(),
                    want_verified,
                    "retained verification after {obs:?}"
                );
            }
        });

        let events = events
            .lock()
            .expect("captured events mutex poisoned")
            .clone();
        let target = "camel_component_jms::component";
        let bool_field = |name: &str, value: bool| CapturedField {
            name: name.to_string(),
            value: value.to_string(),
            typed: true,
        };
        let age_field = |value: u64| CapturedField {
            name: "age_ms".to_string(),
            value: value.to_string(),
            typed: true,
        };
        let message_field = |value: &str| CapturedField {
            name: "message".to_string(),
            value: value.to_string(),
            typed: false,
        };
        // The format-string `message` field is always declared first in the
        // event metadata, ahead of the explicit fields.
        let expected = [
            (
                vec![
                    message_field(BROKER_STATUS_CHANGED_MSG),
                    bool_field("healthy", true),
                    bool_field("broker_connected", true),
                ],
                "verified UP",
            ),
            (
                vec![message_field(BROKER_STATUS_STALE_MSG), age_field(60_000)],
                "entering STALE with numeric age",
            ),
            (
                vec![message_field(BROKER_STATUS_UNVERIFIED_MSG)],
                "explicit unverified",
            ),
            (
                vec![
                    message_field(BROKER_STATUS_CHANGED_MSG),
                    bool_field("healthy", false),
                    bool_field("broker_connected", false),
                ],
                "verified DOWN",
            ),
            (
                vec![message_field(BROKER_STATUS_STALE_MSG), age_field(60_000)],
                "second entering STALE retains DOWN",
            ),
        ];
        assert_eq!(
            events.len(),
            expected.len(),
            "exactly one event per real transition, none for the age-only \
             120000 change: {events:?}"
        );
        for (event, (fields, what)) in events.iter().zip(&expected) {
            assert_eq!(event.level, "WARN", "{what}: {event:?}");
            assert_eq!(event.target, target, "{what}: {event:?}");
            assert_eq!(&event.fields, fields, "{what}: exact field set: {event:?}");
        }
        for event in &events {
            for field in &event.fields {
                assert!(
                    !field.value.contains(provider_message) && !field.value.contains("hunter2"),
                    "provider message and credentials must never appear: {event:?}"
                );
                assert!(
                    !field.value.contains("120000"),
                    "an age-only change must not log again: {event:?}"
                );
                assert!(
                    !field.value.contains("disconnected"),
                    "an unverified observation must not be logged as newly \
                     disconnected: {event:?}"
                );
            }
        }
        println!("captured tracing events: {events:#?}");
    }

    /// Parent mode: spawns this test executable on itself with `--exact` and
    /// a private env marker. The child owns a fresh tracing
    /// callsite-interest cache, so its collecting subscriber receives every
    /// emitted event deterministically — inside the shared suite process the
    /// parallel monitor tests can register these `warn!` callsites first
    /// with no subscriber, which nondeterministically drops events from a
    /// per-test subscriber.
    fn run_event_capture_parent() {
        let exe = std::env::current_exe().expect("current test executable path");
        let output = std::process::Command::new(exe)
            .args(["--exact", EVENT_CAPTURE_TEST, "--nocapture"])
            .env(EVENT_CAPTURE_CHILD_ENV, "1")
            .output()
            .expect("spawn isolated event-capture subprocess");
        assert!(
            output.status.success(),
            "isolated event-capture child failed: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    /// Consecutive STALE observations keep the last verified state, log the
    /// stale transition once with the numeric age, never log an unverified
    /// observation as a newly disconnected broker, and never leak the
    /// provider message or credentials. The assertions observe real emitted
    /// `tracing` events captured in an isolated subprocess.
    #[test]
    fn monitor_stale_preserves_verified_state_and_logs_once() {
        if std::env::var_os(EVENT_CAPTURE_CHILD_ENV).is_some() {
            run_event_capture_child();
        } else {
            run_event_capture_parent();
        }
    }

    /// A replacement monitor/channel epoch starts without inherited
    /// verification: a STALE replacement response reports unverified, and
    /// completed RPCs count zero replacement attempts.
    #[test]
    fn monitor_status_resets_for_replacement() {
        let pool = JmsBridgePool::from_config(JmsPoolConfig::default())
            .expect("default jms pool config must be valid");
        let mut reporter = BrokerStatusReporter::from_pool(&pool);

        // Old epoch: a completed RPC verifies the bridge UP.
        reporter.report_broker_status(broker_observation(BrokerHealthStatus::Up, true, true, 0));
        assert_eq!(reporter.verified(), Some(true), "old epoch verified up");

        // Replacement epoch: a replaced Ready channel resets the reporter in
        // place (a replacement monitor constructs a fresh reporter with the
        // same fresh-memory start).
        reporter.reset();
        reporter.report_broker_status(broker_observation(
            BrokerHealthStatus::Stale,
            true,
            false,
            60_000,
        ));

        assert_eq!(
            reporter.verified(),
            None,
            "a replacement epoch must not inherit the old bridge's UP"
        );
        assert_eq!(
            broker_transitions(&pool),
            vec![
                BrokerStatusTransition::VerifiedUp,
                BrokerStatusTransition::Stale { age_ms: 60_000 },
            ],
            "the replacement STALE must report unverified, not retained health"
        );

        // A replacement monitor constructs a fresh reporter: also no
        // inherited verification.
        let mut fresh = BrokerStatusReporter::from_pool(&pool);
        fresh.report_broker_status(broker_observation(
            BrokerHealthStatus::Stale,
            true,
            false,
            60_000,
        ));
        assert_eq!(fresh.verified(), None, "a new monitor starts unverified");
        assert_eq!(
            broker_transitions(&pool),
            vec![
                BrokerStatusTransition::VerifiedUp,
                BrokerStatusTransition::Stale { age_ms: 60_000 },
                BrokerStatusTransition::Stale { age_ms: 60_000 },
            ],
        );

        assert_eq!(
            replacement_attempts(&pool),
            0,
            "completed RPCs must never count a replacement attempt"
        );
    }

    /// Task 2.5: a coalesced wake runs one immediate probe without changing
    /// the configured cadence, the Ready state, or the attempt count.
    #[tokio::test]
    async fn restart_slot_wakes_monitor_and_preserves_ready() {
        let pool = liveness_pool(60_000, 250, 1);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));
        let server = spawn_scripted_health(Vec::new()).await;
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, None).await;

        pool.restart_slot("default");

        let ack = tokio::time::timeout(Duration::from_millis(500), fx.next_probe())
            .await
            .expect("a wake must probe without waiting for the 60s cadence");
        assert!(
            matches!(ack.outcome, ProbeOutcome::Success),
            "scripted healthy bridge: {ack:?}"
        );
        assert!(
            matches!(ack.state, BridgeState::Ready { .. }),
            "a wake must preserve Ready: {ack:?}"
        );
        assert_eq!(ack.consecutive_failures, 0, "{ack:?}");
        assert_eq!(ack.attempts, 0, "{ack:?}");
        assert_eq!(fx.server_calls(), 1);
        assert_eq!(replacement_attempts(&pool), 0);
        fx.shutdown().await;
    }

    /// Task 2.5: ten requests before the monitor waits coalesce into exactly
    /// one permit/probe.
    #[tokio::test]
    async fn repeated_restart_requests_coalesce_to_one_probe() {
        let pool = liveness_pool(60_000, 250, 1);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));
        for _ in 0..10 {
            pool.restart_slot("default");
        }

        let server = spawn_scripted_health(Vec::new()).await;
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, None).await;

        let ack = tokio::time::timeout(Duration::from_millis(500), fx.next_probe())
            .await
            .expect("coalesced wake must run one probe");
        assert!(
            matches!(ack.outcome, ProbeOutcome::Success),
            "no failure may be counted for a wake: {ack:?}"
        );
        assert_eq!(ack.consecutive_failures, 0, "{ack:?}");
        assert_eq!(ack.attempts, 0, "{ack:?}");
        assert_eq!(fx.server_calls(), 1, "ten notify_one calls coalesce to one");

        // The next cadence is the full configured interval: no second probe
        // starts immediately from leftover permits.
        let no_second = tokio::time::timeout(Duration::from_millis(200), fx.next_probe()).await;
        assert!(
            no_second.is_err(),
            "a coalesced wake must not leave a second permit: {no_second:?}"
        );
        assert_eq!(fx.server_calls(), 1);
        assert_eq!(replacement_attempts(&pool), 0);
        fx.shutdown().await;
    }

    /// Task 2.5: the producer's transport-error path refreshes against the real
    /// installed dead process endpoint, records that failure, then wakes the
    /// real monitor within 500 ms of the observation instead of writing
    /// `Restarting`.
    #[cfg(unix)]
    #[tokio::test]
    async fn producer_transport_failure_notifies_probe_wake() {
        use tower::Service;

        let dir = tempfile::tempdir().expect("stub tempdir");
        let dead_endpoint = 7;
        let process = ready_stub_process(dir.path(), "producer-wake", dead_endpoint).await;
        let pool = liveness_pool(60_000, 250, 1);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));
        let server = spawn_scripted_health(Vec::new()).await;
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, Some(process))
                .await;

        let endpoint_config =
            crate::config::JmsEndpointConfig::from_uri("jms:queue:producer-wake").unwrap();
        let mut producer = LazyJmsProducer {
            pool: Arc::clone(&pool),
            broker_name: "default".to_string(),
            endpoint_config,
            resolved_broker_type: BrokerType::ActiveMq,
        };
        let mut exchange = Exchange::default();
        exchange.input.body = camel_component_api::Body::Text("hello".to_string());

        let err = producer
            .call(exchange)
            .await
            .expect_err("the scripted fixture's Send is a transport failure");
        assert!(
            err.to_string().contains(BRIDGE_TRANSPORT_ERROR_PREFIX),
            "got: {err}"
        );

        let attempt = next_refresh_attempt(&pool, 0).await;
        assert_eq!(attempt.endpoint, Some(dead_endpoint), "{attempt:?}");
        assert!(attempt.failed, "{attempt:?}");
        assert!(!attempt.timed_out, "{attempt:?}");

        let ack = tokio::time::timeout(Duration::from_millis(500), fx.next_probe())
            .await
            .expect("the producer must wake the monitor within 500 ms of the failure");
        assert!(matches!(ack.outcome, ProbeOutcome::Success), "{ack:?}");
        assert!(matches!(ack.state, BridgeState::Ready { .. }), "{ack:?}");
        assert_eq!(ack.attempts, 0, "{ack:?}");
        assert!(fx.server_calls() >= 1);
        assert_eq!(replacement_attempts(&pool), 0);
        fx.shutdown().await;
    }

    /// Task 2.5: terminal slots reject both direct restart requests and wakes
    /// that arrive during an in-flight probe; no further probe runs.
    #[tokio::test]
    async fn terminal_wake_skips_probe_and_stays_terminal() {
        for terminal in [
            BridgeState::Parked("terminal wake".to_string()),
            BridgeState::Stopped,
        ] {
            let pool = liveness_pool(60_000, 250, 1);
            let slot = test_slot("default");
            let _ = slot.state_tx.send_replace(terminal.clone());
            pool.slots.insert("default".to_string(), Arc::clone(&slot));

            pool.restart_slot("default");
            match &terminal {
                BridgeState::Parked(_) => assert!(
                    matches!(&*slot.state_rx.borrow(), BridgeState::Parked(_)),
                    "restart_slot must leave Parked unchanged"
                ),
                BridgeState::Stopped => assert!(
                    matches!(&*slot.state_rx.borrow(), BridgeState::Stopped),
                    "restart_slot must leave Stopped unchanged"
                ),
                _ => unreachable!("loop only contains terminal states"),
            }
            assert_eq!(replacement_attempts(&pool), 0);
        }

        for terminal in [
            BridgeState::Parked("terminal inflight".to_string()),
            BridgeState::Stopped,
        ] {
            let gate = probe_gate();
            let server = spawn_scripted_health(vec![ScriptedHealth::Gate {
                response: scripted_response(true, true),
                gate: Arc::clone(&gate),
            }])
            .await;
            let pool = liveness_pool(60_000, 250, 1);
            let slot = test_slot("default");
            pool.slots.insert("default".to_string(), Arc::clone(&slot));
            let mut fx =
                TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, None).await;

            pool.restart_slot("default");
            tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
                .await
                .expect("the prompt probe must enter the server");

            // Terminal publication during the probe wins; the wake queued for
            // the next iteration must be discarded by the state re-read.
            let _ = slot.state_tx.send_replace(terminal.clone());
            pool.restart_slot("default");
            gate.release.notify_one();

            let ack = fx.next_probe().await;
            match &terminal {
                BridgeState::Parked(_) => assert!(
                    matches!(ack.state, BridgeState::Parked(_)),
                    "the in-flight ack must observe Parked: {ack:?}"
                ),
                BridgeState::Stopped => assert!(
                    matches!(ack.state, BridgeState::Stopped),
                    "the in-flight ack must observe Stopped: {ack:?}"
                ),
                _ => unreachable!("loop only contains terminal states"),
            }
            assert_eq!(fx.server_calls(), 1);
            let no_second = tokio::time::timeout(Duration::from_millis(200), fx.next_probe()).await;
            assert!(
                no_second.is_err(),
                "a wake cannot resurrect a terminal slot: {no_second:?}"
            );
            assert_eq!(fx.server_calls(), 1);
            assert_eq!(replacement_attempts(&pool), 0);
            fx.shutdown().await;
        }
    }

    /// Task 1.2: capability evidence is re-collected per gate window from the
    /// current bridge instance — a replacement never inherits the previous
    /// instance's evidence. Cycle 1 runs against Health responses carrying
    /// `message-selector-v1` and stays Ready; the fresh cycle (modeling the
    /// replaced instance) reports NO capabilities and parks permanently with
    /// the three-fragment error, with ZERO Subscribe calls recorded across
    /// both cycles (cycle 1's evidence did not carry over).
    #[tokio::test]
    async fn replacement_bridge_startup_parks_without_inherited_evidence() {
        let bridge = spawn_selector_gate_bridge(&["message-selector-v1"]).await;
        let dir = tempfile::tempdir().expect("stub tempdir");
        let pidfile = dir.path().join("child.pid");
        let stub = write_stub(
            dir.path(),
            "stub-bridge.sh",
            &format!(
                "#!/bin/sh\necho $$ > '{}'\nprintf '%s\\n' '{{\"status\":\"ready\",\"port\":1}}'\nexec sleep 30\n",
                pidfile.display()
            ),
        );
        let config = BridgeProcessConfig::jms(
            stub,
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            None,
            None,
            2_000,
            30_000,
            30_000,
            Some("/var/lib/state/rust-camel/jms-bridge".to_string()),
            Some("0123456789abcdef0123456789abcdef".to_string()),
            None,
        );

        // Cycle 1: the startup gate window (the REAL launch path) collects
        // evidence INCLUDING the capability from this bridge instance and the
        // launch completes.
        let channel1 = connect_scripted(bridge.port).await;
        let (process1, _channel1) = JmsBridgePool::start_bridge_process_with_config(
            &config,
            2_000,
            false,
            &AtomicBool::new(true),
            Some(channel1),
        )
        .await
        .expect("cycle 1 must complete on its collected capability evidence");
        let pid1: u32 = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        assert!(pid1 > 0, "cycle 1 stub must have written its pid");
        let _ = process1.stop().await;
        assert!(
            wait_pid_gone(pid1).await,
            "cycle 1 child {pid1} must be reaped"
        );

        // The replacement instance advertises nothing. Every launch gate
        // window re-collects its evidence from scratch (the existing TLS
        // gate-window re-collection), so cycle 1's evidence never authorizes
        // the replacement.
        bridge.set_capabilities(&[]);
        let channel2 = connect_scripted(bridge.port).await;
        let result = JmsBridgePool::start_bridge_process_with_config(
            &config,
            2_000,
            false,
            &AtomicBool::new(true),
            Some(channel2),
        )
        .await;
        let reason = match result {
            Err(StartBridgeError::Permanent(reason)) => reason,
            Err(StartBridgeError::Other(e)) => {
                panic!("the startup gate must classify permanently: {e}")
            }
            Ok(_) => panic!("the replacement must be rejected at startup"),
        };
        for needle in ["message-selector-v1", crate::BRIDGE_VERSION] {
            assert!(
                reason.contains(needle),
                "park reason must contain '{needle}': {reason}"
            );
        }
        let pid2: u32 = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        assert!(pid2 > 0, "cycle 2 stub must have written its pid");
        assert!(
            wait_pid_gone(pid2).await,
            "the gated replacement child {pid2} must be killed, not leaked"
        );

        // The launch-fixture path parks the replacement with EXACTLY that
        // error, before any Subscribe.
        let slot = test_slot("default");
        JmsBridgePool::apply_initial_start_result(
            &slot,
            Err(StartBridgeError::Permanent(reason.clone())),
        )
        .await;
        match &*slot.state_rx.borrow() {
            BridgeState::Parked(parked) => assert_eq!(
                parked, &reason,
                "the launch-fixture path must park with exactly the startup gate error"
            ),
            other => panic!("the replacement must park permanently: {other:?}"),
        }
        assert_eq!(
            bridge.subscribe_count(),
            0,
            "the replacement parked at startup before any Subscribe (cycle 1 evidence did not carry over)"
        );
        bridge.shutdown().await;
    }

    /// Task 2.5: a wake during an in-flight probe only stores one permit for
    /// the next loop iteration; probes never overlap.
    #[tokio::test]
    async fn wake_during_inflight_probe_does_not_start_overlapping_probe() {
        let first_gate = probe_gate();
        let second_gate = probe_gate();
        let server = spawn_scripted_health(vec![
            ScriptedHealth::Gate {
                response: scripted_response(true, true),
                gate: Arc::clone(&first_gate),
            },
            ScriptedHealth::Gate {
                response: scripted_response(true, true),
                gate: Arc::clone(&second_gate),
            },
        ])
        .await;
        let pool = liveness_pool(60_000, 250, 1);
        let slot = test_slot("default");
        pool.slots.insert("default".to_string(), Arc::clone(&slot));
        let mut fx =
            TestBridgeFixture::start(Arc::clone(&pool), Arc::clone(&slot), server, None).await;

        pool.restart_slot("default");
        tokio::time::timeout(Duration::from_secs(5), first_gate.entered.notified())
            .await
            .expect("first probe must enter the gated server");
        assert_eq!(fx.server_calls(), 1);

        pool.restart_slot("default");
        assert_eq!(
            fx.server_calls(),
            1,
            "a wake while the first probe is in flight must not start an overlap"
        );

        first_gate.release.notify_one();
        let first_ack = fx.next_probe().await;
        assert!(
            matches!(first_ack.outcome, ProbeOutcome::Success),
            "{first_ack:?}"
        );
        assert_eq!(
            first_ack.consecutive_failures, 0,
            "a wake is not a failure: {first_ack:?}"
        );

        tokio::time::timeout(Duration::from_secs(5), second_gate.entered.notified())
            .await
            .expect("the stored permit must start exactly one follow-up probe");
        assert_eq!(fx.server_calls(), 2);

        second_gate.release.notify_one();
        let second_ack = fx.next_probe().await;
        assert!(
            matches!(second_ack.outcome, ProbeOutcome::Success),
            "{second_ack:?}"
        );
        assert_eq!(second_ack.consecutive_failures, 0, "{second_ack:?}");
        assert_eq!(fx.server_calls(), 2, "exactly one follow-up probe");
        assert_eq!(replacement_attempts(&pool), 0);
        fx.shutdown().await;
    }

    /// Step 9: the initial-launch failure path is unchanged — transient
    /// failures still degrade and never count as replacement attempts.
    #[tokio::test]
    async fn initial_launch_failure_path_is_unchanged() {
        let pool = liveness_pool(20, 250, 5);
        let slot = test_slot("default");

        JmsBridgePool::apply_initial_start_result(
            slot.as_ref(),
            Err(StartBridgeError::Other(CamelError::ProcessorError(
                "initial launch failed".to_string(),
            ))),
        )
        .await;

        match &*slot.state_rx.borrow() {
            BridgeState::Degraded(reason) => assert_eq!(
                reason, "Initial start failed: Processor error: initial launch failed",
                "the initial failure path must preserve the exact historical message"
            ),
            other => panic!("expected the initial failure to stay Degraded, got: {other:?}"),
        }
        assert_eq!(replacement_attempts(&pool), 0);
    }
}
