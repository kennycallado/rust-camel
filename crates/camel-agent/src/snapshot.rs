use std::collections::{BTreeMap, BTreeSet};

use camel_api::CamelError;
use camel_dsl::{
    parse_yaml_to_declarative, DeclarativeRoute, DeclarativeStep, MulticastStepDef,
};
use camel_dsl::model::{LoadBalanceStepDef, LoopStepDef, ThrottleStepDef};
use serde_json::json;

use crate::types::{ComponentSnapshot, RouteSnapshot, SystemSnapshot};

pub fn parse_routes_yaml(input: &str) -> Result<Vec<DeclarativeRoute>, CamelError> {
    parse_yaml_to_declarative(input)
}

pub fn build_system_snapshot(routes: &[DeclarativeRoute]) -> SystemSnapshot {
    let mut component_usage = BTreeMap::new();
    let mut route_snapshots = Vec::with_capacity(routes.len());

    for route in routes {
        let mut route_components = BTreeSet::new();
        let mut step_descriptions = Vec::new();

        collect_uri_component(&route.from, &mut route_components, &mut component_usage);
        collect_step_facts(
            &route.steps,
            &mut step_descriptions,
            &mut route_components,
            &mut component_usage,
        );

        let uses_ai = route_components
            .iter()
            .any(|c| matches!(c.as_str(), "llm" | "embedding" | "vector"))
            || step_descriptions
                .iter()
                .any(|step| contains_any(step, &["ai_classify", "ai_extract"]));

        route_snapshots.push(RouteSnapshot {
            id: route.route_id.clone(),
            from: route.from.clone(),
            steps: step_descriptions,
            components: route_components.into_iter().collect(),
            has_error_handler: route.error_handler.is_some(),
            has_circuit_breaker: route.circuit_breaker.is_some(),
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

fn collect_step_facts(
    steps: &[DeclarativeStep],
    step_descriptions: &mut Vec<String>,
    route_components: &mut BTreeSet<String>,
    component_usage: &mut BTreeMap<String, usize>,
) {
    for step in steps {
        match step {
            DeclarativeStep::To(def) => {
                step_descriptions.push(format!("to:{}", def.uri));
                collect_uri_component(&def.uri, route_components, component_usage);
            }
            DeclarativeStep::WireTap(def) => {
                step_descriptions.push(format!("wire_tap:{}", def.uri));
                collect_uri_component(&def.uri, route_components, component_usage);
            }
            DeclarativeStep::Filter(def) => {
                step_descriptions.push("filter".to_string());
                collect_step_facts(
                    &def.steps,
                    step_descriptions,
                    route_components,
                    component_usage,
                );
            }
            DeclarativeStep::Choice(def) => {
                step_descriptions.push("choice".to_string());
                for when in &def.whens {
                    collect_step_facts(
                        &when.steps,
                        step_descriptions,
                        route_components,
                        component_usage,
                    );
                }
                if let Some(otherwise) = &def.otherwise {
                    collect_step_facts(
                        otherwise,
                        step_descriptions,
                        route_components,
                        component_usage,
                    );
                }
            }
            DeclarativeStep::Split(def) => {
                step_descriptions.push("split".to_string());
                collect_step_facts(
                    &def.steps,
                    step_descriptions,
                    route_components,
                    component_usage,
                );
            }
            DeclarativeStep::Multicast(def) => {
                step_descriptions.push("multicast".to_string());
                collect_nested_multicast(def, step_descriptions, route_components, component_usage);
            }
            DeclarativeStep::LoadBalance(def) => {
                step_descriptions.push("load_balance".to_string());
                collect_nested_load_balance(
                    def,
                    step_descriptions,
                    route_components,
                    component_usage,
                );
            }
            DeclarativeStep::Throttle(def) => {
                step_descriptions.push("throttle".to_string());
                collect_nested_throttle(def, step_descriptions, route_components, component_usage);
            }
            DeclarativeStep::Loop(def) => {
                step_descriptions.push("loop".to_string());
                collect_nested_loop(def, step_descriptions, route_components, component_usage);
            }
            _ => {
                step_descriptions.push(normalize_step_debug(step));
            }
        }
    }
}

fn collect_nested_multicast(
    step: &MulticastStepDef,
    step_descriptions: &mut Vec<String>,
    route_components: &mut BTreeSet<String>,
    component_usage: &mut BTreeMap<String, usize>,
) {
    collect_step_facts(
        &step.steps,
        step_descriptions,
        route_components,
        component_usage,
    );
}

fn collect_nested_load_balance(
    step: &LoadBalanceStepDef,
    step_descriptions: &mut Vec<String>,
    route_components: &mut BTreeSet<String>,
    component_usage: &mut BTreeMap<String, usize>,
) {
    collect_step_facts(
        &step.steps,
        step_descriptions,
        route_components,
        component_usage,
    );
}

fn collect_nested_throttle(
    step: &ThrottleStepDef,
    step_descriptions: &mut Vec<String>,
    route_components: &mut BTreeSet<String>,
    component_usage: &mut BTreeMap<String, usize>,
) {
    collect_step_facts(
        &step.steps,
        step_descriptions,
        route_components,
        component_usage,
    );
}

fn collect_nested_loop(
    step: &LoopStepDef,
    step_descriptions: &mut Vec<String>,
    route_components: &mut BTreeSet<String>,
    component_usage: &mut BTreeMap<String, usize>,
) {
    collect_step_facts(
        &step.steps,
        step_descriptions,
        route_components,
        component_usage,
    );
}

fn collect_uri_component(
    uri: &str,
    route_components: &mut BTreeSet<String>,
    component_usage: &mut BTreeMap<String, usize>,
) {
    if let Some(component) = component_from_uri(uri) {
        *component_usage.entry(component.clone()).or_insert(0usize) += 1;
        route_components.insert(component);
    }
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

fn normalize_step_debug(step: &DeclarativeStep) -> String {
    let debug = format!("{step:?}");
    let kind = debug
        .split_once('(')
        .map(|(prefix, _)| prefix)
        .unwrap_or(debug.as_str());
    kind.to_ascii_lowercase()
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    let lower = text.to_ascii_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_dsl_yaml_and_marks_ai_components() {
        let routes = parse_routes_yaml(
            r#"
routes:
  - id: "r1"
    from: "timer:tick"
    steps:
      - to: "embedding:model"
      - to: "vector:index"
      - to: "llm:chat"
"#,
        )
        .unwrap();

        let snapshot = build_system_snapshot(&routes);
        assert!(snapshot.routes[0].uses_ai);
        assert!(snapshot.routes[0]
            .components
            .iter()
            .any(|c| c == "embedding"));
    }

    #[test]
    fn collects_components_from_nested_choice_steps() {
        let routes = parse_routes_yaml(
            r#"
routes:
  - id: "r2"
    from: "direct:start"
    steps:
      - choice:
          when:
            - simple: "${header.mode} == 'http'"
              steps:
                - to: "http://service.local/a"
          otherwise:
            - to: "log:fallback"
"#,
        )
        .unwrap();

        let snapshot = build_system_snapshot(&routes);
        let route = &snapshot.routes[0];
        assert!(route.components.iter().any(|c| c == "http"));
        assert!(route.components.iter().any(|c| c == "log"));
    }
}
