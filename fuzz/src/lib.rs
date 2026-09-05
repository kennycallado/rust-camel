use camel_dsl::SecurityCompileContext;
use camel_dsl::route_ast::RouteDslRoutes;

/// Feed arbitrary bytes as UTF-8 text to camel-dsl YAML route parsing.
///
/// Invalid UTF-8 input is skipped. Parsing must never panic: the parse call
/// returns either `Ok` or `Err`, and the result is discarded.
pub fn dsl_yaml_harness(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = camel_dsl::yaml::parse_yaml_with_threshold_and_security(
            s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        );
    }
}

/// Feed arbitrary bytes as UTF-8 text to camel-dsl JSON route parsing.
///
/// Invalid UTF-8 input is skipped. Parsing must never panic: the parse call
/// returns either `Ok` or `Err`, and the result is discarded.
pub fn dsl_json_harness(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = camel_dsl::json::parse_json_with_threshold_and_security(
            s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        );
    }
}

/// Feed arbitrary bytes as UTF-8 text to camel-dsl template parsing and
/// materialization.
///
/// Invalid UTF-8 input is skipped. Parsing must never panic. Each templated
/// route instance whose `route_template_ref` matches a parsed template id is
/// materialized and compiled with the result discarded; instances without a
/// matching template id are skipped (ordinary rejection, not a divergence).
pub fn dsl_template_harness(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        let templates = camel_dsl::template::json::parse_json_templates(s);
        let instances = camel_dsl::template::json::parse_json_templated_routes(s);
        if let (Ok(templates), Ok(instances)) = (templates, instances) {
            for instance in &instances {
                if let Some(template) = templates
                    .iter()
                    .find(|t| t.id == instance.route_template_ref)
                {
                    let _ = camel_dsl::materialize_and_compile(
                        template,
                        instance,
                        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
                        SecurityCompileContext::default(),
                    );
                }
            }
        }
    }
}

/// Assert the JSON and YAML deserializations of one document agree at the
/// step layer.
///
/// Same mechanism as the camel-dsl parity matrix: route count must match,
/// every route pair must share `id` and `from`, and the pretty-Debug
/// rendering of the flattened `Vec<&RouteDslStep>` must be identical.
fn assert_step_layer_parity(json_routes: &RouteDslRoutes, yaml_routes: &RouteDslRoutes) {
    if json_routes.routes.len() != yaml_routes.routes.len() {
        panic!(
            "parity divergence: route count differs (json: {}, yaml: {})",
            json_routes.routes.len(),
            yaml_routes.routes.len()
        );
    }
    for (jr, yr) in json_routes.routes.iter().zip(yaml_routes.routes.iter()) {
        if jr.id != yr.id || jr.from != yr.from {
            panic!(
                "parity divergence: route metadata differs (json id: {:?} from: {:?}, yaml id: {:?} from: {:?})",
                jr.id, jr.from, yr.id, yr.from
            );
        }
    }
    let json_steps = json_routes
        .routes
        .iter()
        .flat_map(|r| r.steps.iter())
        .collect::<Vec<_>>();
    let yaml_steps = yaml_routes
        .routes
        .iter()
        .flat_map(|r| r.steps.iter())
        .collect::<Vec<_>>();
    if format!("{json_steps:#?}") != format!("{yaml_steps:#?}") {
        panic!("parity divergence: step layers differ");
    }
}

/// Feed arbitrary bytes as UTF-8 text to both parse front-ends, then check
/// YAML/JSON deserialization parity on JSON-valid documents.
///
/// Both full parsers run first with results discarded (panic coverage).
/// Documents `serde_json` rejects are outside the parity overlap and return
/// early. A document `serde_json` accepts must also deserialize under the
/// YAML front-end and produce the same step layer.
pub fn dsl_parity_harness(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = camel_dsl::yaml::parse_yaml_with_threshold_and_security(
            s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        );
        let _ = camel_dsl::json::parse_json_with_threshold_and_security(
            s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        );
        let Ok(json_routes) = serde_json::from_str::<RouteDslRoutes>(s) else {
            return;
        };
        expect_yaml_overlap(s, &json_routes);
    }
}

/// Deserialize `s` with the YAML serde front-end and enforce parity with the
/// JSON deserialization.
fn expect_yaml_overlap(s: &str, json_routes: &RouteDslRoutes) {
    let result = noyalib::compat::serde_yaml::from_str::<RouteDslRoutes>(s);
    let yaml_routes = panic_if_yaml_rejects(result);
    assert_step_layer_parity(json_routes, &yaml_routes);
}

/// Panic when the YAML front-end rejects a JSON-valid document; otherwise
/// return the deserialized routes.
fn panic_if_yaml_rejects(
    result: Result<RouteDslRoutes, noyalib::compat::serde_yaml::Error>,
) -> RouteDslRoutes {
    match result {
        Ok(yaml_routes) => yaml_routes,
        Err(_) => panic!("parity divergence: yaml rejects json-valid document"),
    }
}

#[cfg(test)]
mod tests {
    use super::{assert_step_layer_parity, panic_if_yaml_rejects};
    use crate::{dsl_json_harness, dsl_parity_harness, dsl_template_harness};
    use camel_dsl::route_ast::RouteDslRoute;
    use camel_dsl::route_ast::RouteDslRoutes;
    use camel_dsl::route_ast::RouteDslStep;

    const MINIMAL_JSON: &str =
        r#"{"routes":[{"id":"r1","from":"timer:tick","steps":[{"to":"log:info"}]}]}"#;

    const TEMPLATE_JSON: &str = r#"{
        "routes": [],
        "templates": [
            {
                "id": "tpl",
                "parameters": [{"name": "uri"}],
                "routes": [
                    {"id": "inst-route", "from": "timer:{{uri}}", "steps": [{"to": "log:info"}]}
                ]
            }
        ],
        "templated_routes": [
            {"route_template_ref": "tpl", "parameters": {"uri": "tick"}}
        ]
    }"#;

    const MISSING_REF_JSON: &str = r#"{
        "routes": [],
        "templates": [
            {
                "id": "tpl",
                "parameters": [{"name": "uri"}],
                "routes": [
                    {"id": "inst-route", "from": "timer:{{uri}}", "steps": [{"to": "log:info"}]}
                ]
            }
        ],
        "templated_routes": [
            {"route_template_ref": "no-such-template", "parameters": {"uri": "tick"}}
        ]
    }"#;

    fn to_step() -> RouteDslStep {
        // `ToStep` is `#[non_exhaustive]`, so the fixture value is built
        // through the crate's public `Deserialize` impl instead of a struct
        // literal. Same canonical JSON as the camel-dsl parity matrix.
        serde_json::from_str(r#"{"to":"log:info"}"#).expect("valid To step fixture")
    }

    fn route(id: &str, from: &str, steps: Vec<RouteDslStep>) -> RouteDslRoute {
        RouteDslRoute {
            id: id.to_string(),
            from: from.to_string(),
            parameters: Default::default(),
            steps,
            auto_startup: true,
            startup_order: 0,
            sequential: false,
            concurrent: None,
            error_handler: None,
            circuit_breaker: None,
            security_policy: None,
            on_complete: None,
            on_failure: None,
        }
    }

    fn doc(routes: Vec<RouteDslRoute>) -> RouteDslRoutes {
        RouteDslRoutes {
            schema_url: None,
            routes,
            templates: Vec::new(),
            templated_routes: Vec::new(),
            rest: Vec::new(),
            mcp: Vec::new(),
        }
    }

    #[test]
    fn dsl_json_harness_valid_minimal_returns() {
        dsl_json_harness(MINIMAL_JSON.as_bytes());
    }

    #[test]
    fn dsl_json_harness_invalid_utf8_returns() {
        dsl_json_harness(b"\xff\xfe\xfd");
    }

    #[test]
    fn dsl_json_harness_malformed_returns() {
        dsl_json_harness(b"{");
    }

    #[test]
    fn dsl_template_harness_valid_templated_returns() {
        dsl_template_harness(TEMPLATE_JSON.as_bytes());
    }

    #[test]
    fn dsl_template_harness_missing_ref_returns() {
        dsl_template_harness(MISSING_REF_JSON.as_bytes());
    }

    #[test]
    fn dsl_template_harness_invalid_utf8_returns() {
        dsl_template_harness(b"\xff");
    }

    #[test]
    fn dsl_template_harness_malformed_returns() {
        dsl_template_harness(b"{\"templates\": [{");
    }

    #[test]
    fn dsl_parity_harness_valid_both_returns() {
        dsl_parity_harness(MINIMAL_JSON.as_bytes());
    }

    #[test]
    fn dsl_parity_harness_json_only_syntax_skips() {
        dsl_parity_harness(b"routes: []");
    }

    #[test]
    fn dsl_parity_harness_invalid_utf8_returns() {
        dsl_parity_harness(b"\xff");
    }

    #[test]
    fn assert_step_layer_parity_equal_returns() {
        let json = doc(vec![route("r1", "timer:tick", vec![to_step()])]);
        let yaml = doc(vec![route("r1", "timer:tick", vec![to_step()])]);
        assert_step_layer_parity(&json, &yaml);
    }

    #[test]
    #[should_panic(expected = "parity divergence")]
    fn assert_step_layer_parity_count_divergence_panics() {
        let json = doc(vec![route("r1", "timer:tick", vec![])]);
        let yaml = doc(vec![
            route("r1", "timer:tick", vec![]),
            route("r2", "timer:tick", vec![]),
        ]);
        assert_step_layer_parity(&json, &yaml);
    }

    #[test]
    #[should_panic(expected = "parity divergence")]
    fn assert_step_layer_parity_id_divergence_panics() {
        let json = doc(vec![route("r1", "timer:tick", vec![])]);
        let yaml = doc(vec![route("other", "timer:tick", vec![])]);
        assert_step_layer_parity(&json, &yaml);
    }

    #[test]
    #[should_panic(expected = "parity divergence")]
    fn assert_step_layer_parity_from_divergence_panics() {
        let json = doc(vec![route("r1", "timer:tick", vec![])]);
        let yaml = doc(vec![route("r1", "direct:start", vec![])]);
        assert_step_layer_parity(&json, &yaml);
    }

    #[test]
    #[should_panic(expected = "parity divergence")]
    fn assert_step_layer_parity_steps_divergence_panics() {
        let json = doc(vec![route("r1", "timer:tick", vec![])]);
        let yaml = doc(vec![route("r1", "timer:tick", vec![to_step()])]);
        assert_step_layer_parity(&json, &yaml);
    }

    #[test]
    #[should_panic(expected = "parity divergence: yaml rejects")]
    fn panic_if_yaml_rejects_err_panics() {
        let result = noyalib::compat::serde_yaml::from_str::<RouteDslRoutes>("{");
        let _ = panic_if_yaml_rejects(result);
    }

    #[test]
    fn panic_if_yaml_rejects_ok_returns() {
        let result = noyalib::compat::serde_yaml::from_str::<RouteDslRoutes>(MINIMAL_JSON);
        let _ = panic_if_yaml_rejects(result);
    }

    // --- Committed seed corpus contract tests (Task 1.2) ---
    //
    // Each test resolves committed seeds from the crate's `seeds/` directory
    // and enforces the deserialization contract of the corresponding target.
    // Paths are resolved from `env!("CARGO_MANIFEST_DIR")` so the tests run
    // from any working directory.

    fn seeds_dir(target: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("seeds")
            .join(target)
    }

    /// Sorted `valid_*.json` seed paths for a target directory.
    fn valid_seed_paths(target: &str) -> Vec<std::path::PathBuf> {
        let dir = seeds_dir(target);
        let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read seeds dir {}: {e}", dir.display()))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("valid_") && n.ends_with(".json"))
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        assert!(
            !paths.is_empty(),
            "no valid_*.json seeds in {}",
            dir.display()
        );
        paths
    }

    #[test]
    fn seeds_dsl_json_contract() {
        for path in valid_seed_paths("dsl_json") {
            let s = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            serde_json::from_str::<RouteDslRoutes>(&s)
                .unwrap_or_else(|e| panic!("{} json deserialize failed: {e}", path.display()));
        }
        let malformed = seeds_dir("dsl_json").join("malformed_truncated.json");
        let s = std::fs::read_to_string(&malformed).expect("malformed_truncated.json must exist");
        assert!(
            serde_json::from_str::<RouteDslRoutes>(&s).is_err(),
            "malformed_truncated.json must be rejected"
        );
    }

    #[test]
    fn seeds_dsl_parity_contract() {
        for path in valid_seed_paths("dsl_parity") {
            let s = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            serde_json::from_str::<RouteDslRoutes>(&s)
                .unwrap_or_else(|e| panic!("{} json deserialize failed: {e}", path.display()));
            noyalib::compat::serde_yaml::from_str::<RouteDslRoutes>(&s)
                .unwrap_or_else(|e| panic!("{} yaml deserialize failed: {e}", path.display()));
        }
        let malformed = seeds_dir("dsl_parity").join("malformed_both.json");
        let s = std::fs::read_to_string(&malformed).expect("malformed_both.json must exist");
        assert!(
            serde_json::from_str::<RouteDslRoutes>(&s).is_err(),
            "malformed_both.json must be rejected by serde_json"
        );
        assert!(
            noyalib::compat::serde_yaml::from_str::<RouteDslRoutes>(&s).is_err(),
            "malformed_both.json must be rejected by the yaml front-end"
        );
    }

    #[test]
    fn seeds_dsl_template_contract() {
        for name in ["valid_templated.json", "placeholder_heavy.json"] {
            let path = seeds_dir("dsl_template").join(name);
            let s = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let templates = camel_dsl::template::json::parse_json_templates(&s)
                .unwrap_or_else(|e| panic!("{name} parse_json_templates failed: {e}"));
            let instances = camel_dsl::template::json::parse_json_templated_routes(&s)
                .unwrap_or_else(|e| panic!("{name} parse_json_templated_routes failed: {e}"));
            let matched = instances
                .iter()
                .any(|i| templates.iter().any(|t| t.id == i.route_template_ref));
            assert!(
                matched,
                "{name}: no templated route instance matched a template id"
            );
        }
        let malformed = seeds_dir("dsl_template").join("malformed_template.json");
        let s = std::fs::read_to_string(&malformed).expect("malformed_template.json must exist");
        let templates_ok = camel_dsl::template::json::parse_json_templates(&s).is_ok();
        let instances_ok = camel_dsl::template::json::parse_json_templated_routes(&s).is_ok();
        assert!(
            !(templates_ok && instances_ok),
            "malformed_template.json must fail at least one parse"
        );
    }
}
