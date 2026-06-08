use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use camel_bridge::channel::connect_channel;
use camel_bridge::download::ensure_binary_for_spec;
use camel_bridge::health::wait_for_health;
use camel_bridge::process::{BridgeProcess, BridgeProcessConfig, CxfProfileEnvVars, Redacted};
use camel_bridge::spec::CXF_BRIDGE;
use camel_component_api::CamelError;
use dashmap::DashMap;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tracing::{error, info, warn};

use camel_component_api::NetworkRetryPolicy;
use camel_component_api::retry_async;

use crate::config::{CxfPoolConfig, validate_profile_name};
use crate::error::CxfError;
use crate::proto::{HealthRequest, cxf_bridge_client::CxfBridgeClient};

/// Classify a CxfError as transient (retryable) or permanent (fail-fast).
///
/// - **Transient**: `Transport` (network/connectivity), `Timeout`
/// - **Permanent**: `Fault`, `Security`, `Bridge`, `Config`, `StreamClosed`,
///   `StreamBody` — these indicate configuration, protocol, or logic errors
///   that will never succeed on retry.
fn is_retryable_cxf_error(err: &CxfError) -> bool {
    matches!(err, CxfError::Transport { .. } | CxfError::Timeout { .. })
}

// Uses retry_async<CxfError> with is_retryable_cxf_error classifier
// (migrated from manual loop in rc-4s9).
async fn connect_with_retry(port: u16, policy: &NetworkRetryPolicy) -> Result<Channel, CxfError> {
    retry_async(
        policy,
        Some("cxf"),
        || async {
            connect_channel(port)
                .await
                .map_err(|e| CxfError::Transport {
                    message: e.to_string(),
                    endpoint: format!("grpc://127.0.0.1:{port}"),
                    source: None,
                })
        },
        is_retryable_cxf_error,
    )
    .await
}

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
    pub key: String,
    pub configured_profiles: Vec<crate::config::CxfProfileConfig>,
    pub bind_address: Option<String>,
    pub state_rx: watch::Receiver<BridgeState>,
    pub(crate) state_tx: watch::Sender<BridgeState>,
    pub process: Arc<tokio::sync::Mutex<Option<BridgeProcess>>>,
    pub(crate) health_monitor_handle: Arc<tokio::sync::Mutex<Option<JoinHandle<()>>>>,
}

#[cfg(any(test, feature = "test-util"))]
impl BridgeSlot {
    pub fn new_ready_for_test(channel: Channel) -> Self {
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });
        Self {
            key: "test-slot".to_string(),
            configured_profiles: vec![],
            bind_address: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
}

impl std::fmt::Debug for BridgeSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeSlot")
            .field("key", &self.key)
            .field("state", &*self.state_rx.borrow())
            .finish()
    }
}

// ── CxfBridgePool ────────────────────────────────────────────────────────────

const MAX_RESTART_ATTEMPTS: u32 = 10;

pub struct CxfBridgePool {
    pub(crate) slots: DashMap<String, Arc<BridgeSlot>>,
    /// Atomic counter for exact pool-size enforcement (CXF-016).
    /// Incremented on insert, decremented on remove, so that concurrent
    /// callers cannot race past `max_bridges`.
    pub(crate) slot_count: AtomicUsize,
    pub(crate) shutting_down: Arc<AtomicBool>,
    pub(crate) max_bridges: usize,
    pub(crate) bridge_start_timeout_ms: u64,
    pub(crate) health_check_interval_ms: u64,
    pub(crate) bridge_version: String,
    pub(crate) bridge_cache_dir: PathBuf,
    pub(crate) configured_profiles: Vec<crate::config::CxfProfileConfig>,
    pub(crate) bind_address: Option<String>,
    pub(crate) reconnect: NetworkRetryPolicy,
}

impl CxfBridgePool {
    pub fn from_config(pool_config: CxfPoolConfig) -> Result<Self, CamelError> {
        // Validate all profile names before any bridge is spawned.
        for profile in &pool_config.profiles {
            validate_profile_name(&profile.name)?;
        }
        let bridge_cache_dir = pool_config
            .bridge_cache_dir
            .unwrap_or_else(|| camel_bridge::download::default_cache_dir_for_spec(&CXF_BRIDGE));
        let configured_profiles = pool_config.profiles.clone();
        Ok(Self {
            slots: DashMap::new(),
            slot_count: AtomicUsize::new(0),
            shutting_down: Arc::new(AtomicBool::new(false)),
            max_bridges: pool_config.max_bridges,
            bridge_start_timeout_ms: pool_config.bridge_start_timeout_ms,
            health_check_interval_ms: pool_config.health_check_interval_ms,
            bridge_version: pool_config.version,
            bridge_cache_dir,
            configured_profiles,
            bind_address: pool_config.bind_address,
            reconnect: pool_config.reconnect,
        })
    }

    pub fn slot_key() -> String {
        "cxf".to_string()
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn insert_slot_for_test(&self, key: String, slot: BridgeSlot) {
        self.slots.insert(key, Arc::new(slot));
        self.slot_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    pub async fn get_channel(self: &Arc<Self>) -> Result<Channel, CxfError> {
        let key = Self::slot_key();
        let slot = self.get_or_create_slot(&key).await?;
        self.await_ready_channel(&slot).await
    }

    pub async fn get_or_create_slot(
        self: &Arc<Self>,
        key: &str,
    ) -> Result<Arc<BridgeSlot>, CxfError> {
        if let Some(entry) = self.slots.get(key) {
            return Ok(Arc::clone(entry.value()));
        }

        // CXF-016: use atomic compare-and-swap so concurrent callers cannot
        // race past max_bridges. AcqRel ensures the slot insertion is
        // ordered after the count increment.
        loop {
            let current = self.slot_count.load(Ordering::Acquire);
            if current >= self.max_bridges {
                return Err(CxfError::Config(format!(
                    "cannot create bridge slot '{}': max_bridges limit ({}) reached",
                    key, self.max_bridges
                )));
            }
            if self
                .slot_count
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
            // CAS failed — another thread incremented; retry.
        }

        let key_owned = key.to_string();
        let start_timeout_ms = self.bridge_start_timeout_ms;
        let bridge_version = self.bridge_version.clone();
        let bridge_cache_dir = self.bridge_cache_dir.clone();
        let profiles = self.configured_profiles.clone();
        let bind_address = self.bind_address.clone();

        let slot = match self.slots.entry(key_owned.clone()) {
            dashmap::Entry::Occupied(existing) => {
                // Another thread won the race — undo the counter increment.
                self.slot_count.fetch_sub(1, Ordering::AcqRel);
                return Ok(Arc::clone(existing.get()));
            }
            dashmap::Entry::Vacant(entry) => {
                let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
                let slot = Arc::new(BridgeSlot {
                    key: key_owned,
                    configured_profiles: profiles,
                    bind_address,
                    state_rx,
                    state_tx,
                    process: Arc::new(tokio::sync::Mutex::new(None)),
                    health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
                });
                entry.insert(Arc::clone(&slot));
                slot
            }
        };

        let start_result = Self::start_bridge_inner(
            &slot,
            &bridge_version,
            &bridge_cache_dir,
            start_timeout_ms,
            &self.reconnect,
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
                // Intentionally keep slot_count at +1 for degraded slots: the health
                // monitor will attempt to restart this slot, so it continues to occupy
                // one pool position. This prevents unbounded re-allocation attempts.
                let _ = slot
                    .state_tx
                    .send(BridgeState::Degraded(format!("Initial start failed: {e}")));
            }
        }

        Self::spawn_health_monitor(Arc::clone(self), Arc::clone(&slot)).await;

        Ok(slot)
    }

    async fn start_bridge_inner(
        slot: &Arc<BridgeSlot>,
        bridge_version: &str,
        bridge_cache_dir: &Path,
        start_timeout_ms: u64,
        reconnect: &NetworkRetryPolicy,
    ) -> Result<(BridgeProcess, Channel), CxfError> {
        tracing::trace!(
            key = %slot.key,
            profile_count = slot.configured_profiles.len(),
            "starting CXF bridge slot"
        );

        let result = tokio::time::timeout(Duration::from_millis(start_timeout_ms), async {
            let binary_path = ensure_binary_for_spec(&CXF_BRIDGE, bridge_version, bridge_cache_dir)
                .await
                .map_err(|e| CxfError::Bridge {
                    message: e.to_string(),
                    slot_key: slot.key.clone(),
                    source: None,
                })?;

            let profile_env_vars: Vec<CxfProfileEnvVars> = slot
                .configured_profiles
                .iter()
                .map(|p| CxfProfileEnvVars {
                    name: p.name.clone(),
                    wsdl_path: p.wsdl_path.clone(),
                    service_name: p.service_name.clone(),
                    port_name: p.port_name.clone(),
                    address: p.address.clone(),
                    keystore_path: p.security.keystore_path.clone(),
                    keystore_password: p.security.keystore_password.clone().map(Redacted::new),
                    truststore_path: p.security.truststore_path.clone(),
                    truststore_password: p.security.truststore_password.clone().map(Redacted::new),
                    sig_username: p.security.sig_username.clone(),
                    sig_password: p.security.sig_password.clone().map(Redacted::new),
                    enc_username: p.security.enc_username.clone(),
                    security_actions_out: p.security.security_actions_out.clone(),
                    security_actions_in: p.security.security_actions_in.clone(),
                    signature_algorithm: p.security.signature_algorithm.clone(),
                    signature_digest_algorithm: p.security.signature_digest_algorithm.clone(),
                    signature_c14n_algorithm: p.security.signature_c14n_algorithm.clone(),
                    signature_parts: p.security.signature_parts.clone(),
                })
                .collect();

            let config =
                BridgeProcessConfig::cxf_profiles(binary_path, &profile_env_vars, start_timeout_ms);
            let mut config = config;
            if let Some(addr) = slot.bind_address.as_ref() {
                config
                    .env_vars
                    .push(("CXF_ADDRESS".to_string(), addr.clone()));
            }

            tracing::trace!(
                key = %slot.key,
                env_vars = ?config.env_vars,
                "starting CXF bridge process with env vars"
            );

            let process = BridgeProcess::start(&config)
                .await
                .map_err(|e| CxfError::Bridge {
                    message: e.to_string(),
                    slot_key: slot.key.clone(),
                    source: None,
                })?;

            let port = process.grpc_port();
            let channel = connect_with_retry(port, reconnect).await?;

            wait_for_health(&channel, Duration::from_secs(10), |ch| {
                let mut client = CxfBridgeClient::new(ch);
                async move {
                    let resp = client.health(HealthRequest {}).await?;
                    Ok(resp.into_inner().healthy)
                }
            })
            .await
            .map_err(|e| CxfError::Bridge {
                message: e.to_string(),
                slot_key: slot.key.clone(),
                source: None,
            })?;

            Ok::<(BridgeProcess, Channel), CxfError>((process, channel))
        })
        .await
        .map_err(|_| CxfError::Bridge {
            message: format!("CXF bridge start timed out after {}ms", start_timeout_ms),
            slot_key: slot.key.clone(),
            source: None,
        })??;
        Ok(result)
    }

    async fn spawn_health_monitor(pool: Arc<CxfBridgePool>, slot: Arc<BridgeSlot>) {
        let health_interval = pool.health_check_interval_ms;
        let bridge_version = pool.bridge_version.clone();
        let bridge_cache_dir = pool.bridge_cache_dir.clone();
        let start_timeout_ms = pool.bridge_start_timeout_ms;

        let monitor_slot = Arc::clone(&slot);
        let handle = tokio::spawn(async move {
            loop {
                let state = monitor_slot.state_rx.borrow().clone();
                match state {
                    BridgeState::Stopped => {
                        info!(
                            "Health monitor for '{}' exiting (Stopped)",
                            monitor_slot.key
                        );
                        break;
                    }
                    BridgeState::Ready { ref channel } => {
                        tokio::time::sleep(Duration::from_millis(health_interval)).await;
                        let mut client = CxfBridgeClient::new(channel.clone());
                        let health_timeout = Duration::from_secs(3);
                        match tokio::time::timeout(health_timeout, client.health(HealthRequest {}))
                            .await
                        {
                            Ok(Ok(resp)) => {
                                let inner = resp.into_inner();
                                if !inner.healthy {
                                    warn!(
                                        key = %monitor_slot.key,
                                        message = %inner.message,
                                        "CXF bridge unhealthy"
                                    );
                                    let _ = monitor_slot.state_tx.send(BridgeState::Degraded(
                                        format!("unhealthy: {}", inner.message),
                                    ));
                                }
                            }
                            Ok(Err(e)) => {
                                warn!(
                                    key = %monitor_slot.key,
                                    error = %e,
                                    "CXF bridge health check failed"
                                );
                                let _ = monitor_slot
                                    .state_tx
                                    .send(BridgeState::Degraded(e.to_string()));
                            }
                            Err(_) => {
                                let msg = format!(
                                    "health RPC timed out after {}ms",
                                    health_timeout.as_millis()
                                );
                                warn!(
                                    key = %monitor_slot.key,
                                    "CXF bridge health check timed out"
                                );
                                let _ = monitor_slot.state_tx.send(BridgeState::Degraded(msg));
                            }
                        }
                    }
                    BridgeState::Degraded(_) | BridgeState::Starting => {
                        if matches!(*monitor_slot.state_rx.borrow(), BridgeState::Stopped) {
                            break;
                        }
                        if pool.shutting_down.load(Ordering::SeqCst) {
                            tracing::info!("Pool shutting down — not restarting bridge");
                            break;
                        }
                        info!(
                            "Health monitor for '{}' found degraded/starting, triggering restart",
                            monitor_slot.key
                        );
                        let _ = monitor_slot.state_tx.send(BridgeState::Restarting {
                            attempt: 0,
                            next_at: Instant::now(),
                        });
                    }
                    BridgeState::Restarting { attempt, next_at } => {
                        if pool.shutting_down.load(Ordering::SeqCst) {
                            tracing::info!("Pool shutting down — aborting restart");
                            break;
                        }
                        if attempt >= MAX_RESTART_ATTEMPTS {
                            // log-policy: system-broken
                            error!(
                                "Max restart attempts ({}) reached — staying degraded",
                                attempt
                            );
                            let _ = monitor_slot.state_tx.send(BridgeState::Degraded(format!(
                                "max restart attempts ({}) exceeded",
                                attempt
                            )));
                            break;
                        }

                        let now = Instant::now();
                        if now < next_at {
                            tokio::time::sleep(next_at - now).await;
                        }

                        if matches!(*monitor_slot.state_rx.borrow(), BridgeState::Stopped) {
                            break;
                        }

                        info!(
                            "Restarting CXF bridge for '{}' (attempt {})",
                            monitor_slot.key,
                            attempt + 1
                        );

                        let old_process = {
                            let mut guard = monitor_slot.process.lock().await;
                            guard.take()
                        };
                        if let Some(p) = old_process {
                            let _ = p.stop().await;
                        }

                        let start_result: Result<(BridgeProcess, Channel), CxfError> = async {
                            let profiles = &monitor_slot.configured_profiles;
                            let binary_path = ensure_binary_for_spec(
                                &CXF_BRIDGE,
                                &bridge_version,
                                &bridge_cache_dir,
                            )
                            .await
                            .map_err(|e| CxfError::Bridge {
                                message: e.to_string(),
                                slot_key: monitor_slot.key.clone(),
                                source: None,
                            })?;

                            let profile_env_vars: Vec<CxfProfileEnvVars> = profiles
                                .iter()
                                .map(|p| CxfProfileEnvVars {
                                    name: p.name.clone(),
                                    wsdl_path: p.wsdl_path.clone(),
                                    service_name: p.service_name.clone(),
                                    port_name: p.port_name.clone(),
                                    address: p.address.clone(),
                                    keystore_path: p.security.keystore_path.clone(),
                                    keystore_password: p
                                        .security
                                        .keystore_password
                                        .clone()
                                        .map(Redacted::new),
                                    truststore_path: p.security.truststore_path.clone(),
                                    truststore_password: p
                                        .security
                                        .truststore_password
                                        .clone()
                                        .map(Redacted::new),
                                    sig_username: p.security.sig_username.clone(),
                                    sig_password: p
                                        .security
                                        .sig_password
                                        .clone()
                                        .map(Redacted::new),
                                    enc_username: p.security.enc_username.clone(),
                                    security_actions_out: p.security.security_actions_out.clone(),
                                    security_actions_in: p.security.security_actions_in.clone(),
                                    signature_algorithm: p.security.signature_algorithm.clone(),
                                    signature_digest_algorithm: p
                                        .security
                                        .signature_digest_algorithm
                                        .clone(),
                                    signature_c14n_algorithm: p
                                        .security
                                        .signature_c14n_algorithm
                                        .clone(),
                                    signature_parts: p.security.signature_parts.clone(),
                                })
                                .collect();

                            let config = BridgeProcessConfig::cxf_profiles(
                                binary_path,
                                &profile_env_vars,
                                start_timeout_ms,
                            );
                            let mut config = config;
                            if let Some(addr) = monitor_slot.bind_address.as_ref() {
                                config
                                    .env_vars
                                    .push(("CXF_ADDRESS".to_string(), addr.clone()));
                            }

                            let process = BridgeProcess::start(&config).await.map_err(|e| {
                                CxfError::Bridge {
                                    message: e.to_string(),
                                    slot_key: monitor_slot.key.clone(),
                                    source: None,
                                }
                            })?;
                            let port = process.grpc_port();
                            let channel = connect_with_retry(port, &pool.reconnect).await?;

                            wait_for_health(&channel, Duration::from_secs(10), |ch| {
                                let mut client = CxfBridgeClient::new(ch);
                                async move {
                                    let resp = client.health(HealthRequest {}).await?;
                                    Ok(resp.into_inner().healthy)
                                }
                            })
                            .await
                            .map_err(|e| CxfError::Bridge {
                                message: e.to_string(),
                                slot_key: monitor_slot.key.clone(),
                                source: None,
                            })?;

                            Ok((process, channel))
                        }
                        .await;

                        match start_result {
                            Ok((process, channel)) => {
                                if matches!(*monitor_slot.state_rx.borrow(), BridgeState::Stopped) {
                                    let _ = process.stop().await;
                                    break;
                                }
                                {
                                    let mut guard = monitor_slot.process.lock().await;
                                    *guard = Some(process);
                                }
                                let _ = monitor_slot.state_tx.send(BridgeState::Ready { channel });
                                info!("CXF bridge '{}' restarted successfully", monitor_slot.key);
                            }
                            Err(e) => {
                                if matches!(*monitor_slot.state_rx.borrow(), BridgeState::Stopped) {
                                    break;
                                }
                                let delay_secs = std::cmp::min(2u64.pow(attempt.min(5)), 30);
                                let next = Instant::now() + Duration::from_secs(delay_secs);
                                warn!(
                                    "Failed to restart CXF bridge for '{}' (attempt {}): {e}. Retry in {delay_secs}s",
                                    monitor_slot.key,
                                    attempt + 1
                                );
                                let _ = monitor_slot.state_tx.send(BridgeState::Restarting {
                                    attempt: attempt + 1,
                                    next_at: next,
                                });
                            }
                        }
                    }
                }
            }
        });

        let mut guard = slot.health_monitor_handle.lock().await;
        *guard = Some(handle);
    }

    pub fn restart_slot(&self, key: &str) {
        if let Some(slot) = self.slots.get(key) {
            let _ = slot.state_tx.send(BridgeState::Restarting {
                attempt: 0,
                next_at: Instant::now(),
            });
        }
    }

    pub async fn refresh_slot_channel(&self, key: &str) -> Result<(), CxfError> {
        let slot = self.slots.get(key).ok_or_else(|| CxfError::Bridge {
            message: format!("no slot for key '{}'", key),
            slot_key: key.to_string(),
            source: None,
        })?;

        let guard = slot.process.lock().await;
        if let Some(process) = guard.as_ref() {
            let port = process.grpc_port();
            let channel = connect_with_retry(port, &self.reconnect).await?;
            let _ = slot.state_tx.send(BridgeState::Ready { channel });
        }
        Ok(())
    }

    async fn await_ready_channel(&self, slot: &Arc<BridgeSlot>) -> Result<Channel, CxfError> {
        let mut rx = slot.state_rx.clone();
        loop {
            match &*rx.borrow() {
                BridgeState::Ready { channel } => return Ok(channel.clone()),
                BridgeState::Stopped => {
                    return Err(CxfError::Bridge {
                        message: format!("CXF bridge '{}' is stopped", slot.key),
                        slot_key: slot.key.clone(),
                        source: None,
                    });
                }
                _ => {}
            }
            if rx.changed().await.is_err() {
                return Err(CxfError::Bridge {
                    message: format!("CXF bridge '{}' state channel closed", slot.key),
                    slot_key: slot.key.clone(),
                    source: None,
                });
            }
        }
    }

    pub async fn shutdown(&self) -> Result<(), CamelError> {
        self.begin_shutdown();
        let mut tasks = Vec::new();
        for entry in self.slots.iter() {
            let slot = Arc::clone(entry.value());
            tasks.push(tokio::spawn(async move {
                let process = {
                    let mut guard = slot.process.lock().await;
                    guard.take()
                };
                let _ = slot.state_tx.send(BridgeState::Stopped);
                if let Some(p) = process {
                    let _ = p.stop().await;
                }
                if let Some(handle) = slot.health_monitor_handle.lock().await.take()
                    && tokio::time::timeout(Duration::from_secs(5), handle)
                        .await
                        .is_err()
                {
                    warn!("Health monitor task did not stop in 5s; may have been aborted");
                }
            }));
        }
        for t in tasks {
            let _ = t.await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::transport::Endpoint;

    fn test_pool_config() -> CxfPoolConfig {
        CxfPoolConfig {
            profiles: vec![],
            max_bridges: 2,
            bridge_start_timeout_ms: 15_000,
            health_check_interval_ms: 3_000,
            bridge_cache_dir: None,
            version: crate::BRIDGE_VERSION.to_string(),
            bind_address: None,
            reconnect: NetworkRetryPolicy::default(),
        }
    }

    #[test]
    fn slot_key_is_constant() {
        let key = CxfBridgePool::slot_key();
        assert_eq!(key, "cxf");
    }

    #[test]
    fn from_config_uses_defaults() {
        let pool_config = test_pool_config();
        let pool = CxfBridgePool::from_config(pool_config).expect("valid config");
        assert_eq!(pool.max_bridges, 2);
        assert_eq!(pool.bridge_start_timeout_ms, 15_000);
        assert_eq!(pool.bridge_version, crate::BRIDGE_VERSION);
    }

    #[test]
    fn from_config_uses_explicit_cache_dir_and_bind_address() {
        let mut pool_config = test_pool_config();
        pool_config.bridge_cache_dir = Some(PathBuf::from("/tmp/cxf-cache"));
        pool_config.bind_address = Some("http://127.0.0.1:9000/cxf".to_string());

        let pool = CxfBridgePool::from_config(pool_config).expect("valid config");
        assert_eq!(pool.bridge_cache_dir, PathBuf::from("/tmp/cxf-cache"));
        assert_eq!(
            pool.bind_address.as_deref(),
            Some("http://127.0.0.1:9000/cxf")
        );
    }

    #[test]
    fn from_config_rejects_invalid_profile_name() {
        let mut pool_config = test_pool_config();
        pool_config.profiles = vec![crate::config::CxfProfileConfig {
            name: "Bad-Name".to_string(),
            address: None,
            wsdl_path: "service.wsdl".to_string(),
            service_name: "MyService".to_string(),
            port_name: "MyPort".to_string(),
            security: crate::config::CxfSecurityFields::default(),
        }];

        let err = CxfBridgePool::from_config(pool_config)
            .err()
            .expect("expected invalid profile");
        assert!(
            err.to_string()
                .contains("must contain only lowercase letters")
        );
    }

    #[test]
    fn bridge_state_debug_format() {
        let state = BridgeState::Starting;
        let s = format!("{:?}", state);
        assert!(s.contains("Starting"), "got: {s}");

        let state = BridgeState::Degraded("connection lost".to_string());
        let s = format!("{:?}", state);
        assert!(s.contains("Degraded"), "got: {s}");
        assert!(s.contains("connection lost"), "got: {s}");

        let state = BridgeState::Stopped;
        let s = format!("{:?}", state);
        assert!(s.contains("Stopped"), "got: {s}");
    }

    #[test]
    fn restart_slot_updates_state() {
        let pool_config = test_pool_config();
        let pool = CxfBridgePool::from_config(pool_config).expect("valid config");

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = Arc::new(BridgeSlot {
            key: "test-key".to_string(),
            configured_profiles: vec![],
            bind_address: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
        });
        pool.slots.insert("test-key".to_string(), slot);

        pool.restart_slot("test-key");

        let state = pool
            .slots
            .get("test-key")
            .unwrap()
            .state_rx
            .borrow()
            .clone();
        match state {
            BridgeState::Restarting { attempt, .. } => assert_eq!(attempt, 0),
            other => panic!("expected Restarting, got: {:?}", other),
        }
    }

    #[test]
    fn max_bridges_enforced() {
        let mut pool_config = test_pool_config();
        pool_config.max_bridges = 0;
        let pool = Arc::new(CxfBridgePool::from_config(pool_config).expect("valid config"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(pool.get_or_create_slot("key"));
        assert!(result.is_err(), "should fail when max_bridges is 0");
        let err = result.unwrap_err();
        assert!(err.to_string().contains("max_bridges"), "got: {err}");
    }

    #[test]
    fn get_or_create_slot_returns_existing_without_starting_bridge() {
        let pool = Arc::new(CxfBridgePool::from_config(test_pool_config()).expect("valid config"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let slot = rt
            .block_on(async {
                let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
                pool.insert_slot_for_test(
                    "existing".to_string(),
                    BridgeSlot::new_ready_for_test(channel),
                );
                pool.get_or_create_slot("existing").await
            })
            .expect("existing slot");
        assert_eq!(slot.key, "test-slot");
    }

    #[test]
    fn await_ready_channel_returns_error_when_stopped() {
        let pool = CxfBridgePool::from_config(test_pool_config()).expect("valid config");
        let (state_tx, state_rx) = watch::channel(BridgeState::Stopped);
        let slot = Arc::new(BridgeSlot {
            key: "stopped-slot".to_string(),
            configured_profiles: vec![],
            bind_address: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
            health_monitor_handle: Arc::new(tokio::sync::Mutex::new(None)),
        });

        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(pool.await_ready_channel(&slot))
            .expect_err("expected stopped error");
        assert!(err.to_string().contains("is stopped"));
    }

    #[test]
    fn shutdown_marks_slot_stopped() {
        let pool = Arc::new(CxfBridgePool::from_config(test_pool_config()).expect("valid config"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
            pool.insert_slot_for_test("slot1".to_string(), BridgeSlot::new_ready_for_test(channel));
            pool.shutdown().await
        })
        .expect("shutdown ok");

        let state = pool
            .slots
            .get("slot1")
            .expect("slot exists")
            .state_rx
            .borrow()
            .clone();
        assert!(matches!(state, BridgeState::Stopped));
    }

    #[tokio::test]
    async fn slot_count_reflects_atomic_tracking() {
        let pool = CxfBridgePool::from_config(test_pool_config()).expect("valid config");
        assert_eq!(pool.slot_count.load(Ordering::Relaxed), 0);

        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        pool.insert_slot_for_test("a".to_string(), BridgeSlot::new_ready_for_test(channel));
        assert_eq!(pool.slot_count.load(Ordering::Relaxed), 1);

        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        pool.insert_slot_for_test("b".to_string(), BridgeSlot::new_ready_for_test(channel));
        assert_eq!(pool.slot_count.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn atomic_count_prevents_exceeding_max_bridges_concurrently() {
        let mut pool_config = test_pool_config();
        pool_config.max_bridges = 1;
        let pool = Arc::new(CxfBridgePool::from_config(pool_config).expect("valid config"));

        // Pre-fill the one allowed slot.
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        pool.insert_slot_for_test("slot0".to_string(), BridgeSlot::new_ready_for_test(channel));

        // Attempting a second slot should fail because count == max_bridges.
        let err = pool.get_or_create_slot("slot1").await;
        assert!(err.is_err(), "should fail when max_bridges already reached");
        assert!(
            err.unwrap_err().to_string().contains("max_bridges"),
            "error should mention max_bridges"
        );
    }

    /// Regression: max_attempts=N → exactly N invocations (caught OpenSearch off-by-one 1f5c4c2a).
    /// Replicates the exact retry loop used in `connect_with_retry`.
    #[test]
    fn is_retryable_cxf_error_classifies_variants_correctly() {
        // Transient
        assert!(is_retryable_cxf_error(&CxfError::Transport {
            message: "connection refused".to_string(),
            endpoint: "grpc://127.0.0.1:50051".to_string(),
            source: None,
        }));
        assert!(is_retryable_cxf_error(&CxfError::Timeout {
            operation: "sayHello".to_string(),
            endpoint: "http://localhost:8080".to_string(),
        }));

        // Permanent
        assert!(!is_retryable_cxf_error(&CxfError::Fault {
            code: "soap:Server".to_string(),
            message: "Internal error".to_string(),
            endpoint: "http://localhost:8080".to_string(),
        }));
        assert!(!is_retryable_cxf_error(&CxfError::Security {
            message: "token expired".to_string(),
            operation: "sayHello".to_string(),
        }));
        assert!(!is_retryable_cxf_error(&CxfError::Bridge {
            message: "process crashed".to_string(),
            slot_key: "cxf".to_string(),
            source: None,
        }));
        assert!(!is_retryable_cxf_error(&CxfError::Config(
            "missing wsdl_path".to_string()
        )));
        assert!(!is_retryable_cxf_error(&CxfError::StreamClosed {
            endpoint: "http://localhost:8080".to_string(),
        }));
        assert!(!is_retryable_cxf_error(&CxfError::StreamBody {
            endpoint: "http://localhost:8080".to_string(),
        }));
    }

    #[tokio::test]
    async fn retry_loop_invokes_operation_exactly_max_attempts_times() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = Arc::clone(&calls);

        // Drive retry_async directly with is_retryable_cxf_error — adapts
        // the pre-rc-4s9 manual-loop-mirror test to the migrated code path.
        let result: Result<(), CxfError> = retry_async(
            &policy,
            Some("cxf"),
            || {
                let c = Arc::clone(&calls_clone);
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(CxfError::Transport {
                        message: "always fails".to_string(),
                        endpoint: "grpc://127.0.0.1:1".to_string(),
                        source: None,
                    })
                }
            },
            is_retryable_cxf_error,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "max_attempts=3 must yield exactly 3 invocations"
        );
    }
}
