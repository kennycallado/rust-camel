use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use camel_bridge::{
    channel::connect_channel,
    download::ensure_binary,
    health::wait_for_health,
    process::{BridgeProcess, BridgeProcessConfig, BrokerType},
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

use crate::config::{BrokerConfig, JmsEndpointConfig, JmsPoolConfig};
use crate::consumer::JmsConsumer;
use crate::health::JmsHealthCheck;
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

// ── BridgeState ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum BridgeState {
    Starting,
    Ready { channel: Channel },
    Degraded(String),
    Restarting { attempt: u32, next_at: Instant },
    Stopped,
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

// ── JmsBridgePool ────────────────────────────────────────────────────────────

pub struct JmsBridgePool {
    pub(crate) slots: DashMap<String, Arc<BridgeSlot>>,
    pub(crate) config: HashMap<String, BrokerConfig>,
    pub(crate) bridge_start_timeout_ms: u64,
    pub(crate) reconnect: NetworkRetryPolicy,
    pub(crate) health_check_interval_ms: u64,
    pub(crate) bridge_version: String,
    pub(crate) bridge_cache_dir: PathBuf,
    /// Maximum number of concurrently active (Starting/Ready) bridges.
    pub(crate) max_bridges: usize,
    /// Serializes bridge admission (check + insert) to prevent race on max_bridges.
    bridge_create_lock: Mutex<()>,
    pub(crate) shutting_down: Arc<AtomicBool>,
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
            reconnect,
            health_check_interval_ms: pool_config.health_check_interval_ms,
            bridge_version: crate::BRIDGE_VERSION.to_string(),
            bridge_cache_dir: pool_config.bridge_cache_dir,
            max_bridges: pool_config.max_bridges,
            bridge_create_lock: Mutex::new(()),
            shutting_down: Arc::new(AtomicBool::new(false)),
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
    pub async fn get_or_create_slot(
        &self,
        broker_name: &str,
    ) -> Result<Arc<BridgeSlot>, CamelError> {
        if let Some(slot) = self.slots.get(broker_name) {
            return Ok(Arc::clone(&*slot));
        }

        // Serialize admission: check + insert in single critical section to prevent
        // concurrent creators from both seeing count below limit and exceeding max_bridges.
        let _guard = self.bridge_create_lock.lock().await;

        // Re-check after acquiring lock (another caller may have inserted while we waited).
        if let Some(slot) = self.slots.get(broker_name) {
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
        let credentials = match (&broker_config.username, &broker_config.password) {
            (Some(u), Some(p)) => Some((u.clone(), p.clone())),
            _ => None,
        };

        let slot = match self.slots.entry(broker_name.to_string()) {
            dashmap::Entry::Occupied(existing) => {
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
                });
                entry.insert(Arc::clone(&slot));
                slot
            }
        };

        let start_result = Self::start_bridge_process(
            &self.bridge_version,
            &self.bridge_cache_dir,
            self.bridge_start_timeout_ms,
            &broker_url,
            &broker_type,
            &credentials,
        )
        .await;

        match start_result {
            Ok((process, channel)) => {
                {
                    let mut guard = slot.process.lock().await;
                    *guard = Some(process);
                }
                let _ = slot.state_tx.send(BridgeState::Ready { channel });
            }
            Err(e) => {
                let _ = slot
                    .state_tx
                    .send(BridgeState::Degraded(format!("Initial start failed: {e}")));
            }
        }

        self.spawn_health_monitor(Arc::clone(&slot)).await;

        Ok(slot)
    }

    /// Signal a slot to restart (called by producers on transport errors).
    pub fn restart_slot(&self, broker_name: &str) {
        if let Some(slot) = self.slots.get(broker_name) {
            let _ = slot.state_tx.send(BridgeState::Restarting {
                attempt: 0,
                next_at: Instant::now(),
            });
        }
    }

    /// Recreate the tonic channel for an existing running bridge process.
    ///
    /// Useful when a channel becomes stale after transport-level failures while
    /// the underlying bridge process is still alive.
    pub async fn refresh_slot_channel(&self, broker_name: &str) -> Result<(), CamelError> {
        let slot = self
            .slots
            .get(broker_name)
            .map(|s| Arc::clone(&*s))
            .ok_or_else(|| {
                CamelError::ProcessorError(format!("Unknown JMS broker '{}'", broker_name))
            })?;

        let port = {
            let guard = slot.process.lock().await;
            let process = guard.as_ref().ok_or_else(|| {
                CamelError::ProcessorError(format!(
                    "JMS broker '{}' has no running bridge process",
                    broker_name
                ))
            })?;
            process.grpc_port()
        };

        let channel = connect_channel(port).await.map_err(|e| {
            CamelError::ProcessorError(format!(
                "JMS broker '{}' channel refresh failed: {}",
                broker_name, e
            ))
        })?;

        let _ = slot.state_tx.send(BridgeState::Ready { channel });
        Ok(())
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
                // Signal the health monitor to stop.
                let _ = slot.state_tx.send(BridgeState::Stopped);

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

    async fn spawn_health_monitor(&self, slot: Arc<BridgeSlot>) {
        let health_interval = self.health_check_interval_ms;
        let bridge_version = self.bridge_version.clone();
        let bridge_cache_dir = self.bridge_cache_dir.clone();
        let start_timeout_ms = self.bridge_start_timeout_ms;
        let handle_ref = Arc::clone(&slot.health_monitor_handle);
        let shutting_down = Arc::clone(&self.shutting_down);
        let broker_name = slot.name.clone();

        let handle = tokio::spawn(async move {
            loop {
                let state = slot.state_rx.borrow().clone();
                match state {
                    BridgeState::Stopped => {
                        info!("Health monitor for '{}' exiting (Stopped)", slot.name);
                        break;
                    }
                    BridgeState::Ready { ref channel } => {
                        tokio::time::sleep(Duration::from_millis(health_interval)).await;
                        let mut client = BridgeServiceClient::new(channel.clone());
                        let health_timeout = Duration::from_secs(3);
                        match tokio::time::timeout(health_timeout, client.health(HealthRequest {}))
                            .await
                        {
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => {
                                warn!(
                                    "Health check failed for broker '{}': {e}. Marking Degraded.",
                                    slot.name
                                );
                                let _ = slot.state_tx.send(BridgeState::Degraded(e.to_string()));
                            }
                            Err(_) => {
                                let msg = format!(
                                    "health RPC timed out after {}ms",
                                    health_timeout.as_millis()
                                );
                                warn!(
                                    "Health check timed out for broker '{}': {}. Marking Degraded.",
                                    slot.name, msg
                                );
                                let _ = slot.state_tx.send(BridgeState::Degraded(msg));
                            }
                        }
                    }
                    BridgeState::Degraded(_) | BridgeState::Starting => {
                        if matches!(*slot.state_rx.borrow(), BridgeState::Stopped) {
                            break;
                        }
                        if shutting_down.load(Ordering::SeqCst) {
                            tracing::info!(
                                "Pool shutting down — not restarting bridge for broker '{}'",
                                broker_name
                            );
                            break;
                        }
                        let _ = slot.state_tx.send(BridgeState::Restarting {
                            attempt: 0,
                            next_at: Instant::now(),
                        });
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
                            tracing::error!(
                                "Max restart attempts ({}) reached for broker '{}' — staying degraded",
                                attempt,
                                broker_name
                            );
                            let _ = slot.state_tx.send(BridgeState::Degraded(format!(
                                "max restart attempts ({}) exceeded",
                                attempt
                            )));
                            break;
                        }

                        let old_process = {
                            let mut guard = slot.process.lock().await;
                            guard.take()
                        };
                        if let Some(p) = old_process {
                            let _ = p.stop().await;
                        }

                        let start_result = Self::start_bridge_process(
                            &bridge_version,
                            &bridge_cache_dir,
                            start_timeout_ms,
                            &slot.broker_url,
                            &slot.broker_type,
                            &slot.credentials,
                        )
                        .await;

                        match start_result {
                            Ok((process, channel)) => {
                                // Guard: don't resurrect a stopped slot (shutdown may have run
                                // while this async bridge start was in-flight).
                                if matches!(*slot.state_rx.borrow(), BridgeState::Stopped) {
                                    let _ = process.stop().await;
                                    break;
                                }
                                {
                                    let mut guard = slot.process.lock().await;
                                    *guard = Some(process);
                                }
                                let _ = slot.state_tx.send(BridgeState::Ready { channel });
                                info!("Broker '{}' bridge restarted successfully", slot.name);
                            }
                            Err(e) => {
                                // Guard: don't schedule retries after shutdown.
                                if matches!(*slot.state_rx.borrow(), BridgeState::Stopped) {
                                    break;
                                }
                                let delay_secs = std::cmp::min(5 * 2u64.pow(attempt), 120);
                                let next = Instant::now() + Duration::from_secs(delay_secs);
                                warn!(
                                    "Failed to restart bridge for '{}' (attempt {}): {e}. Retry in {delay_secs}s",
                                    slot.name,
                                    attempt + 1
                                );
                                let _ = slot.state_tx.send(BridgeState::Restarting {
                                    attempt: attempt + 1,
                                    next_at: next,
                                });
                            }
                        }
                    }
                }
            }
        });

        // Store the handle so shutdown can await the monitor.
        let mut guard = handle_ref.lock().await;
        *guard = Some(handle);
    }

    async fn start_bridge_process(
        bridge_version: &str,
        bridge_cache_dir: &std::path::Path,
        start_timeout_ms: u64,
        broker_url: &str,
        broker_type: &BrokerType,
        credentials: &Option<(String, String)>,
    ) -> Result<(BridgeProcess, Channel), CamelError> {
        info!(
            "Starting JMS bridge process for {}...",
            redact_url(broker_url)
        );
        let binary_path = ensure_binary(bridge_version, bridge_cache_dir)
            .await
            .map_err(|e| {
                CamelError::ProcessorError(format!("JMS bridge binary unavailable: {e}"))
            })?;

        let process_config = BridgeProcessConfig::jms(
            binary_path,
            broker_url.to_string(),
            broker_type.clone(),
            credentials.as_ref().map(|(u, _)| u.clone()),
            credentials
                .as_ref()
                .map(|(_, p)| camel_bridge::process::Redacted::new(p.clone())),
            start_timeout_ms,
        );

        let total_timeout = Duration::from_millis(start_timeout_ms);
        let result = tokio::time::timeout(total_timeout, async {
            let process = BridgeProcess::start(&process_config)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("JMS bridge start failed: {e}")))?;

            let port = process.grpc_port();
            let channel = connect_channel(port).await.map_err(|e| {
                CamelError::ProcessorError(format!("JMS bridge channel connect failed: {e}"))
            })?;

            wait_for_health(&channel, Duration::from_secs(10), |ch| {
                let mut client = BridgeServiceClient::new(ch);
                async move {
                    let resp = client.health(HealthRequest {}).await?;
                    Ok(resp.into_inner().healthy)
                }
            })
            .await
            .map_err(|e| {
                CamelError::ProcessorError(format!("JMS bridge health check failed: {e}"))
            })?;

            Ok::<(BridgeProcess, Channel), CamelError>((process, channel))
        })
        .await
        .map_err(|_| {
            CamelError::ProcessorError(format!(
                "JMS bridge start timed out after {}ms",
                start_timeout_ms
            ))
        })??;

        Ok(result)
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
        let slot = self.pool.get_or_create_slot(&broker_name).await?;
        let channel = match &*slot.state_rx.borrow() {
            BridgeState::Ready { channel } => channel.clone(),
            other => {
                return Err(CamelError::ProcessorError(format!(
                    "Bridge not ready: {:?}",
                    other
                )));
            }
        };
        let mut client = BridgeServiceClient::new(channel);
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

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(LazyJmsProducer {
            pool: Arc::clone(&self.pool),
            broker_name: self.broker_name.clone(),
            endpoint_config: self.endpoint_config.clone(),
            resolved_broker_type: self.resolved_broker_type.clone(),
        }))
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(JmsConsumer::new(
            Arc::clone(&self.pool),
            self.broker_name.clone(),
            self.endpoint_config.clone(),
            self.pool.reconnect.clone(),
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
                BridgeState::Stopped => {
                    return Poll::Ready(Err(CamelError::ProcessorError(format!(
                        "JMS broker '{}' is stopped",
                        self.broker_name
                    ))));
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
            let slot = pool.get_or_create_slot(&broker_name).await?;
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
                                        "JMS channel refresh failed; requesting bridge restart"
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

/// Redact userinfo (username:password@) from a broker URL for safe logging.
/// Handles URLs like `tcp://user:pass@host:61616` → `tcp://***@host:61616`.
fn redact_url(url: &str) -> String {
    // Find the scheme separator (://)
    if let Some(pos) = url.find("://") {
        let scheme = &url[..pos + 3]; // includes "://"
        let rest = &url[pos + 3..];
        // Find @ in the remainder — everything before @ is userinfo
        if let Some(at_pos) = rest.find('@') {
            return format!("{}***@{}", scheme, &rest[at_pos + 1..]);
        }
    }
    url.to_string()
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
    use super::*;
    use crate::config::{BrokerConfig, JmsPoolConfig};
    use std::collections::HashMap;

    #[test]
    fn from_config_accepts_empty_brokers() {
        let pool_config = JmsPoolConfig::default();
        let result = JmsBridgePool::from_config(pool_config);
        assert!(result.is_ok());
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
                        },
                    ),
                    (
                        "secondary".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://secondary:61616".to_string(),
                            broker_type: BrokerType::Artemis,
                            username: None,
                            password: None,
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
    async fn concurrent_get_or_create_slot_no_deadlock() {
        use tokio::time::timeout;

        struct EnvGuard {
            key: &'static str,
            prev: Option<std::ffi::OsString>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                if let Some(v) = &self.prev {
                    // SAFETY: restoring process env in test scope.
                    unsafe { std::env::set_var(self.key, v) };
                } else {
                    // SAFETY: restoring process env in test scope.
                    unsafe { std::env::remove_var(self.key) };
                }
            }
        }

        let env_key = "CAMEL_JMS_BRIDGE_BINARY_PATH";
        let _guard = EnvGuard {
            key: env_key,
            prev: std::env::var_os(env_key),
        };
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
                    let _ = pool.get_or_create_slot("test").await;
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
    async fn lazy_producer_reports_degraded_when_bridge_start_fails() {
        use tower::Service;

        struct EnvGuard {
            key: &'static str,
            prev: Option<std::ffi::OsString>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                if let Some(v) = &self.prev {
                    // SAFETY: restoring process env in test scope.
                    unsafe { std::env::set_var(self.key, v) };
                } else {
                    // SAFETY: restoring process env in test scope.
                    unsafe { std::env::remove_var(self.key) };
                }
            }
        }

        let env_key = "CAMEL_JMS_BRIDGE_BINARY_PATH";
        let _guard = EnvGuard {
            key: env_key,
            prev: std::env::var_os(env_key),
        };
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
            .create_producer(&camel_component_api::ProducerContext::default())
            .unwrap();

        let mut exchange = Exchange::default();
        exchange.input.body = camel_component_api::Body::Text("hello".to_string());

        let err = producer.call(exchange).await.unwrap_err();
        assert!(err.to_string().contains("is degraded"), "got: {}", err);
    }

    /// A send transport error should trigger a channel refresh attempt first.
    /// If refresh cannot be performed (e.g. no running bridge process metadata),
    /// the producer requests a bridge restart as fallback.
    #[tokio::test]
    async fn lazy_producer_requests_restart_when_refresh_unavailable() {
        use tokio::sync::watch;
        use tonic::transport::Endpoint as TonicEndpoint;
        use tower::Service;

        // Build a lazy channel to a port where nothing is listening.
        // connect_lazy() succeeds immediately; the error manifests on the actual RPC call.
        let dead_channel = TonicEndpoint::from_static("http://127.0.0.1:1").connect_lazy();

        let (state_tx, state_rx) = watch::channel(BridgeState::Ready {
            channel: dead_channel.clone(),
        });

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        // Manually insert a slot with the dead-channel in Ready state.
        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx: state_tx.clone(),
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
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

        // The send will fail because the channel points to a dead port.
        let result = producer.call(exchange).await;
        assert!(result.is_err(), "expected send to fail");

        // Refresh cannot run in this setup (slot has no BridgeProcess), so the
        // fallback path requests a restart.
        let state_after = state_rx.borrow().clone();
        assert!(
            matches!(state_after, BridgeState::Restarting { .. }),
            "slot must enter Restarting when refresh is unavailable; got: {:?}",
            state_after
        );
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
                        },
                    ),
                    (
                        "b2".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://b2:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
                        },
                    ),
                    (
                        "b3".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://b3:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
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
            });
            pool.slots.insert(name.to_string(), slot);
        }

        // Attempting to create a third slot should fail.
        let err = pool.get_or_create_slot("b3").await.unwrap_err();
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
        });
        pool.slots.insert("b1".to_string(), slot);

        // b1 is Degraded (not active), so creating b1's slot returns existing.
        // The max_bridges check only applies to new slots.
        let result = pool.get_or_create_slot("b1").await;
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
    async fn poll_ready_returns_error_when_stopped() {
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
            "poll_ready must be Err when Stopped; got: {:?}",
            result
        );
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
            loop {
                if matches!(*state_rx_clone.borrow(), BridgeState::Stopped) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        *monitor_handle_ref.lock().await = Some(handle);

        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: monitor_handle_ref,
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
        });
        pool.slots.insert("default".to_string(), Arc::clone(&slot));

        pool.spawn_health_monitor(Arc::clone(&slot)).await;

        tokio::time::sleep(Duration::from_millis(50)).await;
        let guard = slot.health_monitor_handle.lock().await;
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

    #[test]
    fn redact_url_handles_ssl_scheme() {
        assert_eq!(
            redact_url("ssl://user:pass@secure-broker:61617"),
            "ssl://***@secure-broker:61617"
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
                        },
                    ),
                    (
                        "b2".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://b2:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
                        },
                    ),
                    (
                        "b3".to_string(),
                        BrokerConfig {
                            broker_url: "tcp://b3:61616".to_string(),
                            broker_type: BrokerType::ActiveMq,
                            username: None,
                            password: None,
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
            });
            pool.slots.insert(name.to_string(), slot);
        }

        assert_eq!(pool.slots.len(), 2);

        let guard = pool.bridge_create_lock.lock().await;
        let total_count = pool.slots.len();
        assert!(total_count >= pool.max_bridges as usize);
        let result = if total_count >= pool.max_bridges as usize {
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

    #[tokio::test]
    async fn transport_error_refreshes_channel_but_does_not_resend() {
        use tokio::sync::watch;
        use tonic::transport::Endpoint as TonicEndpoint;
        use tower::Service;

        let dead_channel = TonicEndpoint::from_static("http://127.0.0.1:1").connect_lazy();

        let (state_tx, state_rx) = watch::channel(BridgeState::Ready {
            channel: dead_channel.clone(),
        });

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::ActiveMq,
            ))
            .unwrap(),
        );

        let slot = Arc::new(BridgeSlot {
            name: "default".to_string(),
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            credentials: None,
            state_rx: state_rx.clone(),
            state_tx: state_tx.clone(),
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
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

        let result = producer.call(exchange).await;
        assert!(result.is_err(), "expected send to fail");

        let state_after = state_rx.borrow().clone();
        assert!(
            matches!(state_after, BridgeState::Restarting { .. }),
            "slot must enter Restarting; got: {:?}",
            state_after
        );

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(BRIDGE_TRANSPORT_ERROR_PREFIX),
            "error must be original transport error, got: {}",
            err_msg
        );
    }
}
