use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::CamelError;
use crate::declarative::LanguageExpressionDef;

pub const CANONICAL_CONTRACT_NAME: &str = "canonical-v1";
pub const CANONICAL_CONTRACT_VERSION: u32 = 1;
pub const CANONICAL_CONTRACT_SUPPORTED_STEPS: &[&str] = &[
    "to",
    "log",
    "wire_tap",
    "script",
    "filter",
    "choice",
    "split",
    "aggregate",
    "stop",
];
pub const CANONICAL_CONTRACT_DECLARATIVE_ONLY_STEPS: &[&str] =
    &["script", "filter", "choice", "split"];
pub const CANONICAL_CONTRACT_EXCLUDED_DECLARATIVE_STEPS: &[&str] = &[
    "set_header",
    "set_body",
    "multicast",
    "convert_body_to",
    "bean",
    "marshal",
    "unmarshal",
];
pub const CANONICAL_CONTRACT_RUST_ONLY_STEPS: &[&str] = &[
    "processor",
    "process",
    "process_fn",
    "map_body",
    "set_body_fn",
    "set_header_fn",
];

pub fn canonical_contract_supports_step(step: &str) -> bool {
    CANONICAL_CONTRACT_SUPPORTED_STEPS.contains(&step)
}

pub fn canonical_contract_rejection_reason(step: &str) -> Option<&'static str> {
    if CANONICAL_CONTRACT_EXCLUDED_DECLARATIVE_STEPS.contains(&step) {
        return Some(
            "declared out-of-scope for canonical v1; use declarative route compilation path outside CQRS canonical commands",
        );
    }

    if CANONICAL_CONTRACT_RUST_ONLY_STEPS.contains(&step) {
        return Some("rust-only programmable step; not representable in canonical v1 contract");
    }

    if canonical_contract_supports_step(step)
        && CANONICAL_CONTRACT_DECLARATIVE_ONLY_STEPS.contains(&step)
    {
        return Some(
            "supported only as declarative/serializable expression form; closure/processor variants are outside canonical v1",
        );
    }

    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalRouteSpec {
    /// Stable minimal route representation for runtime command registration.
    ///
    /// Scope note:
    /// - This is intentionally a partial model (v1) and does not mirror every `BuilderStep`.
    /// - Advanced EIPs continue to use the existing RouteDefinition/BuilderStep path.
    pub route_id: String,
    pub from: String,
    pub steps: Vec<CanonicalStepSpec>,
    pub circuit_breaker: Option<CanonicalCircuitBreakerSpec>,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalStepSpec {
    To {
        uri: String,
    },
    Log {
        message: String,
    },
    WireTap {
        uri: String,
    },
    Script {
        expression: LanguageExpressionDef,
    },
    Filter {
        predicate: LanguageExpressionDef,
        steps: Vec<CanonicalStepSpec>,
    },
    Choice {
        whens: Vec<CanonicalWhenSpec>,
        otherwise: Option<Vec<CanonicalStepSpec>>,
    },
    Split {
        expression: CanonicalSplitExpressionSpec,
        aggregation: CanonicalSplitAggregationSpec,
        parallel: bool,
        parallel_limit: Option<usize>,
        stop_on_exception: bool,
        steps: Vec<CanonicalStepSpec>,
    },
    Aggregate {
        config: CanonicalAggregateSpec,
    },
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalWhenSpec {
    pub predicate: LanguageExpressionDef,
    pub steps: Vec<CanonicalStepSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalSplitExpressionSpec {
    BodyLines,
    BodyJsonArray,
    Language(LanguageExpressionDef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalSplitAggregationSpec {
    LastWins,
    CollectAll,
    Original,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalAggregateStrategySpec {
    CollectAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalAggregateSpec {
    pub header: String,
    pub completion_size: Option<usize>,
    pub strategy: CanonicalAggregateStrategySpec,
    pub max_buckets: Option<usize>,
    pub bucket_ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalCircuitBreakerSpec {
    pub failure_threshold: u32,
    pub open_duration_ms: u64,
}

impl CanonicalRouteSpec {
    pub fn new(route_id: impl Into<String>, from: impl Into<String>) -> Self {
        Self {
            route_id: route_id.into(),
            from: from.into(),
            steps: Vec::new(),
            circuit_breaker: None,
            version: CANONICAL_CONTRACT_VERSION,
        }
    }

    pub fn validate_contract(&self) -> Result<(), CamelError> {
        if self.route_id.trim().is_empty() {
            return Err(CamelError::RouteError(
                "canonical contract violation: route_id cannot be empty".to_string(),
            ));
        }
        if self.from.trim().is_empty() {
            return Err(CamelError::RouteError(
                "canonical contract violation: from cannot be empty".to_string(),
            ));
        }
        if self.version != CANONICAL_CONTRACT_VERSION {
            return Err(CamelError::RouteError(format!(
                "canonical contract violation: expected version {}, got {}",
                CANONICAL_CONTRACT_VERSION, self.version
            )));
        }
        validate_steps(&self.steps)?;
        if let Some(cb) = &self.circuit_breaker {
            if cb.failure_threshold == 0 {
                return Err(CamelError::RouteError(
                    "canonical contract violation: circuit_breaker.failure_threshold must be > 0"
                        .to_string(),
                ));
            }
            if cb.open_duration_ms == 0 {
                return Err(CamelError::RouteError(
                    "canonical contract violation: circuit_breaker.open_duration_ms must be > 0"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }
}

fn validate_steps(steps: &[CanonicalStepSpec]) -> Result<(), CamelError> {
    for step in steps {
        match step {
            CanonicalStepSpec::To { uri } | CanonicalStepSpec::WireTap { uri } => {
                if uri.trim().is_empty() {
                    return Err(CamelError::RouteError(
                        "canonical contract violation: endpoint uri cannot be empty".to_string(),
                    ));
                }
            }
            CanonicalStepSpec::Filter { steps, .. } => validate_steps(steps)?,
            CanonicalStepSpec::Choice { whens, otherwise } => {
                for when in whens {
                    validate_steps(&when.steps)?;
                }
                if let Some(otherwise) = otherwise {
                    validate_steps(otherwise)?;
                }
            }
            CanonicalStepSpec::Split {
                parallel_limit,
                steps,
                ..
            } => {
                if let Some(limit) = parallel_limit
                    && *limit == 0
                {
                    return Err(CamelError::RouteError(
                        "canonical contract violation: split.parallel_limit must be > 0"
                            .to_string(),
                    ));
                }
                validate_steps(steps)?;
            }
            CanonicalStepSpec::Aggregate { config } => {
                if config.header.trim().is_empty() {
                    return Err(CamelError::RouteError(
                        "canonical contract violation: aggregate.header cannot be empty"
                            .to_string(),
                    ));
                }
                if let Some(size) = config.completion_size
                    && size == 0
                {
                    return Err(CamelError::RouteError(
                        "canonical contract violation: aggregate.completion_size must be > 0"
                            .to_string(),
                    ));
                }
            }
            CanonicalStepSpec::Log { .. }
            | CanonicalStepSpec::Script { .. }
            | CanonicalStepSpec::Stop => {}
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommand {
    RegisterRoute {
        spec: CanonicalRouteSpec,
        command_id: String,
        causation_id: Option<String>,
    },
    StartRoute {
        route_id: String,
        command_id: String,
        causation_id: Option<String>,
    },
    StopRoute {
        route_id: String,
        command_id: String,
        causation_id: Option<String>,
    },
    SuspendRoute {
        route_id: String,
        command_id: String,
        causation_id: Option<String>,
    },
    ResumeRoute {
        route_id: String,
        command_id: String,
        causation_id: Option<String>,
    },
    ReloadRoute {
        route_id: String,
        command_id: String,
        causation_id: Option<String>,
    },
    /// Internal lifecycle command emitted by runtime adapters when a route crashes at runtime.
    ///
    /// This keeps aggregate/projection state aligned with controller-observed failures.
    FailRoute {
        route_id: String,
        error: String,
        command_id: String,
        causation_id: Option<String>,
    },
    RemoveRoute {
        route_id: String,
        command_id: String,
        causation_id: Option<String>,
    },
}

impl RuntimeCommand {
    pub fn command_id(&self) -> &str {
        match self {
            RuntimeCommand::RegisterRoute { command_id, .. }
            | RuntimeCommand::StartRoute { command_id, .. }
            | RuntimeCommand::StopRoute { command_id, .. }
            | RuntimeCommand::SuspendRoute { command_id, .. }
            | RuntimeCommand::ResumeRoute { command_id, .. }
            | RuntimeCommand::ReloadRoute { command_id, .. }
            | RuntimeCommand::FailRoute { command_id, .. }
            | RuntimeCommand::RemoveRoute { command_id, .. } => command_id,
        }
    }

    pub fn causation_id(&self) -> Option<&str> {
        match self {
            RuntimeCommand::RegisterRoute { causation_id, .. }
            | RuntimeCommand::StartRoute { causation_id, .. }
            | RuntimeCommand::StopRoute { causation_id, .. }
            | RuntimeCommand::SuspendRoute { causation_id, .. }
            | RuntimeCommand::ResumeRoute { causation_id, .. }
            | RuntimeCommand::ReloadRoute { causation_id, .. }
            | RuntimeCommand::FailRoute { causation_id, .. }
            | RuntimeCommand::RemoveRoute { causation_id, .. } => causation_id.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommandResult {
    Accepted,
    Duplicate { command_id: String },
    RouteRegistered { route_id: String },
    RouteStateChanged { route_id: String, status: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeQuery {
    GetRouteStatus {
        route_id: String,
    },
    /// **Note:** This variant is intercepted by `RuntimeBus::ask` *before* reaching
    /// `execute_query`. Do not handle it in `execute_query` — it has no access to
    /// the in-flight counter. See `runtime_bus.rs` for the intercept.
    InFlightCount {
        route_id: String,
    },
    ListRoutes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeQueryResult {
    InFlightCount { route_id: String, count: u64 },
    RouteNotFound { route_id: String },
    RouteStatus { route_id: String, status: String },
    Routes { route_ids: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeEvent {
    RouteRegistered { route_id: String },
    RouteStartRequested { route_id: String },
    RouteStarted { route_id: String },
    RouteFailed { route_id: String, error: String },
    RouteStopped { route_id: String },
    RouteSuspended { route_id: String },
    RouteResumed { route_id: String },
    RouteReloaded { route_id: String },
    RouteRemoved { route_id: String },
}

#[async_trait]
pub trait RuntimeCommandBus: Send + Sync {
    async fn execute(&self, cmd: RuntimeCommand) -> Result<RuntimeCommandResult, CamelError>;
}

#[async_trait]
pub trait RuntimeQueryBus: Send + Sync {
    async fn ask(&self, query: RuntimeQuery) -> Result<RuntimeQueryResult, CamelError>;
}

pub trait RuntimeHandle: RuntimeCommandBus + RuntimeQueryBus {}

impl<T> RuntimeHandle for T where T: RuntimeCommandBus + RuntimeQueryBus {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_and_query_ids_are_exposed() {
        let cmd = RuntimeCommand::StartRoute {
            route_id: "r1".into(),
            command_id: "c1".into(),
            causation_id: None,
        };
        assert_eq!(cmd.command_id(), "c1");
    }

    #[test]
    fn canonical_spec_requires_route_id_and_from() {
        let spec = CanonicalRouteSpec::new("r1", "timer:tick");
        assert_eq!(spec.route_id, "r1");
        assert_eq!(spec.from, "timer:tick");
        assert_eq!(spec.version, CANONICAL_CONTRACT_VERSION);
        assert!(spec.steps.is_empty());
        assert!(spec.circuit_breaker.is_none());
    }

    #[test]
    fn canonical_contract_rejects_invalid_version() {
        let mut spec = CanonicalRouteSpec::new("r1", "timer:tick");
        spec.version = 2;
        let err = spec.validate_contract().unwrap_err().to_string();
        assert!(err.contains("expected version"));
    }

    #[test]
    fn canonical_contract_declares_subset_scope() {
        assert!(canonical_contract_supports_step("to"));
        assert!(canonical_contract_supports_step("split"));
        assert!(!canonical_contract_supports_step("set_header"));

        assert!(CANONICAL_CONTRACT_DECLARATIVE_ONLY_STEPS.contains(&"split"));
        assert!(CANONICAL_CONTRACT_EXCLUDED_DECLARATIVE_STEPS.contains(&"set_header"));
        assert!(CANONICAL_CONTRACT_RUST_ONLY_STEPS.contains(&"processor"));
    }

    #[test]
    fn canonical_contract_rejection_reason_is_explicit() {
        let set_header_reason = canonical_contract_rejection_reason("set_header")
            .expect("set_header should have explicit reason");
        assert!(set_header_reason.contains("out-of-scope"));

        let processor_reason = canonical_contract_rejection_reason("processor")
            .expect("processor should be rust-only");
        assert!(processor_reason.contains("rust-only"));

        let split_reason = canonical_contract_rejection_reason("split")
            .expect("split should require declarative form");
        assert!(split_reason.contains("declarative"));
    }

    #[test]
    fn command_causation_id_is_exposed() {
        let cmd = RuntimeCommand::StopRoute {
            route_id: "r1".into(),
            command_id: "c2".into(),
            causation_id: Some("c1".into()),
        };
        assert_eq!(cmd.command_id(), "c2");
        assert_eq!(cmd.causation_id(), Some("c1"));
    }
}
