use camel_api::function::FunctionId;
use dashmap::DashMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct RunnerPoolKey {
    pub runtime: String,
}

#[derive(Debug, Clone)]
pub enum RunnerState {
    Booting,
    Healthy,
    Unhealthy { since: std::time::Instant, reason: String },
    Failed { reason: String },
}

#[derive(Debug, Clone)]
pub struct RunnerHandle {
    pub id: String,
    pub state: Arc<Mutex<RunnerState>>,
    pub cancel: CancellationToken,
}

pub struct RunnerPool {
    pub(crate) handles: DashMap<RunnerPoolKey, RunnerHandle>,
    pub(crate) ref_counts: DashMap<(FunctionId, Option<String>), u32>,
    pub(crate) function_to_key: DashMap<(FunctionId, Option<String>), RunnerPoolKey>,
}

impl RunnerPool {
    pub fn new() -> Self {
        Self {
            handles: DashMap::new(),
            ref_counts: DashMap::new(),
            function_to_key: DashMap::new(),
        }
    }
}

impl Default for RunnerPool {
    fn default() -> Self {
        Self::new()
    }
}
