use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use camel_bridge::channel::connect_channel;
use camel_bridge::download::ensure_binary_for_spec;
use camel_bridge::health::wait_for_health;
use camel_bridge::process::{BridgeProcess, BridgeProcessConfig, CxfProfileEnvVars};
use camel_bridge::spec::CXF_BRIDGE;
use camel_component_api::CamelError;
use dashmap::DashMap;
use tokio::sync::watch;
use tonic::transport::Channel;
use tracing::{info, warn};

use crate::config::{CxfPoolConfig, validate_profile_name};
use crate::error::CxfError;
use crate::proto::{HealthRequest, cxf_bridge_client::CxfBridgeClient};

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

pub struct CxfBridgePool {
    pub(crate) slots: DashMap<String, Arc<BridgeSlot>>,
    pub(crate) max_bridges: usize,
    pub(crate) bridge_start_timeout_ms: u64,
    pub(crate) health_check_interval_ms: u64,
    pub(crate) bridge_version: String,
    pub(crate) bridge_cache_dir: PathBuf,
    pub(crate) configured_profiles: Vec<crate::config::CxfProfileConfig>,
    pub(crate) bind_address: Option<String>,
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
            max_bridges: pool_config.max_bridges,
            bridge_start_timeout_ms: pool_config.bridge_start_timeout_ms,
            health_check_interval_ms: pool_config.health_check_interval_ms,
            bridge_version: pool_config.version,
            bridge_cache_dir,
            configured_profiles,
            bind_address: pool_config.bind_address,
        })
    }

    pub fn slot_key() -> String {
        "cxf".to_string()
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn insert_slot_for_test(&self, key: String, slot: BridgeSlot) {
        self.slots.insert(key, Arc::new(slot));
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

        if self.slots.len() >= self.max_bridges {
            return Err(CxfError::Config(format!(
                "cannot create bridge slot '{}': max_bridges limit ({}) reached",
                key, self.max_bridges
            )));
        }

        let key_owned = key.to_string();
        let start_timeout_ms = self.bridge_start_timeout_ms;
        let bridge_version = self.bridge_version.clone();
        let bridge_cache_dir = self.bridge_cache_dir.clone();
        let profiles = self.configured_profiles.clone();
        let bind_address = self.bind_address.clone();

        let slot = match self.slots.entry(key_owned.clone()) {
            dashmap::Entry::Occupied(existing) => {
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
                });
                entry.insert(Arc::clone(&slot));
                slot
            }
        };

        let start_result =
            Self::start_bridge_inner(&slot, &bridge_version, &bridge_cache_dir, start_timeout_ms)
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

        Self::spawn_health_monitor(Arc::clone(self), Arc::clone(&slot));

        Ok(slot)
    }

    async fn start_bridge_inner(
        slot: &Arc<BridgeSlot>,
        bridge_version: &str,
        bridge_cache_dir: &Path,
        start_timeout_ms: u64,
    ) -> Result<(BridgeProcess, Channel), CxfError> {
        tracing::trace!(
            key = %slot.key,
            profile_count = slot.configured_profiles.len(),
            "starting CXF bridge slot"
        );

        let result = tokio::time::timeout(Duration::from_millis(start_timeout_ms), async {
            let binary_path = ensure_binary_for_spec(&CXF_BRIDGE, bridge_version, bridge_cache_dir)
                .await
                .map_err(|e| CxfError::Bridge(e.to_string()))?;

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
                    keystore_password: p.security.keystore_password.clone(),
                    truststore_path: p.security.truststore_path.clone(),
                    truststore_password: p.security.truststore_password.clone(),
                    sig_username: p.security.sig_username.clone(),
                    sig_password: p.security.sig_password.clone(),
                    enc_username: p.security.enc_username.clone(),
                    security_actions_out: p.security.security_actions_out.clone(),
                    security_actions_in: p.security.security_actions_in.clone(),
                    signature_algorithm: p.security.signature_algorithm.clone(),
                    signature_digest_algorithm: p.security.signature_digest_algorithm.clone(),
                    signature_c14n_algorithm: p.security.signature_c14n_algorithm.clone(),
                    signature_parts: p.security.signature_parts.clone(),
                })
                .collect();

            let config = BridgeProcessConfig::cxf_profiles(
                binary_path,
                &profile_env_vars,
                start_timeout_ms,
            );
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
                .map_err(|e| CxfError::Bridge(e.to_string()))?;

            let port = process.grpc_port();
            let channel = connect_channel(port)
                .await
                .map_err(|e| CxfError::Transport(e.to_string()))?;

            wait_for_health(&channel, Duration::from_secs(10), |ch| {
                let mut client = CxfBridgeClient::new(ch);
                async move {
                    let resp = client.health(HealthRequest {}).await?;
                    Ok(resp.into_inner().healthy)
                }
            })
            .await
            .map_err(|e| CxfError::Bridge(e.to_string()))?;

            Ok::<(BridgeProcess, Channel), CxfError>((process, channel))
        })
        .await
        .map_err(|_| {
            CxfError::Bridge(format!(
                "CXF bridge start timed out after {}ms",
                start_timeout_ms
            ))
        })??;
        Ok(result)
    }

    fn spawn_health_monitor(pool: Arc<CxfBridgePool>, slot: Arc<BridgeSlot>) {
        let health_interval = pool.health_check_interval_ms;
        let bridge_version = pool.bridge_version.clone();
        let bridge_cache_dir = pool.bridge_cache_dir.clone();
        let start_timeout_ms = pool.bridge_start_timeout_ms;

        tokio::spawn(async move {
            loop {
                let state = slot.state_rx.borrow().clone();
                match state {
                    BridgeState::Stopped => {
                        info!("Health monitor for '{}' exiting (Stopped)", slot.key);
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
                                        key = %slot.key,
                                        message = %inner.message,
                                        "CXF bridge unhealthy"
                                    );
                                    let _ = slot.state_tx.send(BridgeState::Degraded(format!(
                                        "unhealthy: {}",
                                        inner.message
                                    )));
                                }
                            }
                            Ok(Err(e)) => {
                                warn!(
                                    key = %slot.key,
                                    error = %e,
                                    "CXF bridge health check failed"
                                );
                                let _ = slot.state_tx.send(BridgeState::Degraded(e.to_string()));
                            }
                            Err(_) => {
                                let msg = format!(
                                    "health RPC timed out after {}ms",
                                    health_timeout.as_millis()
                                );
                                warn!(
                                    key = %slot.key,
                                    "CXF bridge health check timed out"
                                );
                                let _ = slot.state_tx.send(BridgeState::Degraded(msg));
                            }
                        }
                    }
                    BridgeState::Degraded(_) | BridgeState::Starting => {
                        if matches!(*slot.state_rx.borrow(), BridgeState::Stopped) {
                            break;
                        }
                        info!(
                            "Health monitor for '{}' found degraded/starting, triggering restart",
                            slot.key
                        );
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

                        if matches!(*slot.state_rx.borrow(), BridgeState::Stopped) {
                            break;
                        }

                        info!(
                            "Restarting CXF bridge for '{}' (attempt {})",
                            slot.key,
                            attempt + 1
                        );

                        let old_process = {
                            let mut guard = slot.process.lock().await;
                            guard.take()
                        };
                        if let Some(p) = old_process {
                            let _ = p.stop().await;
                        }

                        let start_result: Result<(BridgeProcess, Channel), CxfError> = async {
                            let profiles = &slot.configured_profiles;
                            let binary_path = ensure_binary_for_spec(
                                &CXF_BRIDGE,
                                &bridge_version,
                                &bridge_cache_dir,
                            )
                            .await
                            .map_err(|e| CxfError::Bridge(e.to_string()))?;

                            let profile_env_vars: Vec<CxfProfileEnvVars> = profiles
                                .iter()
                                .map(|p| CxfProfileEnvVars {
                                    name: p.name.clone(),
                                    wsdl_path: p.wsdl_path.clone(),
                                    service_name: p.service_name.clone(),
                                    port_name: p.port_name.clone(),
                                    address: p.address.clone(),
                                    keystore_path: p.security.keystore_path.clone(),
                                    keystore_password: p.security.keystore_password.clone(),
                                    truststore_path: p.security.truststore_path.clone(),
                                    truststore_password: p.security.truststore_password.clone(),
                                    sig_username: p.security.sig_username.clone(),
                                    sig_password: p.security.sig_password.clone(),
                                    enc_username: p.security.enc_username.clone(),
                                    security_actions_out: p.security.security_actions_out.clone(),
                                    security_actions_in: p.security.security_actions_in.clone(),
                                    signature_algorithm: p.security.signature_algorithm.clone(),
                                    signature_digest_algorithm: p.security.signature_digest_algorithm.clone(),
                                    signature_c14n_algorithm: p.security.signature_c14n_algorithm.clone(),
                                    signature_parts: p.security.signature_parts.clone(),
                                })
                                .collect();

                            let config = BridgeProcessConfig::cxf_profiles(
                                binary_path,
                                &profile_env_vars,
                                start_timeout_ms,
                            );
                            let mut config = config;
                            if let Some(addr) = slot.bind_address.as_ref() {
                                config
                                    .env_vars
                                    .push(("CXF_ADDRESS".to_string(), addr.clone()));
                            }

                            let process = BridgeProcess::start(&config)
                                .await
                                .map_err(|e| CxfError::Bridge(e.to_string()))?;
                            let port = process.grpc_port();
                            let channel = connect_channel(port)
                                .await
                                .map_err(|e| CxfError::Transport(e.to_string()))?;

                            wait_for_health(&channel, Duration::from_secs(10), |ch| {
                                let mut client = CxfBridgeClient::new(ch);
                                async move {
                                    let resp = client.health(HealthRequest {}).await?;
                                    Ok(resp.into_inner().healthy)
                                }
                            })
                            .await
                            .map_err(|e| CxfError::Bridge(e.to_string()))?;

                            Ok((process, channel))
                        }
                        .await;

                        match start_result {
                            Ok((process, channel)) => {
                                if matches!(*slot.state_rx.borrow(), BridgeState::Stopped) {
                                    let _ = process.stop().await;
                                    break;
                                }
                                {
                                    let mut guard = slot.process.lock().await;
                                    *guard = Some(process);
                                }
                                let _ = slot.state_tx.send(BridgeState::Ready { channel });
                                info!("CXF bridge '{}' restarted successfully", slot.key);
                            }
                            Err(e) => {
                                if matches!(*slot.state_rx.borrow(), BridgeState::Stopped) {
                                    break;
                                }
                                let delay_secs = std::cmp::min(2u64.pow(attempt.min(5)), 30);
                                let next = Instant::now() + Duration::from_secs(delay_secs);
                                warn!(
                                    "Failed to restart CXF bridge for '{}' (attempt {}): {e}. Retry in {delay_secs}s",
                                    slot.key,
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

    pub fn restart_slot(&self, key: &str) {
        if let Some(slot) = self.slots.get(key) {
            let _ = slot.state_tx.send(BridgeState::Restarting {
                attempt: 0,
                next_at: Instant::now(),
            });
        }
    }

    pub async fn refresh_slot_channel(&self, key: &str) -> Result<(), CxfError> {
        let slot = self
            .slots
            .get(key)
            .ok_or_else(|| CxfError::Bridge(format!("no slot for key '{}'", key)))?;

        let guard = slot.process.lock().await;
        if let Some(process) = guard.as_ref() {
            let port = process.grpc_port();
            let channel = connect_channel(port)
                .await
                .map_err(|e| CxfError::Transport(e.to_string()))?;
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
                    return Err(CxfError::Bridge(format!(
                        "CXF bridge '{}' is stopped",
                        slot.key
                    )));
                }
                _ => {}
            }
            if rx.changed().await.is_err() {
                return Err(CxfError::Bridge(format!(
                    "CXF bridge '{}' state channel closed",
                    slot.key
                )));
            }
        }
    }

    pub async fn shutdown(&self) -> Result<(), CamelError> {
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

    #[test]
    fn slot_key_is_constant() {
        let key = CxfBridgePool::slot_key();
        assert_eq!(key, "cxf");
    }

    #[test]
    fn from_config_uses_defaults() {
        let pool_config = CxfPoolConfig {
            profiles: vec![],
            max_bridges: 2,
            bridge_start_timeout_ms: 15_000,
            health_check_interval_ms: 3_000,
            bridge_cache_dir: None,
            version: "0.1.0".to_string(),
            bind_address: None,
        };
        let pool = CxfBridgePool::from_config(pool_config).expect("valid config");
        assert_eq!(pool.max_bridges, 2);
        assert_eq!(pool.bridge_start_timeout_ms, 15_000);
        assert_eq!(pool.bridge_version, "0.1.0");
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
        let pool_config = CxfPoolConfig {
            profiles: vec![],
            max_bridges: 2,
            bridge_start_timeout_ms: 15_000,
            health_check_interval_ms: 3_000,
            bridge_cache_dir: None,
            version: "0.1.0".to_string(),
            bind_address: None,
        };
        let pool = CxfBridgePool::from_config(pool_config).expect("valid config");

        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = Arc::new(BridgeSlot {
            key: "test-key".to_string(),
            configured_profiles: vec![],
            bind_address: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
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
        let pool_config = CxfPoolConfig {
            profiles: vec![],
            max_bridges: 0,
            bridge_start_timeout_ms: 15_000,
            health_check_interval_ms: 3_000,
            bridge_cache_dir: None,
            version: "0.1.0".to_string(),
            bind_address: None,
        };
        let pool = Arc::new(CxfBridgePool::from_config(pool_config).expect("valid config"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(pool.get_or_create_slot("key"));
        assert!(result.is_err(), "should fail when max_bridges is 0");
        let err = result.unwrap_err();
        assert!(err.to_string().contains("max_bridges"), "got: {err}");
    }
}
