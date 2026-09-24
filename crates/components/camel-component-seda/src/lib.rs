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

/// Byte-exact wording assertion for producer rejections: the captured
/// error must carry exactly `expected` as its endpoint-failure payload
/// (rc-3px7o — wording assertions are equality, never substring).
#[cfg(test)]
fn assert_endpoint_failure_payload(err: CamelError, expected: &str) {
    match err {
        CamelError::EndpointCreationFailed(detail)
        | CamelError::EndpointCreationFailedWithSource(detail, _) => {
            assert_eq!(detail, expected, "gate payload must be byte-exact");
        }
        other => panic!("unexpected rejection variant: {other}"),
    }
}

use async_trait::async_trait;
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::Service;

use camel_api::{BoxProcessorExt, OpaqueErrorSource};
use camel_component_api::UriConfig;
use camel_component_api::parse_uri;
use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, ComponentMetadata, ConcurrencyModel,
    Consumer, ConsumerContext, ConsumerStartupMode, Endpoint, Exchange, ExchangeEnvelope,
    InFlightClaim, ProducerContext,
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

/// Provenance marker for the SEDA producer's no-active-consumers gate
/// (rc-3px7o): the producer embeds this crate-private value in a gate
/// rejection's source chain and [`is_no_active_consumers_gate`] classifies
/// by downcasting along that chain. Its `Display` is deliberately
/// NON-canonical diagnostic text — it never participates in classification
/// and never equals a canonical gate message.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NoActiveConsumersGate {
    Single,
    Fanout,
}

impl std::fmt::Display for NoActiveConsumersGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Single => f.write_str("seda no-active-consumers gate rejection (single mode)"),
            Self::Fanout => f.write_str("seda no-active-consumers gate rejection (fanout mode)"),
        }
    }
}

impl std::error::Error for NoActiveConsumersGate {}

impl NoActiveConsumersGate {
    /// The canonical gate detail for `endpoint_name` — the wording this
    /// crate owns and the only text a gate rejection carries.
    fn detail(&self, endpoint_name: &str) -> String {
        match self {
            Self::Single => {
                format!("SEDA endpoint '{endpoint_name}' has no active consumers")
            }
            Self::Fanout => {
                format!("SEDA endpoint '{endpoint_name}' has no active subscribers")
            }
        }
    }

    /// Build the typed gate rejection: canonical detail plus THIS marker
    /// kind as the opaque source, so classification rides on typed
    /// provenance rather than Display text (rc-3px7o).
    fn rejection(&self, endpoint_name: &str) -> CamelError {
        CamelError::EndpointCreationFailedWithSource(
            self.detail(endpoint_name),
            OpaqueErrorSource::new(Arc::new(self.clone())),
        )
    }
}

/// Provenance marker for the SEDA terminal configuration rejections: the
/// producer's `multipleConsumers` + `waitForTaskToComplete` != Never
/// conflict and the same-name endpoint config conflict (detretry,
/// rc-zovuy). The producer embeds this crate-private value in the
/// rejection's source chain and [`is_seda_terminal_config_error`]
/// classifies by downcasting along that chain. Its `Display` is
/// deliberately NON-canonical diagnostic text — it never participates in
/// classification and never equals a canonical rejection message.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalConfigError {
    MultipleConsumersWaitConflict,
    EndpointConfigConflict,
}

impl std::fmt::Display for TerminalConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MultipleConsumersWaitConflict => f.write_str(
                "seda terminal-config-error rejection (multipleConsumers wait conflict)",
            ),
            Self::EndpointConfigConflict => {
                f.write_str("seda terminal-config-error rejection (endpoint config conflict)")
            }
        }
    }
}

impl std::error::Error for TerminalConfigError {}

/// Single-mode pre-enqueue gate rejection (`SedaMode::Single` producer
/// entry).
fn single_mode_gate_rejection(name: &str) -> CamelError {
    NoActiveConsumersGate::Single.rejection(name)
}

/// Fanout-mode pre-enqueue gate rejection (`SedaMode::Fanout` producer
/// entry).
fn fanout_preenqueue_gate_rejection(name: &str) -> CamelError {
    NoActiveConsumersGate::Fanout.rejection(name)
}

/// Fanout-mode subscriber-list gate rejection (empty subscriber list at
/// dispatch).
fn fanout_subscriber_list_gate_rejection(name: &str) -> CamelError {
    NoActiveConsumersGate::Fanout.rejection(name)
}

/// Terminal configuration rejection (`SedaProducer::call`): the
/// `multipleConsumers=true` + `waitForTaskToComplete` != Never combination
/// is a deterministic configuration conflict — no consumer state can ever
/// satisfy it, so a retry can never succeed. The outer detail stays
/// BYTE-IDENTICAL to the historical wording (folding preserved);
/// classification rides the typed [`TerminalConfigError`] marker in the
/// source chain, not the text (rc-3px7o doctrine).
fn terminal_config_rejection() -> CamelError {
    CamelError::EndpointCreationFailedWithSource(
        "multipleConsumers=true with waitForTaskToComplete != Never \
         is not supported — a single request cannot have N valid \
         replies without aggregator semantics"
            .to_string(),
        OpaqueErrorSource::new(Arc::new(TerminalConfigError::MultipleConsumersWaitConflict)),
    )
}

/// Endpoint config-conflict rejection (`get_or_create_state`): a same-name
/// endpoint already exists with an incompatible config — deterministic
/// configuration state, so a retry can never succeed. The `detail` is the
/// historical `is_compatible_with` wording, passed through BYTE-IDENTICAL;
/// classification rides the typed [`TerminalConfigError`] marker in the
/// source chain, not the text (rc-3px7o doctrine; detretry rc-zovuy).
fn endpoint_config_conflict_rejection(detail: String) -> CamelError {
    CamelError::EndpointCreationFailedWithSource(
        detail,
        OpaqueErrorSource::new(Arc::new(TerminalConfigError::EndpointConfigConflict)),
    )
}

/// Bound on the [`is_no_active_consumers_gate`] provenance walk: the
/// marker must sit within this many source hops of the rejection.
const MAX_SOURCE_HOPS: usize = 8;

/// Unwrap the `Arc<dyn Error + Send + Sync>` source-chain wrapper that std
/// (rustc 1.98+) inserts into source chains.
///
/// std implements `Error for Arc<T: Error + ?Sized>`, so a source hop
/// stored as such an Arc surfaces as a wrapper object that delegates
/// Display/Debug/source but fails `downcast_ref::<T>()` for the wrapped
/// `T`. Semantics copied from camel-redis `transport_error.rs`
/// (db512039); the wrapper's own `source()` delegates to the pointee's
/// `source()`, so unwrapping never skips a chain node.
fn unwrap_arc_dyn_error<'a>(
    src: &'a (dyn std::error::Error + 'static),
) -> &'a (dyn std::error::Error + 'static) {
    match src.downcast_ref::<Arc<dyn std::error::Error + Send + Sync>>() {
        Some(arc) => &**arc,
        None => src,
    }
}

/// Shared bounded provenance walk for the crate-private markers: downcast
/// along an [`CamelError::EndpointCreationFailedWithSource`] source chain,
/// probing for `T` and returning a clone of the matched marker.
///
/// Classification is by TYPED PROVENANCE: only a
/// [`CamelError::EndpointCreationFailedWithSource`] whose source chain
/// carries `T` within [`MAX_SOURCE_HOPS`] hops matches. Display text never
/// participates — a foreign error whose message mimics a canonical wording
/// is NOT classified (rc-3px7o; same doctrine as camel-redis's
/// retryclass/rediserr walks). The walk starts from the variant's own
/// source pointee — thiserror surfaces the `#[source] OpaqueErrorSource`
/// field as the wrapper node itself, whose `source()` is the wrapped chain
/// start (hop 1) — and probes each hop after unwrapping any std
/// `Arc<dyn Error>` wrapper.
fn marker_in_source_chain<T>(err: &CamelError) -> Option<T>
where
    T: std::error::Error + Clone + 'static,
{
    let CamelError::EndpointCreationFailedWithSource(_, source) = err else {
        return None;
    };
    let mut node: &(dyn std::error::Error + 'static) = std::error::Error::source(source)?;
    for _ in 0..MAX_SOURCE_HOPS {
        let probe = unwrap_arc_dyn_error(node);
        if let Some(marker) = probe.downcast_ref::<T>() {
            return Some(marker.clone());
        }
        node = probe.source()?;
    }
    None
}

/// Extract the crate-private gate marker from a rejection's source chain,
/// if present (thin delegation to the shared [`marker_in_source_chain`]
/// walk).
fn gate_from_error(err: &CamelError) -> Option<NoActiveConsumersGate> {
    marker_in_source_chain::<NoActiveConsumersGate>(err)
}

/// True when the rejection's source chain carries the crate-private
/// [`TerminalConfigError`] marker within the bounded walk (thin delegation
/// to the shared [`marker_in_source_chain`] walk).
fn terminal_config_from_error(err: &CamelError) -> bool {
    marker_in_source_chain::<TerminalConfigError>(err).is_some()
}

/// True for the SEDA producer's startup-race gate rejections: the
/// pre-enqueue gate that fires when no consumer has started yet, in both
/// endpoint modes. Classification is by TYPED PROVENANCE, not wording: the
/// producer embeds a crate-private `NoActiveConsumersGate` marker in the
/// rejection's source chain and this predicate runs a bounded walk (at
/// most `MAX_SOURCE_HOPS` = 8 source hops) over that chain (rc-3px7o; the
/// retryclass/rediserr doctrine). Display text never classifies — a
/// foreign error whose message merely contains a gate wording is NOT a
/// gate. Both gate kinds reject
/// BEFORE enqueue but INSIDE the caller's pipeline: steps that already ran
/// (e.g. route-interception divert copies) have executed, so RETRYING the
/// send duplicates their side effects. Senders that must not retry use this
/// predicate to fail fast instead (rc-zjrx); readiness probing with
/// [`SedaComponent::has_active_consumer`] avoids the error up front.
pub fn is_no_active_consumers_gate(err: &CamelError) -> bool {
    gate_from_error(err).is_some()
}

/// True for the SEDA terminal configuration rejections, both deterministic
/// configuration conflicts no consumer timing can ever satisfy: the
/// producer's `multipleConsumers=true` + `waitForTaskToComplete` != Never
/// conflict, and the same-name endpoint config conflict raised at endpoint
/// creation when an incompatible config already exists (detretry,
/// rc-zovuy). Classification is by TYPED PROVENANCE, not wording: the
/// rejecting site embeds a crate-private `TerminalConfigError` marker in
/// the rejection's source chain and this predicate runs a bounded walk (at
/// most `MAX_SOURCE_HOPS` = 8 source hops) over that chain. Display text
/// never classifies — a foreign error whose message merely byte-matches a
/// canonical conflict wording is NOT a terminal-config error. Both marker
/// kinds reject deterministically, so a retry can never succeed and
/// senders use this predicate to fail fast instead of burning the retry
/// window; the config conflict fires at endpoint creation, not producer
/// call.
pub fn is_seda_terminal_config_error(err: &CamelError) -> bool {
    terminal_config_from_error(err)
}

/// True for the consumer-startup race a sender may safely retry: an
/// endpoint-creation failure that is NEITHER the SEDA no-active-consumers
/// gate NOR the SEDA terminal-config rejection. The direct component
/// reports its startup race ("direct endpoint '{}' not registered" at
/// poll_ready, "no consumer registered for direct:{name}" at call) under
/// [`CamelError::EndpointCreationFailed`], and any other typed endpoint
/// failure ([`CamelError::EndpointCreationFailedWithSource`]) whose source
/// chain lacks both markers is equally a plain creation race — the
/// structure, not the Display wording, carries the classification
/// (rc-fr20u doctrine). Two typed failures FAIL FAST: the gate rejects
/// pre-enqueue yet INSIDE the caller's pipeline, so a retry duplicates
/// already-executed side effects (rc-tgaxf), and the terminal-config
/// marker class covers BOTH deterministic configuration conflicts — the
/// multipleConsumers+wait conflict at producer call and the endpoint
/// config conflict at endpoint creation (detretry, rc-zovuy) — that no
/// retry can ever satisfy ([`is_seda_terminal_config_error`]). Boundary
/// (rc-utx98): a text-carrying `ProcessorError` ("… not registered") is
/// NOT a startup race — terminal, never retried; only the variant decides.
pub fn is_direct_startup_race(err: &CamelError) -> bool {
    match err {
        CamelError::EndpointCreationFailed(_) => true,
        CamelError::EndpointCreationFailedWithSource(..) => {
            !(is_no_active_consumers_gate(err) || is_seda_terminal_config_error(err))
        }
        _ => false,
    }
}

type SedaRegistry = Arc<Mutex<HashMap<String, Arc<SedaEndpointState>>>>;

/// Cloning shares the endpoint registry, so a clone registered into a
/// `CamelContext` and the original handle observe the same per-endpoint
/// state (the `MockComponent` pattern).
#[derive(Clone)]
pub struct SedaComponent {
    endpoints: SedaRegistry,
}

impl SedaComponent {
    pub fn new() -> Self {
        Self {
            endpoints: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// True when the named endpoint has at least one consumer that has
    /// started and not yet stopped — the same signal the producer-side
    /// no-active-consumers gate checks, so polling this predicate is a
    /// side-effect-free readiness probe for senders that must not retry
    /// (a retried pipeline re-executes steps with side effects, such as
    /// route-interception divert copies). Singular: one endpoint name,
    /// unlike the per-endpoint-state `has_active_consumers` check.
    /// Unknown endpoint names report `false`.
    pub fn has_active_consumer(&self, endpoint_name: &str) -> bool {
        let endpoints = self.endpoints.lock().unwrap_or_else(|e| e.into_inner());
        endpoints
            .get(endpoint_name)
            .is_some_and(|state| state.has_active_consumers())
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
                .map_err(endpoint_config_conflict_rejection)?;
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
        // Captured once (drainclaim): every enqueue mints its
        // InFlightClaims against the context-global counter.
        let in_flight = rt.in_flight_counter();
        let producer = SedaProducer {
            state: Arc::clone(&self.state),
            producer_config: ProducerConfig::from(&self.config),
            runtime: rt,
            in_flight,
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
                let forwarder_ctx = ctx.clone();
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
                        forward_envelope(&forwarder_ctx, &component_metrics, envelope).await;
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
        // Explicit startup contract (rc-dbrkr): signal readiness only now —
        // both mode arms have published the consumer-activation state
        // (Single: `active` stored + receiver taken + forwarders spawned;
        // Fanout: subscriber registered + forwarder spawned), so route
        // startup resolves only against a fully activated consumer set and
        // a producer send after `start()` passes the pre-enqueue gate on
        // the first attempt. The error paths above return before this point
        // and signal nothing.
        ctx.mark_ready();
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

    fn startup_mode(&self) -> ConsumerStartupMode {
        // Explicit (rc-dbrkr): readiness is signalled via
        // `ConsumerContext::mark_ready()` at the end of `start()`, after the
        // endpoint's consumer-activation state is published, so
        // `ctx.start()` never returns ahead of an active consumer set.
        ConsumerStartupMode::Explicit
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
    /// Context-global accepted-not-completed counter (drainclaim),
    /// captured once at `create_producer` from
    /// [`camel_component_api::RuntimeObservability::in_flight_counter`].
    /// `None` keeps this producer's enqueues uncounted (test runtimes).
    in_flight: Option<Arc<AtomicU64>>,
}

/// Mint the fanout claim set: ONE minted claim plus one `split()` sibling
/// per additional subscriber copy (drainclaim). The siblings are collected
/// BEFORE the minted claim moves into the first envelope — `split()` borrows
/// the minted claim, so the set must be fully materialised first. Yields one
/// claim per copy in send order; an empty iterator when the runtime installs
/// no counter (uncounted enqueues, e.g. test runtimes).
fn fanout_claims(
    counter: Option<&Arc<AtomicU64>>,
    copies: usize,
) -> std::vec::IntoIter<InFlightClaim> {
    let mut claims = Vec::with_capacity(copies);
    if let Some(counter) = counter {
        let minted = InFlightClaim::attach(counter);
        let siblings: Vec<InFlightClaim> = (1..copies).map(|_| minted.split()).collect();
        claims.push(minted);
        claims.extend(siblings);
    }
    claims.into_iter()
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
        let in_flight = self.in_flight.clone();
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
                    // The gate is typed per endpoint mode: Single rejects
                    // via [`single_mode_gate_rejection`], Fanout via
                    // [`fanout_preenqueue_gate_rejection`] (rc-tgaxf). The
                    // shared [`is_no_active_consumers_gate`] predicate
                    // classifies both by typed provenance in the source
                    // chain (rc-3px7o) — only the diagnostic text differs.
                    return Err(match &state.mode {
                        SedaMode::Single { .. } => {
                            single_mode_gate_rejection(&state.config.name)
                        }
                        SedaMode::Fanout { .. } => {
                            fanout_preenqueue_gate_rejection(&state.config.name)
                        }
                    });
                }

                let should_wait = match producer_config.wait_for_task_to_complete {
                    WaitForTaskToComplete::Never => false,
                    WaitForTaskToComplete::Always => true,
                    WaitForTaskToComplete::IfReplyExpected => {
                        state.config.exchange_pattern == ExchangePattern::InOut
                    }
                };

                if state.config.multiple_consumers && should_wait {
                    // Deterministic configuration conflict — a retry can
                    // never succeed. Typed provenance via
                    // [`terminal_config_rejection`]; the rendered detail
                    // stays byte-identical to the historical wording.
                    return Err(terminal_config_rejection());
                }

                let (reply_tx, reply_rx) = if should_wait {
                    let (tx, rx) = oneshot::channel();
                    (Some(tx), Some(rx))
                } else {
                    (None, None)
                };

                match &state.mode {
                    SedaMode::Single { tx, .. } => {
                        // Count the envelope into the queue depth before it
                        // enters the channel; the guard rolls the count back on
                        // send failure, panic, or abort before the commit, so
                        // the lock-free counter never under-reads or leaks.
                        let guard = DepthGuard::count_in(&state.depth, 1);
                        // Attach the in-flight claim at the same acceptance
                        // boundary (drainclaim): the envelope owns it from
                        // enqueue until the route pipeline's successor claim
                        // takes over at the dispatch handoff. Every failed
                        // push below (timeout, queue-full, closed) drops the
                        // envelope and releases the claim, mirroring the
                        // guard rollback above.
                        let envelope = ExchangeEnvelope {
                            exchange,
                            reply_tx,
                            in_flight_claim: in_flight.as_ref().map(InFlightClaim::attach),
                        };
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
                                // Typed provenance gate (rc-3px7o): same
                                // marker kind as the pre-enqueue fanout
                                // gate, distinct site for the empty
                                // subscriber list at dispatch.
                                return Err(fanout_subscriber_list_gate_rejection(
                                    &state.config.name,
                                ));
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
                            // One in-flight claim per subscriber copy
                            // (drainclaim): the minted claim rides the first
                            // copy, each additional copy rides a `split()`
                            // sibling. Un-sent claims drop on panic and
                            // self-release, mirroring the guard rollback.
                            let mut claims = fanout_claims(in_flight.as_ref(), permits.len());
                            for permit in permits {
                                permit.send(ExchangeEnvelope {
                                    exchange: original.clone(),
                                    reply_tx: None,
                                    in_flight_claim: claims.next(),
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
                            // One in-flight claim per subscriber copy
                            // (drainclaim): the minted claim rides the first
                            // copy, each additional copy rides a `split()`
                            // sibling. Un-sent claims drop on panic and
                            // self-release, mirroring the guard rollback.
                            let mut claims = fanout_claims(in_flight.as_ref(), permits.len());
                            for permit in permits {
                                permit.send(ExchangeEnvelope {
                                    exchange: original.clone(),
                                    reply_tx: None,
                                    in_flight_claim: claims.next(),
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
                in_flight_claim: None,
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
        assert_endpoint_failure_payload(
            result.expect_err("send without consumers must be rejected"),
            "SEDA endpoint 'nocons' has no active consumers",
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
        assert_endpoint_failure_payload(
            result.expect_err("send after consumer stop must be rejected"),
            "SEDA endpoint 'stop' has no active consumers",
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

    // --- Explicit startup handshake (rc-dbrkr) ---

    fn started_consumer_ctx() -> (
        ConsumerContext,
        camel_component_api::StartupReceiver,
        mpsc::Receiver<ExchangeEnvelope>,
    ) {
        let (signal, receiver) = camel_component_api::StartupSignal::pair();
        let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string())
            .with_startup(signal);
        (ctx, receiver, rx)
    }

    #[test]
    fn test_seda_consumer_startup_mode_is_explicit() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:explicit", &NoOpComponentContext)
            .unwrap();
        let consumer = ep.create_consumer(rt()).unwrap();
        assert_eq!(
            consumer.startup_mode(),
            ConsumerStartupMode::Explicit,
            "seda consumers must gate route startup on activation"
        );
    }

    #[tokio::test]
    async fn test_seda_start_signals_readiness_after_activation_single() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:ready", &NoOpComponentContext)
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (ctx, receiver, _route_rx) = started_consumer_ctx();
        consumer.start(ctx).await.unwrap();

        // Readiness must be resolvable the moment start() returned Ok.
        receiver
            .await_ready()
            .await
            .expect("readiness must be signalled after activation");
        // Behavioral activation proof: the pre-enqueue gate passes on the
        // first attempt after start() returned Ok.
        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("activated")))
            .await
            .expect("send after start must pass the pre-enqueue gate");

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_start_signals_readiness_after_registration_fanout() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:fanready?multipleConsumers=true",
                &NoOpComponentContext,
            )
            .unwrap();

        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (ctx, receiver, mut route_rx) = started_consumer_ctx();
        consumer.start(ctx).await.unwrap();

        receiver
            .await_ready()
            .await
            .expect("readiness must be signalled after activation");
        // Behavioral registration proof: the fanout producer finds the
        // subscriber immediately after start() returned Ok.
        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .oneshot(Exchange::new(Message::new("fan registered")))
            .await
            .expect("fanout send after start must find the registered subscriber");
        let forwarded = tokio::time::timeout(Duration::from_millis(500), route_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            forwarded.exchange.input.body.as_text(),
            Some("fan registered")
        );

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_start_error_does_not_signal_readiness() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:noready", &NoOpComponentContext)
            .unwrap();

        // First consumer holds the Single-mode receiver.
        let mut first = ep.create_consumer(rt()).unwrap();
        let (ctx, _receiver, _route_rx) = started_consumer_ctx();
        first.start(ctx).await.unwrap();

        // Second consumer's start() fails before readiness.
        let mut second = ep.create_consumer(rt()).unwrap();
        let (ctx, receiver, _route_rx) = started_consumer_ctx();
        let result = second.start(ctx).await;
        assert!(result.is_err(), "duplicate Single consumer must fail start");

        // The failure surfaced as an Err without signalling readiness: the
        // receiver must stay Pending or resolve as Err (drop semantics) —
        // never Ok.
        let outcome = tokio::time::timeout(Duration::from_millis(50), receiver.await_ready()).await;
        assert!(
            !matches!(&outcome, Ok(Ok(()))),
            "error path must not signal readiness, got {outcome:?}"
        );

        first.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_seda_restart_signals_readiness_and_flows_buffered() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:restart", &NoOpComponentContext)
            .unwrap();

        let mut first = ep.create_consumer(rt()).unwrap();
        let (ctx, _receiver, mut rx1) = started_consumer_ctx();
        first.start(ctx).await.unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("survivor")))
            .await
            .unwrap();

        first.stop().await.unwrap();

        // Fresh consumer instance for the same endpoint: readiness must be
        // signalled only after the new active flag was stored, and the
        // buffered envelope must flow without any test-side probe.
        let mut second = ep.create_consumer(rt()).unwrap();
        let (ctx, receiver, mut rx2) = started_consumer_ctx();
        second.start(ctx).await.unwrap();
        receiver
            .await_ready()
            .await
            .expect("restart readiness must be signalled");
        // Behavioral reactivation proof: the fresh consumer's activation
        // publishes the gate again — the first send after the restart passes.
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("after restart")))
            .await
            .expect("send after restart must pass the pre-enqueue gate");

        // The envelope survived the restart: it arrives on either the old
        // consumer's route channel (already forwarded) or the new one
        // (still queued across stop).
        let delivered = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                tokio::select! {
                    env = rx1.recv() => {
                        if let Some(env) = env
                            && env.exchange.input.body.as_text() == Some("survivor")
                        {
                            return true;
                        }
                    }
                    env = rx2.recv() => {
                        if let Some(env) = env
                            && env.exchange.input.body.as_text() == Some("survivor")
                        {
                            return true;
                        }
                    }
                }
            }
        })
        .await
        .expect("buffered envelope must survive the restart");
        assert!(delivered);

        second.stop().await.unwrap();
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
            loop {
                match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                    Ok(Some(envelope)) => {
                        counter_clone.fetch_add(1, Ordering::SeqCst);
                        let _ = envelope;
                    }
                    Ok(None) => break, // closed: drain complete
                    Err(_) => break,   // stalled: drainer ends
                }
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
            loop {
                match tokio::time::timeout(Duration::from_secs(2), route_rx.recv()).await {
                    Ok(Some(envelope)) => {
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            if let Some(reply_tx) = envelope.reply_tx {
                                let _ = reply_tx.send(Ok(envelope.exchange));
                            }
                        });
                    }
                    Ok(None) => break, // closed: drain complete
                    Err(_) => break,   // stalled: drainer ends
                }
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
                        in_flight_claim: None,
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

    /// Plain `EndpointCreationFailed` literals — even byte-exact
    /// reproductions of the canonical gate wording — never classify as the
    /// gate under typed provenance (rc-3px7o): only the marker in the
    /// source chain classifies, so such text stays a retryable startup
    /// race. The canonical-string rows mirror
    /// `foreign_text_never_classifies_gate` rows c/d; the negative rows
    /// below keep other variants ineligible.
    #[test]
    fn gate_predicate_matches_both_modes_only() {
        assert!(!is_no_active_consumers_gate(
            &CamelError::EndpointCreationFailed(
                "SEDA endpoint 'x' has no active consumers".to_string()
            )
        ));
        assert!(is_direct_startup_race(&CamelError::EndpointCreationFailed(
            "SEDA endpoint 'x' has no active consumers".to_string()
        )));
        assert!(!is_no_active_consumers_gate(
            &CamelError::EndpointCreationFailed(
                "SEDA endpoint 'x' has no active subscribers".to_string()
            )
        ));
        assert!(is_direct_startup_race(&CamelError::EndpointCreationFailed(
            "SEDA endpoint 'x' has no active subscribers".to_string()
        )));
        assert!(!is_no_active_consumers_gate(
            &CamelError::EndpointCreationFailed(
                "endpoint 'x' already has a registered consumer".to_string()
            )
        ));
        assert!(is_direct_startup_race(&CamelError::EndpointCreationFailed(
            "endpoint 'x' already has a registered consumer".to_string()
        )));
        assert!(!is_no_active_consumers_gate(&CamelError::Config(
            "unrelated".to_string()
        )));
    }

    /// The producer emits the Single-mode gate wording when the endpoint has
    /// no active consumers — the predicate tests above pin classification,
    /// these pin the message text the producer actually produces (rc-fr20u:
    /// the crate owns its message text).
    #[tokio::test]
    async fn producer_gate_wording_single_mode() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:wording-single", &NoOpComponentContext)
            .unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let err = producer
            .oneshot(Exchange::new(Message::new("no consumers")))
            .await
            .expect_err("send without consumers must be rejected");
        assert_endpoint_failure_payload(
            err,
            "SEDA endpoint 'wording-single' has no active consumers",
        );
    }

    /// The producer emits the Fanout-mode gate wording when the endpoint has
    /// no active subscribers (rc-fr20u).
    #[tokio::test]
    async fn producer_gate_wording_fanout_mode() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:wording-fanout?multipleConsumers=true",
                &NoOpComponentContext,
            )
            .unwrap();

        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let err = producer
            .oneshot(Exchange::new(Message::new("no subscribers")))
            .await
            .expect_err("send without subscribers must be rejected");
        assert_endpoint_failure_payload(
            err,
            "SEDA endpoint 'wording-fanout' has no active subscribers",
        );
    }

    /// `is_direct_startup_race` is the shared structural classification for
    /// the consumer-startup race a sender may safely retry (rc-utx98): the
    /// `EndpointCreationFailed` variant minus the SEDA no-active-consumers
    /// gate, which fails fast. The direct component's poll_ready wording
    /// ("direct endpoint '{}' not registered") is retryable.
    #[test]
    fn direct_startup_race_poll_ready_wording_is_retryable() {
        assert!(is_direct_startup_race(&CamelError::EndpointCreationFailed(
            "direct endpoint 'out' not registered".to_string()
        )));
    }

    /// The direct component's call-site wording ("no consumer registered
    /// for direct:{name}") is retryable too — the variant, not the Display
    /// wording, carries the classification (rc-fr20u doctrine).
    #[test]
    fn direct_startup_race_call_wording_is_retryable() {
        assert!(is_direct_startup_race(&CamelError::EndpointCreationFailed(
            "no consumer registered for direct:out".to_string()
        )));
    }

    /// Conservative by design: any non-gate `EndpointCreationFailed` is
    /// treated as a retryable startup race.
    #[test]
    fn generic_endpoint_creation_failed_is_retryable() {
        assert!(is_direct_startup_race(&CamelError::EndpointCreationFailed(
            "boom".to_string()
        )));
    }

    /// The genuine Single-mode typed gate fails fast — retrying duplicates
    /// already-executed side effects (rc-tgaxf). The gate is captured
    /// behaviorally from a real consumerless SEDA endpoint, so the typed
    /// provenance marker must be present for classification (rc-3px7o).
    #[tokio::test]
    async fn seda_gate_single_fails_fast() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:gate-single", &NoOpComponentContext)
            .unwrap();
        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let err = producer
            .oneshot(Exchange::new(Message::new("gate")))
            .await
            .expect_err("send without consumers must be rejected");
        assert!(!is_direct_startup_race(&err));
        assert!(is_no_active_consumers_gate(&err));
    }

    /// The genuine Fanout-mode typed gate fails fast (rc-tgaxf); behavioral
    /// capture as in [`seda_gate_single_fails_fast`].
    #[tokio::test]
    async fn seda_gate_fanout_fails_fast() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:gate-fanout?multipleConsumers=true",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = ep.create_producer(rt(), &test_producer_ctx()).unwrap();
        let err = producer
            .oneshot(Exchange::new(Message::new("gate")))
            .await
            .expect_err("send without subscribers must be rejected");
        assert!(!is_direct_startup_race(&err));
        assert!(is_no_active_consumers_gate(&err));
    }

    /// The 187-doctrine boundary: the former text sniff retried a
    /// `ProcessorError` carrying "not registered"; structurally it is
    /// terminal — never retried. Only the variant decides (rc-utx98).
    #[test]
    fn text_carrying_processor_error_is_terminal() {
        assert!(!is_direct_startup_race(&CamelError::ProcessorError(
            "endpoint 'x' not registered".to_string()
        )));
    }

    /// A generic `ProcessorError` is terminal — never a startup race.
    #[test]
    fn generic_processor_error_is_terminal() {
        assert!(!is_direct_startup_race(&CamelError::ProcessorError(
            "boom".to_string()
        )));
    }

    /// Unrelated variants are terminal: no variant, no retry.
    #[test]
    fn unrelated_variants_are_terminal() {
        assert!(!is_direct_startup_race(&CamelError::ComponentNotFound(
            "direct".to_string()
        )));
        assert!(!is_direct_startup_race(&CamelError::Io("boom".to_string())));
        assert!(!is_direct_startup_race(&CamelError::Config(
            "unrelated".to_string()
        )));
    }

    // --- Typed gate provenance (rc-3px7o) ---

    static GATE_MARKER: NoActiveConsumersGate = NoActiveConsumersGate::Single;

    /// Marker-chain wrapper pair: `WrapA` → `WrapB` → gate marker.
    #[derive(Debug)]
    struct WrapA;

    impl std::fmt::Display for WrapA {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("wrap a")
        }
    }

    impl std::error::Error for WrapA {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&WrapB)
        }
    }

    /// Tail of the marker chain: its source is the gate marker.
    #[derive(Debug)]
    struct WrapB;

    impl std::fmt::Display for WrapB {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("wrap b")
        }
    }

    impl std::error::Error for WrapB {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&GATE_MARKER)
        }
    }

    /// A foreign source chain head that never reaches the gate marker.
    #[derive(Debug)]
    struct ForeignSource;

    impl std::fmt::Display for ForeignSource {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("foreign source")
        }
    }

    impl std::error::Error for ForeignSource {}

    /// Linked wrapper chain: `ChainHop(n)` links through `n` wrappers to
    /// the gate marker, so `ChainHop(7)` places the marker exactly at the
    /// walk limit. Nodes are tiny leaked test fixtures.
    #[derive(Debug)]
    struct ChainHop(u16);

    impl std::fmt::Display for ChainHop {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "chain hop {}", self.0)
        }
    }

    impl std::error::Error for ChainHop {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            if self.0 <= 1 {
                Some(&GATE_MARKER)
            } else {
                Some(Box::leak(Box::new(ChainHop(self.0 - 1))))
            }
        }
    }

    /// A Single-mode gate rejection round-trips through both classifiers:
    /// typed provenance marks the gate (fail fast) and the Display carries
    /// the canonical Single-mode wording.
    #[test]
    fn gate_rejection_single_round_trip() {
        let e = single_mode_gate_rejection("q");
        assert!(is_no_active_consumers_gate(&e));
        assert!(!is_direct_startup_race(&e));
        assert_eq!(
            e.to_string(),
            "Endpoint creation failed: SEDA endpoint 'q' has no active consumers"
        );
    }

    /// A Fanout-mode gate rejection round-trips through both classifiers
    /// with the canonical Fanout wording.
    #[test]
    fn gate_rejection_fanout_round_trip() {
        let e = fanout_preenqueue_gate_rejection("q");
        assert!(is_no_active_consumers_gate(&e));
        assert!(!is_direct_startup_race(&e));
        assert_eq!(
            e.to_string(),
            "Endpoint creation failed: SEDA endpoint 'q' has no active subscribers"
        );
    }

    /// A gate marker deeper in the source chain (behind foreign wrappers)
    /// still classifies within the bounded walk.
    #[test]
    fn gate_rejection_nested_source_chain_classifies_within_bound() {
        let e = CamelError::EndpointCreationFailedWithSource(
            "outer".to_string(),
            OpaqueErrorSource::new(Arc::new(WrapA)),
        );
        assert!(is_no_active_consumers_gate(&e));
        assert!(!is_direct_startup_race(&e));
    }

    /// Seven wrappers place the marker at exactly hop depth 8 (1 = the
    /// variant's own source, 2-8 = the wrapper chain) — the walk limit.
    #[test]
    fn gate_rejection_at_hop_limit_classifies() {
        let e = CamelError::EndpointCreationFailedWithSource(
            "outer".to_string(),
            OpaqueErrorSource::new(Arc::new(ChainHop(7))),
        );
        assert!(is_no_active_consumers_gate(&e));
        assert!(!is_direct_startup_race(&e));
    }

    /// Eight wrappers push the marker to hop depth 9 — beyond the limit —
    /// so the typed failure stays a retryable startup race.
    #[test]
    fn gate_rejection_beyond_hop_limit_stays_retryable() {
        let e = CamelError::EndpointCreationFailedWithSource(
            "outer".to_string(),
            OpaqueErrorSource::new(Arc::new(ChainHop(8))),
        );
        assert!(!is_no_active_consumers_gate(&e));
        assert!(is_direct_startup_race(&e));
    }

    /// The marker's Display is deliberately non-canonical diagnostic text:
    /// it never equals a canonical gate message, so text can never stand in
    /// for provenance.
    #[test]
    fn marker_display_is_non_canonical() {
        let single = single_mode_gate_rejection("q");
        let fanout = fanout_preenqueue_gate_rejection("q");
        let single_marker = gate_from_error(&single).expect("single gate marker");
        let fanout_marker = gate_from_error(&fanout).expect("fanout gate marker");
        assert_eq!(
            single_marker.to_string(),
            "seda no-active-consumers gate rejection (single mode)"
        );
        assert_eq!(
            fanout_marker.to_string(),
            "seda no-active-consumers gate rejection (fanout mode)"
        );
        assert_ne!(
            single_marker.to_string(),
            "SEDA endpoint 'q' has no active consumers"
        );
        assert_ne!(
            single_marker.to_string(),
            "SEDA endpoint 'q' has no active subscribers"
        );
    }

    /// Foreign messages containing the consumer/subscriber wordings —
    /// including byte-exact imitations of both canonical strings — never
    /// classify as the gate and stay retryable startup races.
    #[test]
    fn foreign_text_never_classifies_gate() {
        let foreign_wordings = [
            "kafka topic 'orders' has no active consumers (broker=1)",
            "ws channel 'ch' has no active subscribers upstream",
            "SEDA endpoint 'q' has no active consumers",
            "SEDA endpoint 'q' has no active subscribers",
            "WARNING: SEDA endpoint 'q' has no active consumers (attempt 1)",
            "SEDA endpoint 'q' has no active consumers and 2 more issues",
            "seda endpoint 'q' HAS NO ACTIVE CONSUMERS",
            "SEDA endpoint '' has no active consumers",
        ];
        for wording in foreign_wordings {
            let err = CamelError::EndpointCreationFailed(wording.to_string());
            assert!(
                !is_no_active_consumers_gate(&err),
                "foreign wording must not classify as gate: {wording}"
            );
            assert!(
                is_direct_startup_race(&err),
                "foreign wording must stay retryable: {wording}"
            );
        }
    }

    /// A typed endpoint failure whose source chain lacks the gate marker
    /// stays a retryable startup race.
    #[test]
    fn typed_non_gate_source_stays_retryable() {
        let e = CamelError::EndpointCreationFailedWithSource(
            "foreign".to_string(),
            OpaqueErrorSource::new(Arc::new(ForeignSource)),
        );
        assert!(!is_no_active_consumers_gate(&e));
        assert!(is_direct_startup_race(&e));
    }

    /// Other SEDA endpoint-creation failures stay retryable — regression
    /// pin that the gate never widens over sibling wordings.
    #[test]
    fn other_seda_wordings_stay_retryable() {
        let other_wordings = [
            "SEDA queue 'q' is full (size=1)",
            "SEDA producer timeout enqueueing on 'q' (1000ms)",
            "SEDA fanout timeout on 'q' (1000ms)",
            "multipleConsumers=true with waitForTaskToComplete != Never is not \
             supported — a single request cannot have N valid replies without \
             aggregator semantics",
        ];
        for wording in other_wordings {
            let err = CamelError::EndpointCreationFailed(wording.to_string());
            assert!(
                !is_no_active_consumers_gate(&err),
                "sibling wording must not classify as gate: {wording}"
            );
            assert!(
                is_direct_startup_race(&err),
                "sibling wording must stay retryable: {wording}"
            );
        }
    }

    // --- Typed terminal-config provenance (sedaretry) ---

    static TERMINAL_MARKER: TerminalConfigError =
        TerminalConfigError::MultipleConsumersWaitConflict;

    /// A LOCAL test-only marker mimicking a FOREIGN crate's terminal-config
    /// marker: the seda walk probes only for the seda marker type, so this
    /// imitation must never classify.
    #[derive(Debug)]
    struct ForeignTerminalMarker;

    impl std::fmt::Display for ForeignTerminalMarker {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("foreign terminal-config marker")
        }
    }

    impl std::error::Error for ForeignTerminalMarker {}

    /// Terminal-config twin of [`ChainHop`]: `TerminalChainHop(n)` links
    /// through `n` wrappers to the terminal-config marker, so
    /// `TerminalChainHop(7)` places the marker exactly at the walk limit.
    /// Nodes are tiny leaked test fixtures.
    #[derive(Debug)]
    struct TerminalChainHop(u16);

    impl std::fmt::Display for TerminalChainHop {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "terminal chain hop {}", self.0)
        }
    }

    impl std::error::Error for TerminalChainHop {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            if self.0 <= 1 {
                Some(&TERMINAL_MARKER)
            } else {
                Some(Box::leak(Box::new(TerminalChainHop(self.0 - 1))))
            }
        }
    }

    /// The genuine multipleConsumers+wait reject fails fast (sedaretry):
    /// behavioral capture from a fanout endpoint with a STARTED consumer and
    /// a producer whose `waitForTaskToComplete` is Always — the gate check
    /// passes (active consumer) and the config conflict rejects. Both
    /// predicates are pinned, and the outer detail stays byte-identical to
    /// the historical wording.
    #[tokio::test]
    async fn genuine_config_reject_classifies_terminal() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:tconf?multipleConsumers=true", &NoOpComponentContext)
            .unwrap();

        // STARTED consumer so the pre-enqueue gate passes (reuse of the
        // explicit-startup handshake pieces).
        let mut consumer = ep.create_consumer(rt()).unwrap();
        let (ctx, receiver, _route_rx) = started_consumer_ctx();
        consumer.start(ctx).await.unwrap();
        receiver
            .await_ready()
            .await
            .expect("readiness must be signalled after activation");

        // Producer-only option on a same-name endpoint (compatible per
        // `is_compatible_with`, which ignores producer-only options).
        let producer_ep = comp
            .create_endpoint(
                "seda:tconf?multipleConsumers=true&waitForTaskToComplete=Always",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = producer_ep
            .create_producer(rt(), &test_producer_ctx())
            .unwrap();
        let err = producer
            .oneshot(Exchange::new(Message::new("config conflict")))
            .await
            .expect_err("multipleConsumers+wait send must be rejected");
        assert!(is_seda_terminal_config_error(&err));
        assert!(!is_direct_startup_race(&err));
        assert_endpoint_failure_payload(
            err,
            "multipleConsumers=true with waitForTaskToComplete != Never is not \
             supported — a single request cannot have N valid replies without \
             aggregator semantics",
        );

        consumer.stop().await.unwrap();
    }

    /// A plain `EndpointCreationFailed` byte-matching the canonical config
    /// wording carries no typed marker — typed provenance only, so the
    /// imitation stays a retryable startup race.
    #[test]
    fn foreign_config_wording_imitation_stays_retryable() {
        let e = CamelError::EndpointCreationFailed(
            "multipleConsumers=true with waitForTaskToComplete != Never is not \
             supported — a single request cannot have N valid replies without \
             aggregator semantics"
                .to_string(),
        );
        assert!(!is_seda_terminal_config_error(&e));
        assert!(is_direct_startup_race(&e));
    }

    /// A typed endpoint failure whose source chain carries a foreign error
    /// type instead of the terminal-config marker stays a retryable startup
    /// race.
    #[test]
    fn typed_with_foreign_source_not_terminal_config() {
        let e = CamelError::EndpointCreationFailedWithSource(
            "foreign".to_string(),
            OpaqueErrorSource::new(Arc::new(ForeignSource)),
        );
        assert!(!is_seda_terminal_config_error(&e));
        assert!(is_direct_startup_race(&e));
    }

    /// An endpoint-creation failure whose source is a local test-only marker
    /// mimicking a FOREIGN crate's terminal marker never classifies as the
    /// seda terminal-config error — the walk matches only the seda type —
    /// so it stays a retryable startup race.
    #[test]
    fn foreign_terminal_marker_imitation_stays_retryable() {
        let e = CamelError::EndpointCreationFailedWithSource(
            "foreign terminal marker".to_string(),
            OpaqueErrorSource::new(Arc::new(ForeignTerminalMarker)),
        );
        assert!(!is_seda_terminal_config_error(&e));
        assert!(is_direct_startup_race(&e));
    }

    /// Seven wrappers place the terminal-config marker at exactly hop depth
    /// 8 (1 = the variant's own source, 2-8 = the wrapper chain) — the walk
    /// limit — so the typed failure classifies as terminal.
    #[test]
    fn terminal_config_marker_at_hop_limit_classifies() {
        let e = CamelError::EndpointCreationFailedWithSource(
            "outer".to_string(),
            OpaqueErrorSource::new(Arc::new(TerminalChainHop(7))),
        );
        assert!(is_seda_terminal_config_error(&e));
        assert!(!is_direct_startup_race(&e));
    }

    /// Eight wrappers push the marker to hop depth 9 — beyond the limit —
    /// so the typed failure stays a retryable startup race.
    #[test]
    fn terminal_config_marker_beyond_limit_stays_retryable() {
        let e = CamelError::EndpointCreationFailedWithSource(
            "outer".to_string(),
            OpaqueErrorSource::new(Arc::new(TerminalChainHop(8))),
        );
        assert!(!is_seda_terminal_config_error(&e));
        assert!(is_direct_startup_race(&e));
    }

    /// The endpoint config-conflict rejection (detretry, rc-zovuy):
    /// creating a same-name endpoint with incompatible `size` carries the
    /// typed [`TerminalConfigError`] marker, so the real carrier classifies
    /// as a terminal-config error.
    #[test]
    fn config_conflict_rejection_carries_terminal_marker() {
        let comp = create_component();
        let _ep = comp
            .create_endpoint("seda:conflict?size=10", &NoOpComponentContext)
            .unwrap();
        let err = match comp.create_endpoint("seda:conflict?size=5", &NoOpComponentContext) {
            Err(e) => e,
            Ok(_) => panic!("incompatible same-name config must be rejected"),
        };
        assert!(is_seda_terminal_config_error(&err));
    }

    /// The endpoint config-conflict rejection is deterministic (rc-zovuy):
    /// the same carrier must NOT classify as a retryable startup race.
    #[test]
    fn config_conflict_rejection_not_startup_race() {
        let comp = create_component();
        let _ep = comp
            .create_endpoint("seda:conflict?size=10", &NoOpComponentContext)
            .unwrap();
        let err = match comp.create_endpoint("seda:conflict?size=5", &NoOpComponentContext) {
            Err(e) => e,
            Ok(_) => panic!("incompatible same-name config must be rejected"),
        };
        assert!(!is_direct_startup_race(&err));
    }

    /// The endpoint config-conflict detail stays BYTE-IDENTICAL to the
    /// historical `is_compatible_with` wording (rc-zovuy): exact equality
    /// on the variant's detail field, not a substring of the rendered
    /// Display.
    #[test]
    fn config_conflict_detail_byte_identical() {
        let comp = create_component();
        let _ep = comp
            .create_endpoint("seda:q?size=10", &NoOpComponentContext)
            .unwrap();
        let err = match comp.create_endpoint("seda:q?size=5", &NoOpComponentContext) {
            Err(e) => e,
            Ok(_) => panic!("incompatible same-name config must be rejected"),
        };
        let CamelError::EndpointCreationFailedWithSource(detail, _) = err else {
            panic!("config conflict must arrive as EndpointCreationFailedWithSource");
        };
        assert_eq!(
            detail,
            "endpoint 'q' already exists with different config: size: 10 vs 5"
        );
    }

    /// A plain `EndpointCreationFailed` byte-matching the endpoint
    /// config-conflict wording carries no typed marker — typed provenance
    /// only, so the imitation stays a retryable startup race (rc-3px7o).
    #[test]
    fn foreign_config_conflict_imitation_stays_retryable() {
        let e = CamelError::EndpointCreationFailed(
            "endpoint 'q' already exists with different config: size: 10 vs 5".to_string(),
        );
        assert!(!is_seda_terminal_config_error(&e));
        assert!(is_direct_startup_race(&e));
    }

    /// The Single-mode site function produces the byte-exact canonical
    /// Single-mode detail and classifies as the gate.
    #[test]
    fn single_gate_site_detail_byte_exact() {
        let e = single_mode_gate_rejection("site-q");
        assert_eq!(
            e.to_string(),
            "Endpoint creation failed: SEDA endpoint 'site-q' has no active consumers"
        );
        assert!(is_no_active_consumers_gate(&e));
    }

    /// The Fanout pre-enqueue site function produces the byte-exact
    /// canonical Fanout detail and classifies as the gate.
    #[test]
    fn fanout_preenqueue_gate_site_detail_byte_exact() {
        let e = fanout_preenqueue_gate_rejection("site-q");
        assert_eq!(
            e.to_string(),
            "Endpoint creation failed: SEDA endpoint 'site-q' has no active subscribers"
        );
        assert!(is_no_active_consumers_gate(&e));
    }

    /// The Fanout subscriber-list site function produces the byte-exact
    /// canonical Fanout detail and classifies as the gate.
    #[test]
    fn fanout_subscriber_list_gate_site_detail_byte_exact() {
        let e = fanout_subscriber_list_gate_rejection("site-q");
        assert_eq!(
            e.to_string(),
            "Endpoint creation failed: SEDA endpoint 'site-q' has no active subscribers"
        );
        assert!(is_no_active_consumers_gate(&e));
    }

    /// `has_active_consumer` is the readiness-probe signal for senders that
    /// must not retry (rc-zjrx): unknown names and known-but-consumerless
    /// endpoints report false; a started consumer flips it true; stop flips
    /// it back; clones share the registry.
    #[tokio::test]
    async fn has_active_consumer_tracks_consumer_lifecycle() {
        let comp = create_component();
        let _ep = comp
            .create_endpoint("seda:probe1", &NoOpComponentContext)
            .unwrap();

        assert!(
            !comp.has_active_consumer("probe1"),
            "endpoint without consumer must report inactive"
        );
        assert!(
            !comp.has_active_consumer("never-created"),
            "unknown endpoint name must report inactive"
        );

        let state = comp
            .endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get("probe1")
            .cloned()
            .unwrap();
        let mut consumer = SedaConsumer::new(state, next_consumer_id(), rt());
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "seda-test-route".to_string());
        consumer.start(ctx).await.unwrap();

        let cloned = comp.clone();
        assert!(
            cloned.has_active_consumer("probe1"),
            "started consumer must report active through a clone"
        );

        consumer.stop().await.unwrap();
        assert!(
            !comp.has_active_consumer("probe1"),
            "stopped consumer must report inactive"
        );
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
                in_flight_claim: None,
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
            in_flight_claim: None,
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
            in_flight_claim: None,
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
            loop {
                match tokio::time::timeout(Duration::from_secs(2), rx_b.recv()).await {
                    Ok(Some(env)) => {
                        drained_clone.fetch_add(1, Ordering::SeqCst);
                        let _ = env;
                    }
                    Ok(None) => break, // closed: drain complete
                    Err(_) => break,   // stalled: drainer ends
                }
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
        tokio::time::timeout(Duration::from_millis(1350), async {
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
        })
        .await
        .expect("post-settle gauge samples must arrive within 1350ms");

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

// ---------------------------------------------------------------------------
// In-flight claim tests (drainclaim task 1.5)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod in_flight_tests {
    use super::*;
    use camel_api::MetricsCollector;
    use camel_component_api::{
        HealthCheckRegistry, Message, NoOpComponentContext, RuntimeObservability,
    };
    use tower::ServiceExt;

    /// Test runtime reporting a shared in-flight counter through
    /// `RuntimeObservability::in_flight_counter` (drainclaim): producers
    /// created with it mint real enqueue claims against `counter`.
    struct CountingRuntime {
        in_flight: Arc<AtomicU64>,
    }

    impl RuntimeObservability for CountingRuntime {
        fn metrics(&self) -> Arc<dyn MetricsCollector> {
            Arc::new(NoopRuntimeObservability)
        }
        fn health(&self) -> Arc<dyn HealthCheckRegistry> {
            Arc::new(NoopRuntimeObservability)
        }
        fn in_flight_counter(&self) -> Option<Arc<AtomicU64>> {
            Some(Arc::clone(&self.in_flight))
        }
    }

    fn counting_rt(counter: &Arc<AtomicU64>) -> Arc<dyn RuntimeObservability> {
        Arc::new(CountingRuntime {
            in_flight: Arc::clone(counter),
        })
    }

    /// Deadline-bounded poll for the in-flight counter to reach exactly
    /// `expected` (helper-fn poll pattern — no bare test-fn sleeps). Sound
    /// only for states that are STABLE once reached (settled claims,
    /// parked forwarders); never a sampler of transient windows.
    async fn await_in_flight(counter: &AtomicU64, expected: u64) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let seen = counter.load(Ordering::Acquire);
            if seen == expected {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "in-flight counter stuck at {seen}, expected {expected}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// spec: "seda queue residency is counted" — one exchange stays
    /// counted from enqueue until full pipeline completion: the enqueue
    /// shell claim hands the count over to the pipeline's successor claim
    /// (minted by `ConsumerContext::send`), so the counter reads exactly 1
    /// while the pipeline parks and returns to baseline on completion.
    #[tokio::test]
    async fn enqueue_residency_counted() {
        let counter = Arc::new(AtomicU64::new(0));
        let runtime = counting_rt(&counter);

        let comp = SedaComponent::new();
        let ep = comp
            .create_endpoint("seda:res", &NoOpComponentContext)
            .unwrap();

        // Parked pipeline: takes-and-holds the first envelope on a gate,
        // exactly like the real pipeline's take-and-hold at the drain
        // sites (drainclaim task 1.4).
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new(), "res-route".to_string())
            .with_in_flight_counter(Arc::clone(&counter));
        let mut consumer = ep.create_consumer(Arc::clone(&runtime)).unwrap();
        consumer.start(ctx).await.unwrap();

        let (got_tx, got_rx) = oneshot::channel::<()>();
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let pipeline = tokio::spawn(async move {
            let held = tokio::time::timeout(Duration::from_secs(2), route_rx.recv())
                .await
                .expect("pipeline receives exchange within 2s")
                .expect("route channel alive");
            let _ = got_tx.send(());
            let _ = release_rx.await;
            drop(held); // pipeline completion releases the claim
        });

        let producer = ep
            .create_producer(runtime, &ProducerContext::default())
            .unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("resident")))
            .await
            .unwrap();

        // Exchange taken-and-held by the parked pipeline; after the
        // handoff settles only its successor claim is live.
        got_rx.await.unwrap();
        await_in_flight(&counter, 1).await;

        release_tx.send(()).unwrap();
        pipeline.await.unwrap();
        consumer.stop().await.unwrap();

        // Full pipeline completion returns the counter to baseline.
        await_in_flight(&counter, 0).await;
    }

    /// spec: "fanout splits one claim per subscriber copy" — one enqueue
    /// on a two-subscriber fanout mints one claim and splits a sibling, so
    /// each parked pipeline holds exactly one claim; completing the
    /// subscribers releases them one by one.
    #[tokio::test]
    async fn fanout_splits_claim_per_copy() {
        let counter = Arc::new(AtomicU64::new(0));
        let runtime = counting_rt(&counter);

        let comp = SedaComponent::new();
        let ep = comp
            .create_endpoint("seda:fosplit?multipleConsumers=true", &NoOpComponentContext)
            .unwrap();

        let mut consumer_a = ep.create_consumer(Arc::clone(&runtime)).unwrap();
        let (tx_a, mut rx_a) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx_a = ConsumerContext::new(tx_a, CancellationToken::new(), "fan-a".to_string())
            .with_in_flight_counter(Arc::clone(&counter));
        consumer_a.start(ctx_a).await.unwrap();

        let mut consumer_b = ep.create_consumer(Arc::clone(&runtime)).unwrap();
        let (tx_b, mut rx_b) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx_b = ConsumerContext::new(tx_b, CancellationToken::new(), "fan-b".to_string())
            .with_in_flight_counter(Arc::clone(&counter));
        consumer_b.start(ctx_b).await.unwrap();

        let (got_a_tx, got_a_rx) = oneshot::channel::<()>();
        let (release_a_tx, release_a_rx) = oneshot::channel::<()>();
        let pipe_a = tokio::spawn(async move {
            let held = tokio::time::timeout(Duration::from_secs(2), rx_a.recv())
                .await
                .expect("subscriber A copy within 2s")
                .expect("fan-a channel alive");
            let _ = got_a_tx.send(());
            let _ = release_a_rx.await;
            drop(held);
        });
        let (got_b_tx, got_b_rx) = oneshot::channel::<()>();
        let (release_b_tx, release_b_rx) = oneshot::channel::<()>();
        let pipe_b = tokio::spawn(async move {
            let held = tokio::time::timeout(Duration::from_secs(2), rx_b.recv())
                .await
                .expect("subscriber B copy within 2s")
                .expect("fan-b channel alive");
            let _ = got_b_tx.send(());
            let _ = release_b_rx.await;
            drop(held);
        });

        let producer = ep
            .create_producer(runtime, &ProducerContext::default())
            .unwrap();
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("fan")))
            .await
            .unwrap();

        // Both copies taken-and-held: after the handoffs settle, exactly
        // one claim per subscriber copy is live (2 = split minted one
        // claim per copy at enqueue).
        got_a_rx.await.unwrap();
        got_b_rx.await.unwrap();
        await_in_flight(&counter, 2).await;

        // Complete subscriber A: its copy's claim releases; B's stays.
        release_a_tx.send(()).unwrap();
        pipe_a.await.unwrap();
        await_in_flight(&counter, 1).await;

        // Complete subscriber B: back to baseline.
        release_b_tx.send(()).unwrap();
        pipe_b.await.unwrap();
        await_in_flight(&counter, 0).await;

        consumer_a.stop().await.unwrap();
        consumer_b.stop().await.unwrap();
    }

    /// spec: "dispatch handoff never uncovers an exchange" — deterministic
    /// observation of the handoff overlap itself. The pipeline parks its
    /// FIRST exchange (E1) on a gate and the dispatch channel has capacity
    /// 1: E2's successor envelope fills the channel, so the forwarder
    /// holding E3's shell claim parks INSIDE `ctx.send` with E3's successor
    /// claim already minted onto the in-send envelope. That parked state is
    /// STABLE, and the counter reads exactly
    /// baseline + E1 (pipeline-held) + E2 (queued) + E3 shell + E3 successor
    /// = 4 — both handoff claims live at once, observed, not sampled.
    #[tokio::test]
    async fn handoff_overlap_observed_at_boundary() {
        let counter = Arc::new(AtomicU64::new(0));
        let runtime = counting_rt(&counter);

        let comp = SedaComponent::new();
        let ep = comp
            .create_endpoint("seda:ovl", &NoOpComponentContext)
            .unwrap();

        // Capacity-1 dispatch channel: full once E2's successor envelope
        // is queued, forcing the E3 handoff to park inside `ctx.send`.
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new(), "ovl-route".to_string())
            .with_in_flight_counter(Arc::clone(&counter));
        let mut consumer = ep.create_consumer(Arc::clone(&runtime)).unwrap();
        consumer.start(ctx).await.unwrap();

        let (got_e1_tx, got_e1_rx) = oneshot::channel::<()>();
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let pipeline = tokio::spawn(async move {
            // E1: take-and-hold on the gate.
            let e1 = tokio::time::timeout(Duration::from_secs(2), route_rx.recv())
                .await
                .expect("pipeline receives E1 within 2s")
                .expect("route channel alive");
            let _ = got_e1_tx.send(());
            let _ = release_rx.await;
            drop(e1);
            // Post-gate: complete E2 and E3 (exactly two successor
            // envelopes are behind the gate), then end the pipeline.
            for _ in 0..2 {
                let _ = tokio::time::timeout(Duration::from_secs(2), route_rx.recv())
                    .await
                    .expect("successor envelope within 2s")
                    .expect("dispatch channel alive");
            }
        });

        let producer = ep
            .create_producer(runtime, &ProducerContext::default())
            .unwrap();

        // E1 → taken by the parked pipeline: 1 pipeline-held claim.
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("e1")))
            .await
            .unwrap();
        got_e1_rx.await.unwrap();
        await_in_flight(&counter, 1).await;

        // E2 → successor claim queued in the capacity-1 dispatch channel.
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("e2")))
            .await
            .unwrap();
        await_in_flight(&counter, 2).await;

        // E3 → the forwarder parks inside `ctx.send`: E3's shell claim is
        // still held by the forwarder's envelope AND E3's successor claim
        // is already attached to the in-send envelope. Exactly 4 = 0
        // baseline + 1 (E1 pipeline-held) + 1 (E2 queued) + 1 (E3 shell)
        // + 1 (E3 in-send successor). This observes the overlap window
        // itself (stable until the gate opens), not a sample of it.
        producer
            .clone()
            .oneshot(Exchange::new(Message::new("e3")))
            .await
            .unwrap();
        await_in_flight(&counter, 4).await;

        // Release: E1 completes, E2 and E3 drain through the pipeline,
        // every claim releases — eventual return to baseline.
        release_tx.send(()).unwrap();
        pipeline.await.unwrap();
        consumer.stop().await.unwrap();
        await_in_flight(&counter, 0).await;
    }

    /// Enqueue against an endpoint with no active consumers and
    /// `discard_if_no_consumers = false` (default) is rejected BEFORE any
    /// claim is minted — the counter stays at baseline.
    #[tokio::test]
    async fn no_consumer_rejection_leaves_counter_zero() {
        let counter = Arc::new(AtomicU64::new(0));
        let runtime = counting_rt(&counter);

        let comp = SedaComponent::new();
        let ep = comp
            .create_endpoint("seda:noc", &NoOpComponentContext)
            .unwrap();

        // No consumer ever started.
        let producer = ep
            .create_producer(runtime, &ProducerContext::default())
            .unwrap();
        let err = producer
            .oneshot(Exchange::new(Message::new("rejected")))
            .await
            .unwrap_err();
        assert_endpoint_failure_payload(err, "SEDA endpoint 'noc' has no active consumers");
        assert_eq!(
            counter.load(Ordering::Acquire),
            0,
            "rejection must not leave a claim behind"
        );
    }
}
