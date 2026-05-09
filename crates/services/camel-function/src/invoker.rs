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
        let did = def.id.clone();
        let rid_owned = rid.clone();
        entry
            .retain(|(existing, existing_rid)| !(existing.id == did && existing_rid == &rid_owned));
        entry.push((def, rid_owned));
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

    fn function_refs_for_route(&self, route_id: &str) -> Vec<(FunctionId, Option<String>)> {
        self.pool
            .function_to_key
            .iter()
            .filter(|kv| kv.key().1.as_deref() == Some(route_id))
            .map(|kv| kv.key().clone())
            .collect()
    }

    fn staged_refs_for_route(
        &self,
        route_id: &str,
        generation: u64,
    ) -> Vec<(FunctionId, Option<String>)> {
        let staging = self.staging.lock().expect("staging");
        staging
            .get(&generation)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|(_, rid)| rid.as_deref() == Some(route_id))
                    .map(|(def, rid)| (def.id.clone(), rid.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn staged_defs_for_route(
        &self,
        route_id: &str,
        generation: u64,
    ) -> Vec<(FunctionDefinition, Option<String>)> {
        let staging = self.staging.lock().expect("staging");
        staging
            .get(&generation)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|(_, rid)| rid.as_deref() == Some(route_id))
                    .map(|(def, rid)| (def.clone(), rid.clone()))
                    .collect()
            })
            .unwrap_or_default()
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
        let handle = {
            let existing = self.pool.handles.get(&key).map(|h| h.clone());
            match existing {
                Some(h) => h,
                None => {
                    let spawned = self.provider.spawn(&key).await.map_err(|e| {
                        FunctionInvocationError::RunnerUnavailable {
                            reason: e.to_string(),
                        }
                    })?;
                    self.wait_until_healthy(&spawned).await?;
                    self.pool
                        .handles
                        .entry(key.clone())
                        .or_insert(spawned)
                        .clone()
                }
            }
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
                should_unregister = true;
            }
        }
        if should_unregister {
            self.pool.ref_counts.remove(&key);
        }
        if should_unregister && let Some((_, pool_key)) = self.pool.function_to_key.remove(&key) {
            let still_used_by_other_route =
                self.pool.function_to_key.iter().any(|kv| kv.key().0 == *id);
            if !still_used_by_other_route {
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

    async fn prepare_reload(
        &self,
        diff: FunctionDiff,
        generation: u64,
    ) -> Result<PrepareToken, FunctionInvocationError> {
        let current_gen = self.current_generation.load(Ordering::SeqCst);
        if generation != current_gen {
            self.discard_staging(generation);
            return Err(FunctionInvocationError::Transport(format!(
                "stale generation: expected {}, got {}",
                current_gen, generation
            )));
        }

        {
            let mut staging = self.staging.lock().expect("staging");
            let before = staging.len();
            staging.retain(|g, _| *g >= current_gen);
            let purged = before - staging.len();
            if purged > 0 {
                tracing::info!(purged, "hot-reload: purged stale staging buffers");
            }
        }

        let mut token = PrepareToken::default();
        for (def, route_id) in &diff.added {
            match self.register(def.clone(), route_id.as_deref()).await {
                Ok(()) => {
                    token.registered.push((def.clone(), route_id.clone()));
                }
                Err(e) => {
                    for (reg_def, reg_rid) in &token.registered {
                        if let Err(unreg_err) =
                            self.unregister(&reg_def.id, reg_rid.as_deref()).await
                        {
                            tracing::warn!(
                                function_id = %reg_def.id,
                                error = %unreg_err,
                                "hot-reload: failed to unregister during prepare rollback"
                            );
                        }
                    }
                    self.staging.lock().expect("staging").remove(&generation);
                    return Err(e);
                }
            }
        }
        Ok(token)
    }

    async fn finalize_reload(
        &self,
        diff: &FunctionDiff,
        generation: u64,
    ) -> Result<(), FunctionInvocationError> {
        for (id, route_id) in &diff.removed {
            self.unregister(id, route_id.as_deref()).await?;
        }
        self.staging.lock().expect("staging").remove(&generation);
        Ok(())
    }

    async fn rollback_reload(
        &self,
        token: PrepareToken,
        generation: u64,
    ) -> Result<(), FunctionInvocationError> {
        for (def, route_id) in &token.registered {
            if let Err(e) = self.unregister(&def.id, route_id.as_deref()).await {
                tracing::warn!(
                    function_id = %def.id,
                    error = %e,
                    "hot-reload: failed to unregister during rollback"
                );
            }
        }
        self.staging.lock().expect("staging").remove(&generation);
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
