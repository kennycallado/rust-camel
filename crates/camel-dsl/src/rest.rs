use camel_api::CamelError;

use crate::route_ast::{
    MarshalStep, RouteDslRest, RouteDslRestOperation, RouteDslRoute, RouteDslStep, SetHeaderData,
    SetHeaderStep, ToStep, UnmarshalStep,
};

/// Lower a REST resource into N RouteDslRoute entries (one per verb+path).
pub fn lower_rest_to_routes(rest: &RouteDslRest) -> Result<Vec<RouteDslRoute>, CamelError> {
    validate_rest_block(rest)?;
    let mut routes = Vec::with_capacity(rest.operations.len());

    for (verb, op) in &rest.operations {
        let route = lower_operation(rest, verb, op)?;
        routes.push(route);
    }

    Ok(routes)
}

/// Lower ALL REST blocks in a document into route entries, enforcing the
/// cross-block validation rules the spec mandates (§6.3 + §7.2, review C3):
///
/// - **Duplicate `(host, port, method, full_path)` tuples** — two operations
///   claiming the same listener, verb, and full path are rejected. The key is
///   scoped per listener: the same `(method, path)` on two different
///   `host:port` combinations are independent servers and NOT a duplicate
///   (spec §6.3). (Within a single `rest:` block this is impossible:
///   `operations` is a `BTreeMap` keyed by verb, so each verb appears at most
///   once. The check catches cross-block collisions on the same listener.)
/// - **Ambiguous templates** — on the SAME listener, two templates of the SAME
///   method, SAME segment count, SAME specificity (literal count), that could
///   BOTH match some request are rejected at compile time. The canonical case
///   is `/users/{id}` vs `/users/{name}`.
///
/// Different verbs on the same path (the normal REST `GET`/`DELETE`/…
/// coexistence) are NOT ambiguous — ambiguity is per-method.
pub fn lower_all_rest_to_routes(
    rest_blocks: &[RouteDslRest],
) -> Result<Vec<RouteDslRoute>, CamelError> {
    let mut routes = Vec::new();
    // `(host, port, method_upper, segments)` accumulator for duplicate +
    // ambiguity checks. Detection is scoped per listener (host, port):
    // different host:port combinations are independent servers, so the same
    // (method, path) on two different listeners is NOT a duplicate. Only the
    // same (host, port, method, path) collides (spec §6.3).
    let mut registered: Vec<(String, u16, String, Vec<PathSegment>)> = Vec::new();

    for rest in rest_blocks {
        validate_rest_block(rest)?;
        for (verb, op) in &rest.operations {
            let route = lower_operation(rest, verb, op)?;
            let verb_upper = verb.to_uppercase();
            let full_path = build_full_path(&rest.path, &op.path);
            let segs = parse_path_template(&full_path);
            let op_label = op.operation_id.as_deref().unwrap_or(verb);

            // Duplicate (host, port, method, path) tuple on the SAME listener?
            if registered.iter().any(|(h, p, m, s)| {
                h == &rest.host && *p == rest.port && m == &verb_upper && *s == segs
            }) {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{op_label}': duplicate (method, path) tuple \
                     {verb_upper} {full_path} on listener {}:{} — two operations \
                     cannot claim the same listener, verb, and full path (spec §6.3)",
                    rest.host, rest.port
                )));
            }
            // Ambiguous template (same listener, same method, overlapping
            // shape, equal specificity)?
            for (h, p, m, s) in &registered {
                if h == &rest.host
                    && *p == rest.port
                    && m == &verb_upper
                    && templates_ambiguous(s, &segs)
                {
                    return Err(CamelError::RouteError(format!(
                        "rest operation '{op_label}': path '{full_path}' is ambiguous \
                         with an existing template of equal specificity for method \
                         {verb_upper} on listener {}:{} (spec §7.2) — disambiguate by \
                         making one path more specific",
                        rest.host, rest.port
                    )));
                }
            }
            registered.push((rest.host.clone(), rest.port, verb_upper, segs));
            routes.push(route);
        }
    }

    Ok(routes)
}

/// Expand REST blocks into route entries and append them to `routes`.
/// Shared by the YAML and JSON parsers (review I2). Performs the cross-block
/// validation in [`lower_all_rest_to_routes`]. No-op when there are no REST
/// blocks.
pub fn expand_rest_into(
    routes: &mut Vec<RouteDslRoute>,
    rest_blocks: &[RouteDslRest],
) -> Result<(), CamelError> {
    if rest_blocks.is_empty() {
        return Ok(());
    }
    let lowered = lower_all_rest_to_routes(rest_blocks)?;
    routes.extend(lowered);
    Ok(())
}

/// Reject duplicate route ids across the full route set (including
/// REST-expanded entries). Shared by the YAML and JSON parsers so JSON
/// authoring gets the same guarantee (review I2).
pub fn check_duplicate_route_ids(routes: &[RouteDslRoute]) -> Result<(), CamelError> {
    let mut seen = std::collections::HashSet::with_capacity(routes.len());
    for route in routes {
        if !seen.insert(&route.id) {
            return Err(CamelError::RouteError(format!(
                "duplicate route id '{}' after REST expansion",
                route.id
            )));
        }
    }
    Ok(())
}

/// Validate a REST block's required structure (spec §6.3): `path` must be
/// non-empty and `operations` must declare at least one verb. A block with an
/// empty path or no operations would otherwise silently produce zero routes.
fn validate_rest_block(rest: &RouteDslRest) -> Result<(), CamelError> {
    if rest.path.trim().is_empty() {
        return Err(CamelError::RouteError(format!(
            "rest block on listener {}:{}: 'path' is required and must be non-empty \
             (spec §6.3) — an empty base path is not allowed",
            rest.host, rest.port
        )));
    }
    if rest.operations.is_empty() {
        return Err(CamelError::RouteError(format!(
            "rest block {}:{}{}: 'operations' must declare at least one verb \
             (spec §6.3) — a REST block with no operations produces no routes",
            rest.host, rest.port, rest.path
        )));
    }
    Ok(())
}

/// Validate a path template: reject empty `{}` param segments and duplicate
/// param names within one path. Uses the path-template helpers so they earn
/// their keep in production (review I1) rather than existing only for tests.
fn validate_path_template(full_path: &str, op_label: &str) -> Result<(), CamelError> {
    let segs = parse_path_template(full_path);
    for seg in &segs {
        if let PathSegment::Param(name) = seg
            && name.is_empty()
        {
            return Err(CamelError::RouteError(format!(
                "rest operation '{op_label}': path '{full_path}' contains an empty \
                 parameter '{{}}' — parameter names are required"
            )));
        }
    }
    let param_names = extract_param_names(&segs);
    let mut seen = std::collections::HashSet::new();
    for name in &param_names {
        if !seen.insert(name.clone()) {
            return Err(CamelError::RouteError(format!(
                "rest operation '{op_label}': path '{full_path}' declares parameter \
                 '{name}' more than once — path parameter names must be unique"
            )));
        }
    }
    Ok(())
}

/// Two templates are ambiguous iff they could BOTH match at least one
/// request with equal specificity: same length, same literal count, and at
/// every position the segments are compatible (both params, both equal
/// literals, or one literal + one param). Differing literals at any position
/// mean no single request can match both, so they are not ambiguous.
/// Used by [`lower_all_rest_to_routes`] (review C3 + I1).
fn templates_ambiguous(a: &[PathSegment], b: &[PathSegment]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    if template_specificity(a) != template_specificity(b) {
        return false;
    }
    for (sa, sb) in a.iter().zip(b.iter()) {
        if let (PathSegment::Literal(x), PathSegment::Literal(y)) = (sa, sb)
            && x != y
        {
            return false;
        }
    }
    true
}

fn lower_operation(
    rest: &RouteDslRest,
    verb: &str,
    op: &RouteDslRestOperation,
) -> Result<RouteDslRoute, CamelError> {
    // Validate the HTTP verb key against the closed set (spec §6.2/§6.3,
    // review I5). Comparison is case-insensitive so an uppercase YAML key
    // like `GET:` is accepted; `verb_lc` feeds the lowercase-keyed helpers
    // below, `verb.to_uppercase()` feeds the from-URI.
    const KNOWN_VERBS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];
    let verb_lc = verb.to_lowercase();
    if !KNOWN_VERBS.contains(&verb_lc.as_str()) {
        return Err(CamelError::RouteError(format!(
            "rest operation '{}': unknown HTTP verb '{}' (expected one of: {})",
            op.operation_id.as_deref().unwrap_or(verb),
            verb,
            KNOWN_VERBS.join(", "),
        )));
    }

    // Validate: to XOR steps
    let has_to = op.to.is_some();
    let has_steps = !op.steps.is_empty();
    if has_to && has_steps {
        return Err(CamelError::RouteError(format!(
            "rest operation '{}' cannot have both 'to' and 'steps'",
            op.operation_id.as_deref().unwrap_or(verb)
        )));
    }
    if !has_to && !has_steps {
        return Err(CamelError::RouteError(format!(
            "rest operation '{}' must have 'to' or 'steps'",
            op.operation_id.as_deref().unwrap_or(verb)
        )));
    }

    // Validate: JSON only in v1
    if op.consumes != "application/json" {
        return Err(CamelError::RouteError(format!(
            "rest operation '{}': v1 supports only consumes=application/json (got '{}')",
            op.operation_id.as_deref().unwrap_or(verb),
            op.consumes
        )));
    }
    if op.produces != "application/json" {
        return Err(CamelError::RouteError(format!(
            "rest operation '{}': v1 supports only produces=application/json (got '{}')",
            op.operation_id.as_deref().unwrap_or(verb),
            op.produces
        )));
    }

    let route_id = op.operation_id.clone().unwrap_or_else(|| {
        // Sanitize: replace / and {} for valid route IDs (reviewer fix M1)
        format!("{}_{}{}", verb, rest.path.trim_start_matches('/'), op.path)
            .replace('/', "-")
            .replace(['{', '}'], "")
    });

    let full_path = build_full_path(&rest.path, &op.path);

    // Validate the path template itself (review I1 wires the path-template
    // helpers into real production use): no empty `{}` segments and no
    // duplicate param names within one path.
    validate_path_template(&full_path, op.operation_id.as_deref().unwrap_or(verb))?;

    // Path goes in the URL path segment, NOT a query param (reviewer fix C1).
    // The http: component dispatches by URL path: dispatch_handler does
    // api_routes.get(&request_path). httpMethod is the query param for method routing.
    let from = format!(
        "http://{}:{}{}?httpMethod={}",
        rest.host,
        rest.port,
        full_path,
        verb.to_uppercase()
    );

    // Default status injection — "default-if-absent" (reviewer fix I1).
    //
    // The status is injected as the FIRST step so it sets the initial value
    // of `CamelHttpResponseCode`; any later user step that sets the same
    // header overwrites it (default-first, user-overrides-after).
    //
    // v1 simplification / fragility (review I3): this relies on the HTTP
    // reply finaliser OVERRIDING the status on error paths — a pipeline
    // error (e.g. malformed-JSON `Unmarshal` failure) routes through the
    // error disposition and the finaliser returns 401/403/500 regardless of
    // the default-status header. That holds today. A fully spec-compliant
    // form would be a `set_header_if_absent` step variant that sets the
    // default ONLY when no prior step populated the header — tracked as a
    // future improvement (spec plan note E5). Until then, keep the default
    // first so user steps can still override it on the success path.
    let default_status = op
        .success_status
        .unwrap_or_else(|| default_status_for_verb(&verb_lc));

    let mut steps: Vec<RouteDslStep> = vec![RouteDslStep::SetHeader(SetHeaderStep {
        set_header: SetHeaderData {
            key: "CamelHttpResponseCode".to_string(),
            value: Some(serde_json::Value::Number(default_status.into())),
            language: None,
            source: None,
            simple: None,
            rhai: None,
            jsonpath: None,
            xpath: None,
        },
    })];

    // 1. Request binding: unmarshal JSON body (only if verb has a body)
    if verb_has_body(&verb_lc) {
        steps.push(RouteDslStep::Unmarshal(UnmarshalStep {
            unmarshal: "json".to_string(),
            // When `request_schema` is set, the step compiler wraps the
            // JSON unmarshal with a JsonSchemaValidateService; mismatches
            // surface as CamelError::ValidationError → 400 Bad Request
            // (see camel-component-http `pipeline_error_to_reply`).
            schema: op.request_schema.clone(),
        }));
    }

    // 2. User steps (either `to` shorthand or explicit `steps`)
    if let Some(ref to_uri) = op.to {
        steps.push(RouteDslStep::To(ToStep { to: to_uri.clone() }));
    } else {
        steps.extend(op.steps.iter().cloned());
    }

    // 3. Response binding: marshal JSON. The JSON data format serialises the
    // body to Body::Text (the JSON wire form); the HTTP reply finaliser would
    // infer that as text/plain, so we set Content-Type explicitly below.
    steps.push(RouteDslStep::Marshal(MarshalStep {
        marshal: "json".to_string(),
    }));

    // 4. Response Content-Type: the finaliser prioritises a user-supplied
    // Content-Type header over its body-type inference (Body::Text →
    // text/plain). REST JSON responses must be application/json, so set it
    // here as the last step (spec §8.1). An earlier user step that set a
    // different Content-Type is intentionally overridden — v1 is JSON-only.
    steps.push(RouteDslStep::SetHeader(SetHeaderStep {
        set_header: SetHeaderData {
            key: "Content-Type".to_string(),
            value: Some(serde_json::Value::String("application/json".to_string())),
            language: None,
            source: None,
            simple: None,
            rhai: None,
            jsonpath: None,
            xpath: None,
        },
    }));

    Ok(RouteDslRoute {
        id: route_id,
        from,
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
    })
}

pub fn build_full_path(base: &str, suffix: &str) -> String {
    let base = base.trim_end_matches('/');
    let suffix = suffix.trim_start_matches('/');
    if suffix.is_empty() {
        base.to_string()
    } else {
        format!("{}/{}", base, suffix)
    }
}

pub fn default_status_for_verb(verb: &str) -> u16 {
    match verb {
        "post" => 201,
        "delete" => 204,
        _ => 200,
    }
}

pub fn verb_has_body(verb: &str) -> bool {
    matches!(verb, "post" | "put" | "patch")
}

// ---------------------------------------------------------------------------
// Path template parser (Task 4)
// ---------------------------------------------------------------------------

/// A segment of a REST path template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    Literal(String),
    Param(String),
}

/// Parse a path like `/users/{id}/posts` into segments.
pub fn parse_path_template(path: &str) -> Vec<PathSegment> {
    path.trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                PathSegment::Param(seg[1..seg.len() - 1].to_string())
            } else {
                PathSegment::Literal(seg.to_string())
            }
        })
        .collect()
}

/// Count literal (non-param) segments for specificity comparison.
/// More literal segments = more specific.
pub fn template_specificity(segments: &[PathSegment]) -> usize {
    segments
        .iter()
        .filter(|s| matches!(s, PathSegment::Literal(_)))
        .count()
}

/// Extract param names from a template path.
pub fn extract_param_names(segments: &[PathSegment]) -> Vec<String> {
    segments
        .iter()
        .filter_map(|s| match s {
            PathSegment::Param(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn make_rest(op_id: &str, verb: &str, path: &str, to: &str) -> RouteDslRest {
        let mut ops = BTreeMap::new();
        ops.insert(
            verb.to_string(),
            RouteDslRestOperation {
                path: path.to_string(),
                operation_id: Some(op_id.to_string()),
                to: Some(to.to_string()),
                steps: vec![],
                consumes: "application/json".to_string(),
                produces: "application/json".to_string(),
                success_status: None,
                request_schema: None,
                response: None,
                description: None,
                parameters: BTreeMap::new(),
            },
        );
        RouteDslRest {
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: "/users".to_string(),
            operations: ops,
        }
    }

    #[test]
    fn lower_single_get_operation() {
        let rest = make_rest("getUser", "get", "/{id}", "bean:userService");
        let routes = lower_rest_to_routes(&rest).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].id, "getUser");
        assert!(routes[0].from.contains("httpMethod=GET"));
        assert!(routes[0].from.contains("/users/{id}"));
        assert!(routes[0].from.contains("0.0.0.0:8080"));
    }

    #[test]
    fn lower_post_has_201_default() {
        let rest = make_rest("createUser", "post", "/", "bean:createUser");
        let routes = lower_rest_to_routes(&rest).unwrap();
        // The default status (201 for POST) should be injected as the FIRST step
        // (default-first, user-overrides-after semantics — reviewer fix I1)
        let first = routes[0].steps.first().unwrap();
        match first {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.key, "CamelHttpResponseCode");
                assert_eq!(h.set_header.value, Some(serde_json::json!(201)));
            }
            _ => panic!("expected SetHeader as first step for default status"),
        }
    }

    #[test]
    fn lower_get_defaults_to_200() {
        let rest = make_rest("getUser", "get", "/{id}", "bean:userService");
        let routes = lower_rest_to_routes(&rest).unwrap();
        let first = routes[0].steps.first().unwrap();
        match first {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.value, Some(serde_json::json!(200)));
            }
            _ => panic!("expected SetHeader first"),
        }
    }

    #[test]
    fn lower_delete_defaults_to_204() {
        let rest = make_rest("deleteUser", "delete", "/{id}", "bean:deleteUser");
        let routes = lower_rest_to_routes(&rest).unwrap();
        let first = routes[0].steps.first().unwrap();
        match first {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.value, Some(serde_json::json!(204)));
            }
            _ => panic!("expected SetHeader first"),
        }
    }

    #[test]
    fn lower_rejects_to_and_steps_both() {
        let mut rest = make_rest("op", "get", "/", "bean:svc");
        rest.operations.get_mut("get").unwrap().steps =
            vec![RouteDslStep::To(crate::route_ast::ToStep {
                to: "bean:other".into(),
            })];
        let result = lower_rest_to_routes(&rest);
        assert!(result.is_err());
    }

    #[test]
    fn lower_rejects_non_json_consumes() {
        let mut rest = make_rest("op", "get", "/", "bean:svc");
        rest.operations.get_mut("get").unwrap().consumes = "application/xml".to_string();
        let result = lower_rest_to_routes(&rest);
        assert!(result.is_err());
    }

    // ── Path template parser tests (Task 4) ──

    #[test]
    fn parse_path_template_segments() {
        let segments = parse_path_template("/users/{id}/posts/{postId}");
        assert_eq!(segments.len(), 4);
        assert!(matches!(segments[0], PathSegment::Literal(ref s) if s == "users"));
        assert!(matches!(segments[1], PathSegment::Param(ref s) if s == "id"));
        assert!(matches!(segments[2], PathSegment::Literal(ref s) if s == "posts"));
        assert!(matches!(segments[3], PathSegment::Param(ref s) if s == "postId"));
    }

    #[test]
    fn path_template_specificity_count() {
        let s1 = parse_path_template("/users/me");
        let s2 = parse_path_template("/users/{id}");
        assert!(template_specificity(&s1) > template_specificity(&s2));
    }

    #[test]
    fn parse_path_template_empty() {
        let segments = parse_path_template("");
        assert!(segments.is_empty());
    }

    #[test]
    fn parse_path_template_root() {
        let segments = parse_path_template("/");
        assert!(segments.is_empty());
    }

    #[test]
    fn extract_param_names_from_template() {
        let segments = parse_path_template("/users/{id}/posts/{postId}");
        let params = extract_param_names(&segments);
        assert_eq!(params, vec!["id", "postId"]);
    }

    #[test]
    fn lower_rejects_duplicate_operation_ids() {
        // Two REST resources with same operation_id (reviewer fix M2)
        let rest1 = make_rest("getUser", "get", "/{id}", "bean:svc");
        let rest2 = make_rest("getUser", "get", "/{id}", "bean:other");
        // The shared check_duplicate_route_ids helper (now used by both the
        // YAML and JSON parsers — review I2) enforces uniqueness across the
        // full lowered route set.
        let mut all = lower_rest_to_routes(&rest1).unwrap();
        all.extend(lower_rest_to_routes(&rest2).unwrap());
        assert!(check_duplicate_route_ids(&all).is_err());
    }

    // ── Cross-block validation (review C3) ──

    fn make_rest_with_base(
        op_id: &str,
        verb: &str,
        base: &str,
        path: &str,
        to: &str,
    ) -> RouteDslRest {
        let mut ops = BTreeMap::new();
        ops.insert(
            verb.to_string(),
            RouteDslRestOperation {
                path: path.to_string(),
                operation_id: Some(op_id.to_string()),
                to: Some(to.to_string()),
                steps: vec![],
                consumes: "application/json".to_string(),
                produces: "application/json".to_string(),
                success_status: None,
                request_schema: None,
                response: None,
                description: None,
                parameters: BTreeMap::new(),
            },
        );
        RouteDslRest {
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: base.to_string(),
            operations: ops,
        }
    }

    #[test]
    fn lower_all_rejects_cross_block_ambiguous_templates() {
        // Two blocks, same verb, same shape + specificity, overlapping →
        // ambiguous (spec §7.2). The lowering must reject at compile time.
        let a = make_rest_with_base("getUserA", "get", "/users", "/{id}", "bean:a");
        let b = make_rest_with_base("getUserB", "get", "/users", "/{name}", "bean:b");
        let err = lower_all_rest_to_routes(&[a, b])
            .err()
            .expect("ambiguous templates across blocks must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("ambiguous"), "got: {msg}");
    }

    #[test]
    fn lower_all_rejects_duplicate_method_path_across_blocks() {
        // Two blocks claiming the same (GET, /users/{id}) → duplicate tuple
        // (spec §6.3).
        let a = make_rest_with_base("getUserA", "get", "/users", "/{id}", "bean:a");
        let b = make_rest_with_base("getUserB", "get", "/users", "/{id}", "bean:b");
        let err = lower_all_rest_to_routes(&[a, b])
            .err()
            .expect("duplicate (method, path) must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("duplicate"), "got: {msg}");
    }

    #[test]
    fn lower_all_allows_different_verbs_same_path() {
        // Different verbs on the same path is the NORMAL REST case and must
        // NOT be flagged as ambiguous (ambiguity is per-method).
        let get = make_rest_with_base("getUser", "get", "/users", "/{id}", "bean:g");
        let delete = make_rest_with_base("deleteUser", "delete", "/users", "/{id}", "bean:d");
        let routes = lower_all_rest_to_routes(&[get, delete]).unwrap();
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn lower_all_allows_different_specificity_overlap() {
        // /users/me (specificity 2) and /users/{id} (specificity 1) overlap
        // but differ in specificity → not ambiguous (more specific wins).
        let me = make_rest_with_base("getMe", "get", "/users", "/me", "bean:me");
        let id = make_rest_with_base("getUser", "get", "/users", "/{id}", "bean:id");
        let routes = lower_all_rest_to_routes(&[me, id]).unwrap();
        assert_eq!(routes.len(), 2);
    }

    fn make_rest_on_listener(
        host: &str,
        port: u16,
        op_id: &str,
        verb: &str,
        base: &str,
        path: &str,
        to: &str,
    ) -> RouteDslRest {
        let mut rest = make_rest_with_base(op_id, verb, base, path, to);
        rest.host = host.to_string();
        rest.port = port;
        rest
    }

    #[test]
    fn lower_all_allows_same_method_path_on_different_port() {
        // Different host:port are independent servers: the same (GET,
        // /users/{id}) on two listeners is NOT a duplicate (spec §6.3).
        let a = make_rest_on_listener(
            "0.0.0.0", 8080, "getUserA", "get", "/users", "/{id}", "bean:a",
        );
        let b = make_rest_on_listener(
            "0.0.0.0", 9090, "getUserB", "get", "/users", "/{id}", "bean:b",
        );
        let routes = lower_all_rest_to_routes(&[a, b]).unwrap();
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn lower_all_allows_same_method_path_on_different_host() {
        // Different host, same port: also an independent listener.
        let a = make_rest_on_listener(
            "0.0.0.0", 8080, "getUserA", "get", "/users", "/{id}", "bean:a",
        );
        let b = make_rest_on_listener(
            "127.0.0.1",
            8080,
            "getUserB",
            "get",
            "/users",
            "/{id}",
            "bean:b",
        );
        let routes = lower_all_rest_to_routes(&[a, b]).unwrap();
        assert_eq!(routes.len(), 2);
    }

    // ── Required structure: path + operations (spec §6.3) ──

    #[test]
    fn lower_rejects_empty_base_path() {
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.path = "".to_string();
        let err = lower_rest_to_routes(&rest)
            .err()
            .expect("empty base path must be rejected");
        assert!(err.to_string().contains("path"), "got: {err}");
    }

    #[test]
    fn lower_rejects_whitespace_only_base_path() {
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.path = "   ".to_string();
        assert!(lower_rest_to_routes(&rest).is_err());
    }

    #[test]
    fn lower_rejects_empty_operations() {
        let rest = RouteDslRest {
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: "/users".to_string(),
            operations: BTreeMap::new(),
        };
        let err = lower_rest_to_routes(&rest)
            .err()
            .expect("empty operations must be rejected");
        assert!(err.to_string().contains("operations"), "got: {err}");
    }

    #[test]
    fn lower_all_rejects_empty_operations_in_second_block() {
        // The per-block validation also fires inside lower_all_rest_to_routes.
        let a = make_rest_with_base("getUser", "get", "/users", "/{id}", "bean:a");
        let mut b = make_rest_with_base("createUser", "post", "/users", "/", "bean:b");
        b.path = "".to_string();
        assert!(lower_all_rest_to_routes(&[a, b]).is_err());
    }

    // ── Response Content-Type (spec §8.1) ──

    #[test]
    fn lower_injects_application_json_content_type() {
        // The JSON marshal produces Body::Text (→ text/plain inference); the
        // lowering must inject Content-Type: application/json as the final
        // step so the HTTP finaliser advertises JSON (spec §8.1).
        let rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        let routes = lower_rest_to_routes(&rest).unwrap();
        let last = routes[0].steps.last().expect("steps must be non-empty");
        match last {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.key, "Content-Type");
                assert_eq!(
                    h.set_header.value,
                    Some(serde_json::json!("application/json"))
                );
            }
            other => panic!("expected SetHeader(Content-Type) last, got {other:?}"),
        }
    }

    #[test]
    fn lower_content_type_is_after_marshal() {
        // The Content-Type header MUST come after marshal (which produces the
        // Body::Text wire form) so it overrides the text/plain inference.
        let rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        let steps = &lower_rest_to_routes(&rest).unwrap()[0].steps;
        let marshal_idx = steps
            .iter()
            .position(|s| matches!(s, RouteDslStep::Marshal(_)))
            .expect("marshal step present");
        let ct_idx = steps
            .iter()
            .position(
                |s| matches!(s, RouteDslStep::SetHeader(h) if h.set_header.key == "Content-Type"),
            )
            .expect("Content-Type step present");
        assert!(ct_idx > marshal_idx, "Content-Type must follow marshal");
    }

    // ── Verb validation + case normalization (review I5) ──

    #[test]
    fn lower_rejects_unknown_verb() {
        let rest = make_rest("op", "geet", "/", "bean:svc");
        let err = lower_rest_to_routes(&rest).err();
        assert!(err.is_some(), "unknown verb must be rejected");
        let msg = format!("{}", err.unwrap());
        assert!(msg.contains("unknown HTTP verb 'geet'"), "got: {msg}");
    }

    #[test]
    fn lower_accepts_uppercase_verb_key() {
        // Case-insensitive verb key; lowercase helpers must still pick the
        // right defaults (POST → 201).
        let rest = make_rest("createUser", "POST", "/", "bean:create");
        let routes = lower_rest_to_routes(&rest).unwrap();
        assert!(routes[0].from.contains("httpMethod=POST"));
        let first = match routes[0].steps.first().unwrap() {
            RouteDslStep::SetHeader(h) => h,
            _ => panic!("expected SetHeader first"),
        };
        assert_eq!(first.set_header.value, Some(serde_json::json!(201)));
    }

    // ── Path-template param validation (review I1) ──

    #[test]
    fn lower_rejects_empty_path_param() {
        let rest = make_rest_with_base("op", "get", "/users", "/{}", "bean:svc");
        let err = lower_rest_to_routes(&rest).err();
        assert!(err.is_some(), "empty {{}} param must be rejected");
    }

    #[test]
    fn lower_rejects_duplicate_path_param() {
        let rest = make_rest_with_base("op", "get", "/users", "/{id}/{id}", "bean:svc");
        let err = lower_rest_to_routes(&rest).err();
        assert!(err.is_some(), "duplicate path param must be rejected");
    }

    // ── Shared helpers used by YAML + JSON parsers (review I2) ──

    #[test]
    fn expand_rest_into_is_noop_for_empty() {
        let mut routes = vec![];
        expand_rest_into(&mut routes, &[]).unwrap();
        assert!(routes.is_empty());
    }

    #[test]
    fn check_duplicate_route_ids_detects_collision() {
        let rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        let mut routes = lower_rest_to_routes(&rest).unwrap();
        // Duplicate the single route → collision.
        let dup = routes[0].clone();
        routes.push(dup);
        assert!(check_duplicate_route_ids(&routes).is_err());
    }

    // ── request_schema → JSON schema validation step (rc-dt8q) ──

    #[test]
    fn rest_lowering_omits_schema_when_absent() {
        // No request_schema → UnmarshalStep.schema is None.
        let rest = make_rest("createUser", "post", "/", "bean:create");
        let routes = lower_rest_to_routes(&rest).unwrap();
        let unmarshal = routes[0]
            .steps
            .iter()
            .find_map(|s| match s {
                RouteDslStep::Unmarshal(u) => Some(u),
                _ => None,
            })
            .expect("POST must inject unmarshal step");
        assert!(unmarshal.schema.is_none());
    }

    #[test]
    fn rest_lowering_includes_schema_when_present() {
        // request_schema → UnmarshalStep.schema carries the schema verbatim.
        let mut rest = make_rest("createUser", "post", "/", "bean:create");
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        });
        rest.operations.get_mut("post").unwrap().request_schema = Some(schema.clone());
        let routes = lower_rest_to_routes(&rest).unwrap();
        let unmarshal = routes[0]
            .steps
            .iter()
            .find_map(|s| match s {
                RouteDslStep::Unmarshal(u) => Some(u),
                _ => None,
            })
            .expect("POST must inject unmarshal step");
        assert_eq!(unmarshal.schema, Some(schema));
    }

    #[test]
    fn rest_lowering_skips_schema_for_verb_without_body() {
        // GET (no body) → no unmarshal step at all, regardless of schema.
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.operations.get_mut("get").unwrap().request_schema =
            Some(serde_json::json!({ "type": "object" }));
        let routes = lower_rest_to_routes(&rest).unwrap();
        let has_unmarshal = routes[0]
            .steps
            .iter()
            .any(|s| matches!(s, RouteDslStep::Unmarshal(_)));
        assert!(!has_unmarshal, "GET must not emit unmarshal");
    }
}
