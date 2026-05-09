use crate::Exchange;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct FunctionId(pub String);

impl std::fmt::Display for FunctionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl FunctionId {
    pub fn compute(runtime: &str, source: &str, timeout_ms_resolved: u64) -> Self {
        let mut hasher = blake3::Hasher::new();
        let rlen = (runtime.len() as u64).to_le_bytes();
        hasher.update(&rlen);
        hasher.update(runtime.as_bytes());
        let slen = (source.len() as u64).to_le_bytes();
        hasher.update(&slen);
        hasher.update(source.as_bytes());
        hasher.update(&timeout_ms_resolved.to_le_bytes());
        let hash = hasher.finalize();
        let truncated = &hash.as_bytes()[..16];
        let hex: String = truncated.iter().map(|b| format!("{:02x}", b)).collect();
        Self(hex)
    }
}

#[derive(Debug, Clone)]
pub struct FunctionDefinition {
    pub id: FunctionId,
    pub runtime: String,
    pub source: String,
    pub timeout_ms: u64,
    pub route_id: Option<String>,
    pub step_index: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct ExchangePatch {
    pub body: Option<PatchBody>,
    pub headers_set: Vec<(String, serde_json::Value)>,
    pub headers_removed: Vec<String>,
    pub properties_set: Vec<(String, serde_json::Value)>,
}

#[derive(Debug, Clone)]
pub enum PatchBody {
    Text(String),
    Json(serde_json::Value),
    Empty,
}

#[derive(Debug, thiserror::Error)]
pub enum FunctionInvocationError {
    #[error("function {function_id} not registered on runtime")]
    NotRegistered { function_id: FunctionId },
    #[error("function {function_id} timed out after {timeout_ms}ms")]
    Timeout {
        function_id: FunctionId,
        timeout_ms: u64,
    },
    #[error("runner unavailable: {reason}")]
    RunnerUnavailable { reason: String },
    #[error("user code failed: {message}")]
    UserError {
        function_id: FunctionId,
        message: String,
        stack: Option<String>,
    },
    #[error("transport error: {0}")]
    Transport(String),
    #[error("invalid patch: {0}")]
    InvalidPatch(String),
}

#[derive(Debug, Default, Clone)]
pub struct FunctionDiff {
    pub added: Vec<(FunctionDefinition, Option<String>)>,
    pub removed: Vec<(FunctionId, Option<String>)>,
    pub unchanged: Vec<FunctionId>,
}

#[derive(Debug, Default, Clone)]
pub struct PrepareToken {
    pub registered: Vec<(FunctionDefinition, Option<String>)>,
}

pub trait FunctionInvokerSync: Send + Sync {
    fn stage_pending(&self, def: FunctionDefinition, route_id: Option<&str>, generation: u64);
    fn discard_staging(&self, generation: u64);
    fn begin_reload(&self) -> u64;
    fn function_refs_for_route(&self, route_id: &str) -> Vec<(FunctionId, Option<String>)>;
    fn staged_refs_for_route(
        &self,
        route_id: &str,
        generation: u64,
    ) -> Vec<(FunctionId, Option<String>)>;
    fn staged_defs_for_route(
        &self,
        route_id: &str,
        generation: u64,
    ) -> Vec<(FunctionDefinition, Option<String>)>;
}

#[async_trait::async_trait]
pub trait FunctionInvoker: FunctionInvokerSync + Send + Sync {
    async fn register(
        &self,
        def: FunctionDefinition,
        route_id: Option<&str>,
    ) -> Result<(), FunctionInvocationError>;
    async fn unregister(
        &self,
        id: &FunctionId,
        route_id: Option<&str>,
    ) -> Result<(), FunctionInvocationError>;
    async fn invoke(
        &self,
        id: &FunctionId,
        exchange: &Exchange,
    ) -> Result<ExchangePatch, FunctionInvocationError>;
    async fn prepare_reload(
        &self,
        diff: FunctionDiff,
        generation: u64,
    ) -> Result<PrepareToken, FunctionInvocationError>;
    async fn finalize_reload(
        &self,
        diff: &FunctionDiff,
        generation: u64,
    ) -> Result<(), FunctionInvocationError>;
    async fn rollback_reload(
        &self,
        token: PrepareToken,
        generation: u64,
    ) -> Result<(), FunctionInvocationError>;
    async fn commit_reload(
        &self,
        diff: FunctionDiff,
        generation: u64,
    ) -> Result<(), FunctionInvocationError> {
        let token = self.prepare_reload(diff.clone(), generation).await?;
        self.finalize_reload(&diff, generation).await
    }
    async fn commit_staged(&self) -> Result<(), FunctionInvocationError>;
}
