//! In-memory SEDA component for rust-camel — asynchronous staging channel
//! between routes sharing the same context via bounded queues.
//!
//! Main types: `SedaComponent`, `SedaEndpoint`, `SedaConsumer`, `SedaProducer`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

#[cfg(test)]
use camel_component_api::test_support::NoopRuntimeObservability;
#[cfg(test)]
fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    // The consumer reports queue depth through `rt.metrics()` on its
    // forwarder/sampler loop, so tests need the permissive double (the
    // panicking double is for components that must not touch observability).
    std::sync::Arc::new(NoopRuntimeObservability)
}

use async_trait::async_trait;
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::Service;

use camel_api::BoxProcessorExt;
use camel_component_api::UriConfig;
use camel_component_api::parse_uri;
use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, ComponentMetadata, ConcurrencyModel,
    Consumer, ConsumerContext, Endpoint, Exchange, ExchangeEnvelope, ProducerContext,
};
use tracing::{info, warn};

/// Queue-depth sampling cadence for the per-endpoint
/// `camel_queue_depth{queue="seda:<name>"}` gauge (dashboard-observability
/// T3.3). Short enough that a scrape between ticks never misses a backlog,
/// long enough that the len() read is negligible.
const QUEUE_DEPTH_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitForTaskToComplete {
    Never,
    IfReplyExpected,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangePattern {
    InOnly,
    InOut,
}

// ---------------------------------------------------------------------------
// SedaConfig
// ---------------------------------------------------------------------------

/// Configuration parsed from a SEDA URI.
///
/// URI format: `seda:name[?options]`
///
/// Options are split into two groups:
/// - **shared**: validated for consistency when multiple endpoints reference
///   the same endpoint name (`size`, `multiple_consumers`, `exchange_pattern`,
///   `concurrent_consumers`).
/// - **producer only**: stored per-endpoint, used only by the producer
///   (`block_when_full`, `discard_if_no_consumers`, `timeout_ms`,
///   `wait_for_task_to_complete`).
#[derive(Debug, Clone)]
pub struct SedaConfig {
    pub name: String,
    pub size: usize,
    pub concurrent_consumers: usize,
    pub multiple_consumers: bool,
    pub block_when_full: bool,
    pub discard_if_no_consumers: bool,
    pub timeout_ms: u64,
    pub wait_for_task_to_complete: WaitForTaskToComplete,
    pub exchange_pattern: ExchangePattern,
}

/// Private container for macro-derived `uri_options()` and `metadata()`.
///
/// Mirrors `SedaConfig`'s URI-parsed fields with `String` for enum types.
/// `SedaConfig` holds the typed enum variants; metadata delegation targets
/// this inner type.
#[derive(Debug, Clone, UriConfig)]
#[allow(dead_code)]
#[uri_scheme = "seda"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "seda",
        description = "Asynchronous staged event-driven architecture with bounded queue",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
struct SedaUriConfig {
    #[allow(dead_code)]
    _name: String,
    #[uri_param(
        name = "size",
        default = "1000",
        desc = "Bounded queue capacity. Must be > 0"
    )]
    size: usize,
    #[uri_param(
        name = "concurrentConsumers",
        default = "1",
        desc = "Consumer concurrency. Clamped to 1 minimum"
    )]
    concurrent_consumers: usize,
    #[uri_param(
        name = "multipleConsumers",
        default = "false",
        desc = "Fanout mode — clone to all subscribers"
    )]
    multiple_consumers: bool,
    #[uri_param(
        name = "blockWhenFull",
        default = "false",
        desc = "Block producer when queue full vs fail fast"
    )]
    block_when_full: bool,
    #[uri_param(
        name = "discardIfNoConsumers",
        default = "false",
        desc = "Silently drop if no consumers vs error"
    )]
    discard_if_no_consumers: bool,
    #[uri_param(
        name = "timeout",
        default = "30000",
        desc = "Timeout for enqueue and reply wait in milliseconds"
    )]
    timeout_ms: u64,
    #[uri_param(
        name = "waitForTaskToComplete",
        kind = "enum:Never,IfReplyExpected,Always",
        default = "IfReplyExpected",
        desc = "When to wait for task completion"
    )]
    wait_for_task_to_complete: String,
    #[uri_param(
        name = "exchangePattern",
        kind = "enum:InOnly,InOut",
        default = "InOnly",
        desc = "Exchange pattern"
    )]
    exchange_pattern: String,
}

impl SedaConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != "seda" {
            return Err(CamelError::InvalidUri(format!(
                "invalid scheme '{}', expected 'seda'",
                parts.scheme
            )));
        }

        let name = parts.path;
        if name.trim().is_empty() {
            return Err(CamelError::InvalidUri(
                "seda: endpoint name must not be empty".to_string(),
            ));
        }
        if name.contains(char::is_whitespace) {
            return Err(CamelError::InvalidUri(
                "seda: endpoint name must not contain whitespace".to_string(),
            ));
        }

        let size: usize = parts
            .params
            .get("size")
            .map(|v| v.parse::<usize>())
            .transpose()
            .map_err(|e: std::num::ParseIntError| {
                CamelError::InvalidUri(format!("invalid size: {e}"))
            })?
            .unwrap_or(1000);

        if size == 0 {
            return Err(CamelError::InvalidUri(
                "seda: size must be greater than 0".to_string(),
            ));
        }

        let concurrent_consumers: usize = parts
            .params
            .get("concurrentConsumers")
            .map(|v| v.parse::<usize>())
            .transpose()
            .map_err(|e: std::num::ParseIntError| {
                CamelError::InvalidUri(format!("invalid concurrentConsumers: {e}"))
            })?
            .unwrap_or(1);

        let multiple_consumers = parts
            .params
            .get("multipleConsumers")
            .map(|v| parse_bool("multipleConsumers", v))
            .transpose()?
            .unwrap_or(false);

        let block_when_full = parts
            .params
            .get("blockWhenFull")
            .map(|v| parse_bool("blockWhenFull", v))
            .transpose()?
            .unwrap_or(false);

        let discard_if_no_consumers = parts
            .params
            .get("discardIfNoConsumers")
            .map(|v| parse_bool("discardIfNoConsumers", v))
            .transpose()?
            .unwrap_or(false);

        let timeout_ms: u64 = parts
            .params
            .get("timeout")
            .map(|v| v.parse::<u64>())
            .transpose()
            .map_err(|e: std::num::ParseIntError| {
                CamelError::InvalidUri(format!("invalid timeout: {e}"))
            })?
            .unwrap_or(30_000);

        let wait_for_task_to_complete = parts
            .params
            .get("waitForTaskToComplete")
            .map(|v| parse_wait_for_task(v))
            .transpose()?
            .unwrap_or(WaitForTaskToComplete::IfReplyExpected);

        let exchange_pattern = parts
            .params
            .get("exchangePattern")
            .map(|v| parse_exchange_pattern(v))
            .transpose()?
            .unwrap_or(ExchangePattern::InOnly);

        let concurrent_consumers = if concurrent_consumers == 0 {
            warn!(name, "concurrentConsumers=0 clamped to 1");
            1
        } else {
            concurrent_consumers
        };

        Ok(Self {
            name,
            size,
            concurrent_consumers,
            multiple_consumers,
            block_when_full,
            discard_if_no_consumers,
            timeout_ms,
            wait_for_task_to_complete,
            exchange_pattern,
        })
    }

    fn is_compatible_with(&self, other: &SedaConfig) -> Result<(), String> {
        let mut diffs = Vec::new();
        if self.size != other.size {
            diffs.push(format!("size: {} vs {}", self.size, other.size));
        }
        if self.multiple_consumers != other.multiple_consumers {
            diffs.push(format!(
                "multipleConsumers: {} vs {}",
                self.multiple_consumers, other.multiple_consumers
            ));
        }
        if self.exchange_pattern != other.exchange_pattern {
            diffs.push(format!(
                "exchangePattern: {:?} vs {:?}",
                self.exchange_pattern, other.exchange_pattern
            ));
        }
        if self.concurrent_consumers != other.concurrent_consumers {
            diffs.push(format!(
                "concurrentConsumers: {} vs {}",
                self.concurrent_consumers, other.concurrent_consumers
            ));
        }
        if diffs.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "endpoint '{}' already exists with different config: {}",
                self.name,
                diffs.join(", ")
            ))
        }
    }

    /// Component metadata for the seda scheme, derived from `#[uri_param]`
    /// annotations on `SedaUriConfig`.
    pub fn metadata() -> ComponentMetadata {
        SedaUriConfig::metadata()
    }

    /// Generated URI option definitions for the seda scheme, derived from
    /// `#[uri_param]` annotations on `SedaUriConfig`.
    pub fn uri_options() -> Vec<camel_api::component_metadata::UriOption> {
        SedaUriConfig::uri_options()
    }
}

fn parse_bool(name: &str, v: &str) -> Result<bool, CamelError> {
    match v.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(CamelError::InvalidUri(format!(
            "invalid boolean for {name}: '{v}'"
        ))),
    }
}

fn parse_wait_for_task(v: &str) -> Result<WaitForTaskToComplete, CamelError> {
    match v.to_lowercase().replace('_', "").as_str() {
        "never" => Ok(WaitForTaskToComplete::Never),
        "ifreplyexpected" => Ok(WaitForTaskToComplete::IfReplyExpected),
        "always" => Ok(WaitForTaskToComplete::Always),
        _ => Err(CamelError::InvalidUri(format!(
            "invalid waitForTaskToComplete: '{v}' (expected: Never, IfReplyExpected, Always)"
        ))),
    }
}

fn parse_exchange_pattern(v: &str) -> Result<ExchangePattern, CamelError> {
    match v.to_lowercase().replace('_', "").as_str() {
        "inonly" => Ok(ExchangePattern::InOnly),
        "inout" => Ok(ExchangePattern::InOut),
        _ => Err(CamelError::InvalidUri(format!(
            "invalid exchangePattern: '{v}' (expected: InOnly, InOut)"
        ))),
    }
}

// ---------------------------------------------------------------------------
// ConsumerId generator (no uuid dependency needed)
// ---------------------------------------------------------------------------

static CONSUMER_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_consumer_id() -> String {
    format!(
        "seda-consumer-{}",
        CONSUMER_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

// ---------------------------------------------------------------------------
// SedaMode + SedaEndpointState
// ---------------------------------------------------------------------------

type ConsumerId = String;

/// Transport mode for a SEDA endpoint.
///
/// - `Single`: one bounded mpsc channel, one consumer allowed.
///   `active` tracks whether a consumer has started (separate from receiver
///   ownership, which is taken by the forwarder task on start).
/// - `Fanout`: one bounded mpsc per subscriber, multiple consumers allowed.
enum SedaMode {
    Single {
        tx: mpsc::Sender<ExchangeEnvelope>,
        rx: Mutex<Option<mpsc::Receiver<ExchangeEnvelope>>>,
        active: std::sync::atomic::AtomicBool,
    },
    Fanout {
        subscribers: Mutex<HashMap<ConsumerId, mpsc::Sender<ExchangeEnvelope>>>,
    },
}

struct SedaEndpointState {
    config: SedaConfig,
    mode: SedaMode,
    /// Lock-free queue-depth counter backing the per-endpoint
    /// `camel_queue_depth{queue="seda:<name>"}` gauge (dashboard-observability
    /// T3.3). ONE counter per endpoint, shared by producers and every
    /// forwarder (hoisted out of `SedaMode` so both modes use it).
    ///
    /// Semantics: Single counts each envelope once from producer send until
    /// the forwarder finishes forwarding it. Fanout is broadcast — each
    /// produced exchange is cloned to every subscriber — so the honest
    /// shared-label metric counts each *undelivered copy*: the producer adds
    /// one per reserved subscriber and each forwarder subtracts one when its
    /// copy leaves the endpoint. (Per-subscriber `rx.len()` publishes under
    /// the shared label — the scheme this replaces — let an idle subscriber
    /// clobber a busy subscriber's backlog with intermittent false zeros.)
    ///
    /// The forwarder holds the shared-receiver mutex while parked in
    /// `recv()` (Single), so a sampler cannot take that lock; producers
    /// count an envelope in before sending and forwarders count it out via
    /// the RAII [`DepthGuard`]. Reads are exact once sends settle (transient
    /// over-count only, never negative).
    depth: Arc<AtomicUsize>,
}

impl SedaEndpointState {
    fn new(config: &SedaConfig) -> Self {
        let (tx, rx) = mpsc::channel(config.size);
        let mode = if config.multiple_consumers {
            SedaMode::Fanout {
                subscribers: Mutex::new(HashMap::new()),
            }
        } else {
            SedaMode::Single {
                tx,
                rx: Mutex::new(Some(rx)),
                active: std::sync::atomic::AtomicBool::new(false),
            }
        };
        Self {
            config: config.clone(),
            mode,
            depth: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns true if at least one consumer has started and not yet stopped.
    /// For Single mode: checks the `active` flag (not the receiver, which is
    /// moved into the forwarder task on start).
    /// For Fanout mode: checks if subscribers map is non-empty.
    fn has_active_consumers(&self) -> bool {
        match &self.mode {
            SedaMode::Single { active, .. } => active.load(Ordering::SeqCst),
            SedaMode::Fanout { subscribers } => !subscribers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
        }
    }
}

/// RAII pairing for the per-endpoint [`SedaEndpointState::depth`] counter:
/// every counted-in envelope (or fanout copy) must be counted out exactly
/// once, including on panic unwind and forwarder task abort (review F4).
///
/// - Producers create the guard with [`DepthGuard::count_in`] after reserving
///   channel capacity and [`DepthGuard::commit`] it once the envelope(s) are
///   handed to the channel — any early return, panic, or abort before the
///   commit drops the guard and rolls the count back.
/// - Forwarders create it with [`DepthGuard::claim`] immediately after
///   receiving an envelope; normal completion of `forward_envelope`, a panic
///   inside it, and a task abort at one of its await points all drop the
///   guard, counting the envelope out. Until then the envelope counts as
///   in-flight through the endpoint (queued or being forwarded).
struct DepthGuard {
    depth: Arc<AtomicUsize>,
    count: usize,
}

impl DepthGuard {
    /// Producer side: count `count` envelopes in; dropping rolls back.
    fn count_in(depth: &Arc<AtomicUsize>, count: usize) -> Self {
        depth.fetch_add(count, Ordering::AcqRel);
        Self {
            depth: Arc::clone(depth),
            count,
        }
    }

    /// Forwarder side: take ownership of one already-counted-in envelope
    /// (no increment); dropping counts it out.
    fn claim(depth: &Arc<AtomicUsize>) -> Self {
        Self {
            depth: Arc::clone(depth),
            count: 1,
        }
    }

    /// Producer commit: the count now belongs to envelopes inside the
    /// channel; abandon the rollback.
    fn commit(self) {
        std::mem::forget(self);
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        self.depth.fetch_sub(self.count, Ordering::AcqRel);
    }
}

/// Spawn the detached per-endpoint queue-depth sampler (T3.3). A forwarder
/// parked inside a blocked pipeline (or parked in `recv()` holding the
/// shared-receiver mutex) cannot publish loop-edge samples, so a fixed-tick
/// task reads the lock-free `depth` counter instead. Detached by design —
/// it exits on the consumer's cancel token (stop()) and is not a forwarder,
/// so it stays out of `forwarder_handles`.
///
/// Fanout consumers each spawn one; every sampler publishes the SAME shared
/// atomic, so concurrent ticks are idempotent and an idle subscriber can
/// never clobber a busy subscriber's backlog with a false zero (the
/// per-subscriber `rx.len()` publish this replaces did exactly that).
fn spawn_queue_depth_sampler(
    metrics: Arc<dyn camel_api::MetricsCollector>,
    label: String,
    depth: Arc<AtomicUsize>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(QUEUE_DEPTH_SAMPLE_INTERVAL);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => {
                    metrics.set_queue_depth(&label, depth.load(Ordering::Acquire));
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// SedaComponent
// ---------------------------------------------------------------------------

type SedaRegistry = Arc<Mutex<HashMap<String, Arc<SedaEndpointState>>>>;

pub struct SedaComponent {
    endpoints: SedaRegistry,
}

impl SedaComponent {
    pub fn new() -> Self {
        Self {
            endpoints: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn get_or_create_state(
        &self,
        config: &SedaConfig,
    ) -> Result<Arc<SedaEndpointState>, CamelError> {
        let mut endpoints = self.endpoints.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = endpoints.get(&config.name) {
            existing
                .config
                .is_compatible_with(config)
                .map_err(CamelError::EndpointCreationFailed)?;
            Ok(Arc::clone(existing))
        } else {
            let state = Arc::new(SedaEndpointState::new(config));
            endpoints.insert(config.name.clone(), Arc::clone(&state));
            Ok(state)
        }
    }
}

impl Default for SedaComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Component for SedaComponent {
    fn scheme(&self) -> &str {
        "seda"
    }

    fn metadata(&self) -> ComponentMetadata {
        SedaConfig::metadata()
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = SedaConfig::from_uri(uri)?;
        let state = self.get_or_create_state(&config)?;
        Ok(Box::new(SedaEndpoint {
            uri: uri.to_string(),
            config,
            state,
        }))
    }
}

// ---------------------------------------------------------------------------
// SedaEndpoint
// ---------------------------------------------------------------------------

struct SedaEndpoint {
    uri: String,
    config: SedaConfig,
    state: Arc<SedaEndpointState>,
}

impl Endpoint for SedaEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(SedaConsumer::new(
            Arc::clone(&self.state),
            next_consumer_id(),
            rt,
        )))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let producer = SedaProducer {
            state: Arc::clone(&self.state),
            producer_config: ProducerConfig::from(&self.config),
            runtime: rt,
        };
        Ok(BoxProcessor::from_fn(move |ex| {
            let mut svc = producer.clone();
            Box::pin(async move { svc.call(ex).await })
        }))
    }
}

/// Per-endpoint producer options. These are NOT shared at the SedaEndpointState
/// level because two endpoints referencing the same seda name may have different
/// producer-only options (e.g. different blockWhenFull settings).
#[derive(Clone)]
struct ProducerConfig {
    block_when_full: bool,
    discard_if_no_consumers: bool,
    timeout_ms: u64,
    wait_for_task_to_complete: WaitForTaskToComplete,
}

impl From<&SedaConfig> for ProducerConfig {
    fn from(config: &SedaConfig) -> Self {
        Self {
            block_when_full: config.block_when_full,
            discard_if_no_consumers: config.discard_if_no_consumers,
            timeout_ms: config.timeout_ms,
            wait_for_task_to_complete: config.wait_for_task_to_complete,
        }
    }
}

// ---------------------------------------------------------------------------
// SedaConsumer
// ---------------------------------------------------------------------------

struct SedaConsumer {
    state: Arc<SedaEndpointState>,
    consumer_id: ConsumerId,
    started: bool,
    cancel_token: CancellationToken,
    forwarder_handles: Vec<JoinHandle<Result<(), CamelError>>>,
    /// Handle to the forwarder-shared receiver. Set on start for BOTH
    /// modes (Single: shared by the concurrent forwarders; Fanout: owned by
    /// the single forwarder). Used by `stop()` — Single restores the
    /// receiver into the endpoint state so a later consumer can start
    /// again; Fanout takes it back to return the discarded backlog's
    /// queue-depth counts.
    shared_rx: Option<Arc<AsyncMutex<Option<mpsc::Receiver<ExchangeEnvelope>>>>>,
    /// Runtime observability handle: `metrics()` powers the per-endpoint
    /// `camel_queue_depth{queue="seda:<name>"}` gauge on the forwarder loop.
    runtime: Arc<dyn camel_component_api::RuntimeObservability>,
}

impl SedaConsumer {
    fn new(
        state: Arc<SedaEndpointState>,
        consumer_id: ConsumerId,
        runtime: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Self {
        Self {
            state,
            consumer_id,
            started: false,
            cancel_token: CancellationToken::new(),
            forwarder_handles: Vec::new(),
            shared_rx: None,
            runtime,
        }
    }

    #[cfg(test)]
    pub(crate) fn forwarder_count(&self) -> usize {
        self.forwarder_handles.len()
    }
}

#[async_trait]
impl Consumer for SedaConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        if self.started {
            return Err(CamelError::EndpointCreationFailed(
                "consumer already started".to_string(),
            ));
        }

        match &self.state.mode {
            SedaMode::Single { rx, active, .. } => {
                let mut rx_guard = rx.lock().unwrap_or_else(|e| e.into_inner());
                if rx_guard.is_none() {
                    return Err(CamelError::EndpointCreationFailed(format!(
                        "endpoint '{}' already has a registered consumer",
                        self.state.config.name
                    )));
                }
                active.store(true, Ordering::SeqCst);
                let receiver = rx_guard.take().ok_or_else(|| {
                    CamelError::EndpointCreationFailed(format!(
                        "endpoint '{}' receiver already taken",
                        self.state.config.name
                    ))
                })?;
                drop(rx_guard);

                let shared_rx = Arc::new(AsyncMutex::new(Some(receiver)));
                self.shared_rx = Some(Arc::clone(&shared_rx));
                let concurrent = self.state.config.concurrent_consumers;
                let queue_metrics = self.runtime.metrics();
                let queue_label = format!("seda:{}", self.state.config.name);
                let depth = Arc::clone(&self.state.depth);

                for _ in 0..concurrent {
                    let shared_rx = Arc::clone(&shared_rx);
                    let cancel = self.cancel_token.clone();
                    let ctx = ctx.clone();
                    let depth = Arc::clone(&depth);
                    let component_metrics = self.runtime.component_metrics();
                    let handle = tokio::spawn(async move {
                        loop {
                            let envelope = {
                                let mut guard = shared_rx.lock().await;
                                let Some(rx) = guard.as_mut() else {
                                    // Stop took the receiver back; exit cleanly.
                                    return Ok(());
                                };
                                let env = tokio::select! {
                                    env = rx.recv() => env,
                                    _ = cancel.cancelled() => return Ok(()),
                                };
                                env
                            };
                            let Some(envelope) = envelope else {
                                return Ok(());
                            };
                            // Own the counted-in envelope until it leaves the
                            // endpoint: the claim's Drop counts it out when
                            // forwarding completes, panics, or the task is
                            // aborted at an await point.
                            let _claim = DepthGuard::claim(&depth);
                            forward_envelope(&ctx, &component_metrics, envelope).await;
                        }
                    });
                    self.forwarder_handles.push(handle);
                }

                // Periodic queue-depth sampler: a forwarder parked inside a
                // blocked pipeline (or parked in `recv()` holding the
                // shared-receiver mutex) cannot publish loop-edge samples,
                // so the consumer reports the lock-free depth counter on a
                // fixed tick instead. Detached by design — it exits on the
                // consumer's cancel token (stop()) and is not a forwarder,
                // so it stays out of `forwarder_handles`.
                spawn_queue_depth_sampler(
                    queue_metrics,
                    queue_label,
                    depth,
                    self.cancel_token.clone(),
                );
            }
            SedaMode::Fanout { subscribers } => {
                let (tx, rx) = mpsc::channel(self.state.config.size);
                subscribers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(self.consumer_id.clone(), tx);

                let cancel = self.cancel_token.clone();
                let queue_metrics = self.runtime.metrics();
                let queue_label = format!("seda:{}", self.state.config.name);
                let depth = Arc::clone(&self.state.depth);
                let sampler_depth = Arc::clone(&depth);
                let shared_rx = Arc::new(AsyncMutex::new(Some(rx)));
                self.shared_rx = Some(Arc::clone(&shared_rx));
                let forwarder_rx = Arc::clone(&shared_rx);
                let component_metrics = self.runtime.component_metrics();
                let handle = tokio::spawn(async move {
                    loop {
                        let envelope = {
                            let mut guard = forwarder_rx.lock().await;
                            let Some(rx) = guard.as_mut() else {
                                // Stop took the receiver back; exit cleanly.
                                return Ok(());
                            };
                            let env = tokio::select! {
                                env = rx.recv() => env,
                                _ = cancel.cancelled() => return Ok(()),
                            };
                            env
                        };
                        let Some(envelope) = envelope else {
                            break;
                        };
                        // Fanout copy: counted in by the producer (one per
                        // reserved subscriber); the claim counts it out when
                        // forwarding finishes, panics, or the task is aborted.
                        let _claim = DepthGuard::claim(&depth);
                        forward_envelope(&ctx, &component_metrics, envelope).await;
                    }
                    Ok(())
                });
                self.forwarder_handles.push(handle);

                // Shared-atomic sampler (see `spawn_queue_depth_sampler`):
                // replaces per-subscriber `rx.len()` publishes under the
                // same label, which let an idle subscriber clobber a busy
                // subscriber's backlog with false zeros.
                spawn_queue_depth_sampler(
                    queue_metrics,
                    queue_label,
                    sampler_depth,
                    self.cancel_token.clone(),
                );
            }
        }

        self.started = true;
        info!(
            name = %self.state.config.name,
            consumer_id = %self.consumer_id,
            concurrent = self.state.config.concurrent_consumers,
            "SEDA consumer started"
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if !self.started {
            return Ok(());
        }
        self.cancel_token.cancel();
        for handle in self.forwarder_handles.drain(..) {
            handle.abort();
        }
        match &self.state.mode {
            SedaMode::Single { rx, active, .. } => {
                // Flag-first: clear `active` before publishing the restored
                // receiver so a concurrent start that acquires the receiver
                // does so only after `active` is false, making its own
                // `active.store(true)` the final write (race closure).
                active.store(false, Ordering::SeqCst);
                if let Some(shared_rx) = self.shared_rx.take() {
                    let receiver = shared_rx.lock().await.take();
                    if let Some(recv) = receiver {
                        *rx.lock().unwrap_or_else(|e| e.into_inner()) = Some(recv);
                    }
                }
            }
            SedaMode::Fanout { subscribers } => {
                subscribers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&self.consumer_id);
                // The aborted forwarder leaves its backlog in the
                // subscriber channel; those copies are discarded with the
                // subscription, so return their queue-depth counts —
                // otherwise the shared gauge stays inflated forever. The
                // mutex is free once the aborted task is dropped, so this
                // is deterministic even when the abort raced the cancel
                // branch. (Narrow residual race, accepted: a producer that
                // reserved a permit on this subscriber just before removal
                // still sends into the discarded channel, leaking +1.)
                if let Some(shared_rx) = self.shared_rx.take()
                    && let Some(mut rx) = shared_rx.lock().await.take()
                {
                    let mut discarded = 0;
                    while rx.try_recv().is_ok() {
                        discarded += 1;
                    }
                    if discarded > 0 {
                        self.state.depth.fetch_sub(discarded, Ordering::AcqRel);
                    }
                }
            }
        }
        self.started = false;
        info!(
            name = %self.state.config.name,
            consumer_id = %self.consumer_id,
            "SEDA consumer stopped"
        );
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Concurrent {
            max: Some(self.state.config.concurrent_consumers),
        }
    }

    fn background_task_handle(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
        // SEDA may have multiple forwarder handles; return the first one.
        // The remaining handles are cancelled in stop().
        self.forwarder_handles.pop()
    }
}

/// Forward an envelope from the SEDA queue into the route pipeline.
///
/// Key rule: if the envelope carries a `reply_tx`, the forwarder MUST use
/// `send_and_wait()` to route the pipeline result back to the producer.
/// This handles both InOut and `waitForTaskToComplete=Always` cases.
/// If no `reply_tx`, use fire-and-forget `send()`.
///
/// The consume operation is observed through the uniform
/// component-operations facade (dashboard-observability Task 4.2):
/// failures ALWAYS reach the error family as `e:seda:consume`, the
/// component series only with the lever on.
async fn forward_envelope(
    ctx: &ConsumerContext,
    component_metrics: &camel_api::ComponentMetrics,
    envelope: ExchangeEnvelope,
) {
    if let Some(reply_tx) = envelope.reply_tx {
        let result = ctx.send_and_wait(envelope.exchange).await;
        component_metrics.observe("seda", "consume", result.is_err());
        let _ = reply_tx.send(result);
    } else if let Err(e) = ctx.send(envelope.exchange).await {
        component_metrics.observe("seda", "consume", true);
        warn!(error = %e, "SEDA consumer send failed");
    } else {
        component_metrics.observe("seda", "consume", false);
    }
}

// ---------------------------------------------------------------------------
// SedaProducer
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SedaProducer {
    state: Arc<SedaEndpointState>,
    producer_config: ProducerConfig,
    /// Observability handle: `component_metrics()` powers the uniform
    /// `seda:produce` emission (dashboard-observability Task 4.2).
    runtime: Arc<dyn camel_component_api::RuntimeObservability>,
}

impl Service<Exchange> for SedaProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let state = Arc::clone(&self.state);
        let producer_config = self.producer_config.clone();
        let original = exchange.clone();
        let component_metrics = self.runtime.component_metrics();
        Box::pin(async move {
            // The produce operation covers the whole enqueue outcome
            // (no-consumers rejection, queue-full, timeout, reply wait):
            // failures ALWAYS reach the error family as `e:seda:produce`,
            // the component series only with the lever on.
            let result: Result<Exchange, CamelError> = async {
                    if !state.has_active_consumers() {
                    if producer_config.discard_if_no_consumers {
                        return Ok(exchange);
                    }
                    return Err(CamelError::EndpointCreationFailed(format!(
                        "SEDA endpoint '{}' has no active consumers",
                        state.config.name
                    )));
                }

                let should_wait = match producer_config.wait_for_task_to_complete {
                    WaitForTaskToComplete::Never => false,
                    WaitForTaskToComplete::Always => true,
                    WaitForTaskToComplete::IfReplyExpected => {
                        state.config.exchange_pattern == ExchangePattern::InOut
                    }
                };

                if state.config.multiple_consumers && should_wait {
                    return Err(CamelError::EndpointCreationFailed(
                        "multipleConsumers=true with waitForTaskToComplete != Never \
                         is not supported — a single request cannot have N valid \
                         replies without aggregator semantics"
                            .to_string(),
                    ));
                }

                let (reply_tx, reply_rx) = if should_wait {
                    let (tx, rx) = oneshot::channel();
                    (Some(tx), Some(rx))
                } else {
                    (None, None)
                };

                let envelope = ExchangeEnvelope { exchange, reply_tx };

                match &state.mode {
                    SedaMode::Single { tx, .. } => {
                        // Count the envelope into the queue depth before it
                        // enters the channel; the guard rolls the count back on
                        // send failure, panic, or abort before the commit, so
                        // the lock-free counter never under-reads or leaks.
                        let guard = DepthGuard::count_in(&state.depth, 1);
                        if producer_config.block_when_full {
                            let result = tokio::time::timeout(
                                Duration::from_millis(producer_config.timeout_ms),
                                tx.send(envelope),
                            )
                            .await;
                            match result {
                                Ok(Ok(())) => guard.commit(),
                                Ok(Err(_)) => {
                                    return Err(CamelError::ChannelClosed);
                                }
                                Err(_) => {
                                    return Err(CamelError::EndpointCreationFailed(format!(
                                        "SEDA producer timeout enqueueing on '{}' ({}ms)",
                                        state.config.name, producer_config.timeout_ms
                                    )));
                                }
                            }
                        } else {
                            if let Err(e) = tx.try_send(envelope) {
                                return Err(match e {
                                    mpsc::error::TrySendError::Full(_) => {
                                        CamelError::EndpointCreationFailed(format!(
                                            "SEDA queue '{}' is full (size={})",
                                            state.config.name, state.config.size
                                        ))
                                    }
                                    _ => CamelError::ChannelClosed,
                                });
                            }
                            guard.commit();
                        }
                    }
                    SedaMode::Fanout { subscribers } => {
                        let sender_list: Vec<mpsc::Sender<ExchangeEnvelope>> = {
                            let subs_guard = subscribers.lock().unwrap_or_else(|e| e.into_inner());
                            if subs_guard.is_empty() {
                                if producer_config.discard_if_no_consumers {
                                    return Ok(original);
                                }
                                return Err(CamelError::EndpointCreationFailed(format!(
                                    "SEDA endpoint '{}' has no active subscribers",
                                    state.config.name
                                )));
                            }
                            subs_guard.values().cloned().collect()
                        };

                        if producer_config.block_when_full {
                            let mut permits: Vec<mpsc::OwnedPermit<ExchangeEnvelope>> =
                                Vec::with_capacity(sender_list.len());
                            for sender in &sender_list {
                                let result = tokio::time::timeout(
                                    Duration::from_millis(producer_config.timeout_ms),
                                    sender.clone().reserve_owned(),
                                )
                                .await;
                                match result {
                                    Ok(Ok(permit)) => permits.push(permit),
                                    Ok(Err(_)) => return Err(CamelError::ChannelClosed),
                                    Err(_) => {
                                        return Err(CamelError::EndpointCreationFailed(format!(
                                            "SEDA fanout timeout on '{}' ({}ms)",
                                            state.config.name, producer_config.timeout_ms
                                        )));
                                    }
                                }
                            }
                            // All copies reserved: count one in per subscriber
                            // copy. The guard rolls back if anything between
                            // here and the sends panics; commit afterwards.
                            let guard = DepthGuard::count_in(&state.depth, permits.len());
                            for permit in permits {
                                permit.send(ExchangeEnvelope {
                                    exchange: original.clone(),
                                    reply_tx: None,
                                });
                            }
                            guard.commit();
                        } else {
                            let mut permits: Vec<mpsc::OwnedPermit<ExchangeEnvelope>> =
                                Vec::with_capacity(sender_list.len());
                            for sender in &sender_list {
                                match sender.clone().try_reserve_owned() {
                                    Ok(permit) => permits.push(permit),
                                    Err(e) => {
                                        if matches!(e, mpsc::error::TrySendError::Full(_)) {
                                            return Err(CamelError::EndpointCreationFailed(format!(
                                                "SEDA queue '{}' subscriber full during fanout (size={})",
                                                state.config.name, state.config.size
                                            )));
                                        } else {
                                            return Err(CamelError::ChannelClosed);
                                        }
                                    }
                                }
                            }
                            let guard = DepthGuard::count_in(&state.depth, permits.len());
                            for permit in permits {
                                permit.send(ExchangeEnvelope {
                                    exchange: original.clone(),
                                    reply_tx: None,
                                });
                            }
                            guard.commit();
                        }
                    }
                }

                if !should_wait {
                    return Ok(original);
                }

                let reply_rx = reply_rx.ok_or(CamelError::ChannelClosed)?;
                let result =
                    tokio::time::timeout(Duration::from_millis(producer_config.timeout_ms), reply_rx)
                        .await;
                match result {
                    Ok(Ok(reply)) => reply,
                    Ok(Err(_)) => Err(CamelError::ChannelClosed),
                    Err(_) => Err(CamelError::EndpointCreationFailed(format!(
                        "SEDA producer timeout waiting for reply on '{}' ({}ms)",
                        state.config.name, producer_config.timeout_ms
                    ))),
                }
            }
            .await;
            // Uniform component-operations emission (Task 4.2): failures
            // always reach the error family, component series lever-gated.
            component_metrics.observe("seda", "produce", result.is_err());
            result
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn test_seda_config_from_uri_minimal() {
        let config = SedaConfig::from_uri("seda:foo").unwrap();
        assert_eq!(config.name, "foo");
        assert_eq!(config.size, 1000);
        assert_eq!(config.concurrent_consumers, 1);
        assert!(!config.multiple_consumers);
        assert!(!config.block_when_full);
        assert!(!config.discard_if_no_consumers);
        assert_eq!(config.timeout_ms, 30_000);
        assert_eq!(
            config.wait_for_task_to_complete,
            WaitForTaskToComplete::IfReplyExpected
        );
        assert_eq!(config.exchange_pattern, ExchangePattern::InOnly);
    }

    #[test]
    fn test_seda_config_from_uri_full() {
        let config = SedaConfig::from_uri(
            "seda:bar?size=500&concurrentConsumers=4&multipleConsumers=true\
             &blockWhenFull=true&discardIfNoConsumers=false&timeout=5000\
             &waitForTaskToComplete=Never&exchangePattern=InOut",
        )
        .unwrap();
        assert_eq!(config.name, "bar");
        assert_eq!(config.size, 500);
        assert_eq!(config.concurrent_consumers, 4);
        assert!(config.multiple_consumers);
        assert!(config.block_when_full);
        assert!(!config.discard_if_no_consumers);
        assert_eq!(config.timeout_ms, 5000);
        assert_eq!(
            config.wait_for_task_to_complete,
            WaitForTaskToComplete::Never
        );
        assert_eq!(config.exchange_pattern, ExchangePattern::InOut);
    }

    #[test]
    fn test_seda_config_invalid_scheme() {
        let err = SedaConfig::from_uri("timer:foo").unwrap_err();
        assert!(err.to_string().contains("expected 'seda'"));
    }

    #[test]
    fn test_seda_config_empty_name() {
        let err = SedaConfig::from_uri("seda:").unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_seda_size_zero() {
        let err = SedaConfig::from_uri("seda:foo?size=0").unwrap_err();
        assert!(err.to_string().contains("size must be greater than 0"));
    }

    #[test]
    fn test_seda_config_concurrent_consumers_zero_clamped() {
        let config = SedaConfig::from_uri("seda:foo?concurrentConsumers=0").unwrap();
        assert_eq!(config.concurrent_consumers, 1);
    }

    #[test]
    fn test_seda_config_case_insensitive_enums() {
        let config =
            SedaConfig::from_uri("seda:foo?waitForTaskToComplete=never&exchangePattern=inonly")
                .unwrap();
        assert_eq!(
            config.wait_for_task_to_complete,
            WaitForTaskToComplete::Never
        );
        assert_eq!(config.exchange_pattern, ExchangePattern::InOnly);
    }

    #[test]
    fn test_seda_config_invalid_enum() {
        let err = SedaConfig::from_uri("seda:foo?exchangePattern=invalid").unwrap_err();
        assert!(err.to_string().contains("invalid exchangePattern"));
    }

    #[test]
    fn uri_options_count_parity() {
        assert_eq!(
            SedaConfig::uri_options().len(),
            8,
            "SedaUriConfig #[uri_param] count drifted from parser"
        );
    }
}

#[cfg(test)]
mod consumer_producer_tests {
    use super::*;
    use camel_api::Value;
    use camel_component_api::Message;
    use camel_component_api::NoOpComponentContext;
    use tokio::time::Duration;
    use tower::ServiceExt;

    fn test_producer_ctx() -> ProducerContext {
        ProducerContext::default()
    }

    fn create_component() -> SedaComponent {
        SedaComponent::new()
    }

    #[tokio::test]
    async fn test_seda_single_consumer_producer_roundtrip() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:test1", &NoOpComponentContext)
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let exchange = Exchange::new(Message::new("hello seda"));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_ok());

        let received = tokio::time::timeout(Duration::from_millis(500), route_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.exchange.input.body.as_text(), Some("hello seda"));

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_inout_roundtrip() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:io?exchangePattern=InOut", &NoOpComponentContext)
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (route_tx, _) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let exchange = Exchange::new(Message::new("io test"));

        let result =
            tokio::time::timeout(Duration::from_millis(500), producer.oneshot(exchange)).await;
        assert!(result.is_err() || result.unwrap().is_err());

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_inonly_fire_and_forget() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:ff", &NoOpComponentContext)
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let exchange = Exchange::new(Message::new("fire and forget"));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_ok());

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_queue_full_fail() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:full?size=2", &NoOpComponentContext)
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("1")))
            .await
            .unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("2")))
            .await
            .unwrap();

        let result = producer.oneshot(Exchange::new(Message::new("3"))).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("full"));

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_block_when_full_with_timeout() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:bwf?size=1&blockWhenFull=true&timeout=50",
                &NoOpComponentContext,
            )
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(1);
        route_tx
            .send(ExchangeEnvelope {
                exchange: Exchange::new(Message::new("dummy")),
                reply_tx: None,
            })
            .await
            .unwrap();
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("1")))
            .await
            .unwrap();

        producer
            .clone()
            .oneshot(Exchange::new(Message::new("2")))
            .await
            .unwrap();

        let result = tokio::time::timeout(
            Duration::from_millis(200),
            producer.oneshot(Exchange::new(Message::new("3"))),
        )
        .await;
        assert!(result.is_ok());
        let inner = result.unwrap();
        assert!(inner.is_err());
        assert!(inner.unwrap_err().to_string().contains("timeout"));

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_no_consumers_fail() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:nocons", &NoOpComponentContext)
            .unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let result = producer.oneshot(Exchange::new(Message::new("test"))).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no active consumers")
        );
    }

    #[tokio::test]
    async fn test_seda_no_consumers_discard() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:discard?discardIfNoConsumers=true",
                &NoOpComponentContext,
            )
            .unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let result = producer.oneshot(Exchange::new(Message::new("test"))).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_seda_duplicate_single_consumer() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:dup", &NoOpComponentContext)
            .unwrap();

        let mut consumer_a = ep.create_consumer(rt()).unwrap();
        let (tx_a, _rx_a) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_a = ConsumerContext::new(
            tx_a,
            CancellationToken::new(),
            "seda-test-route-a".to_string(),
        );
        consumer_a.start(ctx_a).await.unwrap();

        let mut consumer_b = ep.create_consumer(rt()).unwrap();
        let (tx_b, _rx_b) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_b = ConsumerContext::new(
            tx_b,
            CancellationToken::new(),
            "seda-test-route-b".to_string(),
        );
        let result = consumer_b.start(ctx_b).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("already has a registered consumer")
        );

        consumer_a.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_fanout_two_consumers() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:fan?multipleConsumers=true", &NoOpComponentContext)
            .unwrap();

        let mut consumer_a = ep.create_consumer(rt()).unwrap();
        let (tx_a, mut rx_a) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_a = ConsumerContext::new(
            tx_a,
            CancellationToken::new(),
            "seda-test-route-a".to_string(),
        );
        consumer_a.start(ctx_a).await.unwrap();

        let mut consumer_b = ep.create_consumer(rt()).unwrap();
        let (tx_b, mut rx_b) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_b = ConsumerContext::new(
            tx_b,
            CancellationToken::new(),
            "seda-test-route-b".to_string(),
        );
        consumer_b.start(ctx_b).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .oneshot(Exchange::new(Message::new("fanout msg")))
            .await
            .unwrap();

        let recv_a = tokio::time::timeout(Duration::from_millis(500), rx_a.recv())
            .await
            .unwrap()
            .unwrap();
        let recv_b = tokio::time::timeout(Duration::from_millis(500), rx_b.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(recv_a.exchange.input.body.as_text(), Some("fanout msg"));
        assert_eq!(recv_b.exchange.input.body.as_text(), Some("fanout msg"));

        consumer_a.stop().await.unwrap();
        consumer_b.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_fanout_inout_rejected() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:fanout?multipleConsumers=true&exchangePattern=InOut",
                &NoOpComponentContext,
            )
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let result = producer.oneshot(Exchange::new(Message::new("test"))).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("multipleConsumers")
        );

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_consumer_stop_unregisters() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:stop", &NoOpComponentContext)
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("before stop")))
            .await
            .unwrap();

        consumer.stop().await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let result = producer
            .oneshot(Exchange::new(Message::new("after stop")))
            .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no active consumers")
        );
    }

    #[test]
    fn test_seda_concurrent_consumers_hint() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:conc?concurrentConsumers=4", &NoOpComponentContext)
            .unwrap();
        let consumer = ep.create_consumer(rt()).unwrap();
        assert_eq!(
            consumer.concurrency_model(),
            ConcurrencyModel::Concurrent { max: Some(4) }
        );
    }

    #[tokio::test]
    async fn test_seda_config_mismatch() {
        let comp = create_component();
        let _ep1 = comp
            .create_endpoint("seda:mm?size=100", &NoOpComponentContext)
            .unwrap();
        let result = comp.create_endpoint("seda:mm?size=200", &NoOpComponentContext);
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected config mismatch error"),
        };
        assert!(err.to_string().contains("size"));
    }

    #[tokio::test]
    async fn test_seda_wait_always_inonly() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:waitalways?waitForTaskToComplete=Always",
                &NoOpComponentContext,
            )
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (route_tx, _) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            producer.oneshot(Exchange::new(Message::new("always wait"))),
        )
        .await;
        assert!(result.is_err() || result.unwrap().is_err());

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_fanout_all_or_nothing() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:aon?multipleConsumers=true&size=2",
                &NoOpComponentContext,
            )
            .unwrap();

        let mut consumer_a = ep.create_consumer(rt()).unwrap();
        let (tx_a, _rx_a) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx_a = ConsumerContext::new(
            tx_a,
            CancellationToken::new(),
            "seda-test-route-a".to_string(),
        );
        consumer_a.start(ctx_a).await.unwrap();

        let mut consumer_b = ep.create_consumer(rt()).unwrap();
        let (tx_b, _rx_b) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx_b = ConsumerContext::new(
            tx_b,
            CancellationToken::new(),
            "seda-test-route-b".to_string(),
        );
        consumer_b.start(ctx_b).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("1")))
            .await
            .unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("2")))
            .await
            .unwrap();

        let result = producer.oneshot(Exchange::new(Message::new("3"))).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("full") || err_msg.contains("subscriber"));

        consumer_a.stop().await.unwrap();
        consumer_b.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_fanout_block_when_full_rejects_closed_subscriber_without_partial_delivery() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:aonblock?multipleConsumers=true&size=2&blockWhenFull=true&timeout=100",
                &NoOpComponentContext,
            )
            .unwrap();

        let mut consumer_a = ep.create_consumer(rt()).unwrap();
        let (tx_a, mut rx_a) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx_a = ConsumerContext::new(
            tx_a,
            CancellationToken::new(),
            "seda-test-route-a".to_string(),
        );
        consumer_a.start(ctx_a).await.unwrap();

        let state = comp
            .endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get("aonblock")
            .cloned()
            .unwrap();
        let (closed_tx, closed_rx) = mpsc::channel::<ExchangeEnvelope>(1);
        drop(closed_rx);
        match &state.mode {
            SedaMode::Fanout { subscribers } => {
                subscribers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert("closed-subscriber".to_string(), closed_tx);
            }
            SedaMode::Single { .. } => panic!("expected fanout mode"),
        }

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let result = producer
            .oneshot(Exchange::new(Message::new("partial")))
            .await;

        assert!(matches!(result, Err(CamelError::ChannelClosed)));
        let delivered = tokio::time::timeout(Duration::from_millis(50), rx_a.recv()).await;
        assert!(
            delivered.is_err(),
            "fanout delivered to only one subscriber"
        );

        consumer_a.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_discard_if_no_consumers_fanout() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:discardfan?multipleConsumers=true&discardIfNoConsumers=true",
                &NoOpComponentContext,
            )
            .unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let result = producer
            .oneshot(Exchange::new(Message::new("discard")))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_seda_multiple_producers_single_consumer() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:mpsc", &NoOpComponentContext)
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string());
        consumer.start(ctx).await.unwrap();

        let producer_a = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let producer_b = ep.create_producer(rt(), &test_producer_ctx()).unwrap();

        producer_a
            .oneshot(Exchange::new(Message::new("A")))
            .await
            .unwrap();
        producer_b
            .oneshot(Exchange::new(Message::new("B")))
            .await
            .unwrap();

        let mut bodies = Vec::new();
        for _ in 0..2 {
            let received = tokio::time::timeout(Duration::from_millis(500), rx.recv())
                .await
                .unwrap()
                .unwrap();
            bodies.push(received.exchange.input.body.as_text().unwrap().to_string());
        }
        bodies.sort();
        assert_eq!(bodies, vec!["A", "B"]);

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_inout_timeout_no_reply() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:iotimeout?exchangePattern=InOut&timeout=100",
                &NoOpComponentContext,
            )
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            producer.oneshot(Exchange::new(Message::new("no reply"))),
        )
        .await
        .unwrap();

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timeout"));

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_producer_preserves_headers() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:hdr", &NoOpComponentContext)
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let mut msg = Message::new("with headers");
        msg.set_header("X-Custom", Value::String("test-value".into()));
        msg.set_header("X-Count", Value::Number(42.into()));
        producer.oneshot(Exchange::new(msg)).await.unwrap();

        let received = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            received.exchange.input.header("X-Custom"),
            Some(&Value::String("test-value".into()))
        );
        assert_eq!(
            received.exchange.input.header("X-Count"),
            Some(&Value::Number(42.into()))
        );

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_concurrent_send_receive() {
        use std::sync::atomic::AtomicU64;

        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:concsend?size=1000", &NoOpComponentContext)
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(1000);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string());
        consumer.start(ctx).await.unwrap();

        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        let recv_handle = tokio::spawn(async move {
            while let Some(envelope) = rx.recv().await {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                let _ = envelope;
            }
        });

        let mut handles = Vec::new();
        for i in 0..10u64 {
            let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
            handles.push(tokio::spawn(async move {
                for j in 0..10u64 {
                    producer
                        .clone()
                        .oneshot(Exchange::new(Message::new(format!("{}-{}", i, j))))
                        .await
                        .unwrap();
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if counter.load(Ordering::SeqCst) == 100 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        recv_handle.abort();
        assert_eq!(counter.load(Ordering::SeqCst), 100);

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_size_one_queue() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:sz1?size=1", &NoOpComponentContext)
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("1")))
            .await
            .unwrap();

        let result = producer
            .clone()
            .oneshot(Exchange::new(Message::new("2")))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("full"));

        let _dropped = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .unwrap()
            .unwrap();

        producer
            .oneshot(Exchange::new(Message::new("3")))
            .await
            .unwrap();

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_concurrent_forwarders_count() {
        let comp = create_component();
        let _ep = comp
            .create_endpoint("seda:cfc?concurrentConsumers=4", &NoOpComponentContext)
            .unwrap();

        let state = comp
            .endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get("cfc")
            .cloned()
            .unwrap();
        let mut consumer = SedaConsumer::new(state, next_consumer_id(), rt());
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string());
        consumer.start(ctx).await.unwrap();

        assert_eq!(consumer.forwarder_count(), 4);

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_concurrent_parallel_processing() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:cpp?concurrentConsumers=2&size=10",
                &NoOpComponentContext,
            )
            .unwrap();

        // Set up route pipeline: receives envelope, sleeps 100ms, sends reply
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let mut consumer = ep.create_consumer(rt()).unwrap();
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        // Spawn a concurrent pipeline: each envelope gets its own task so
        // parallel processing is measurable even with InOut exchanges.
        tokio::spawn(async move {
            while let Some(envelope) = route_rx.recv().await {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if let Some(reply_tx) = envelope.reply_tx {
                        let _ = reply_tx.send(Ok(envelope.exchange));
                    }
                });
            }
        });

        // Enqueue 2 InOut envelopes to the SEDA channel (both at once, not awaiting replies)
        let state = comp
            .endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get("cpp")
            .cloned()
            .unwrap();
        let mut reply_rxs = Vec::new();
        match &state.mode {
            SedaMode::Single { tx, .. } => {
                for i in 0..2u32 {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    tx.send(ExchangeEnvelope {
                        exchange: Exchange::new(Message::new(format!("msg-{}", i))),
                        reply_tx: Some(reply_tx),
                    })
                    .await
                    .unwrap();
                    reply_rxs.push(reply_rx);
                }
            }
            SedaMode::Fanout { .. } => panic!("expected single mode"),
        }

        // Await both replies; with 2 concurrent forwarders this completes in ~200ms
        let result = tokio::time::timeout(Duration::from_millis(300), async {
            for reply_rx in reply_rxs {
                let _ = reply_rx.await.unwrap().unwrap();
            }
        })
        .await;
        assert!(
            result.is_ok(),
            "parallel processing timed out — must complete within 300ms"
        );

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_concurrent_consumers_one_still_single() {
        let comp = create_component();
        let _ep = comp
            .create_endpoint("seda:cco?concurrentConsumers=1", &NoOpComponentContext)
            .unwrap();

        let state = comp
            .endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get("cco")
            .cloned()
            .unwrap();
        let mut consumer = SedaConsumer::new(state, next_consumer_id(), rt());
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string());
        consumer.start(ctx).await.unwrap();

        assert_eq!(consumer.forwarder_count(), 1);

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn single_consumer_restart_restores_receiver() {
        let state = Arc::new(SedaEndpointState::new(
            &SedaConfig::from_uri("seda:restart1").unwrap(),
        ));

        // First cycle: A starts, stops; fresh B starts -> Ok, active.
        let mut a = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        let (tx_a, _rx_a) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_a = ConsumerContext::new(
            tx_a,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        a.start(ctx_a).await.unwrap();
        a.stop().await.unwrap();

        let mut b = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        let (tx_b, _rx_b) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_b = ConsumerContext::new(
            tx_b,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        b.start(ctx_b).await.unwrap();
        assert!(state.has_active_consumers());
        b.stop().await.unwrap();

        // Repeat full stop/start cycle on fresh instances 3x — every start Ok.
        for _ in 0..3 {
            let mut c = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
            let (tx_c, _rx_c) = mpsc::channel::<ExchangeEnvelope>(16);
            let ctx_c = ConsumerContext::new(
                tx_c,
                CancellationToken::new(),
                "seda-test-route".to_string(),
            );
            c.start(ctx_c).await.unwrap();
            assert!(state.has_active_consumers());
            c.stop().await.unwrap();
        }

        // After a restart, producer send succeeds (unfenced).
        let mut d = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        let (tx_d, _rx_d) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_d = ConsumerContext::new(
            tx_d,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        d.start(ctx_d).await.unwrap();

        let ep = SedaEndpoint {
            uri: "seda:restart1".to_string(),
            config: SedaConfig::from_uri("seda:restart1").unwrap(),
            state: Arc::clone(&state),
        };
        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let result = producer
            .oneshot(Exchange::new(Message::new("post-restart")))
            .await;
        assert!(result.is_ok());

        d.stop().await.unwrap();
    }

    #[tokio::test]
    async fn single_consumer_restart_preserves_buffered_envelopes() {
        let state = Arc::new(SedaEndpointState::new(
            &SedaConfig::from_uri("seda:restart2").unwrap(),
        ));

        // Capacity-1 context channel; receiver retained but NOT read (blocked context).
        let (ctx_tx, mut retained_rx) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx = ConsumerContext::new(
            ctx_tx.clone(),
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );

        let mut a = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        a.start(ctx).await.unwrap();

        // Push 3 identifiable envelopes directly through the Single-mode tx.
        let tx = match &state.mode {
            SedaMode::Single { tx, .. } => tx.clone(),
            SedaMode::Fanout { .. } => panic!("expected single mode"),
        };
        // Establish the steady state observably instead of with a fixed sleep.
        // Phase 1: e1 alone. Wait until the forwarder delivered e1 into the
        // (unread) context channel: retained receiver length == 1 proves the
        // forwarder parked its send and went back to recv.
        for body in ["e1"] {
            tx.send(ExchangeEnvelope {
                exchange: Exchange::new(Message::new(body)),
                reply_tx: None,
            })
            .await
            .unwrap();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(2_000);
        while retained_rx.len() != 1 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "forwarder never delivered e1; retained_rx.len() = {}",
                retained_rx.len()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Phase 2: e2 only. The forwarder dequeues e2 (FIFO) and parks on
        // the send into the still-full context channel — that send cannot
        // progress because nothing reads the retained receiver before stop.
        // Yield a few slots so the forwarder reaches that parked send.
        tx.send(ExchangeEnvelope {
            exchange: Exchange::new(Message::new("e2")),
            reply_tx: None,
        })
        .await
        .unwrap();
        for _ in 0..3 {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Phase 3: e3 last. The forwarder is parked on the e2 send, so e3
        // CANNOT be dequeued before stop — it is deterministically the
        // still-queued envelope the restore path must preserve.
        tx.send(ExchangeEnvelope {
            exchange: Exchange::new(Message::new("e3")),
            reply_tx: None,
        })
        .await
        .unwrap();
        tokio::task::yield_now().await;

        a.stop().await.unwrap();

        // Start fresh B on a clone of the SAME sender wired to the retained receiver.
        let mut b = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        let ctx_b = ConsumerContext::new(
            ctx_tx,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        b.start(ctx_b).await.unwrap();

        // Drain the retained receiver with a timeout; assert e1 and e3 arrive.
        let mut bodies = Vec::new();
        let drained = tokio::time::timeout(Duration::from_millis(500), async {
            while let Some(env) = retained_rx.recv().await {
                bodies.push(env.exchange.input.body.as_text().unwrap().to_string());
                if bodies.len() >= 2 {
                    break;
                }
            }
        })
        .await;
        assert!(drained.is_ok(), "timed out draining retained receiver");
        assert!(bodies.contains(&"e1".to_string()));
        assert!(bodies.contains(&"e3".to_string()));

        b.stop().await.unwrap();
    }

    #[tokio::test]
    async fn single_consumer_concurrent_restart() {
        let state = Arc::new(SedaEndpointState::new(
            &SedaConfig::from_uri("seda:restart3?concurrentConsumers=4").unwrap(),
        ));

        let mut a = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        let (tx_a, _rx_a) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_a = ConsumerContext::new(
            tx_a,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        a.start(ctx_a).await.unwrap();
        assert_eq!(a.forwarder_count(), 4);
        a.stop().await.unwrap();

        let mut b = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        let (tx_b, mut rx_b) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_b = ConsumerContext::new(
            tx_b,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        b.start(ctx_b).await.unwrap();
        assert_eq!(b.forwarder_count(), 4);

        // Envelope sent post-restart (via producer after B active) is delivered on B's context receiver.
        let ep = SedaEndpoint {
            uri: "seda:restart3?concurrentConsumers=4".to_string(),
            config: SedaConfig::from_uri("seda:restart3?concurrentConsumers=4").unwrap(),
            state: Arc::clone(&state),
        };
        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .oneshot(Exchange::new(Message::new("post-restart")))
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_millis(500), rx_b.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.exchange.input.body.as_text(), Some("post-restart"));

        b.stop().await.unwrap();
    }

    #[tokio::test]
    async fn single_second_start_while_active_still_errors() {
        let state = Arc::new(SedaEndpointState::new(
            &SedaConfig::from_uri("seda:restart4").unwrap(),
        ));

        let mut a = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        let (tx_a, _rx_a) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_a = ConsumerContext::new(
            tx_a,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        a.start(ctx_a).await.unwrap();

        let mut c = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        let (tx_c, _rx_c) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_c = ConsumerContext::new(
            tx_c,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        let result = c.start(ctx_c).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("already has a registered consumer")
        );

        a.stop().await.unwrap();
    }

    #[tokio::test]
    async fn fanout_consumer_restart_cycle() {
        let state = Arc::new(SedaEndpointState::new(
            &SedaConfig::from_uri("seda:fanrestart?multipleConsumers=true").unwrap(),
        ));

        let mut a = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        let (tx_a, _rx_a) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_a = ConsumerContext::new(
            tx_a,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        a.start(ctx_a).await.unwrap();
        a.stop().await.unwrap();

        let mut b = SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt());
        let (tx_b, mut rx_b) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_b = ConsumerContext::new(
            tx_b,
            CancellationToken::new(),
            "seda-test-route".to_string(),
        );
        b.start(ctx_b).await.unwrap();

        let ep = SedaEndpoint {
            uri: "seda:fanrestart?multipleConsumers=true".to_string(),
            config: SedaConfig::from_uri("seda:fanrestart?multipleConsumers=true").unwrap(),
            state: Arc::clone(&state),
        };
        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .oneshot(Exchange::new(Message::new("fanout restart")))
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_millis(500), rx_b.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            received.exchange.input.body.as_text(),
            Some("fanout restart")
        );

        b.stop().await.unwrap();
    }
}

#[cfg(test)]
mod queue_depth_tests {
    use super::*;
    use camel_api::MetricsCollector;
    use camel_component_api::{
        HealthCheckRegistry, Message, NoOpComponentContext, RuntimeObservability,
    };
    use tokio::time::Duration;
    use tower::ServiceExt;

    /// Shared queue-depth recorder: `metrics()` hands out clones that all
    /// append to the same log.
    #[derive(Clone, Default)]
    struct QueueDepthRecorder(Arc<Mutex<Vec<(String, usize)>>>);

    impl MetricsCollector for QueueDepthRecorder {
        fn record_exchange_duration(&self, _: &str, _: std::time::Duration) {}
        fn increment_errors(&self, _: &str, _: &str) {}
        fn increment_exchanges(&self, _: &str) {}
        fn set_queue_depth(&self, queue: &str, depth: usize) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((queue.to_string(), depth));
        }
        fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
    }

    struct RecordingObservability(QueueDepthRecorder);

    impl RuntimeObservability for RecordingObservability {
        fn metrics(&self) -> Arc<dyn MetricsCollector> {
            Arc::new(self.0.clone())
        }
        fn health(&self) -> Arc<dyn HealthCheckRegistry> {
            Arc::new(NoopRuntimeObservability)
        }
    }

    fn recording_rt() -> (Arc<RecordingObservability>, QueueDepthRecorder) {
        let rec = QueueDepthRecorder::default();
        (Arc::new(RecordingObservability(rec.clone())), rec)
    }

    fn rt_handle(obs: &Arc<RecordingObservability>) -> Arc<dyn RuntimeObservability> {
        Arc::clone(obs) as Arc<dyn RuntimeObservability>
    }

    /// F1: fanout subscribers must not clobber the shared
    /// `camel_queue_depth{queue="seda:<name>"}` gauge. Two subscribers; one
    /// is blocked (route channel capacity 1, never read) with a backlog
    /// behind it, the other drains freely. While the blocked subscriber's
    /// backlog exists, every gauge sample must stay > 0 — the old
    /// per-subscriber `rx.len()` publishes let the idle subscriber write 0
    /// over the busy subscriber's backlog.
    #[tokio::test]
    async fn fanout_gauge_stays_positive_while_blocked_subscriber_has_backlog() {
        let comp = SedaComponent::new();
        let ep = comp
            .create_endpoint("seda:fq?multipleConsumers=true", &NoOpComponentContext)
            .unwrap();
        let (obs, recorder) = recording_rt();

        // Subscriber A: blocked. Route channel capacity 1 and never read —
        // the first copy is delivered, the forwarder then parks forwarding
        // the second, and the rest queue behind it. Keeping the receiver
        // alive (never read) makes this state permanent.
        let mut consumer_a = ep.create_consumer(rt_handle(&obs)).unwrap();
        let (tx_a, blocked_rx_a) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx_a = ConsumerContext::new(tx_a, CancellationToken::new(), "route-a".to_string());
        consumer_a.start(ctx_a).await.unwrap();

        // Subscriber B: free-draining. A reader task consumes every copy.
        let mut consumer_b = ep.create_consumer(rt_handle(&obs)).unwrap();
        let (tx_b, mut rx_b) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_b = ConsumerContext::new(tx_b, CancellationToken::new(), "route-b".to_string());
        consumer_b.start(ctx_b).await.unwrap();
        let b_drained = Arc::new(AtomicUsize::new(0));
        let drained_clone = Arc::clone(&b_drained);
        tokio::spawn(async move {
            while let Some(env) = rx_b.recv().await {
                drained_clone.fetch_add(1, Ordering::SeqCst);
                let _ = env;
            }
        });

        let producer = ep
            .create_producer(rt_handle(&obs), &ProducerContext::default())
            .unwrap();
        for i in 0..4u32 {
            producer
                .clone()
                .oneshot(Exchange::new(Message::new(format!("m{i}"))))
                .await
                .unwrap();
        }

        // B consumes all 4 copies (proves the endpoint delivers normally).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while b_drained.load(Ordering::SeqCst) < 4 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "subscriber B never drained its copies"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // From here on, the shared depth is deterministically >= 3 (A's
        // one in-forward claim + two queued copies; the first copy's claim
        // already dropped on delivery into A's route channel; A's claims
        // cannot drop further because its route channel is never read).
        // Any 0 sample after this settle point is a false zero — the F1
        // bug.
        let settled = recorder.0.lock().unwrap_or_else(|e| e.into_inner()).len();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(900);
        loop {
            let post: Vec<usize> = {
                let log = recorder.0.lock().unwrap_or_else(|e| e.into_inner());
                log[settled..]
                    .iter()
                    .filter(|(q, _)| q == "seda:fq")
                    .map(|(_, d)| *d)
                    .collect()
            };
            if post.len() >= 3 {
                assert!(
                    post.iter().all(|d| *d > 0),
                    "false-zero gauge samples while backlog exists: {post:?}"
                );
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "sampler produced too few samples in 900ms: {post:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        drop(blocked_rx_a);
        consumer_a.stop().await.unwrap();
        consumer_b.stop().await.unwrap();
    }

    /// F4: the producer-side guard rolls its count back on drop.
    #[test]
    fn depth_guard_count_in_rolls_back_on_drop() {
        let depth = Arc::new(AtomicUsize::new(0));
        let guard = DepthGuard::count_in(&depth, 2);
        assert_eq!(depth.load(Ordering::Acquire), 2);
        drop(guard);
        assert_eq!(depth.load(Ordering::Acquire), 0);
    }

    /// F4: `commit` keeps the count — it now belongs to envelopes inside
    /// the channel.
    #[test]
    fn depth_guard_commit_keeps_count() {
        let depth = Arc::new(AtomicUsize::new(0));
        DepthGuard::count_in(&depth, 3).commit();
        assert_eq!(depth.load(Ordering::Acquire), 3);
    }

    /// F4: the forwarder-side claim counts its envelope out even when the
    /// code it guards panics — the unwind drops the guard.
    #[test]
    fn depth_guard_claim_decrements_on_panic_unwind() {
        let depth = Arc::new(AtomicUsize::new(1)); // one envelope counted in
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _claim = DepthGuard::claim(&depth);
            assert_eq!(depth.load(Ordering::Acquire), 1);
            panic!("simulated forward_envelope panic");
        }));
        assert!(result.is_err());
        assert_eq!(
            depth.load(Ordering::Acquire),
            0,
            "unwind must count the claimed envelope out"
        );
    }

    /// Stopping a fanout consumer with a backlog discards the queued
    /// copies with the subscription — their queue-depth counts must be
    /// returned so the shared gauge does not stay inflated forever.
    #[tokio::test]
    async fn fanout_stop_with_backlog_returns_depth_counts() {
        let state = Arc::new(SedaEndpointState::new(
            &SedaConfig::from_uri("seda:fqstop?multipleConsumers=true").unwrap(),
        ));
        let (obs, _recorder) = recording_rt();
        let mut consumer =
            SedaConsumer::new(Arc::clone(&state), next_consumer_id(), rt_handle(&obs));
        // Blocked subscriber: route channel capacity 1, never read.
        let (tx, blocked_rx) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "route-blk".to_string());
        consumer.start(ctx).await.unwrap();

        // Produce 4 copies through the real producer (counted in per copy).
        let ep = SedaEndpoint {
            uri: "seda:fqstop?multipleConsumers=true".to_string(),
            config: SedaConfig::from_uri("seda:fqstop?multipleConsumers=true").unwrap(),
            state: Arc::clone(&state),
        };
        let producer = ep
            .create_producer(rt_handle(&obs), &ProducerContext::default())
            .unwrap();
        for i in 0..4u32 {
            producer
                .clone()
                .oneshot(Exchange::new(Message::new(format!("m{i}"))))
                .await
                .unwrap();
        }

        // Steady state: copy 1 delivered into the blocked route channel
        // (claim dropped), copy 2 parked in the blocked forward (claim
        // held), copies 3-4 queued in the subscriber channel.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while state.depth.load(Ordering::Acquire) != 3 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "depth never settled at 3 (got {})",
                state.depth.load(Ordering::Acquire)
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        consumer.stop().await.unwrap();

        // The abort drops the parked claim and the stop drain returns the
        // two queued copies; both land within milliseconds, poll for it.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while state.depth.load(Ordering::Acquire) != 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "stop must return the discarded backlog's counts (got {})",
                state.depth.load(Ordering::Acquire)
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        drop(blocked_rx);
    }
}
