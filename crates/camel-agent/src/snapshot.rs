use std::collections::{BTreeMap, BTreeSet};

use camel_api::CamelError;
use camel_dsl::{DeclarativeRoute, DeclarativeStep, parse_yaml_to_declarative};
use serde_json::json;
use serde_yaml::Value as YamlValue;

use crate::types::{ComponentSnapshot, RouteSnapshot, SystemSnapshot};

pub fn parse_routes_yaml(input: &str) -> Result<Vec<DeclarativeRoute>, CamelError> {
    parse_yaml_to_declarative(input)
}

pub fn build_system_snapshot_from_yaml(input: &str) -> Result<SystemSnapshot, CamelError> {
    let route_hints = extract_route_hints_from_yaml(input)?;
    match parse_yaml_to_declarative(input) {
        Ok(routes) => Ok(build_system_snapshot_with_hints(&routes, &route_hints)),
        Err(_err) if !route_hints.is_empty() => Ok(build_system_snapshot_from_hints(&route_hints)),
        Err(err) => Err(err),
    }
}

pub fn build_system_snapshot(routes: &[DeclarativeRoute]) -> SystemSnapshot {
    build_system_snapshot_with_hints(routes, &[])
}

fn build_system_snapshot_with_hints(
    routes: &[DeclarativeRoute],
    route_hints: &[RouteHints],
) -> SystemSnapshot {
    let mut hints_by_id = BTreeMap::new();
    for hint in route_hints {
        hints_by_id.insert(hint.id.as_str(), hint);
    }

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

        if let Some(hint) = hints_by_id.get(route.route_id.as_str()) {
            for step in &hint.step_descriptions {
                if !step_descriptions.iter().any(|existing| existing == step) {
                    step_descriptions.push(step.clone());
                }
            }
            for component in &hint.components {
                if route_components.insert(component.clone()) {
                    *component_usage.entry(component.clone()).or_insert(0usize) += 1;
                }
            }
        }

        let uses_ai = route_components
            .iter()
            .any(|c| matches!(c.as_str(), "llm" | "embedding" | "vector"))
            || step_descriptions
                .iter()
                .any(|step| contains_any(step, &["ai_classify", "ai_extract"]))
            || hints_by_id
                .get(route.route_id.as_str())
                .is_some_and(|hint| hint.uses_ai);

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

fn build_system_snapshot_from_hints(route_hints: &[RouteHints]) -> SystemSnapshot {
    let mut component_usage = BTreeMap::new();
    let mut route_snapshots = Vec::with_capacity(route_hints.len());

    for hint in route_hints {
        for component in &hint.components {
            *component_usage.entry(component.clone()).or_insert(0usize) += 1;
        }

        route_snapshots.push(RouteSnapshot {
            id: hint.id.clone(),
            from: hint.from.clone(),
            steps: hint.step_descriptions.clone(),
            components: hint.components.iter().cloned().collect(),
            has_error_handler: hint.has_error_handler,
            has_circuit_breaker: hint.has_circuit_breaker,
            uses_ai: hint.uses_ai,
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
                collect_step_facts(
                    &def.steps,
                    step_descriptions,
                    route_components,
                    component_usage,
                );
            }
            DeclarativeStep::LoadBalance(def) => {
                step_descriptions.push("load_balance".to_string());
                collect_step_facts(
                    &def.steps,
                    step_descriptions,
                    route_components,
                    component_usage,
                );
            }
            DeclarativeStep::Throttle(def) => {
                step_descriptions.push("throttle".to_string());
                collect_step_facts(
                    &def.steps,
                    step_descriptions,
                    route_components,
                    component_usage,
                );
            }
            DeclarativeStep::Loop(def) => {
                step_descriptions.push("loop".to_string());
                collect_step_facts(
                    &def.steps,
                    step_descriptions,
                    route_components,
                    component_usage,
                );
            }
            _ => {
                step_descriptions.push(step_kind(step).to_string());
            }
        }
    }
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

fn step_kind(step: &DeclarativeStep) -> &'static str {
    match step {
        DeclarativeStep::To(_) => "to",
        DeclarativeStep::SetHeader(_) => "set_header",
        DeclarativeStep::SetProperty(_) => "set_property",
        DeclarativeStep::SetBody(_) => "set_body",
        DeclarativeStep::Filter(_) => "filter",
        DeclarativeStep::LoadBalance(_) => "load_balance",
        DeclarativeStep::Log(_) => "log",
        DeclarativeStep::Choice(_) => "choice",
        DeclarativeStep::Split(_) => "split",
        DeclarativeStep::WireTap(_) => "wire_tap",
        DeclarativeStep::Multicast(_) => "multicast",
        DeclarativeStep::Stop => "stop",
        DeclarativeStep::Throttle(_) => "throttle",
        DeclarativeStep::Loop(_) => "loop",
        _ => "custom_step",
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    let lower = text.to_ascii_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct RouteHints {
    id: String,
    from: String,
    step_descriptions: Vec<String>,
    components: BTreeSet<String>,
    has_error_handler: bool,
    has_circuit_breaker: bool,
    uses_ai: bool,
}

fn extract_route_hints_from_yaml(input: &str) -> Result<Vec<RouteHints>, CamelError> {
    let root: YamlValue = serde_yaml::from_str(input)
        .map_err(|e| CamelError::RouteError(format!("YAML parse error: {e}")))?;
    let Some(routes) = root.get("routes").and_then(YamlValue::as_sequence) else {
        return Ok(Vec::new());
    };

    let mut route_hints = Vec::with_capacity(routes.len());

    for route in routes {
        let Some(route_map) = route.as_mapping() else {
            continue;
        };

        let mut hint = RouteHints::default();
        hint.id = map_string(route_map, "id").unwrap_or_default();
        hint.from = map_string(route_map, "from").unwrap_or_default();
        hint.has_error_handler = route_map.contains_key(YamlValue::from("error_handler"));
        hint.has_circuit_breaker = route_map.contains_key(YamlValue::from("circuit_breaker"));

        collect_uri_component(&hint.from, &mut hint.components, &mut BTreeMap::new());
        if let Some(steps) = route_map.get(YamlValue::from("steps")) {
            collect_step_hints(steps, &mut hint);
        }

        hint.uses_ai = hint
            .components
            .iter()
            .any(|c| matches!(c.as_str(), "llm" | "embedding" | "vector"))
            || hint
                .step_descriptions
                .iter()
                .any(|step| contains_any(step, &["ai_classify", "ai_extract"]));

        route_hints.push(hint);
    }

    Ok(route_hints)
}

fn collect_step_hints(steps: &YamlValue, hint: &mut RouteHints) {
    let Some(steps) = steps.as_sequence() else {
        return;
    };

    for step in steps {
        let Some(step_map) = step.as_mapping() else {
            continue;
        };

        if let Some(uri) = map_string(step_map, "to") {
            hint.step_descriptions.push(format!("to:{uri}"));
            collect_uri_component(&uri, &mut hint.components, &mut BTreeMap::new());
        }

        if let Some(uri) = map_string(step_map, "wire_tap") {
            hint.step_descriptions.push(format!("wire_tap:{uri}"));
            collect_uri_component(&uri, &mut hint.components, &mut BTreeMap::new());
        }

        collect_ai_step_hint(step_map, "ai_classify", hint);
        collect_ai_step_hint(step_map, "ai_extract", hint);

        if let Some(filter) = step_map.get(YamlValue::from("filter")) {
            collect_nested_steps(filter, hint);
        }
        if let Some(split) = step_map.get(YamlValue::from("split")) {
            collect_nested_steps(split, hint);
        }
        if let Some(multicast) = step_map.get(YamlValue::from("multicast")) {
            collect_nested_steps(multicast, hint);
        }
        if let Some(load_balance) = step_map.get(YamlValue::from("load_balance")) {
            collect_nested_steps(load_balance, hint);
        }
        if let Some(throttle) = step_map.get(YamlValue::from("throttle")) {
            collect_nested_steps(throttle, hint);
        }
        if let Some(loop_step) = step_map.get(YamlValue::from("loop")) {
            collect_nested_steps(loop_step, hint);
        }
        if let Some(choice) = step_map.get(YamlValue::from("choice")) {
            collect_choice_steps(choice, hint);
        }
    }
}

fn collect_choice_steps(choice: &YamlValue, hint: &mut RouteHints) {
    hint.step_descriptions.push("choice".to_string());
    let Some(choice_map) = choice.as_mapping() else {
        return;
    };

    if let Some(whens) = choice_map
        .get(YamlValue::from("when"))
        .and_then(YamlValue::as_sequence)
    {
        for when in whens {
            if let Some(when_steps) = when.get("steps") {
                collect_step_hints(when_steps, hint);
            }
        }
    }
    if let Some(otherwise_steps) = choice_map.get(YamlValue::from("otherwise")) {
        collect_step_hints(otherwise_steps, hint);
    }
}

fn collect_nested_steps(node: &YamlValue, hint: &mut RouteHints) {
    let Some(node_map) = node.as_mapping() else {
        return;
    };
    if let Some(nested_steps) = node_map.get(YamlValue::from("steps")) {
        collect_step_hints(nested_steps, hint);
    }
}

fn collect_ai_step_hint(step_map: &serde_yaml::Mapping, step_name: &str, hint: &mut RouteHints) {
    let Some(step_data) = step_map.get(YamlValue::from(step_name)) else {
        return;
    };
    hint.uses_ai = true;

    let model_uri = step_data
        .as_mapping()
        .and_then(|map| map_string(map, "model_uri").or_else(|| map_string(map, "model")));

    if let Some(model_uri) = model_uri {
        hint.step_descriptions
            .push(format!("{step_name}:model_uri={model_uri}"));
        collect_uri_component(&model_uri, &mut hint.components, &mut BTreeMap::new());
    } else {
        hint.step_descriptions.push(step_name.to_string());
    }
}

fn map_string(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(YamlValue::from(key))
        .and_then(YamlValue::as_str)
        .map(ToOwned::to_owned)
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
        assert!(
            snapshot.routes[0]
                .components
                .iter()
                .any(|c| c == "embedding")
        );
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

    #[test]
    fn parses_ai_ticket_router_fixture_and_marks_ai_classify_steps() {
        let snapshot = build_system_snapshot_from_yaml(include_str!(
            "../../../examples/ai-ticket-router/routes.yaml"
        ))
        .expect("fixture should parse");

        let route = snapshot
            .routes
            .iter()
            .find(|route| route.id == "ai-staffs")
            .expect("ai-staffs route should exist");

        assert!(route.uses_ai);
        assert!(
            route
                .steps
                .iter()
                .any(|step| step.starts_with("ai_classify:model_uri=llm:"))
        );
        assert!(route.components.iter().any(|component| component == "llm"));
    }

    #[test]
    fn parses_rag_basic_fixture_and_marks_ai_extract_steps() {
        let snapshot = build_system_snapshot_from_yaml(include_str!(
            "../../../examples/rag-basic/routes.yaml"
        ))
        .expect("fixture should parse");

        let route = snapshot
            .routes
            .iter()
            .find(|route| route.id == "rag-query")
            .expect("rag-query route should exist");

        assert!(route.uses_ai);
        assert!(
            route
                .steps
                .iter()
                .any(|step| step.starts_with("ai_extract:model_uri=llm:"))
        );
        assert!(route.components.iter().any(|component| component == "llm"));
    }

    #[test]
    fn parses_rag_file_fixture_and_collects_embedding_vector_llm_components() {
        let snapshot =
            build_system_snapshot_from_yaml(include_str!("../../../examples/rag-file/routes.yaml"))
                .expect("fixture should parse");

        let route = snapshot
            .routes
            .iter()
            .find(|route| route.id == "rag-file-query")
            .expect("rag-file-query route should exist");

        assert!(route.uses_ai);
        assert!(
            route
                .components
                .iter()
                .any(|component| component == "embedding")
        );
        assert!(
            route
                .components
                .iter()
                .any(|component| component == "vector")
        );
        assert!(route.components.iter().any(|component| component == "llm"));
    }
}
