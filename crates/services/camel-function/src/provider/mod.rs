use crate::pool::{RunnerHandle, RunnerPoolKey};
use camel_api::{Exchange, function::*};

mod sealed {
    pub trait Sealed {}
}

#[derive(Debug, Clone)]
pub enum HealthReport {
    Healthy,
    Unhealthy(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("spawn failed: {0}")]
    SpawnFailed(String),
    #[error("health check failed: {0}")]
    HealthFailed(String),
    #[error("register failed: {0}")]
    RegisterFailed(String),
    #[error("unregister failed: {0}")]
    UnregisterFailed(String),
    #[error("invoke failed: {0}")]
    InvokeFailed(String),
    #[error("shutdown failed: {0}")]
    ShutdownFailed(String),
    #[error("boot timeout")]
    BootTimeout,
}

#[async_trait::async_trait]
pub(crate) trait FunctionProvider: Send + Sync + sealed::Sealed {
    async fn spawn(&self, key: &RunnerPoolKey) -> Result<RunnerHandle, ProviderError>;
    async fn shutdown(&self, handle: RunnerHandle) -> Result<(), ProviderError>;
    async fn health(&self, handle: &RunnerHandle) -> Result<HealthReport, ProviderError>;
    async fn register(
        &self,
        handle: &RunnerHandle,
        def: &FunctionDefinition,
    ) -> Result<(), ProviderError>;
    async fn unregister(&self, handle: &RunnerHandle, id: &FunctionId)
    -> Result<(), ProviderError>;
    async fn invoke(
        &self,
        handle: &RunnerHandle,
        id: &FunctionId,
        ex: &Exchange,
    ) -> Result<ExchangePatch, ProviderError>;
}

pub mod fake {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    #[derive(Debug, Clone, Default)]
    pub struct FakeProviderConfig {
        pub fail_on_spawn: bool,
        pub fail_on_register: usize,
        pub fail_on_health: bool,
        pub invoke_response: Option<ExchangePatch>,
    }

    #[derive(Debug, Clone)]
    pub enum FakeCall {
        Spawn(RunnerPoolKey),
        Shutdown(RunnerPoolKey),
        Health(String),
        Register(String, FunctionId),
        Unregister(String, FunctionId),
        Invoke(String, FunctionId),
    }

    pub struct FakeProvider {
        pub config: Arc<Mutex<FakeProviderConfig>>,
        pub calls: Arc<Mutex<Vec<FakeCall>>>,
        pub registered: Arc<Mutex<HashMap<String, HashSet<FunctionId>>>>,
        pub spawned: Arc<Mutex<Vec<RunnerPoolKey>>>,
        pub shutdowns: Arc<Mutex<Vec<RunnerPoolKey>>>,
        register_ok_count: Arc<Mutex<usize>>,
    }

    impl FakeProvider {
        pub fn new(config: FakeProviderConfig) -> Self {
            Self {
                config: Arc::new(Mutex::new(config)),
                calls: Arc::new(Mutex::new(Vec::new())),
                registered: Arc::new(Mutex::new(HashMap::new())),
                spawned: Arc::new(Mutex::new(Vec::new())),
                shutdowns: Arc::new(Mutex::new(Vec::new())),
                register_ok_count: Arc::new(Mutex::new(0)),
            }
        }
    }

    impl super::sealed::Sealed for FakeProvider {}

    #[async_trait::async_trait]
    impl FunctionProvider for FakeProvider {
        async fn spawn(&self, key: &RunnerPoolKey) -> Result<RunnerHandle, ProviderError> {
            self.calls
                .lock()
                .expect("calls")
                .push(FakeCall::Spawn(key.clone()));
            self.spawned.lock().expect("spawned").push(key.clone());
            if self.config.lock().expect("config").fail_on_spawn {
                return Err(ProviderError::SpawnFailed("configured".into()));
            }
            Ok(RunnerHandle {
                id: format!("fake-{}", key.runtime),
                state: Arc::new(Mutex::new(crate::pool::RunnerState::Booting)),
                cancel: CancellationToken::new(),
            })
        }

        async fn shutdown(&self, handle: RunnerHandle) -> Result<(), ProviderError> {
            self.calls
                .lock()
                .expect("calls")
                .push(FakeCall::Shutdown(RunnerPoolKey {
                    runtime: handle.id.replace("fake-", ""),
                }));
            self.shutdowns
                .lock()
                .expect("shutdowns")
                .push(RunnerPoolKey {
                    runtime: handle.id.replace("fake-", ""),
                });
            Ok(())
        }

        async fn health(&self, handle: &RunnerHandle) -> Result<HealthReport, ProviderError> {
            self.calls
                .lock()
                .expect("calls")
                .push(FakeCall::Health(handle.id.clone()));
            if self.config.lock().expect("config").fail_on_health {
                return Ok(HealthReport::Unhealthy("configured".into()));
            }
            Ok(HealthReport::Healthy)
        }

        async fn register(
            &self,
            handle: &RunnerHandle,
            def: &FunctionDefinition,
        ) -> Result<(), ProviderError> {
            self.calls
                .lock()
                .expect("calls")
                .push(FakeCall::Register(handle.id.clone(), def.id.clone()));
            let mut count = self.register_ok_count.lock().expect("count");
            let cfg = self.config.lock().expect("config").clone();
            if cfg.fail_on_register > 0 && *count >= cfg.fail_on_register {
                return Err(ProviderError::RegisterFailed("configured".into()));
            }
            *count += 1;
            self.registered
                .lock()
                .expect("registered")
                .entry(handle.id.clone())
                .or_default()
                .insert(def.id.clone());
            Ok(())
        }

        async fn unregister(
            &self,
            handle: &RunnerHandle,
            id: &FunctionId,
        ) -> Result<(), ProviderError> {
            self.calls
                .lock()
                .expect("calls")
                .push(FakeCall::Unregister(handle.id.clone(), id.clone()));
            if let Some(set) = self
                .registered
                .lock()
                .expect("registered")
                .get_mut(&handle.id)
            {
                set.remove(id);
            }
            Ok(())
        }

        async fn invoke(
            &self,
            handle: &RunnerHandle,
            id: &FunctionId,
            _ex: &Exchange,
        ) -> Result<ExchangePatch, ProviderError> {
            self.calls
                .lock()
                .expect("calls")
                .push(FakeCall::Invoke(handle.id.clone(), id.clone()));
            let exists = self
                .registered
                .lock()
                .expect("registered")
                .get(&handle.id)
                .map(|s| s.contains(id))
                .unwrap_or(false);
            if !exists {
                return Err(ProviderError::InvokeFailed("not registered".into()));
            }
            let cfg = self.config.lock().expect("config").clone();
            Ok(cfg.invoke_response.unwrap_or_default())
        }
    }
}
