use camel_dsl::SecurityCompileContext;
use camel_dsl::route_ast::RouteDslRest;
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

/// Feed arbitrary bytes as UTF-8 text to both camel-dsl parse front-ends
/// with `rest:` lowering enabled.
///
/// Invalid UTF-8 input is skipped. Both front-ends lower `rest:` blocks the
/// same way, and parsing must never panic: each parse call returns either
/// `Ok` or `Err`, and both results are discarded.
pub fn dsl_rest_harness(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = camel_dsl::json::parse_json_with_threshold_and_security(
            s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        );
        let _ = camel_dsl::yaml::parse_yaml_with_threshold_and_security(
            s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        );
    }
}

/// Feed arbitrary bytes as UTF-8 text to both camel-dsl parse front-ends
/// with `mcp:` lowering enabled.
///
/// Invalid UTF-8 input is skipped. Both front-ends lower `mcp:` blocks the
/// same way, and parsing must never panic: each parse call returns either
/// `Ok` or `Err`, and both results are discarded.
pub fn dsl_mcp_harness(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = camel_dsl::json::parse_json_with_threshold_and_security(
            s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        );
        let _ = camel_dsl::yaml::parse_yaml_with_threshold_and_security(
            s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        );
    }
}

/// Lower one validated set of `rest:` blocks and generate the OpenAPI
/// document (discarded).
///
/// Mirrors the camel-cli `openapi generate` validated-generation stage:
/// lowering and the duplicate route id check must both succeed before
/// generation runs. Any `Err` skips generation (ordinary rejection).
fn validate_and_generate_openapi(blocks: &[RouteDslRest]) {
    if let Ok(lowered) = camel_dsl::rest::lower_all_rest_to_routes(blocks)
        && camel_dsl::rest::check_duplicate_route_ids(&lowered).is_ok()
    {
        let _ = camel_dsl::openapi::generate_openapi(blocks, "camel-fuzz", "0.0.0");
    }
}

/// Feed arbitrary bytes as UTF-8 text to the validated `rest:` generation
/// pipeline (YAML and JSON legs).
///
/// Invalid UTF-8 input is skipped. Each leg mirrors the camel-cli
/// `openapi generate` validated-generation stage: extract (or deserialize)
/// `rest:` blocks, then lower, reject duplicate route ids, and generate
/// through `validate_and_generate_openapi` with the result discarded.
/// Documents yielding no `rest:` blocks skip lowering and generation
/// entirely. No stage may panic.
pub fn dsl_openapi_harness(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(blocks) = camel_dsl::yaml::extract_rest_blocks(s)
            && !blocks.is_empty()
        {
            validate_and_generate_openapi(&blocks);
        }
        if let Ok(doc) = serde_json::from_str::<RouteDslRoutes>(s)
            && !doc.rest.is_empty()
        {
            validate_and_generate_openapi(&doc.rest);
        }
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
///
/// A stream carrying raw non-printable characters (YAML 1.2 c-printable
/// prohibited classes) is spec-correctly rejected by the YAML front-end, so
/// only the expected `Err` is asserted — no step-layer comparison is
/// possible without a YAML value. All other documents take the strict path.
fn expect_yaml_overlap(s: &str, json_routes: &RouteDslRoutes) {
    if camel_dsl::yaml::yaml_stream_has_non_printable(s) {
        expect_expected_rejection(s);
        return;
    }
    let result = noyalib::compat::serde_yaml::from_str::<RouteDslRoutes>(s);
    let yaml_routes = panic_if_yaml_rejects(result);
    assert_step_layer_parity(json_routes, &yaml_routes);
}

/// Assert the YAML front-end rejects a stream carrying raw non-printable
/// characters; that rejection is spec-correct, not a parity divergence.
fn expect_expected_rejection(s: &str) {
    assert!(
        noyalib::compat::serde_yaml::from_str::<RouteDslRoutes>(s).is_err(),
        "parity divergence: yaml accepts document with raw non-printable characters"
    );
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
    use crate::{
        dsl_json_harness, dsl_mcp_harness, dsl_openapi_harness, dsl_parity_harness,
        dsl_rest_harness, dsl_template_harness,
    };
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

    /// Minimized DEL document from fuzz run 33984285881 (93 bytes,
    /// JSON-valid): the raw U+007F byte makes YAML spec-correctly reject the
    /// stream.
    const MINIMIZED_DEL_DOC: &str = "{\"routes\":[{\"id\":\"r1\",\"from\":\"dtart\",\"steps\":[{\"to\":\"di*rect:ewwwwwwwwwwwwww\x7fwwwwwwwwnd\"}]}]}";

    /// Same document with the raw DEL byte replaced by the six ASCII bytes
    /// `\u007f` (98 wire bytes): printable wire, so strict parity must still
    /// hold.
    const ESCAPED_DEL_DOC: &str = "{\"routes\":[{\"id\":\"r1\",\"from\":\"dtart\",\"steps\":[{\"to\":\"di*rect:ewwwwwwwwwwwwww\\u007fwwwwwwwwnd\"}]}]}";

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
    fn harness_del_document_does_not_panic() {
        dsl_parity_harness(MINIMIZED_DEL_DOC.as_bytes());
    }

    #[test]
    fn escaped_del_document_keeps_strict_parity() {
        dsl_parity_harness(ESCAPED_DEL_DOC.as_bytes());
    }

    // --- rest:/mcp:/openapi channel harness semantics (Task 1.1) ---

    /// Minimal JSON rest document (host/port/base-path/one GET operation),
    /// adapted from the camel-dsl `json_rest_block_expands_into_routes`
    /// fixture.
    const REST_MINIMAL_JSON: &str = r#"{
        "rest": [
            {
                "host": "0.0.0.0",
                "port": 8080,
                "path": "/users",
                "operations": [
                    { "method": "GET", "path": "/{id}", "operation_id": "getUser", "to": "bean:svc" }
                ]
            }
        ]
    }"#;

    /// The same minimal rest document authored as YAML.
    const REST_MINIMAL_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /users
    operations:
      - method: GET
        path: /{id}
        operation_id: getUser
        to: bean:svc
"#;

    /// Minimal JSON mcp document (server name/bind + one tool with
    /// `input_schema` + one resource with uri), adapted from the camel-dsl
    /// mcp parse fixtures.
    const MCP_MINIMAL_JSON: &str = r#"{
        "mcp": [
            {
                "server": {
                    "name": "crm",
                    "bind": "127.0.0.1:9100"
                },
                "tools": [
                    { "name": "lookup", "input_schema": {"type": "object"} }
                ],
                "resources": [
                    { "name": "customers", "uri": "crm://customers" }
                ]
            }
        ]
    }"#;

    /// Hostile mcp documents that must each be rejected with `Err` inside
    /// the front-end (never panic the harness): bad tool name charset, bad
    /// bind pattern, non-object `input_schema`, blank TLS cert path.
    const MCP_HOSTILE_JSON_DOCS: [&str; 4] = [
        r#"{"mcp":[{"server":{"name":"crm","bind":"127.0.0.1:9100"},"tools":[{"name":"bad name!","input_schema":{"type":"object"}}]}]}"#,
        r#"{"mcp":[{"server":{"name":"crm","bind":"not-an-ip:port"},"tools":[{"name":"lookup","input_schema":{"type":"object"}}]}]}"#,
        r#"{"mcp":[{"server":{"name":"crm","bind":"127.0.0.1:9100"},"tools":[{"name":"lookup","input_schema":"string"}]}]}"#,
        r#"{"mcp":[{"server":{"name":"crm","bind":"127.0.0.1:9100","tls":{"cert_path":"  ","key_path":"/etc/certs/crm-key.pem"}}}]}"#,
    ];

    /// Rest document whose blocks pass lowering: one operation with an
    /// explicit `operationId`, one with no response schemas.
    const OPENAPI_VALID_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /users
    operations:
      - method: GET
        path: /{id}
        operation_id: getUser
        to: bean:svc
      - method: POST
        path: /
        operation_id: createUser
        to: bean:create
"#;

    /// Two blocks on different `host:port` listeners claiming the same
    /// `(path, verb)` with distinct `operationId`s: per-listener validation
    /// passes, so generation runs and records a duplicate warning
    /// (discarded).
    const OPENAPI_DUP_ACROSS_LISTENERS_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /users
    operations:
      - method: GET
        path: /{id}
        operation_id: getUserA
        to: bean:a
  - host: 0.0.0.0
    port: 8081
    path: /users
    operations:
      - method: GET
        path: /{id}
        operation_id: getUserB
        to: bean:b
"#;

    /// Same-listener duplicate `(path, verb)`: lowering rejects the block
    /// set, so generation is skipped.
    const OPENAPI_DUP_SAME_LISTENER_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 8080
    path: /users
    operations:
      - method: GET
        path: /{id}
        operation_id: getUserA
        to: bean:a
  - host: 0.0.0.0
    port: 8080
    path: /users
    operations:
      - method: GET
        path: /{id}
        operation_id: getUserB
        to: bean:b
"#;

    #[test]
    fn dsl_rest_harness_valid_minimal_returns() {
        dsl_rest_harness(REST_MINIMAL_JSON.as_bytes());
        assert!(
            camel_dsl::json::parse_json_with_threshold_and_security(
                REST_MINIMAL_JSON,
                camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
                camel_dsl::SecurityCompileContext::default(),
            )
            .is_ok(),
            "REST_MINIMAL_JSON must parse Ok"
        );
    }

    #[test]
    fn dsl_rest_harness_malformed_returns() {
        dsl_rest_harness(b"{");
        dsl_rest_harness(b"{\"rest\": [{");
    }

    #[test]
    fn dsl_rest_harness_yaml_shaped_returns() {
        dsl_rest_harness(REST_MINIMAL_YAML.as_bytes());
    }

    #[test]
    fn dsl_rest_harness_invalid_utf8_returns() {
        dsl_rest_harness(b"\xff\xfe");
    }

    #[test]
    fn dsl_mcp_harness_valid_minimal_returns() {
        dsl_mcp_harness(MCP_MINIMAL_JSON.as_bytes());
        assert!(
            camel_dsl::json::parse_json_with_threshold_and_security(
                MCP_MINIMAL_JSON,
                camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
                camel_dsl::SecurityCompileContext::default(),
            )
            .is_ok(),
            "MCP_MINIMAL_JSON must parse Ok"
        );
    }

    #[test]
    fn dsl_mcp_harness_hostile_rejected() {
        for doc in MCP_HOSTILE_JSON_DOCS {
            dsl_mcp_harness(doc.as_bytes());
        }
    }

    #[test]
    fn dsl_mcp_harness_invalid_utf8_returns() {
        dsl_mcp_harness(b"\xff");
    }

    #[test]
    fn dsl_openapi_harness_valid_returns() {
        dsl_openapi_harness(OPENAPI_VALID_YAML.as_bytes());
        let blocks = camel_dsl::yaml::extract_rest_blocks(OPENAPI_VALID_YAML)
            .expect("OPENAPI_VALID_YAML must extract rest blocks");
        let lowered = camel_dsl::rest::lower_all_rest_to_routes(&blocks)
            .expect("OPENAPI_VALID_YAML rest blocks must lower");
        let _ = lowered;
    }

    #[test]
    fn dsl_openapi_harness_duplicate_across_listeners_returns() {
        dsl_openapi_harness(OPENAPI_DUP_ACROSS_LISTENERS_YAML.as_bytes());
    }

    #[test]
    fn dsl_openapi_harness_unvalidated_skips_generation() {
        dsl_openapi_harness(OPENAPI_DUP_SAME_LISTENER_YAML.as_bytes());
    }

    #[test]
    fn dsl_openapi_harness_invalid_utf8_returns() {
        dsl_openapi_harness(b"\xff");
    }

    #[test]
    fn dsl_openapi_harness_empty_docs_skip_generation() {
        dsl_openapi_harness(b"{}");
        dsl_openapi_harness(b"{\"routes\":[]}");
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

    // --- Committed seed corpus contract tests (Task 1.3: rest/mcp/openapi) ---
    //
    // Each test pins the exact directory shape (sorted name list equality),
    // runs every seed through its target's harness (no panic), asserts each
    // `valid_*` seed parses `Ok` through BOTH front-ends (JSON is valid
    // YAML), and asserts each `malformed_*` seed is rejected by its
    // documented path. Warning seeds additionally pin the exact warning
    // substrings emitted by `camel_dsl::openapi::generate_openapi`.

    /// Sorted file names in a target's committed seed directory.
    fn seed_names(target: &str) -> Vec<String> {
        let dir = seeds_dir(target);
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read seeds dir {}: {e}", dir.display()))
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        names
    }

    /// Read one committed seed as UTF-8 text.
    fn read_seed(target: &str, name: &str) -> String {
        let path = seeds_dir(target).join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// Parse one seed through the JSON front-end and return the result.
    fn parse_seed_json(s: &str) -> Result<(), camel_api::CamelError> {
        camel_dsl::json::parse_json_with_threshold_and_security(
            s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            camel_dsl::SecurityCompileContext::default(),
        )
        .map(|_| ())
    }

    /// Assert one seed parses `Ok` through BOTH front-ends.
    fn assert_parses_ok_both_front_ends(target: &str, name: &str) {
        let s = read_seed(target, name);
        let json = parse_seed_json(&s);
        let yaml = camel_dsl::yaml::parse_yaml_with_threshold_and_security(
            &s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            camel_dsl::SecurityCompileContext::default(),
        );
        assert!(
            json.is_ok(),
            "{target}/{name} must parse Ok through the JSON front-end: {:?}",
            json.err()
        );
        assert!(
            yaml.is_ok(),
            "{target}/{name} must parse Ok through the YAML front-end: {:?}",
            yaml.err()
        );
    }

    #[test]
    fn seeds_dsl_rest_contract() {
        assert_eq!(
            seed_names("dsl_rest"),
            [
                "malformed_ambiguous_template.json",
                "malformed_bad_media.json",
                "malformed_duplicate_tuple.json",
                "malformed_truncated.json",
                "valid_minimal.json",
                "valid_multiple_operations.json",
            ]
        );
        for name in seed_names("dsl_rest") {
            dsl_rest_harness(read_seed("dsl_rest", &name).as_bytes());
        }
        for name in ["valid_minimal.json", "valid_multiple_operations.json"] {
            assert_parses_ok_both_front_ends("dsl_rest", name);
        }
        // Lowering rejections on the parse-with-lowering path.
        for name in [
            "malformed_truncated.json",
            "malformed_duplicate_tuple.json",
            "malformed_ambiguous_template.json",
        ] {
            let result = parse_seed_json(&read_seed("dsl_rest", name));
            assert!(
                result.is_err(),
                "dsl_rest/{name} must be rejected by the parse-with-lowering path"
            );
        }
        let err = parse_seed_json(&read_seed("dsl_rest", "malformed_bad_media.json"))
            .expect_err("malformed_bad_media.json must be rejected");
        assert!(
            err.to_string().contains("requires a JSON media type"),
            "expected a media error, got: {err}"
        );
    }

    #[test]
    fn seeds_dsl_mcp_contract() {
        assert_eq!(
            seed_names("dsl_mcp"),
            [
                "malformed_bad_bind.json",
                "malformed_bad_name.json",
                "malformed_blank_tls_path.json",
                "malformed_truncated.json",
                "valid_minimal.json",
                "valid_tool_and_resource.json",
            ]
        );
        for name in seed_names("dsl_mcp") {
            dsl_mcp_harness(read_seed("dsl_mcp", &name).as_bytes());
        }
        for name in ["valid_minimal.json", "valid_tool_and_resource.json"] {
            assert_parses_ok_both_front_ends("dsl_mcp", name);
        }
        // (seed, error substring) pairs: bad name fails at lowering; bad bind
        // and blank TLS paths fail at serde load.
        let rejections = [
            ("malformed_truncated.json", "JSON parse error"),
            ("malformed_bad_name.json", "names must match"),
            ("malformed_bad_bind.json", "not an IP:port literal"),
            (
                "malformed_blank_tls_path.json",
                "cert_path must not be empty",
            ),
        ];
        for (name, needle) in rejections {
            let err = parse_seed_json(&read_seed("dsl_mcp", name))
                .err()
                .unwrap_or_else(|| panic!("dsl_mcp/{name} must be rejected"));
            assert!(
                err.to_string().contains(needle),
                "dsl_mcp/{name}: expected '{needle}', got: {err}"
            );
        }
        // Non-object `input_schema` (hostile doc #2) fails at lowering on the
        // parse-with-lowering path.
        let err = parse_seed_json(MCP_HOSTILE_JSON_DOCS[2])
            .expect_err("input_schema doc must be rejected");
        assert!(
            err.to_string()
                .contains("input_schema is invalid: must be a JSON object"),
            "expected an input_schema error, got: {err}"
        );
    }

    #[test]
    fn seeds_dsl_openapi_contract() {
        assert_eq!(
            seed_names("dsl_openapi"),
            [
                "malformed_same_listener_duplicate.json",
                "malformed_truncated.json",
                "valid_minimal.json",
                "warning_duplicate_across_listeners.json",
                "warning_weak_stub.json",
            ]
        );
        for name in seed_names("dsl_openapi") {
            dsl_openapi_harness(read_seed("dsl_openapi", &name).as_bytes());
        }
        for name in [
            "valid_minimal.json",
            "warning_weak_stub.json",
            "warning_duplicate_across_listeners.json",
        ] {
            assert_parses_ok_both_front_ends("dsl_openapi", name);
            let doc: RouteDslRoutes = serde_json::from_str(&read_seed("dsl_openapi", name))
                .unwrap_or_else(|e| panic!("dsl_openapi/{name} must deserialize: {e}"));
            camel_dsl::rest::lower_all_rest_to_routes(&doc.rest)
                .unwrap_or_else(|e| panic!("dsl_openapi/{name} must lower: {e}"));
        }
        let truncated = read_seed("dsl_openapi", "malformed_truncated.json");
        assert!(
            camel_dsl::yaml::extract_rest_blocks(&truncated).is_err(),
            "malformed_truncated.json must fail rest-block extraction"
        );
        assert!(
            serde_json::from_str::<RouteDslRoutes>(&truncated).is_err(),
            "malformed_truncated.json must fail JSON deserialization"
        );
        let dup: RouteDslRoutes = serde_json::from_str(&read_seed(
            "dsl_openapi",
            "malformed_same_listener_duplicate.json",
        ))
        .expect("malformed_same_listener_duplicate.json must deserialize");
        let err = camel_dsl::rest::lower_all_rest_to_routes(&dup.rest)
            .err()
            .expect("malformed_same_listener_duplicate.json must fail lowering");
        assert!(
            err.to_string().contains("duplicate"),
            "expected a duplicate-tuple rejection, got: {err}"
        );
        // Warning contracts pin the exact literals from
        // crates/camel-dsl/src/openapi.rs (`build_operation` weak-stub
        // warning and the duplicate-operation warning).
        let weak: RouteDslRoutes =
            serde_json::from_str(&read_seed("dsl_openapi", "warning_weak_stub.json"))
                .expect("warning_weak_stub.json must deserialize");
        let weak_result = camel_dsl::openapi::generate_openapi(&weak.rest, "camel-fuzz", "0.0.0");
        assert!(
            weak_result
                .warnings
                .iter()
                .any(|w| w.contains("no response schema, using weak stub (type: object)")),
            "expected the weak-stub warning, got: {:?}",
            weak_result.warnings
        );
        let dup_warn: RouteDslRoutes = serde_json::from_str(&read_seed(
            "dsl_openapi",
            "warning_duplicate_across_listeners.json",
        ))
        .expect("warning_duplicate_across_listeners.json must deserialize");
        let dup_result =
            camel_dsl::openapi::generate_openapi(&dup_warn.rest, "camel-fuzz", "0.0.0");
        assert!(
            dup_result
                .warnings
                .iter()
                .any(|w| w.contains("duplicate operation: ")),
            "expected the duplicate-operation warning, got: {:?}",
            dup_result.warnings
        );
    }
}
