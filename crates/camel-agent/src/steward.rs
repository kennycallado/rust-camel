use crate::types::{MaintenanceProposal, ProposalKind, RouteSnapshot, Severity, SystemSnapshot};

#[derive(Debug, Default)]
pub struct MaintainerAgent;

impl MaintainerAgent {
    pub fn analyze(&self, snapshot: &SystemSnapshot) -> Vec<MaintenanceProposal> {
        let mut proposals = Vec::new();

        for route in &snapshot.routes {
            if touches_http_endpoint(route) && !route.has_error_handler {
                proposals.push(MaintenanceProposal {
                    kind: ProposalKind::RouteReliability,
                    severity: Severity::High,
                    route_id: Some(route.id.clone()),
                    finding: "Route reaches HTTP endpoint without route-level error handling".into(),
                    recommendation:
                        "Add retry policy, circuit breaker and dead letter channel for HTTP calls"
                            .into(),
                    rationale:
                        "Network calls are failure-prone and should fail safely to avoid silent data loss"
                            .into(),
                    confidence: 0.96,
                    requires_approval: true,
                });
            }

            if route.uses_ai || has_ai_classifier_steps(route) {
                proposals.push(MaintenanceProposal {
                    kind: ProposalKind::AiSafety,
                    severity: Severity::Medium,
                    route_id: Some(route.id.clone()),
                    finding: "Route uses AI classification/extraction behavior".into(),
                    recommendation:
                        "Validate model output schema and add logs for model, token usage and latency"
                            .into(),
                    rationale:
                        "Model responses can drift, so validation and telemetry reduce downstream breakage"
                            .into(),
                    confidence: 0.91,
                    requires_approval: true,
                });
            }

            if is_rag_shape(route) {
                proposals.push(MaintenanceProposal {
                    kind: ProposalKind::Refactor,
                    severity: Severity::Medium,
                    route_id: Some(route.id.clone()),
                    finding: "Route composes embedding + vector + llm steps manually".into(),
                    recommendation:
                        "Consider introducing a reusable rag_answer/prompt_template abstraction".into(),
                    rationale:
                        "A higher-level abstraction reduces duplicated glue logic and improves consistency"
                            .into(),
                    confidence: 0.87,
                    requires_approval: true,
                });
            }

            if route.steps.len() >= 7 {
                proposals.push(MaintenanceProposal {
                    kind: ProposalKind::Documentation,
                    severity: Severity::Low,
                    route_id: Some(route.id.clone()),
                    finding: "Route has high step count and may be hard to maintain".into(),
                    recommendation:
                        "Document route intent and consider extracting reusable templates".into(),
                    rationale:
                        "Long pipelines become harder to reason about without explicit maintenance notes"
                            .into(),
                    confidence: 0.84,
                    requires_approval: true,
                });
            }

            if contains_api_key_hint(route) {
                proposals.push(MaintenanceProposal {
                    kind: ProposalKind::Refactor,
                    severity: Severity::High,
                    route_id: Some(route.id.clone()),
                    finding: "Potential secret detected in endpoint URI query parameters".into(),
                    recommendation:
                        "Move api_key values to env/config secret storage instead of route URI".into(),
                    rationale:
                        "Secrets in route definitions are easy to leak through code, logs and diagnostics"
                            .into(),
                    confidence: 0.98,
                    requires_approval: true,
                });
            }
        }

        proposals
    }
}

fn touches_http_endpoint(route: &RouteSnapshot) -> bool {
    starts_with_any(&route.from, &["http:", "https:"])
        || route.steps.iter().any(|step| {
            contains_any(
                step,
                &["http://", "https://", "to: http://", "to: https://"],
            )
        })
}

fn has_ai_classifier_steps(route: &RouteSnapshot) -> bool {
    route
        .steps
        .iter()
        .any(|step| contains_any(step, &["ai_classify", "ai_extract"]))
}

fn is_rag_shape(route: &RouteSnapshot) -> bool {
    ["embedding", "vector", "llm"]
        .iter()
        .all(|component| route.components.iter().any(|c| c == component))
}

fn contains_api_key_hint(route: &RouteSnapshot) -> bool {
    contains_any(&route.from, &["api_key="])
        || route
            .steps
            .iter()
            .any(|step| contains_any(step, &["api_key="]))
}

fn starts_with_any(value: &str, prefixes: &[&str]) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    prefixes.iter().any(|prefix| lower.starts_with(prefix))
}

fn contains_any(value: &str, patterns: &[&str]) -> bool {
    let lower = value.to_ascii_lowercase();
    patterns.iter().any(|pattern| lower.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{
        build_system_snapshot, build_system_snapshot_from_yaml, parse_routes_yaml,
    };

    fn snapshot_from_yaml(yaml: &str) -> SystemSnapshot {
        let routes = parse_routes_yaml(yaml).expect("valid declarative YAML");
        build_system_snapshot(&routes)
    }

    #[test]
    fn proposes_reliability_for_http_without_error_handler() {
        let snapshot = snapshot_from_yaml(
            r#"
routes:
  - id: "r1"
    from: "timer:tick"
    steps:
      - to: "http://service.local/orders"
"#,
        );

        let proposals = MaintainerAgent.analyze(&snapshot);
        assert!(
            proposals
                .iter()
                .any(|p| p.kind == ProposalKind::RouteReliability)
        );
    }

    #[test]
    fn proposes_ai_safety_for_route_with_ai_components() {
        let snapshot = snapshot_from_yaml(
            r#"
routes:
  - id: "r2"
    from: "direct:start"
    steps:
      - to: "embedding:model"
      - to: "vector:index"
      - to: "llm:chat"
"#,
        );

        let proposals = MaintainerAgent.analyze(&snapshot);
        assert!(proposals.iter().any(|p| p.kind == ProposalKind::AiSafety));
    }

    #[test]
    fn proposes_refactor_for_rag_shape() {
        let snapshot = snapshot_from_yaml(
            r#"
routes:
  - id: "r3"
    from: "direct:start"
    steps:
      - to: "embedding:model"
      - to: "vector:index"
      - to: "llm:chat"
"#,
        );

        let proposals = MaintainerAgent.analyze(&snapshot);
        assert!(proposals.iter().any(|p| {
            p.kind == ProposalKind::Refactor && p.finding.contains("embedding + vector + llm")
        }));
    }

    #[test]
    fn proposes_documentation_for_long_routes() {
        let snapshot = snapshot_from_yaml(
            r#"
routes:
  - id: "r4"
    from: "direct:start"
    steps:
      - to: "log:1"
      - to: "log:2"
      - to: "log:3"
      - to: "log:4"
      - to: "log:5"
      - to: "log:6"
      - to: "log:7"
"#,
        );

        let proposals = MaintainerAgent.analyze(&snapshot);
        assert!(
            proposals
                .iter()
                .any(|p| p.kind == ProposalKind::Documentation)
        );
    }

    #[test]
    fn proposes_secret_hardening_when_api_key_is_present() {
        let snapshot = snapshot_from_yaml(
            r#"
routes:
  - id: "r5"
    from: "direct:start"
    steps:
      - to: "https://api.local/orders?api_key=plain-text"
"#,
        );

        let proposals = MaintainerAgent.analyze(&snapshot);
        assert!(
            proposals.iter().any(|p| {
                p.finding.contains("secret") || p.recommendation.contains("env/config")
            })
        );
    }

    #[test]
    fn proposes_secret_hardening_for_ai_model_uri_with_api_key() {
        let snapshot = build_system_snapshot_from_yaml(
            r#"
routes:
  - id: "r6"
    from: "direct:start"
    steps:
      - ai_extract:
          model: "llm:ollama?base_url=https://api.local&api_key=plain-text"
          schema: '{"type":"object"}'
          output_header: "result"
"#,
        )
        .expect("valid ai yaml");

        let proposals = MaintainerAgent.analyze(&snapshot);
        assert!(
            proposals
                .iter()
                .any(|p| p.finding.contains("secret") || p.recommendation.contains("env/config"))
        );
    }
}
