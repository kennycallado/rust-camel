use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::types::{ComponentSnapshot, RouteSnapshot, SystemSnapshot};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DemoRoutesFile {
    pub routes: Vec<DemoRouteDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DemoRouteDefinition {
    pub id: String,
    pub from: String,
    pub steps: Vec<String>,
    #[serde(default)]
    pub has_error_handler: bool,
    #[serde(default)]
    pub has_circuit_breaker: bool,
}

pub fn parse_routes_yaml(input: &str) -> Result<DemoRoutesFile, serde_yaml::Error> {
    serde_yaml::from_str(input)
}

pub fn build_system_snapshot(routes: &[DemoRouteDefinition]) -> SystemSnapshot {
    let mut component_usage = BTreeMap::new();
    let mut route_snapshots = Vec::with_capacity(routes.len());

    for route in routes {
        let mut route_components = BTreeSet::new();

        if let Some(component) = component_from_uri(&route.from) {
            *component_usage.entry(component.clone()).or_insert(0usize) += 1;
            route_components.insert(component);
        }

        for step in &route.steps {
            if let Some(uri) = step_to_uri(step) {
                if let Some(component) = component_from_uri(uri) {
                    *component_usage.entry(component.clone()).or_insert(0usize) += 1;
                    route_components.insert(component);
                }
            }
        }

        let uses_ai = route_components
            .iter()
            .any(|c| matches!(c.as_str(), "llm" | "embedding" | "vector"))
            || route
                .steps
                .iter()
                .any(|step| contains_any(step, &["ai_classify", "ai_extract"]));

        route_snapshots.push(RouteSnapshot {
            id: route.id.clone(),
            from: route.from.clone(),
            steps: route.steps.clone(),
            components: route_components.into_iter().collect(),
            has_error_handler: route.has_error_handler,
            has_circuit_breaker: route.has_circuit_breaker,
            uses_ai,
        });
    }

    let components = component_usage
        .into_iter()
        .map(|(name, usage_count)| ComponentSnapshot { name, usage_count })
        .collect::<Vec<_>>();

    SystemSnapshot {
        findings_context: json!({
            "route_count": route_snapshots.len(),
            "component_count": components.len(),
        }),
        routes: route_snapshots,
        components,
    }
}

fn step_to_uri(step: &str) -> Option<&str> {
    step.trim().strip_prefix("to:").map(str::trim)
}

fn component_from_uri(uri: &str) -> Option<String> {
    let candidate = uri.trim();
    if candidate.is_empty() {
        return None;
    }
    let (scheme, _) = candidate.split_once(':').unwrap_or((candidate, ""));
    if scheme.is_empty() {
        return None;
    }
    Some(scheme.to_ascii_lowercase())
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    let lower = text.to_ascii_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_route_using_ai_components() {
        let snapshot = build_system_snapshot(&[DemoRouteDefinition {
            id: "r1".into(),
            from: "timer:tick".into(),
            steps: vec![
                "to: embedding:model".into(),
                "to: vector:index".into(),
                "to: llm:chat".into(),
            ],
            has_error_handler: false,
            has_circuit_breaker: false,
        }]);

        assert!(snapshot.routes[0].uses_ai);
        assert_eq!(snapshot.components.len(), 4);
    }

    #[test]
    fn parses_yaml_routes_file() {
        let parsed = parse_routes_yaml(
            r#"
routes:
  - id: "r1"
    from: "timer:tick"
    steps:
      - "to: log:info"
"#,
        )
        .unwrap();

        assert_eq!(parsed.routes.len(), 1);
        assert_eq!(parsed.routes[0].id, "r1");
    }
}
