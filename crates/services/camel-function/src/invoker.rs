use crate::config::FunctionConfig;
use crate::pool::{RunnerPool, RunnerPoolKey, RunnerState};
use crate::provider::{FunctionProvider, HealthReport};
use camel_api::Exchange;
use camel_api::function::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) struct DefaultFunctionInvoker {
    pub(crate) pool: Arc<RunnerPool>,
    pub(crate) provider: Arc<dyn FunctionProvider>,
    pub(crate) config: FunctionConfig,
    pub(crate) pending: Mutex<Vec<(FunctionDefinition, Option<String>)>>,
    pub(crate) staging: Mutex<HashMap<u64, StagedEntries>>,
    pub(crate) next_generation: AtomicU64,
    pub(crate) current_generation: AtomicU64,
    pub(crate) started: AtomicBool,
}

type StagedEntries = Vec<(FunctionDefinition, Option<String>)>;

impl DefaultFunctionInvoker {
    pub(crate) fn new(
        pool: Arc<RunnerPool>,
        provider: Arc<dyn FunctionProvider>,
        config: FunctionConfig,
    ) -> Self {
        Self {
            pool,
            provider,
            config,
            pending: Mutex::new(Vec::new()),
            staging: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(0),
            current_generation: AtomicU64::new(0),
            started: AtomicBool::new(false),
        }
    }

    pub(crate) async fn wait_until_healthy(
        &self,
        handle: &crate::pool::RunnerHandle,
    ) -> Result<(), FunctionInvocationError> {
        let deadline = tokio::time::Instant::now() + self.config.boot_timeout;
        loop {
            if tokio::time::Instant::now() > deadline {
                return Err(FunctionInvocationError::RunnerUnavailable {
                    reason: "boot timeout".into(),
                });
            }
            match self.provider.health(handle).await {
                Ok(HealthReport::Healthy) => {
                    *handle.state.lock().expect("state") = RunnerState::Healthy;
                    return Ok(());
                }
                Ok(HealthReport::Unhealthy(reason)) => {
                    *handle.state.lock().expect("state") = RunnerState::Unhealthy {
                        since: std::time::Instant::now(),
                        reason,
                    };
                    tokio::time::sleep(self.config.health_interval).await;
                }
                Err(e) => {
                    return Err(FunctionInvocationError::RunnerUnavailable {
                        reason: e.to_string(),
                    });
                }
            }
        }
    }
}

impl FunctionInvokerSync for DefaultFunctionInvoker {
    fn stage_pending(&self, def: FunctionDefinition, route_id: Option<&str>, generation: u64) {
        let rid = route_id.map(ToOwned::to_owned);
        if !self.started.load(Ordering::SeqCst) {
            self.pending.lock().expect("pending").push((def, rid));
            return;
        }
        let mut staging = self.staging.lock().expect("staging");
        let entry = staging.entry(generation).or_default();
        entry.retain(|(existing, _)| existing.id != def.id);
        entry.push((def, rid));
    }

    fn discard_staging(&self, generation: u64) {
        self.staging.lock().expect("staging").remove(&generation);
    }

    fn begin_reload(&self) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.current_generation.store(generation, Ordering::SeqCst);
        self.staging
            .lock()
            .expect("staging")
            .entry(generation)
            .or_default();
        generation
    }
}

#[async_trait::async_trait]
impl FunctionInvoker for DefaultFunctionInvoker {
    async fn register(
        &self,
        def: FunctionDefinition,
        route_id: Option<&str>,
    ) -> Result<(), FunctionInvocationError> {
        let key = RunnerPoolKey {
            runtime: def.runtime.clone(),
        };
        let handle = if let Some(existing) = self.pool.handles.get(&key) {
            existing.clone()
        } else {
            let spawned = self.provider.spawn(&key).await.map_err(|e| {
                FunctionInvocationError::RunnerUnavailable {
                    reason: e.to_string(),
                }
            })?;
            self.wait_until_healthy(&spawned).await?;
            self.pool.handles.insert(key.clone(), spawned.clone());
            spawned
        };
        self.provider
            .register(&handle, &def)
            .await
            .map_err(|e| FunctionInvocationError::Transport(e.to_string()))?;
        let ref_key = (def.id.clone(), route_id.map(ToOwned::to_owned));
        self.pool
            .ref_counts
            .entry(ref_key.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        self.pool.function_to_key.insert(ref_key, key);
        Ok(())
    }

    async fn unregister(
        &self,
        id: &FunctionId,
        route_id: Option<&str>,
    ) -> Result<(), FunctionInvocationError> {
        let key = (id.clone(), route_id.map(ToOwned::to_owned));
        let mut should_unregister = false;
        if let Some(mut c) = self.pool.ref_counts.get_mut(&key) {
            if *c > 1 {
                *c -= 1;
            } else {
                self.pool.ref_counts.remove(&key);
                should_unregister = true;
            }
        }
        if should_unregister && let Some((_, pool_key)) = self.pool.function_to_key.remove(&key) {
            if let Some(handle) = self.pool.handles.get(&pool_key) {
                self.provider
                    .unregister(&handle, id)
                    .await
                    .map_err(|e| FunctionInvocationError::Transport(e.to_string()))?;
            }
            let still_used = self
                .pool
                .function_to_key
                .iter()
                .any(|kv| kv.value() == &pool_key);
            if !still_used && let Some((_, handle)) = self.pool.handles.remove(&pool_key) {
                handle.cancel.cancel();
                self.provider
                    .shutdown(handle)
                    .await
                    .map_err(|e| FunctionInvocationError::Transport(e.to_string()))?;
            }
        }
        Ok(())
    }

    async fn invoke(
        &self,
        id: &FunctionId,
        exchange: &Exchange,
    ) -> Result<ExchangePatch, FunctionInvocationError> {
        let key = self
            .pool
            .function_to_key
            .iter()
            .find(|kv| kv.key().0 == *id)
            .map(|kv| kv.value().clone())
            .ok_or_else(|| FunctionInvocationError::NotRegistered {
                function_id: id.clone(),
            })?;
        let handle = self
            .pool
            .handles
            .get(&key)
            .map(|h| h.clone())
            .ok_or_else(|| FunctionInvocationError::RunnerUnavailable {
                reason: "missing handle".into(),
            })?;
        let state = handle.state.lock().expect("state").clone();
        match state {
            RunnerState::Failed { reason } => {
                return Err(FunctionInvocationError::RunnerUnavailable { reason });
            }
            RunnerState::Unhealthy { reason, .. } => {
                return Err(FunctionInvocationError::RunnerUnavailable { reason });
            }
            _ => {}
        }
        self.provider
            .invoke(&handle, id, exchange)
            .await
            .map_err(|e| FunctionInvocationError::Transport(e.to_string()))
    }

    async fn commit_reload(
        &self,
        _diff: FunctionDiff,
        _generation: u64,
    ) -> Result<(), FunctionInvocationError> {
        Ok(())
    }

    async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
        let entries = self.staging.lock().expect("staging").remove(&0);
        if let Some(entries) = entries {
            for (def, route_id) in entries {
                self.register(def, route_id.as_deref()).await?;
            }
        }
        Ok(())
    }
}
