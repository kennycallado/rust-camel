use crate::config::FunctionConfig;
use crate::invoker::DefaultFunctionInvoker;
use crate::pool::{RunnerPool, RunnerPoolKey, RunnerState};
use crate::provider::{FunctionProvider, HealthReport, ProviderError};
use camel_api::function::{FunctionDefinition, FunctionId, FunctionInvoker};
use camel_api::{CamelError, Lifecycle, ServiceStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

const STATUS_STOPPED: u8 = 0;
const STATUS_STARTED: u8 = 1;
const STATUS_FAILED: u8 = 2;

pub struct FunctionRuntimeService {
    config: FunctionConfig,
    provider: Arc<dyn FunctionProvider>,
    container_provider: Option<Arc<crate::provider::container::ContainerProvider>>,
    pub(crate) invoker: Arc<DefaultFunctionInvoker>,
    status: Arc<AtomicU8>,
}

impl FunctionRuntimeService {
    pub(crate) fn new(config: FunctionConfig, provider: Arc<dyn FunctionProvider>) -> Self {
        let pool = Arc::new(RunnerPool::new());
        let invoker = Arc::new(DefaultFunctionInvoker::new(
            Arc::clone(&pool),
            Arc::clone(&provider),
            config.clone(),
        ));
        Self {
            config,
            provider,
            container_provider: None,
            invoker,
            status: Arc::new(AtomicU8::new(STATUS_STOPPED)),
        }
    }

    pub fn with_fake_provider(
        config: FunctionConfig,
        provider: Arc<crate::provider::fake::FakeProvider>,
    ) -> Self {
        Self::new(config, provider as Arc<dyn FunctionProvider>)
    }

    pub fn with_container_provider(
        config: FunctionConfig,
        provider: crate::provider::container::ContainerProvider,
    ) -> Self {
        let arc = Arc::new(provider);
        let mut svc = Self::new(config, arc.clone() as Arc<dyn FunctionProvider>);
        svc.container_provider = Some(arc);
        svc
    }

    pub fn with_default_container_provider(
        config: FunctionConfig,
    ) -> Result<Self, crate::provider::ProviderError> {
        let provider = crate::provider::container::ContainerProvider::builder().build()?;
        Ok(Self::with_container_provider(config, provider))
    }

    pub fn invoker(&self) -> Arc<dyn FunctionInvoker> {
        self.invoker.clone() as Arc<dyn FunctionInvoker>
    }

    pub fn provider(&self) -> Result<&crate::provider::container::ContainerProvider, CamelError> {
        self.container_provider
            .as_ref()
            .map(|arc| arc.as_ref())
            .ok_or_else(|| {
                CamelError::Config("unsupported provider type: not a container provider".into())
            })
    }

    pub fn runner_state(&self, runtime: &str) -> Option<RunnerState> {
        let key = RunnerPoolKey {
            runtime: runtime.to_string(),
        };
        self.invoker
            .pool
            .handles
            .get(&key)
            .map(|h| h.state.lock().expect("state").clone()) // allow-unwrap
    }

    pub fn force_runner_failed(&self, runtime: &str, reason: &str) {
        let key = RunnerPoolKey {
            runtime: runtime.to_string(),
        };
        if let Some(handle) = self.invoker.pool.handles.get(&key) {
            *handle.state.lock().expect("state") = RunnerState::Failed {
                // allow-unwrap
                reason: reason.to_string(),
            };
        }
    }

    pub(crate) async fn wait_until_healthy(
        &self,
        handle: &crate::pool::RunnerHandle,
    ) -> Result<(), ProviderError> {
        let deadline = tokio::time::Instant::now() + self.config.boot_timeout;
        loop {
            if tokio::time::Instant::now() > deadline {
                return Err(ProviderError::BootTimeout);
            }
            match self.provider.health(handle).await {
                Ok(HealthReport::Healthy) => {
                    *handle.state.lock().expect("state") = RunnerState::Healthy; // allow-unwrap
                    return Ok(());
                }
                Ok(HealthReport::Unhealthy(reason)) => {
                    *handle.state.lock().expect("state") = RunnerState::Unhealthy {
                        // allow-unwrap
                        since: std::time::Instant::now(),
                        reason,
                    };
                    tokio::time::sleep(self.config.health_interval).await;
                }
                Err(_) => {
                    tokio::time::sleep(self.config.health_interval).await;
                }
            }
        }
    }

    pub(crate) fn spawn_health_task(&self, handle: crate::pool::RunnerHandle) {
        let provider = Arc::clone(&self.provider);
        let interval = self.config.health_interval;
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(interval);
            let mut unhealthy_count = 0u8;
            loop {
                tokio::select! {
                    _ = handle.cancel.cancelled() => break,
                    _ = ticks.tick() => {
                        match provider.health(&handle).await {
                            Ok(HealthReport::Healthy) => {
                                unhealthy_count = 0;
                                *handle.state.lock().expect("state") = RunnerState::Healthy; // allow-unwrap
                            }
                            Ok(HealthReport::Unhealthy(reason)) => {
                                unhealthy_count = unhealthy_count.saturating_add(1);
                                if unhealthy_count >= 2 {
                                    *handle.state.lock().expect("state") = RunnerState::Unhealthy { since: std::time::Instant::now(), reason }; // allow-unwrap
                                }
                            }
                            Err(err) => {
                                *handle.state.lock().expect("state") = RunnerState::Failed { reason: err.to_string() }; // allow-unwrap
                            }
                        }
                    }
                }
            }
        });
    }

    pub(crate) async fn rollback_start(
        &self,
        spawned: &[(RunnerPoolKey, crate::pool::RunnerHandle)],
        registered_refs: &[((FunctionId, Option<String>), RunnerPoolKey)],
        pending: &[(FunctionDefinition, Option<String>)],
    ) {
        for (ref_key, _pool_key) in registered_refs {
            self.invoker.pool.ref_counts.remove(ref_key);
            self.invoker.pool.function_to_key.remove(ref_key);
            let still_used = self
                .invoker
                .pool
                .function_to_key
                .iter()
                .any(|kv| kv.key().0 == ref_key.0);
            if !still_used {
                self.invoker
                    .function_timeouts
                    .lock()
                    .expect("function_timeouts") // allow-unwrap
                    .remove(&ref_key.0);
            }
        }
        for (key, handle) in spawned {
            self.invoker.pool.handles.remove(key);
            handle.cancel.cancel();
            let _ = self.provider.shutdown(handle.clone()).await;
        }
        self.invoker
            .pending
            .lock()
            .expect("pending") // allow-unwrap
            .extend(pending.iter().cloned());
    }
}

#[async_trait::async_trait]
impl Lifecycle for FunctionRuntimeService {
    fn name(&self) -> &str {
        "function-runtime"
    }

    fn status(&self) -> ServiceStatus {
        match self.status.load(Ordering::SeqCst) {
            STATUS_STOPPED => ServiceStatus::Stopped,
            STATUS_STARTED => ServiceStatus::Started,
            _ => ServiceStatus::Failed,
        }
    }

    async fn start(&mut self) -> Result<(), CamelError> {
        self.config.validate()?;
        if self.status.load(Ordering::SeqCst) == STATUS_STARTED {
            return Ok(());
        }
        let pending = {
            let mut lock = self.invoker.pending.lock().expect("pending"); // allow-unwrap
            std::mem::take(&mut *lock)
        };
        let mut grouped: std::collections::HashMap<
            RunnerPoolKey,
            Vec<(FunctionDefinition, Option<String>)>,
        > = std::collections::HashMap::new();
        for (def, route_id) in pending.iter().cloned() {
            grouped
                .entry(RunnerPoolKey {
                    runtime: def.runtime.clone(),
                })
                .or_default()
                .push((def, route_id));
        }
        let mut spawned: Vec<(RunnerPoolKey, crate::pool::RunnerHandle)> = Vec::new();
        let mut registered_refs: Vec<((FunctionId, Option<String>), RunnerPoolKey)> = Vec::new();
        for (key, defs) in grouped {
            let handle = match self.provider.spawn(&key).await {
                Ok(h) => h,
                Err(e) => {
                    self.rollback_start(&spawned, &registered_refs, &pending)
                        .await;
                    self.status.store(STATUS_FAILED, Ordering::SeqCst);
                    return Err(CamelError::Config(format!("function: spawn failed: {e}")));
                }
            };
            match self.wait_until_healthy(&handle).await {
                Ok(()) => {}
                Err(e) => {
                    handle.cancel.cancel();
                    let _ = self.provider.shutdown(handle).await;
                    self.rollback_start(&spawned, &registered_refs, &pending)
                        .await;
                    self.status.store(STATUS_FAILED, Ordering::SeqCst);
                    return Err(CamelError::Config(format!("function: boot timeout: {e}")));
                }
            }
            self.invoker
                .pool
                .handles
                .insert(key.clone(), handle.clone());
            self.spawn_health_task(handle.clone());
            spawned.push((key.clone(), handle.clone()));
            for (def, route_id) in defs {
                if let Err(err) = self.provider.register(&handle, &def).await {
                    self.rollback_start(&spawned, &registered_refs, &pending)
                        .await;
                    self.status.store(STATUS_FAILED, Ordering::SeqCst);
                    return Err(CamelError::Config(format!(
                        "function: register failed: {err}"
                    )));
                }
                let ref_key = (def.id.clone(), route_id.clone());
                self.invoker.pool.ref_counts.insert(ref_key.clone(), 1);
                self.invoker
                    .pool
                    .function_to_key
                    .insert(ref_key.clone(), key.clone());
                registered_refs.push((ref_key, key.clone()));
            }
        }
        self.invoker.started.store(true, Ordering::SeqCst);
        self.status.store(STATUS_STARTED, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        let handles: Vec<_> = self
            .invoker
            .pool
            .handles
            .iter()
            .map(|h| h.clone())
            .collect();
        self.invoker.pool.handles.clear();
        self.invoker.pool.ref_counts.clear();
        self.invoker.pool.function_to_key.clear();
        self.invoker
            .function_timeouts
            .lock()
            .expect("function_timeouts") // allow-unwrap
            .clear();
        for handle in handles {
            handle.cancel.cancel();
            self.provider
                .shutdown(handle)
                .await
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;
        }
        self.invoker.started.store(false, Ordering::SeqCst);
        self.status.store(STATUS_STOPPED, Ordering::SeqCst);
        Ok(())
    }

    fn as_function_invoker(&self) -> Option<Arc<dyn FunctionInvoker>> {
        Some(self.invoker.clone() as Arc<dyn FunctionInvoker>)
    }
}
