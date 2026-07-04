//! OpenAPI 3.0.3 code-first generation from `rest:` DSL AST blocks.
//!
//! Spec §10: each operation maps to `paths.<full_path>.<verb>` with
//! parameters from path templates, requestBody from request_schema, and
//! responses from response metadata or weak stubs.

use serde_json::{Map, Value, json};

use crate::rest::{
    build_full_path, default_status_for_verb, extract_param_names, parse_path_template,
    verb_has_body,
};
use crate::route_ast::{RouteDslRest, RouteDslRestOperation};
use std::collections::HashSet;

/// OpenAPI 3.0.3 HTTP verbs — anything else is logged as a warning.
/// Kept in sync with `KNOWN_VERBS` in rest.rs (the lowering authority); verbs
/// not in that set are rejected before reaching OpenAPI generation, so they
/// must not appear here either.
const VALID_OPENAPI_VERBS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];

/// Result of OpenAPI generation — the document plus any warnings.
#[derive(Debug, Clone)]
pub struct OpenApiGenerationResult {
    /// The OpenAPI 3.0.3 document as `serde_json::Value`.
    pub document: Value,
    /// Warnings (e.g. operations with missing schemas using weak stubs).
    pub warnings: Vec<String>,
}

/// Generate an OpenAPI 3.0.3 document from parsed `rest:` AST blocks.
///
/// Each operation maps to a `paths.<full_path>.<verb>` entry. Missing
/// schemas produce weak stubs (`type: object`) and a warning.
pub fn generate_openapi(
    rest_blocks: &[RouteDslRest],
    title: &str,
    version: &str,
) -> OpenApiGenerationResult {
    let mut warnings = Vec::new();
    let mut paths: Map<String, Value> = Map::new();
    let mut seen_operation_ids: HashSet<String> = HashSet::new();

    for rest in rest_blocks {
        for op in &rest.operations {
            let verb_lower = op.method.to_lowercase();
            // I3: warn on unknown HTTP verbs (don't reject — weak-stub philosophy)
            if !VALID_OPENAPI_VERBS.contains(&verb_lower.as_str()) {
                warnings.push(format!(
                    "unknown HTTP verb '{}' — may produce invalid OpenAPI",
                    op.method
                ));
            }
            let full_path = build_full_path(&rest.path, &op.path);
            let path_entry = paths.entry(full_path.clone()).or_insert_with(|| json!({}));
            let op_label = op.operation_id.as_deref().unwrap_or(&verb_lower);
            // I1: detect duplicate (path, verb) — last wins, warn
            if path_entry.get(&verb_lower).is_some() {
                warnings.push(format!(
                    "duplicate operation: {} {} ({}) — overwrites previous definition",
                    verb_lower.to_uppercase(),
                    full_path,
                    op_label
                ));
            }
            // M2: detect duplicate operationId (only for explicitly-set ids; auto-generated
            // operationIds are derived from verb+path and inherit the I1 last-wins dedup)
            if let Some(id) = op.operation_id.as_deref()
                && !seen_operation_ids.insert(id.to_string())
            {
                warnings.push(format!(
                    "duplicate operationId '{id}' — operationIds must be unique within a document"
                ));
            }
            let operation_obj = build_operation(&verb_lower, op, &full_path, &mut warnings);
            path_entry[&verb_lower] = operation_obj;
        }
    }

    // Servers: collect unique (host, port) from all rest blocks
    let mut seen = std::collections::HashSet::new();
    let servers: Vec<Value> = rest_blocks
        .iter()
        .filter(|r| seen.insert((r.host.as_str(), r.port)))
        .map(|r| json!({ "url": format!("http://{}:{}", r.host, r.port) }))
        .collect();

    let document = json!({
        "openapi": "3.0.3",
        "info": {
            "title": title,
            "version": version,
        },
        "servers": servers,
        "paths": Value::Object(paths),
    });

    OpenApiGenerationResult { document, warnings }
}

/// Build a single OpenAPI operation object.
fn build_operation(
    verb: &str,
    op: &RouteDslRestOperation,
    full_path: &str,
    warnings: &mut Vec<String>,
) -> Value {
    let segments = parse_path_template(full_path);
    let param_names = extract_param_names(&segments);

    // I2: path-template params (inferred as string), with declared overrides
    let mut parameters: Vec<Value> = param_names
        .iter()
        .map(|name| {
            let schema = op
                .parameters
                .get(name)
                .cloned()
                .unwrap_or_else(|| json!({ "type": "string" }));
            json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": schema
            })
        })
        .collect();

    // I2: declared params not in path template → additional query params
    for (name, schema) in &op.parameters {
        if !param_names.contains(name) {
            parameters.push(json!({
                "name": name,
                "in": "query",
                "required": false,
                "schema": schema
            }));
        }
    }

    // M1: trim leading slash for cleaner default operationId
    let operation_id = op.operation_id.clone().unwrap_or_else(|| {
        format!(
            "{}_{}",
            verb,
            full_path.trim_start_matches('/').replace('/', "_")
        )
    });

    let mut operation = Map::new();
    operation.insert("operationId".to_string(), json!(operation_id));

    if let Some(desc) = &op.description {
        operation.insert("description".to_string(), json!(desc));
    }

    if !parameters.is_empty() {
        operation.insert("parameters".to_string(), json!(parameters));
    }

    // --- Responses ---
    let success_code = op
        .success_status
        .unwrap_or_else(|| default_status_for_verb(verb));
    let op_label = op.operation_id.as_deref().unwrap_or(verb);
    let response_desc = op
        .response
        .as_ref()
        .and_then(|r| r.description.clone())
        .unwrap_or_else(|| default_response_description(success_code).to_string());

    let mut responses = Map::new();
    if success_code == 204 {
        // Warn if author also supplied a response schema — it will be ignored
        if op
            .response
            .as_ref()
            .and_then(|r| r.schema.as_ref())
            .is_some()
        {
            warnings.push(format!(
                "operation '{op_label}' — success_status 204 ignores response.schema (No Content)"
            ));
        }
        let mut resp204 = Map::new();
        resp204.insert("description".to_string(), json!(response_desc));
        insert_response_headers(&mut resp204, op);
        responses.insert("204".to_string(), Value::Object(resp204));
    } else {
        let response_schema = op
            .response
            .as_ref()
            .and_then(|r| r.schema.clone())
            .unwrap_or_else(|| {
                let label = op.operation_id.as_deref().unwrap_or(verb);
                warnings.push(format!(
                    "operation '{label}' ({verb} {full_path}) — no response schema, using weak stub (type: object)"
                ));
                json!({ "type": "object" })
            });
        let mut resp_obj = Map::new();
        resp_obj.insert("description".to_string(), json!(response_desc));
        resp_obj.insert(
            "content".to_string(),
            json!({
                op.produces.as_str(): { "schema": response_schema }
            }),
        );
        insert_response_headers(&mut resp_obj, op);
        responses.insert(success_code.to_string(), Value::Object(resp_obj));
    }
    operation.insert("responses".to_string(), json!(responses));

    // --- Request body (body verbs only) ---
    if verb_has_body(verb) {
        let schema = op.request_schema.clone().unwrap_or_else(|| {
            let label = op.operation_id.as_deref().unwrap_or(verb);
            warnings.push(format!(
                "operation '{label}' ({verb} {full_path}) — body verb has no request_schema, using weak stub (type: object)"
            ));
            json!({ "type": "object" })
        });
        operation.insert(
            "requestBody".to_string(),
            json!({
                "content": {
                    op.consumes.as_str(): { "schema": schema }
                }
            }),
        );
    }

    Value::Object(operation)
}

fn default_response_description(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Response",
    }
}

/// M3: insert declared response headers into a response object map when present.
/// Shared between the 204 and non-204 response branches to keep header handling DRY.
fn insert_response_headers(resp: &mut Map<String, Value>, op: &RouteDslRestOperation) {
    if let Some(r) = &op.response
        && !r.headers.is_empty()
    {
        resp.insert("headers".to_string(), json!(r.headers));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route_ast::RouteDslRest;
    use std::collections::BTreeMap;

    fn make_rest(path: &str, ops: &[(&str, RouteDslRestOperation)]) -> RouteDslRest {
        let operations = ops
            .iter()
            .map(|(verb, op)| {
                let mut op = op.clone();
                op.method = verb.to_string();
                op
            })
            .collect();
        RouteDslRest {
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: path.to_string(),
            operations,
        }
    }

    fn make_op(operation_id: &str) -> RouteDslRestOperation {
        RouteDslRestOperation {
            method: "GET".to_string(),
            path: "/".to_string(),
            operation_id: Some(operation_id.to_string()),
            to: Some(format!("direct:{operation_id}")),
            steps: vec![],
            consumes: "application/json".to_string(),
            produces: "application/json".to_string(),
            success_status: None,
            request_schema: None,
            response: None,
            description: None,
            parameters: BTreeMap::new(),
        }
    }

    #[test]
    fn generates_valid_openapi_skeleton() {
        let result = generate_openapi(&[], "Test API", "2.0.0");
        assert_eq!(result.document["openapi"], "3.0.3");
        assert_eq!(result.document["info"]["title"], "Test API");
        assert_eq!(result.document["info"]["version"], "2.0.0");
        assert!(result.document["paths"].as_object().unwrap().is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn maps_get_operation_to_paths() {
        let rest = make_rest("/api/users", &[("get", make_op("listUsers"))]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let paths = &result.document["paths"];
        assert_eq!(paths["/api/users"]["get"]["operationId"], "listUsers");
    }

    #[test]
    fn merges_verbs_on_same_path() {
        let rest = make_rest(
            "/api/users",
            &[
                ("get", make_op("listUsers")),
                ("post", make_op("createUser")),
            ],
        );
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let path_entry = &result.document["paths"]["/api/users"];
        assert!(path_entry.get("get").is_some());
        assert!(path_entry.get("post").is_some());
    }

    #[test]
    fn extracts_path_params_from_template() {
        let mut op = make_op("getUser");
        op.path = "/{id}".to_string();
        let rest = make_rest("/api/users", &[("get", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let params = &result.document["paths"]["/api/users/{id}"]["get"]["parameters"];
        assert_eq!(params[0]["name"], "id");
        assert_eq!(params[0]["in"], "path");
        assert_eq!(params[0]["required"], true);
        assert_eq!(params[0]["schema"]["type"], "string");
    }

    #[test]
    fn default_operation_id_when_missing() {
        let mut op = make_op("x");
        op.operation_id = None;
        let rest = make_rest("/api/items", &[("get", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let op_id = &result.document["paths"]["/api/items"]["get"]["operationId"];
        assert!(op_id.as_str().unwrap().contains("get"));
    }

    // --- Schema tests (will pass after full implementation above) ---

    #[test]
    fn body_verb_gets_request_body_with_weak_stub_and_warning() {
        let rest = make_rest("/api/users", &[("post", make_op("createUser"))]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let post = &result.document["paths"]["/api/users"]["post"];
        assert!(post.get("requestBody").is_some());
        assert_eq!(
            post["requestBody"]["content"]["application/json"]["schema"]["type"],
            "object"
        );
        // Should have 2 warnings: no request_schema + no response schema
        assert!(result.warnings.len() >= 2);
    }

    #[test]
    fn get_with_no_schema_uses_weak_stub_and_warns() {
        let rest = make_rest("/api/users", &[("get", make_op("listUsers"))]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let resp = &result.document["paths"]["/api/users"]["get"]["responses"]["200"];
        assert_eq!(
            resp["content"]["application/json"]["schema"]["type"],
            "object"
        );
        assert!(result.warnings.iter().any(|w| w.contains("listUsers")));
    }

    #[test]
    fn delete_204_has_no_content() {
        let mut op = make_op("deleteUser");
        op.success_status = Some(204);
        op.path = "/{id}".to_string();
        let rest = make_rest("/api/users", &[("delete", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let resp = &result.document["paths"]["/api/users/{id}"]["delete"]["responses"]["204"];
        assert!(resp.get("content").is_none());
        assert_eq!(resp["description"], "No Content");
    }

    #[test]
    fn provided_request_schema_used_in_body() {
        let mut op = make_op("createUser");
        op.request_schema = Some(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        }));
        let rest = make_rest("/api/users", &[("post", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let schema = &result.document["paths"]["/api/users"]["post"]["requestBody"]["content"]["application/json"]
            ["schema"];
        assert_eq!(schema["properties"]["name"]["type"], "string");
        // No request-schema warning for this op
        assert!(!result.warnings.iter().any(|w| w.contains("request_schema")));
    }

    #[test]
    fn provided_response_schema_used() {
        let mut op = make_op("listUsers");
        op.response = Some(crate::route_ast::RouteDslRestResponse {
            description: Some("A list of users".to_string()),
            schema: Some(json!({
                "type": "array",
                "items": { "type": "object" }
            })),
            headers: BTreeMap::new(),
        });
        let rest = make_rest("/api/users", &[("get", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let resp = &result.document["paths"]["/api/users"]["get"]["responses"]["200"];
        assert_eq!(resp["description"], "A list of users");
        assert_eq!(
            resp["content"]["application/json"]["schema"]["type"],
            "array"
        );
        // No response-schema warning for this op
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| w.contains("response schema"))
        );
    }

    #[test]
    fn post_defaults_to_201() {
        let mut op = make_op("createUser");
        op.request_schema = Some(json!({ "type": "object" }));
        op.response = Some(crate::route_ast::RouteDslRestResponse {
            description: None,
            schema: Some(json!({ "type": "object" })),
            headers: BTreeMap::new(),
        });
        let rest = make_rest("/api/users", &[("post", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let responses = &result.document["paths"]["/api/users"]["post"]["responses"];
        assert!(responses.get("201").is_some());
        assert!(responses.get("200").is_none());
    }

    #[test]
    fn description_included_when_provided() {
        let mut op = make_op("listUsers");
        op.description = Some("Returns all users".to_string());
        let rest = make_rest("/api/users", &[("get", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        assert_eq!(
            result.document["paths"]["/api/users"]["get"]["description"],
            "Returns all users"
        );
    }

    #[test]
    fn duplicate_path_verb_warns() {
        // I1: two blocks defining GET /api/users → warning
        let rest1 = make_rest("/api/users", &[("get", make_op("listUsers"))]);
        let rest2 = make_rest("/api/users", &[("get", make_op("listUsersAlt"))]);
        let result = generate_openapi(&[rest1, rest2], "API", "1.0.0");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("duplicate operation")),
            "expected duplicate warning, got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn declared_param_overrides_inferred_type() {
        // I2: op.parameters["id"] overrides inferred string type
        let mut op = make_op("getUser");
        op.path = "/{id}".to_string();
        op.parameters.insert(
            "id".to_string(),
            json!({ "type": "integer", "format": "int64" }),
        );
        let rest = make_rest("/api/users", &[("get", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let params = &result.document["paths"]["/api/users/{id}"]["get"]["parameters"];
        assert_eq!(params[0]["schema"]["type"], "integer");
        assert_eq!(params[0]["schema"]["format"], "int64");
    }

    #[test]
    fn declared_additional_param_becomes_query() {
        // I2: non-path param in op.parameters → query param
        let mut op = make_op("listUsers");
        op.parameters
            .insert("limit".to_string(), json!({ "type": "integer" }));
        let rest = make_rest("/api/users", &[("get", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let params = result.document["paths"]["/api/users"]["get"]["parameters"]
            .as_array()
            .unwrap(); // allow-unwrap
        let query_param = params
            .iter()
            .find(|p| p["name"] == "limit")
            .expect("should have limit param"); // allow-unwrap
        assert_eq!(query_param["in"], "query");
        assert_eq!(query_param["schema"]["type"], "integer");
    }

    #[test]
    fn default_operation_id_trims_leading_slash() {
        // M1: /api/users → get_api_users (not get__api_users)
        let mut op = make_op("x");
        op.operation_id = None;
        let rest = make_rest("/api/users", &[("get", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let op_id = result.document["paths"]["/api/users"]["get"]["operationId"]
            .as_str()
            .unwrap(); // allow-unwrap
        assert!(
            !op_id.starts_with("_"),
            "should not start with underscore: {op_id}"
        );
    }

    fn make_rest_with_host(
        host: &str,
        port: u16,
        path: &str,
        ops: &[(&str, RouteDslRestOperation)],
    ) -> RouteDslRest {
        let operations = ops
            .iter()
            .map(|(verb, op)| {
                let mut op = op.clone();
                op.method = verb.to_string();
                op
            })
            .collect();
        RouteDslRest {
            host: host.to_string(),
            port,
            path: path.to_string(),
            operations,
        }
    }

    #[test]
    fn servers_dedup_same_host_port() {
        // I1: two rest blocks with identical (host, port) → one server entry
        let rest1 = make_rest_with_host("0.0.0.0", 8080, "/api/a", &[("get", make_op("a"))]);
        let rest2 = make_rest_with_host("0.0.0.0", 8080, "/api/b", &[("get", make_op("b"))]);
        let result = generate_openapi(&[rest1, rest2], "API", "1.0.0");
        let servers = result.document["servers"].as_array().unwrap(); // allow-unwrap
        assert_eq!(servers.len(), 1, "expected dedup to single server");
        assert_eq!(servers[0]["url"], "http://0.0.0.0:8080");
    }

    #[test]
    fn servers_distinct_when_host_or_port_differs() {
        // I1: two rest blocks with different (host, port) → two server entries
        let rest1 = make_rest_with_host("0.0.0.0", 8080, "/api/a", &[("get", make_op("a"))]);
        let rest2 = make_rest_with_host("127.0.0.1", 9090, "/api/b", &[("get", make_op("b"))]);
        let result = generate_openapi(&[rest1, rest2], "API", "1.0.0");
        let servers = result.document["servers"].as_array().unwrap(); // allow-unwrap
        assert_eq!(servers.len(), 2);
        let urls: Vec<&str> = servers
            .iter()
            .map(|s| s["url"].as_str().unwrap()) // allow-unwrap
            .collect();
        assert!(urls.contains(&"http://0.0.0.0:8080"));
        assert!(urls.contains(&"http://127.0.0.1:9090"));
    }

    #[test]
    fn response_headers_included_for_200() {
        // I2: response.headers populated → ["headers"] present on 200 response
        let mut op = make_op("listUsers");
        op.response = Some(crate::route_ast::RouteDslRestResponse {
            description: None,
            schema: Some(json!({ "type": "array" })),
            headers: BTreeMap::from([(
                "X-Total-Count".to_string(),
                json!({ "schema": { "type": "integer" } }),
            )]),
        });
        let rest = make_rest("/api/users", &[("get", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let resp = &result.document["paths"]["/api/users"]["get"]["responses"]["200"];
        assert!(resp.get("headers").is_some(), "expected headers key");
        assert_eq!(
            resp["headers"]["X-Total-Count"]["schema"]["type"],
            "integer"
        );
    }

    #[test]
    fn response_headers_included_for_204() {
        // I2: response.headers populated on 204 path → still emitted (no content key)
        let mut op = make_op("deleteUser");
        op.path = "/{id}".to_string();
        op.success_status = Some(204);
        op.response = Some(crate::route_ast::RouteDslRestResponse {
            description: None,
            schema: None,
            headers: BTreeMap::from([(
                "X-Request-Id".to_string(),
                json!({ "schema": { "type": "string" } }),
            )]),
        });
        let rest = make_rest("/api/users", &[("delete", op)]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        let resp = &result.document["paths"]["/api/users/{id}"]["delete"]["responses"]["204"];
        assert!(resp.get("headers").is_some(), "expected headers key on 204");
        assert_eq!(resp["headers"]["X-Request-Id"]["schema"]["type"], "string");
        assert!(resp.get("content").is_none(), "204 must not have content");
    }

    #[test]
    fn unknown_verb_warns_but_does_not_reject() {
        // I3: unknown verb → warning, but operation still emitted (weak-stub philosophy)
        let rest = make_rest("/api/foo", &[("xyz", make_op("fooOp"))]);
        let result = generate_openapi(&[rest], "API", "1.0.0");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("unknown HTTP verb 'xyz'")),
            "expected unknown-verb warning, got: {:?}",
            result.warnings
        );
        // Operation is still emitted
        assert!(result.document["paths"]["/api/foo"].get("xyz").is_some());
    }

    #[test]
    fn duplicate_operation_id_warns() {
        // M2: same explicit operationId across distinct operations → warning
        let rest1 = make_rest("/api/a", &[("get", make_op("sameId"))]);
        let rest2 = make_rest("/api/b", &[("get", make_op("sameId"))]);
        let result = generate_openapi(&[rest1, rest2], "API", "1.0.0");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("duplicate operationId 'sameId'")),
            "expected duplicate operationId warning, got: {:?}",
            result.warnings
        );
    }

    // ── Within-block detection (rc-y1xa Task 2) ──

    #[test]
    fn openapi_two_gets_distinct_paths_produce_distinct_path_entries() {
        // Provide response schemas to avoid weak-stub warnings.
        let op_a = RouteDslRestOperation {
            path: "/health".to_string(),
            response: Some(crate::route_ast::RouteDslRestResponse {
                description: None,
                schema: Some(json!({"type": "object"})),
                headers: BTreeMap::new(),
            }),
            ..make_op("health")
        };
        let op_b = RouteDslRestOperation {
            path: "/conteos".to_string(),
            response: Some(crate::route_ast::RouteDslRestResponse {
                description: None,
                schema: Some(json!({"type": "object"})),
                headers: BTreeMap::new(),
            }),
            ..make_op("conteos")
        };
        let rest = make_rest("/api", &[("GET", op_a), ("GET", op_b)]);
        let result = generate_openapi(&[rest], "t", "1.0.0");
        let paths = result.document["paths"].as_object().unwrap();
        assert!(paths.contains_key("/api/health"));
        assert!(paths.contains_key("/api/conteos"));
        assert!(
            result.warnings.is_empty(),
            "unexpected warnings: {:?}",
            result.warnings
        );
    }

    #[test]
    fn openapi_within_block_duplicate_path_verb_warns_last_wins() {
        let rest = make_rest(
            "/api",
            &[
                (
                    "GET",
                    RouteDslRestOperation {
                        path: "/x".to_string(),
                        ..make_op("first")
                    },
                ),
                (
                    "GET",
                    RouteDslRestOperation {
                        path: "/x".to_string(),
                        ..make_op("second")
                    },
                ),
            ],
        );
        let result = generate_openapi(&[rest], "t", "1.0.0");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("duplicate operation")),
            "expected duplicate-operation warning, got: {:?}",
            result.warnings
        );
    }
}
