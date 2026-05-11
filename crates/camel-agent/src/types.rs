use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemSnapshot {
    pub routes: Vec<RouteSnapshot>,
    pub components: Vec<ComponentSnapshot>,
    pub findings_context: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteSnapshot {
    pub id: String,
    pub from: String,
    pub steps: Vec<String>,
    pub components: Vec<String>,
    pub has_error_handler: bool,
    pub has_circuit_breaker: bool,
    pub uses_ai: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentSnapshot {
    pub name: String,
    pub usage_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaintenanceProposal {
    pub kind: ProposalKind,
    pub severity: Severity,
    pub route_id: Option<String>,
    pub finding: String,
    pub recommendation: String,
    pub rationale: String,
    pub confidence: f32,
    pub requires_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProposalKind {
    RouteReliability,
    AiSafety,
    Observability,
    Documentation,
    Testing,
    Refactor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}
