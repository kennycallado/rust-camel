//! In-memory SEDA component for rust-camel — asynchronous staging channel
//! between routes sharing the same context via bounded queues.
//!
//! Main types: `SedaComponent`, `SedaEndpoint`, `SedaConsumer`, `SedaProducer`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::Service;

use camel_api::BoxProcessorExt;
use camel_component_api::parse_uri;
use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, ConcurrencyModel, Consumer,
    ConsumerContext, Endpoint, Exchange, ExchangeEnvelope, ProducerContext,
};
use tracing::{info, warn};

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

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(SedaConsumer {
            state: Arc::clone(&self.state),
            consumer_id: next_consumer_id(),
            started: false,
            cancel_token: CancellationToken::new(),
            forwarder_handles: Vec::new(),
        }))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        let producer = SedaProducer {
            state: Arc::clone(&self.state),
            producer_config: ProducerConfig::from(&self.config),
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

                let cancel = self.cancel_token.clone();
                let handle = tokio::spawn(async move {
                    let mut rx = receiver;
                    loop {
                        tokio::select! {
                            envelope = rx.recv() => {
                                let Some(envelope) = envelope else { break };
                                forward_envelope(&ctx, envelope).await;
                            }
                            _ = cancel.cancelled() => break,
                        }
                    }
                    Ok(())
                });
                self.forwarder_handles.push(handle);
            }
            SedaMode::Fanout { subscribers } => {
                let (tx, rx) = mpsc::channel(self.state.config.size);
                subscribers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(self.consumer_id.clone(), tx);

                let cancel = self.cancel_token.clone();
                let handle = tokio::spawn(async move {
                    let mut rx = rx;
                    loop {
                        tokio::select! {
                            envelope = rx.recv() => {
                                let Some(envelope) = envelope else { break };
                                forward_envelope(&ctx, envelope).await;
                            }
                            _ = cancel.cancelled() => break,
                        }
                    }
                    Ok(())
                });
                self.forwarder_handles.push(handle);
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
            SedaMode::Single { active, .. } => {
                active.store(false, Ordering::SeqCst);
            }
            SedaMode::Fanout { subscribers } => {
                subscribers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&self.consumer_id);
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
async fn forward_envelope(ctx: &ConsumerContext, envelope: ExchangeEnvelope) {
    if let Some(reply_tx) = envelope.reply_tx {
        let result = ctx.send_and_wait(envelope.exchange).await;
        let _ = reply_tx.send(result);
    } else {
        if let Err(e) = ctx.send(envelope.exchange).await {
            warn!(error = %e, "SEDA consumer send failed");
        }
    }
}

// ---------------------------------------------------------------------------
// SedaProducer
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SedaProducer {
    state: Arc<SedaEndpointState>,
    producer_config: ProducerConfig,
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
        Box::pin(async move {
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
                    if producer_config.block_when_full {
                        let result = tokio::time::timeout(
                            Duration::from_millis(producer_config.timeout_ms),
                            tx.send(envelope),
                        )
                        .await;
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => return Err(CamelError::ChannelClosed),
                            Err(_) => {
                                return Err(CamelError::EndpointCreationFailed(format!(
                                    "SEDA producer timeout enqueueing on '{}' ({}ms)",
                                    state.config.name, producer_config.timeout_ms
                                )));
                            }
                        }
                    } else {
                        tx.try_send(envelope).map_err(|e| {
                            if matches!(e, mpsc::error::TrySendError::Full(_)) {
                                CamelError::EndpointCreationFailed(format!(
                                    "SEDA queue '{}' is full (size={})",
                                    state.config.name, state.config.size
                                ))
                            } else {
                                CamelError::ChannelClosed
                            }
                        })?;
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
                        for sender in &sender_list {
                            let cloned = ExchangeEnvelope {
                                exchange: original.clone(),
                                reply_tx: None,
                            };
                            let result = tokio::time::timeout(
                                Duration::from_millis(producer_config.timeout_ms),
                                sender.send(cloned),
                            )
                            .await;
                            if result.is_err() {
                                return Err(CamelError::EndpointCreationFailed(format!(
                                    "SEDA fanout timeout on '{}' ({}ms)",
                                    state.config.name, producer_config.timeout_ms
                                )));
                            }
                        }
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
                        for permit in permits {
                            permit.send(ExchangeEnvelope {
                                exchange: original.clone(),
                                reply_tx: None,
                            });
                        }
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

        let mut consumer = ep.create_consumer().unwrap();
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (route_tx, _) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(1);
        route_tx
            .send(ExchangeEnvelope {
                exchange: Exchange::new(Message::new("dummy")),
                reply_tx: None,
            })
            .await
            .unwrap();
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
        let result = producer.oneshot(Exchange::new(Message::new("test"))).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_seda_duplicate_single_consumer() {
        let comp = create_component();
        let ep = comp
            .create_endpoint("seda:dup", &NoOpComponentContext)
            .unwrap();

        let mut consumer_a = ep.create_consumer().unwrap();
        let (tx_a, _rx_a) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_a = ConsumerContext::new(tx_a, CancellationToken::new());
        consumer_a.start(ctx_a).await.unwrap();

        let mut consumer_b = ep.create_consumer().unwrap();
        let (tx_b, _rx_b) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_b = ConsumerContext::new(tx_b, CancellationToken::new());
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

        let mut consumer_a = ep.create_consumer().unwrap();
        let (tx_a, mut rx_a) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_a = ConsumerContext::new(tx_a, CancellationToken::new());
        consumer_a.start(ctx_a).await.unwrap();

        let mut consumer_b = ep.create_consumer().unwrap();
        let (tx_b, mut rx_b) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_b = ConsumerContext::new(tx_b, CancellationToken::new());
        consumer_b.start(ctx_b).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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
        let consumer = ep.create_consumer().unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (route_tx, _) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer_a = ep.create_consumer().unwrap();
        let (tx_a, _rx_a) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx_a = ConsumerContext::new(tx_a, CancellationToken::new());
        consumer_a.start(ctx_a).await.unwrap();

        let mut consumer_b = ep.create_consumer().unwrap();
        let (tx_b, _rx_b) = mpsc::channel::<ExchangeEnvelope>(1);
        let ctx_b = ConsumerContext::new(tx_b, CancellationToken::new());
        consumer_b.start(ctx_b).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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
    async fn test_seda_discard_if_no_consumers_fanout() {
        let comp = create_component();
        let ep = comp
            .create_endpoint(
                "seda:discardfan?multipleConsumers=true&discardIfNoConsumers=true",
                &NoOpComponentContext,
            )
            .unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer_a = ep.create_producer(&test_producer_ctx()).unwrap();
        let producer_b = ep.create_producer(&test_producer_ctx()).unwrap();

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

        let mut consumer = ep.create_consumer().unwrap();
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(1000);
        let ctx = ConsumerContext::new(tx, CancellationToken::new());
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
            let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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

        let mut consumer = ep.create_consumer().unwrap();
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let producer = ep.create_producer(&test_producer_ctx()).unwrap();
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
}
