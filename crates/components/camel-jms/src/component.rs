use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use camel_bridge::{
    channel::connect_channel,
    download::ensure_binary,
    health::wait_for_health,
    process::{BridgeProcess, BridgeProcessConfig, BrokerType},
};
use camel_component_api::{
    BoxProcessor, CamelError, Component, Consumer, Endpoint, Exchange, ProducerContext,
};
use dashmap::DashMap;
use tokio::sync::watch;
use tonic::transport::Channel;
use tower::Service;
use tracing::{info, warn};

use crate::config::{BrokerConfig, JmsEndpointConfig, JmsPoolConfig};
use crate::consumer::JmsConsumer;
use crate::producer::JmsProducer;
use crate::proto::{HealthRequest, bridge_service_client::BridgeServiceClient};

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
    pub(crate) broker_reconnect_interval_ms: u64,
    pub(crate) health_check_interval_ms: u64,
    pub(crate) bridge_version: String,
    pub(crate) bridge_cache_dir: PathBuf,
}

impl JmsBridgePool {
    pub fn from_config(pool_config: JmsPoolConfig) -> Result<Self, CamelError> {
        pool_config.validate()?;
        Ok(Self {
            slots: DashMap::new(),
            config: pool_config.brokers,
            bridge_start_timeout_ms: pool_config.bridge_start_timeout_ms,
            broker_reconnect_interval_ms: pool_config.broker_reconnect_interval_ms,
            health_check_interval_ms: pool_config.health_check_interval_ms,
            bridge_version: crate::BRIDGE_VERSION.to_string(),
            bridge_cache_dir: pool_config.bridge_cache_dir,
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
                1 => Ok(self.config.keys().next().unwrap().clone()),
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

        self.spawn_health_monitor(Arc::clone(&slot));

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

    /// Shutdown all slots: stop all bridge processes concurrently.
    pub async fn shutdown(&self) -> Result<(), CamelError> {
        let names: Vec<String> = self.slots.iter().map(|e| e.key().clone()).collect();
        let mut tasks = Vec::new();
        for name in names {
            if let Some((_, slot)) = self.slots.remove(&name) {
                tasks.push(tokio::spawn(async move {
                    let process = {
                        let mut guard = slot.process.lock().await;
                        guard.take()
                    };
                    let _ = slot.state_tx.send(BridgeState::Stopped);
                    if let Some(p) = process {
                        let _ = p.stop().await;
                    }
                }));
            }
        }
        for t in tasks {
            let _ = t.await;
        }
        Ok(())
    }

    pub fn broker_reconnect_interval_ms(&self) -> u64 {
        self.broker_reconnect_interval_ms
    }

    fn spawn_health_monitor(&self, slot: Arc<BridgeSlot>) {
        let health_interval = self.health_check_interval_ms;
        let bridge_version = self.bridge_version.clone();
        let bridge_cache_dir = self.bridge_cache_dir.clone();
        let start_timeout_ms = self.bridge_start_timeout_ms;

        tokio::spawn(async move {
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
                        let _ = slot.state_tx.send(BridgeState::Restarting {
                            attempt: 0,
                            next_at: Instant::now(),
                        });
                    }
                    BridgeState::Restarting { attempt, next_at } => {
                        let now = Instant::now();
                        if now < next_at {
                            tokio::time::sleep(next_at - now).await;
                        }

                        info!(
                            "Restarting bridge for broker '{}' (attempt {})",
                            slot.name,
                            attempt + 1
                        );

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
    }

    async fn start_bridge_process(
        bridge_version: &str,
        bridge_cache_dir: &std::path::Path,
        start_timeout_ms: u64,
        broker_url: &str,
        broker_type: &BrokerType,
        credentials: &Option<(String, String)>,
    ) -> Result<(BridgeProcess, Channel), CamelError> {
        info!("Starting JMS bridge process for {broker_url}...");
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
            credentials.as_ref().map(|(_, p)| p.clone()),
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
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = JmsEndpointConfig::from_uri(uri)?;
        let broker_name = self
            .pool
            .resolve_broker_name(endpoint_config.broker_name.as_deref())?;
        let resolved_broker_type = self.pool.resolve_broker_type(&self.scheme, &broker_name);

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
            self.pool.broker_reconnect_interval_ms(),
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

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
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
                        match producer.call(exchange.clone()).await {
                            Ok(done) => return Ok(done),
                            Err(first_err) if is_bridge_transport_error(&first_err) => {
                                warn!(
                                    broker = %broker_name,
                                    error = %first_err,
                                    "JMS send transport error; refreshing channel and retrying once"
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
                                    return Err(first_err);
                                }

                                let refreshed = match slot.state_rx.borrow().clone() {
                                    BridgeState::Ready { channel } => channel,
                                    other => {
                                        return Err(CamelError::ProcessorError(format!(
                                            "JMS broker '{}' not ready after channel refresh: {:?}",
                                            broker_name, other
                                        )));
                                    }
                                };

                                let mut retry_producer =
                                    JmsProducer::new(refreshed, endpoint_config.clone());
                                return retry_producer.call(exchange).await;
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

pub fn is_bridge_transport_error(err: &CamelError) -> bool {
    let msg = err.to_string();
    msg.contains("JMS gRPC send error") || msg.contains("JMS gRPC subscribe error")
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
}
