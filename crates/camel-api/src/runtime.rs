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
    "delay",
];
pub const CANONICAL_CONTRACT_DECLARATIVE_ONLY_STEPS: &[&str] =
    &["script", "filter", "choice", "split"];
pub const CANONICAL_CONTRACT_EXCLUDED_DECLARATIVE_STEPS: &[&str] = &[
    "set_header",
    "set_property",
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct CanonicalRouteSpec {
    /// Stable minimal route representation for runtime command registration.
    ///
    /// Scope note (v1):
    /// - This is intentionally a partial model and does not mirror every `BuilderStep`.
    /// - Not included in v1: auto_startup, startup_order, concurrency, error_handler,
    ///   unit_of_work. These are set to defaults when compiling from canonical.
    /// - Round-trip (YAML → Canonical → YAML) loses these fields.
    /// - Advanced EIPs continue to use the existing RouteDefinition/BuilderStep path.
    pub route_id: String,
    pub from: String,
    pub steps: Vec<CanonicalStepSpec>,
    pub circuit_breaker: Option<CanonicalCircuitBreakerSpec>,
    pub version: u32,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(tag = "step", content = "config", rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
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
    Aggregate(CanonicalAggregateSpec),
    Stop,
    Delay {
        #[ts(type = "number")]
        delay_ms: u64,
        dynamic_header: Option<String>,
    },
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct CanonicalWhenSpec {
    pub predicate: LanguageExpressionDef,
    pub steps: Vec<CanonicalStepSpec>,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CanonicalSplitExpressionSpec {
    BodyLines,
    BodyJsonArray,
    Language(LanguageExpressionDef),
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CanonicalSplitAggregationSpec {
    LastWins,
    CollectAll,
    Original,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CanonicalAggregateStrategySpec {
    CollectAll,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct CanonicalAggregateSpec {
    pub header: String,
    pub completion_size: Option<usize>,
    #[ts(type = "number")]
    pub completion_timeout_ms: Option<u64>,
    pub correlation_key: Option<String>,
    pub force_completion_on_stop: Option<bool>,
    pub discard_on_timeout: Option<bool>,
    pub strategy: CanonicalAggregateStrategySpec,
    pub max_buckets: Option<usize>,
    #[ts(type = "number")]
    pub bucket_ttl_ms: Option<u64>,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct CanonicalCircuitBreakerSpec {
    pub failure_threshold: u32,
    #[ts(type = "number")]
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
            CanonicalStepSpec::Aggregate(config) => {
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
            | CanonicalStepSpec::Stop
            | CanonicalStepSpec::Delay { .. } => {}
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
    use async_trait::async_trait;
    use futures::executor::block_on;

    struct NoopRuntime;

    #[async_trait]
    impl RuntimeCommandBus for NoopRuntime {
        async fn execute(&self, cmd: RuntimeCommand) -> Result<RuntimeCommandResult, CamelError> {
            Ok(match cmd {
                RuntimeCommand::RegisterRoute { spec, .. } => {
                    RuntimeCommandResult::RouteRegistered {
                        route_id: spec.route_id,
                    }
                }
                RuntimeCommand::StartRoute { route_id, .. }
                | RuntimeCommand::StopRoute { route_id, .. }
                | RuntimeCommand::SuspendRoute { route_id, .. }
                | RuntimeCommand::ResumeRoute { route_id, .. }
                | RuntimeCommand::ReloadRoute { route_id, .. }
                | RuntimeCommand::FailRoute { route_id, .. }
                | RuntimeCommand::RemoveRoute { route_id, .. } => {
                    RuntimeCommandResult::RouteStateChanged {
                        route_id,
                        status: "ok".to_string(),
                    }
                }
            })
        }
    }

    #[async_trait]
    impl RuntimeQueryBus for NoopRuntime {
        async fn ask(&self, query: RuntimeQuery) -> Result<RuntimeQueryResult, CamelError> {
            Ok(match query {
                RuntimeQuery::GetRouteStatus { route_id } => RuntimeQueryResult::RouteStatus {
                    route_id,
                    status: "Started".to_string(),
                },
                RuntimeQuery::InFlightCount { route_id } => {
                    RuntimeQueryResult::InFlightCount { route_id, count: 0 }
                }
                RuntimeQuery::ListRoutes => RuntimeQueryResult::Routes {
                    route_ids: vec!["r1".to_string()],
                },
            })
        }
    }

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
        assert!(!canonical_contract_supports_step("set_property"));

        assert!(CANONICAL_CONTRACT_DECLARATIVE_ONLY_STEPS.contains(&"split"));
        assert!(CANONICAL_CONTRACT_EXCLUDED_DECLARATIVE_STEPS.contains(&"set_header"));
        assert!(CANONICAL_CONTRACT_EXCLUDED_DECLARATIVE_STEPS.contains(&"set_property"));
        assert!(CANONICAL_CONTRACT_RUST_ONLY_STEPS.contains(&"processor"));
    }

    #[test]
    fn canonical_contract_rejection_reason_is_explicit() {
        let set_header_reason = canonical_contract_rejection_reason("set_header")
            .expect("set_header should have explicit reason");
        assert!(set_header_reason.contains("out-of-scope"));

        let set_property_reason = canonical_contract_rejection_reason("set_property")
            .expect("set_property should have explicit reason");
        assert!(set_property_reason.contains("out-of-scope"));

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

    #[test]
    fn canonical_contract_rejects_empty_route_id_and_from() {
        let spec = CanonicalRouteSpec::new("   ", "timer:tick");
        let err = spec.validate_contract().unwrap_err().to_string();
        assert!(err.contains("route_id cannot be empty"));

        let spec = CanonicalRouteSpec::new("r1", "  ");
        let err = spec.validate_contract().unwrap_err().to_string();
        assert!(err.contains("from cannot be empty"));
    }

    #[test]
    fn canonical_contract_rejects_invalid_nested_steps() {
        let mut spec = CanonicalRouteSpec::new("r1", "timer:tick");
        spec.steps = vec![CanonicalStepSpec::Split {
            expression: CanonicalSplitExpressionSpec::BodyLines,
            aggregation: CanonicalSplitAggregationSpec::CollectAll,
            parallel: true,
            parallel_limit: Some(0),
            stop_on_exception: false,
            steps: vec![CanonicalStepSpec::To {
                uri: "log:ok".to_string(),
            }],
        }];
        let err = spec.validate_contract().unwrap_err().to_string();
        assert!(err.contains("split.parallel_limit must be > 0"));

        spec.steps = vec![CanonicalStepSpec::To {
            uri: "   ".to_string(),
        }];
        let err = spec.validate_contract().unwrap_err().to_string();
        assert!(err.contains("endpoint uri cannot be empty"));
    }

    #[test]
    fn canonical_contract_rejects_invalid_aggregate_and_circuit_breaker() {
        let mut spec = CanonicalRouteSpec::new("r1", "timer:tick");
        spec.steps = vec![CanonicalStepSpec::Aggregate(CanonicalAggregateSpec {
            header: " ".to_string(),
            completion_size: Some(1),
            completion_timeout_ms: None,
            correlation_key: None,
            force_completion_on_stop: None,
            discard_on_timeout: None,
            strategy: CanonicalAggregateStrategySpec::CollectAll,
            max_buckets: None,
            bucket_ttl_ms: None,
        })];
        let err = spec.validate_contract().unwrap_err().to_string();
        assert!(err.contains("aggregate.header cannot be empty"));

        spec.steps = vec![CanonicalStepSpec::Aggregate(CanonicalAggregateSpec {
            header: "k".to_string(),
            completion_size: Some(0),
            completion_timeout_ms: None,
            correlation_key: None,
            force_completion_on_stop: None,
            discard_on_timeout: None,
            strategy: CanonicalAggregateStrategySpec::CollectAll,
            max_buckets: None,
            bucket_ttl_ms: None,
        })];
        let err = spec.validate_contract().unwrap_err().to_string();
        assert!(err.contains("aggregate.completion_size must be > 0"));

        spec.steps = vec![];
        spec.circuit_breaker = Some(CanonicalCircuitBreakerSpec {
            failure_threshold: 0,
            open_duration_ms: 10,
        });
        let err = spec.validate_contract().unwrap_err().to_string();
        assert!(err.contains("failure_threshold must be > 0"));

        spec.circuit_breaker = Some(CanonicalCircuitBreakerSpec {
            failure_threshold: 1,
            open_duration_ms: 0,
        });
        let err = spec.validate_contract().unwrap_err().to_string();
        assert!(err.contains("open_duration_ms must be > 0"));
    }

    #[test]
    fn canonical_contract_rejection_reason_none_for_regular_steps() {
        assert!(canonical_contract_rejection_reason("to").is_none());
        assert!(canonical_contract_rejection_reason("unknown-step").is_none());
    }

    #[test]
    fn command_helpers_cover_all_variants() {
        let spec = CanonicalRouteSpec::new("r1", "timer:tick");
        let cmds = [
            RuntimeCommand::RegisterRoute {
                spec,
                command_id: "c1".into(),
                causation_id: Some("root".into()),
            },
            RuntimeCommand::StartRoute {
                route_id: "r1".into(),
                command_id: "c2".into(),
                causation_id: None,
            },
            RuntimeCommand::StopRoute {
                route_id: "r1".into(),
                command_id: "c3".into(),
                causation_id: None,
            },
            RuntimeCommand::SuspendRoute {
                route_id: "r1".into(),
                command_id: "c4".into(),
                causation_id: None,
            },
            RuntimeCommand::ResumeRoute {
                route_id: "r1".into(),
                command_id: "c5".into(),
                causation_id: None,
            },
            RuntimeCommand::ReloadRoute {
                route_id: "r1".into(),
                command_id: "c6".into(),
                causation_id: None,
            },
            RuntimeCommand::FailRoute {
                route_id: "r1".into(),
                error: "boom".into(),
                command_id: "c7".into(),
                causation_id: None,
            },
            RuntimeCommand::RemoveRoute {
                route_id: "r1".into(),
                command_id: "c8".into(),
                causation_id: None,
            },
        ];

        let ids: Vec<&str> = cmds.iter().map(RuntimeCommand::command_id).collect();
        assert_eq!(ids, vec!["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8"]);
        assert_eq!(cmds[0].causation_id(), Some("root"));
        assert_eq!(cmds[1].causation_id(), None);
    }

    #[test]
    fn canonical_route_spec_serde_roundtrip() {
        let mut spec = CanonicalRouteSpec::new("test-route", "timer:tick?period=1000");
        spec.steps.push(CanonicalStepSpec::Log {
            message: "Hello".into(),
        });
        spec.steps.push(CanonicalStepSpec::To {
            uri: "log:info".into(),
        });
        spec.steps.push(CanonicalStepSpec::Stop);

        let json = serde_json::to_string(&spec).unwrap();
        let deserialized: CanonicalRouteSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deserialized);
    }

    #[test]
    fn canonical_step_spec_serde_variants() {
        let steps = vec![
            CanonicalStepSpec::To {
                uri: "direct:a".into(),
            },
            CanonicalStepSpec::Log {
                message: "msg".into(),
            },
            CanonicalStepSpec::WireTap {
                uri: "direct:audit".into(),
            },
            CanonicalStepSpec::Stop,
            CanonicalStepSpec::Delay {
                delay_ms: 100,
                dynamic_header: None,
            },
        ];
        let json = serde_json::to_string_pretty(&steps).unwrap();
        let back: Vec<CanonicalStepSpec> = serde_json::from_str(&json).unwrap();
        assert_eq!(steps, back);
    }

    #[test]
    fn canonical_route_spec_json_schema_generates() {
        let schema = schemars::schema_for!(CanonicalRouteSpec);
        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains("CanonicalRouteSpec"));
        assert!(json.contains("route_id"));
    }

    #[test]
    fn canonical_json_schema_has_no_function_step() {
        let schema = schemars::schema_for!(CanonicalRouteSpec);
        let json = serde_json::to_string(&schema).unwrap();
        assert!(
            !json.contains("\"function\""),
            "canonical JSON schema must not contain 'function' step"
        );
    }

    #[test]
    fn canonical_contract_does_not_support_function() {
        assert!(
            !canonical_contract_supports_step("function"),
            "function must not be in CANONICAL_CONTRACT_SUPPORTED_STEPS"
        );
    }

    #[test]
    fn runtime_command_result_all_variants_are_distinct() {
        let accepted = RuntimeCommandResult::Accepted;
        let dup = RuntimeCommandResult::Duplicate {
            command_id: "c1".into(),
        };
        let registered = RuntimeCommandResult::RouteRegistered {
            route_id: "r1".into(),
        };
        let changed = RuntimeCommandResult::RouteStateChanged {
            route_id: "r1".into(),
            status: "Started".into(),
        };

        assert_ne!(accepted, dup);
        assert_ne!(dup, registered);
        assert_ne!(registered, changed);

        let dup2 = RuntimeCommandResult::Duplicate {
            command_id: "c1".into(),
        };
        assert_eq!(dup, dup2);
    }

    #[test]
    fn runtime_event_serialization_round_trip() {
        let event = RuntimeEvent::RouteFailed {
            route_id: "route-a".to_string(),
            error: "boom".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: RuntimeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn noop_runtime_execute_and_ask_return_expected_shapes() {
        let rt = NoopRuntime;
        let cmd = RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("r2", "timer:tick"),
            command_id: "c1".into(),
            causation_id: None,
        };
        let cmd_result = block_on(rt.execute(cmd)).unwrap();
        assert_eq!(
            cmd_result,
            RuntimeCommandResult::RouteRegistered {
                route_id: "r2".into()
            }
        );

        let query_result = block_on(rt.ask(RuntimeQuery::GetRouteStatus {
            route_id: "r2".into(),
        }))
        .unwrap();
        assert_eq!(
            query_result,
            RuntimeQueryResult::RouteStatus {
                route_id: "r2".into(),
                status: "Started".into()
            }
        );
    }

    #[test]
    fn canonical_contract_name_and_version_constants_match() {
        assert_eq!(CANONICAL_CONTRACT_NAME, "canonical-v1");
        assert_eq!(CANONICAL_CONTRACT_VERSION, 1);
    }
}
