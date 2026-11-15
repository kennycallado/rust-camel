use std::collections::BTreeMap;
use std::fmt;

use camel_api::CamelError;

use crate::media::{
    is_form_media_type, is_json_media_type, is_valid_media_declaration, is_xml_media_type,
};
use crate::route_ast::{
    ContentNegotiationStep, HttpErrorMappingTable, MarshalStep, RouteDslRest, RouteDslRestBinding,
    RouteDslRestOperation, RouteDslRoute, RouteDslStep, SetHeaderData, SetHeaderStep, ToStep,
    UnmarshalStep,
};

/// Lower ALL REST blocks in a document into route entries, enforcing the
/// cross-block validation rules the spec mandates (§6.3 + §7.2, review C3):
///
/// - **Duplicate `(host, port, method, full_path)` tuples** — two operations
///   claiming the same listener, verb, and full path are rejected. The key is
///   scoped per listener: the same `(method, path)` on two different
///   `host:port` combinations are independent servers and NOT a duplicate
///   (spec §6.3). With `operations` now a `Vec`, within-block collisions are
///   also possible and are caught by this same `registered` check.
/// - **Ambiguous templates** — on the SAME listener, two templates of the SAME
///   method, SAME segment count, SAME `(L, C)` specificity, whose segment
///   languages overlap at EVERY position are rejected at compile time, because
///   some request could match both. The canonical case is `/users/{id}` vs
///   `/users/{name}`; `/ab{x}` vs `/a{y}b` is ambiguous too (both match
///   `/abb`).
///
/// Different verbs on the same path (the normal REST `GET`/`DELETE`/…
/// coexistence) are NOT ambiguous — ambiguity is per-method.
pub fn lower_all_rest_to_routes(
    rest_blocks: &[RouteDslRest],
) -> Result<Vec<RouteDslRoute>, CamelError> {
    let mut routes = Vec::new();
    // Per-listener accumulator for duplicate + ambiguity checks. Detection is
    // scoped per listener (host, port): different host:port combinations are
    // independent servers, so the same (method, path) on two different
    // listeners is NOT a duplicate. Only the same (host, port, method, path)
    // collides (spec §6.3). The first operation's label and full path ride
    // along so the ambiguity diagnostic can name BOTH sides.
    let mut registered: Vec<RegisteredPath> = Vec::new();

    for rest in rest_blocks {
        validate_rest_block(rest)?;
        for op in &rest.operations {
            let route = lower_operation(rest, &op.method, op)?;
            let verb_upper = op.method.to_uppercase();
            let verb_lc = op.method.to_lowercase();
            let full_path = build_full_path(&rest.path, &op.path);
            let op_label = op.operation_id.as_deref().unwrap_or(&verb_lc);
            let segs = parse_path_template(&full_path).map_err(|err| {
                CamelError::RouteError(format!(
                    "rest operation '{op_label}': path '{full_path}': {err}"
                ))
            })?;

            // Duplicate (host, port, method, path) tuple on the SAME listener?
            if registered.iter().any(|other| {
                other.host == rest.host
                    && other.port == rest.port
                    && other.verb == verb_upper
                    && other.segments == segs
            }) {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{op_label}': duplicate (method, path) tuple \
                     {verb_upper} {full_path} on listener {}:{} — two operations \
                     cannot claim the same listener, verb, and full path (spec §6.3)",
                    rest.host, rest.port
                )));
            }
            // Ambiguous template (same listener, same method, overlapping
            // shape, equal (L, C) specificity)?
            for other in &registered {
                if other.host == rest.host
                    && other.port == rest.port
                    && other.verb == verb_upper
                    && templates_ambiguous(&other.segments, &segs)
                {
                    return Err(CamelError::RouteError(format!(
                        "rest operation '{op_label}': path '{full_path}' is ambiguous \
                         with operation '{}' path '{}' of equal specificity for \
                         method {verb_upper}",
                        other.op_label, other.full_path
                    )));
                }
            }
            registered.push(RegisteredPath {
                host: rest.host.clone(),
                port: rest.port,
                verb: verb_upper,
                segments: segs,
                op_label: op_label.to_string(),
                full_path,
            });
            routes.push(route);
        }
    }

    Ok(routes)
}

/// One already-lowered operation, kept for the per-listener duplicate and
/// ambiguity checks. Carrying the first operation's label and full path lets
/// the ambiguity error name both operations.
struct RegisteredPath {
    host: String,
    port: u16,
    verb: String,
    segments: Vec<SegmentPattern>,
    op_label: String,
    full_path: String,
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
    if let Some(table) = &rest.error_mapping {
        let mapping = camel_api::HttpErrorMapping {
            codes: table.codes.clone(),
            kinds: table.kinds.clone(),
        };
        mapping.validate().map_err(|e| {
            CamelError::RouteError(format!(
                "rest block {}:{}{}: {e}",
                rest.host, rest.port, rest.path
            ))
        })?;
    }
    Ok(())
}

/// Convert a rest block's `error_mapping` table into the camel-api mapping the
/// lowered routes carry on their `http_error_mapping` seam field. An absent
/// block table lowers to an EMPTY mapping — rest routes are ALWAYS seam routes
/// (`Some(_)`, review C1) so the business dead-letter bypass and the HTTP
/// finalizer's built-in default kind map apply uniformly. Validation already
/// ran in [`validate_rest_block`]; this conversion keeps the same typed rules.
fn seam_error_mapping(table: Option<&HttpErrorMappingTable>) -> camel_api::HttpErrorMapping {
    match table {
        Some(table) => camel_api::HttpErrorMapping {
            codes: table.codes.clone(),
            kinds: table.kinds.clone(),
        },
        None => camel_api::HttpErrorMapping::new(),
    }
}

/// Validate a path template: reject malformed brace syntax and duplicate
/// param names within one path. Uses the path-template helpers so they earn
/// their keep in production (review I1) rather than existing only for tests.
fn validate_path_template(full_path: &str, op_label: &str) -> Result<(), CamelError> {
    let segs = parse_path_template(full_path).map_err(|err| {
        CamelError::RouteError(match err {
            TemplateParseError::EmptyPlaceholder { .. } => format!(
                "rest operation '{op_label}': path '{full_path}': {err} — parameter names \
                 are required"
            ),
            _ => format!("rest operation '{op_label}': path '{full_path}': {err}"),
        })
    })?;
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

/// Two templates are ambiguous iff some single request could match BOTH with
/// equal specificity: same segment count, same `(L, C)` specificity, and the
/// segment languages intersect at every position. Differing literals that no
/// single path can satisfy mean no such request exists, so they are not
/// ambiguous. Used by [`lower_all_rest_to_routes`] (review C3 + I1).
fn templates_ambiguous(a: &[SegmentPattern], b: &[SegmentPattern]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    if template_specificity(a) != template_specificity(b) {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(sa, sb)| segment_patterns_overlap(sa, sb))
}

/// Decide whether two segment patterns' languages intersect.
///
/// A segment pattern is literal text joined by placeholder holes matching any
/// substring, the empty one included, so its language is regular. The decision
/// is exact: a product-automaton search over `(part index, literal offset)`
/// pairs on both sides. Matching literal characters advance both sides, a
/// literal character consumed against a placeholder is absorbed by the hole,
/// and a placeholder may always end without consuming anything. Free
/// characters are never needed (a hole can match the empty string), so the
/// search space is finite and every accepted path is a common witness.
///
/// This subsumes positional compatibility: `/ab{x}` and `/a{y}b` overlap at
/// `/abb` even though their part lists differ.
pub fn segment_patterns_overlap(a: &SegmentPattern, b: &SegmentPattern) -> bool {
    type State = (usize, usize, usize, usize);

    // `parts` is a public field, so an empty `Lit` — which the parser never
    // emits — can appear anywhere. It adds nothing to the language and must
    // not be mistaken for a placeholder position, so drop it up front.
    let a_parts = non_empty_parts(&a.parts);
    let b_parts = non_empty_parts(&b.parts);
    let mut visited = std::collections::HashSet::new();
    let mut stack: Vec<State> = vec![(0, 0, 0, 0)];

    while let Some((ai, ao, bi, bo)) = stack.pop() {
        if !visited.insert((ai, ao, bi, bo)) {
            continue;
        }
        if ai >= a_parts.len() && bi >= b_parts.len() {
            return true;
        }
        // A placeholder always matches the empty string, so it can end here.
        if ai < a_parts.len() && matches!(a_parts[ai], SegmentPart::Param(_)) {
            stack.push((ai + 1, 0, bi, bo));
        }
        if bi < b_parts.len() && matches!(b_parts[bi], SegmentPart::Param(_)) {
            stack.push((ai, ao, bi + 1, 0));
        }
        match (
            literal_char(&a_parts, ai, ao),
            literal_char(&b_parts, bi, bo),
        ) {
            // Both sides are at literal characters: only a shared character
            // can label the edge.
            (Some(x), Some(y)) if x == y => {
                let (a_next, a_off) = advance(&a_parts, ai, ao);
                let (b_next, b_off) = advance(&b_parts, bi, bo);
                stack.push((a_next, a_off, b_next, b_off));
            }
            // Literal against placeholder: the hole absorbs the character.
            (Some(_), None) if bi < b_parts.len() => {
                let (a_next, a_off) = advance(&a_parts, ai, ao);
                stack.push((a_next, a_off, bi, bo));
            }
            (None, Some(_)) if ai < a_parts.len() => {
                let (b_next, b_off) = advance(&b_parts, bi, bo);
                stack.push((ai, ao, b_next, b_off));
            }
            _ => {}
        }
    }
    false
}

/// The pattern's parts that constrain the language, in order: an empty `Lit`
/// contributes nothing and is dropped.
fn non_empty_parts(parts: &[SegmentPart]) -> Vec<&SegmentPart> {
    parts
        .iter()
        .filter(|part| !matches!(part, SegmentPart::Lit(text) if text.is_empty()))
        .collect()
}

/// The byte at literal offset `off` of part `idx`, or `None` when the position
/// is not inside a literal (a placeholder, or past the end of the parts).
fn literal_char(parts: &[&SegmentPart], idx: usize, off: usize) -> Option<u8> {
    match parts.get(idx).copied() {
        Some(SegmentPart::Lit(text)) => text.as_bytes().get(off).copied(),
        _ => None,
    }
}

/// Consume one literal byte at `(idx, off)`, normalized to the next part when
/// the literal is exhausted.
fn advance(parts: &[&SegmentPart], mut idx: usize, mut off: usize) -> (usize, usize) {
    off += 1;
    while let Some(SegmentPart::Lit(text)) = parts.get(idx).copied() {
        if off < text.len() {
            return (idx, off);
        }
        idx += 1;
        off = 0;
    }
    (idx, off)
}

fn lower_operation(
    rest: &RouteDslRest,
    verb: &str,
    op: &RouteDslRestOperation,
) -> Result<RouteDslRoute, CamelError> {
    // Validate the HTTP method against the closed set (spec §6.2/§6.3,
    // review I5). Comparison is case-insensitive so an uppercase `method`
    // value like `GET` is accepted; `verb_lc` feeds the lowercase-keyed
    // helpers below, `verb.to_uppercase()` feeds the from-URI.
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

    // Validate media declarations and schema usage against the operation's
    // binding mode (Tasks 1.3/1.4). JSON binding — explicit or the implicit
    // default — requires JSON media types for both directions; the error
    // names the `binding: raw` escape hatch so the fix is one declaration
    // away. Raw binding accepts any valid media declaration but forbids the
    // JSON-schema hooks (`request_schema` / `response.schema`): the raw
    // pipeline emits no data-format steps, so there is nothing to attach
    // validation to. Schema checks precede media checks so the more specific
    // "raw cannot carry schemas" diagnosis wins when both are wrong.
    let binding = op.binding.unwrap_or(RouteDslRestBinding::Json);
    let op_label = op.operation_id.as_deref().unwrap_or(verb);
    match binding {
        RouteDslRestBinding::Json => {
            if !is_json_media_type(&op.consumes) {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{}': binding 'json' requires a JSON media type \
                     for 'consumes' (got '{}') — declare 'binding: raw' to use \
                     non-JSON media",
                    op_label,
                    op.consumes.trim()
                )));
            }
            if !is_json_media_type(&op.produces) {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{}': binding 'json' requires a JSON media type \
                     for 'produces' (got '{}') — declare 'binding: raw' to use \
                     non-JSON media",
                    op_label,
                    op.produces.trim()
                )));
            }
        }
        RouteDslRestBinding::Form => {
            if !is_form_media_type(&op.consumes) {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{op_label}': binding 'form' requires \
                     'application/x-www-form-urlencoded' for 'consumes' (got '{}')",
                    op.consumes.trim()
                )));
            }
            if !is_form_media_type(&op.produces) {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{op_label}': binding 'form' requires \
                     'application/x-www-form-urlencoded' for 'produces' (got '{}')",
                    op.produces.trim()
                )));
            }
        }
        RouteDslRestBinding::Xml => {
            if !is_xml_media_type(&op.consumes) {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{op_label}': binding 'xml' requires an XML media \
                     type (application/xml, text/xml, or */*+xml) for 'consumes' \
                     (got '{}')",
                    op.consumes.trim()
                )));
            }
            if !is_xml_media_type(&op.produces) {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{op_label}': binding 'xml' requires an XML media \
                     type (application/xml, text/xml, or */*+xml) for 'produces' \
                     (got '{}')",
                    op.produces.trim()
                )));
            }
        }
        RouteDslRestBinding::Raw => {
            if op.request_schema.is_some() {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{op_label}': 'binding: raw' does not support \
                     'request_schema' — schema validation requires binding 'json'"
                )));
            }
            if op
                .response
                .as_ref()
                .and_then(|r| r.schema.as_ref())
                .is_some()
            {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{op_label}': 'binding: raw' does not support \
                     'response.schema' — schema validation requires binding 'json'"
                )));
            }
            if !is_valid_media_declaration(&op.consumes) {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{op_label}': binding 'raw' requires a valid media \
                     declaration for 'consumes' (got '{}') — expected 'type/subtype' \
                     built from RFC 9110 tchar characters",
                    op.consumes.trim()
                )));
            }
            if !is_valid_media_declaration(&op.produces) {
                return Err(CamelError::RouteError(format!(
                    "rest operation '{op_label}': binding 'raw' requires a valid media \
                     declaration for 'produces' (got '{}') — expected 'type/subtype' \
                     built from RFC 9110 tchar characters",
                    op.produces.trim()
                )));
            }
        }
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

    // Default status injection — "default-if-absent" (spec §6.2/§7.1/§9).
    //
    // The status is injected as the LAST step so it only applies when no
    // prior user step or error handler set CamelHttpResponseCode. This is
    // spec-compliant: the default never overwrites a dynamic set_status,
    // and it runs AFTER error handling (error paths short-circuit the
    // pipeline before reaching this step).
    let default_status = op
        .success_status
        .unwrap_or_else(|| default_status_for_verb(&verb_lc));

    let mut steps: Vec<RouteDslStep> = Vec::new();

    // Raw binding emits NO automatic data-format steps: the request body
    // stays Body::Stream end-to-end and the handler's reply body goes to the
    // wire untouched (rest-dsl spec: Raw binding mode pipeline). Only the
    // Content-Type declaration and the default-if-absent status remain
    // binding-independent.
    let raw_binding = binding == RouteDslRestBinding::Raw;

    // Binding `form` mirrors the `json` pipeline exactly except for the data
    // format name: the form body is parsed into a JSON object before schema
    // validation, and the response is encoded back to form. Raw emits no
    // data-format steps at all, so its format name is never consumed.
    let data_format = match binding {
        RouteDslRestBinding::Form => "form",
        RouteDslRestBinding::Xml => "xml",
        RouteDslRestBinding::Json | RouteDslRestBinding::Raw => "json",
    };

    // 0. Content-negotiation gate (change add-rest-strict-negotiation):
    //    pushed FIRST for BOTH bindings and ALL verbs — the declared
    //    `consumes`/`produces` (trimmed) drive the 406 Accept gate, and the
    //    415 Content-Type side of the check runs only for verbs that carry
    //    a request body.
    steps.push(RouteDslStep::ContentNegotiation(ContentNegotiationStep {
        consumes: op.consumes.trim().to_string(),
        produces: op.produces.trim().to_string(),
        check_content_type: verb_has_body(&verb_lc),
    }));

    // 1. Request binding: unmarshal JSON body (only if verb has a body).
    //    Skipped entirely under raw binding — the body is consumed as-is.
    if !raw_binding && verb_has_body(&verb_lc) {
        steps.push(RouteDslStep::Unmarshal(UnmarshalStep {
            unmarshal: data_format.to_string(),
            // When `request_schema` is set, the step compiler wraps the
            // JSON unmarshal with a JsonSchemaValidateService; mismatches
            // surface as CamelError::ValidationError → 400 Bad Request
            // (see camel-component-http `pipeline_error_to_reply`).
            schema: op.request_schema.clone(),
            config: None,
            // request-borne seam (ADR-0096): format failures reclassify to ValidationError (400)
            request_borne: true,
        }));
    }

    // 2. User steps (either `to` shorthand or explicit `steps`)
    if let Some(ref to_uri) = op.to {
        steps.push(RouteDslStep::To(ToStep {
            to: to_uri.clone(),
            parameters: BTreeMap::new(),
        }));
    } else {
        steps.extend(op.steps.iter().cloned());
    }

    // 3. Response binding: marshal JSON. The JSON data format serialises the
    //    body to Body::Text (the JSON wire form); the HTTP reply finaliser
    //    would infer that as text/plain, so we set Content-Type explicitly
    //    below. Skipped under raw binding — the handler's body is already in
    //    its declared wire form and must not be re-encoded.
    if !raw_binding {
        steps.push(RouteDslStep::ResponseMarshal(MarshalStep {
            marshal: data_format.to_string(),
            config: None,
        }));
    }

    // 4. Response Content-Type: the finaliser prioritises a user-supplied
    // Content-Type header over its body-type inference (Body::Text →
    // text/plain). The declared `produces` is used verbatim after trim, so
    // parameterized media (e.g. `application/json; charset=utf-8` or
    // `text/plain; charset=utf-8`) survive onto the wire; for v1-loadable
    // json-mode routes this is identical to the previous hardcoded
    // `application/json` because the json-mode gate only admits JSON media
    // and those routes declared exactly that. Under raw binding this is what
    // advertises the declared media for the un-typed raw body. An earlier
    // user step that set a different Content-Type is intentionally
    // overridden — the declared `produces` wins.
    steps.push(RouteDslStep::SetHeader(SetHeaderStep {
        set_header: SetHeaderData {
            key: "Content-Type".to_string(),
            value: Some(serde_json::Value::String(op.produces.trim().to_string())),
            language: None,
            source: None,
            source_file: None,
            simple: None,
            rhai: None,
            jsonpath: None,
            xpath: None,
        },
    }));

    // 5. Default status injection — "default-if-absent" (spec §6.2/§7.1/§9).
    //
    // The status is injected as the LAST step so it only applies when no
    // prior user step or error handler set CamelHttpResponseCode. This is
    // spec-compliant: the default never overwrites a dynamic set_status,
    // and it runs AFTER error handling (error paths short-circuit the
    // pipeline before reaching this step).
    steps.push(RouteDslStep::SetHeaderIfAbsent(SetHeaderStep {
        set_header: SetHeaderData {
            key: "CamelHttpResponseCode".to_string(),
            value: Some(serde_json::Value::Number(default_status.into())),
            language: None,
            source: None,
            source_file: None,
            simple: None,
            rhai: None,
            jsonpath: None,
            xpath: None,
        },
    }));

    Ok(RouteDslRoute {
        id: route_id,
        from,
        parameters: BTreeMap::new(),
        steps,
        auto_startup: true,
        startup_order: 0,
        sequential: false,
        concurrent: None,
        error_handler: None,
        circuit_breaker: None,
        // Block policy rides every lowered route (Task 2.10), mirroring the
        // mcp.rs copy pattern: enforcement is route-level, so the block
        // `security_policy` must travel with each operation's route.
        security_policy: rest.security_policy.clone(),
        on_complete: None,
        on_failure: None,
        // Business-error-model Task 2.2 (review C1): EVERY lowered rest route
        // is a seam route — `Some(_)` even when the block declares no table —
        // while `error_handler` stays `None` exactly as before.
        http_error_mapping: Some(seam_error_mapping(rest.error_mapping.as_ref())),
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

/// One piece of a path-template segment: literal text or a `{name}`
/// placeholder.
///
/// This is a deliberate duplicate of `camel_component_http::rest_match`'s
/// grammar — the two crates are independent (no dependency), and the
/// cross-crate corpus test pins them byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentPart {
    /// Literal text that must match exactly, e.g. `users`.
    Lit(String),
    /// A `{name}` placeholder.
    Param(String),
}

/// One parsed segment of a path template, made of ordered parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPattern {
    /// Ordered literal and placeholder parts of the segment.
    pub parts: Vec<SegmentPart>,
}

/// Why a path template could not be parsed.
///
/// Every variant carries the offending `segment` text so the caller can name
/// the malformation in its route-load diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum TemplateParseError {
    /// A `{` placeholder was not terminated by `}`.
    UnclosedPlaceholder {
        /// The segment containing the unclosed placeholder.
        segment: String,
    },
    /// A `}` appeared outside a placeholder.
    StrayCloseBrace {
        /// The segment containing the stray `}`.
        segment: String,
    },
    /// A placeholder had an empty name (`{}`).
    EmptyPlaceholder {
        /// The segment containing the empty placeholder.
        segment: String,
    },
}

impl fmt::Display for TemplateParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnclosedPlaceholder { segment } => write!(
                f,
                "unclosed placeholder in segment '{segment}': a '{{' must be closed by '}}'"
            ),
            Self::StrayCloseBrace { segment } => write!(
                f,
                "stray '}}' in segment '{segment}': '}}' outside a placeholder is not allowed"
            ),
            Self::EmptyPlaceholder { segment } => {
                write!(f, "empty placeholder name in segment '{segment}'")
            }
        }
    }
}

impl std::error::Error for TemplateParseError {}

/// Parse a path like `/users/{id}/posts` into segment patterns.
///
/// Grammar (identical to `camel-component-http`): split on `/`, dropping the
/// leading slash and empty segments; a `{` starts a placeholder whose name is
/// the maximal run excluding braces and which must be terminated by `}`; a
/// `}` outside a placeholder is an error; an empty name is an error. Literal
/// text between placeholders becomes `Lit` parts; a segment without
/// placeholders is one `Lit` part.
pub fn parse_path_template(template: &str) -> Result<Vec<SegmentPattern>, TemplateParseError> {
    template
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(parse_segment)
        .collect()
}

/// Parse one `/`-delimited segment into its ordered parts.
///
/// The scan is byte-wise, but only the ASCII braces `{` / `}` are
/// significant, so every slice boundary is a `char` boundary.
fn parse_segment(segment: &str) -> Result<SegmentPattern, TemplateParseError> {
    let bytes = segment.as_bytes();
    let mut parts = Vec::new();
    let mut cursor = 0;
    let mut literal_start = 0;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'{' => {
                let name_start = cursor + 1;
                let mut name_end = name_start;
                while name_end < bytes.len() && bytes[name_end] != b'{' && bytes[name_end] != b'}' {
                    name_end += 1;
                }
                if name_end >= bytes.len() || bytes[name_end] != b'}' {
                    return Err(TemplateParseError::UnclosedPlaceholder {
                        segment: segment.to_string(),
                    });
                }
                if name_end == name_start {
                    return Err(TemplateParseError::EmptyPlaceholder {
                        segment: segment.to_string(),
                    });
                }
                if literal_start < cursor {
                    parts.push(SegmentPart::Lit(segment[literal_start..cursor].to_string()));
                }
                parts.push(SegmentPart::Param(
                    segment[name_start..name_end].to_string(),
                ));
                cursor = name_end + 1;
                literal_start = cursor;
            }
            b'}' => {
                return Err(TemplateParseError::StrayCloseBrace {
                    segment: segment.to_string(),
                });
            }
            _ => cursor += 1,
        }
    }

    if literal_start < bytes.len() {
        parts.push(SegmentPart::Lit(segment[literal_start..].to_string()));
    }
    Ok(SegmentPattern { parts })
}

/// Rank a parsed template's specificity as `(L, C)`.
///
/// `L` is the number of segments carrying at least one literal part and `C`
/// is the total number of literal characters across the whole template. Pairs
/// are compared lexicographically (mirroring
/// `camel_component_http::rest_match::template_specificity`), so a template
/// with more literal-bearing segments wins first, then one with more literal
/// text: `/users/me` is `(2, 7)` and beats `/users/{id}` `(1, 5)`.
#[must_use]
pub fn template_specificity(segments: &[SegmentPattern]) -> (usize, usize) {
    let mut literal_segments = 0;
    let mut literal_chars = 0;
    for pattern in segments {
        let mut has_literal = false;
        for part in &pattern.parts {
            if let SegmentPart::Lit(text) = part {
                has_literal = true;
                literal_chars += text.chars().count();
            }
        }
        if has_literal {
            literal_segments += 1;
        }
    }
    (literal_segments, literal_chars)
}

/// Extract param names from a template path, in order of appearance.
pub fn extract_param_names(segments: &[SegmentPattern]) -> Vec<String> {
    segments
        .iter()
        .flat_map(|pattern| pattern.parts.iter())
        .filter_map(|part| match part {
            SegmentPart::Param(name) => Some(name.clone()),
            SegmentPart::Lit(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::route_ast::RouteDslRestResponse;

    fn make_rest(op_id: &str, verb: &str, path: &str, to: &str) -> RouteDslRest {
        let operations = vec![RouteDslRestOperation {
            method: verb.to_string(),
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
            binding: None,
        }];
        RouteDslRest {
            error_mapping: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: "/users".to_string(),
            security_policy: None,
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
            binding: None,
        }
    }

    #[test]
    fn lower_single_get_operation() {
        let rest = make_rest("getUser", "get", "/{id}", "bean:userService");
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].id, "getUser");
        assert!(routes[0].from.contains("httpMethod=GET"));
        assert!(routes[0].from.contains("/users/{id}"));
        assert!(routes[0].from.contains("0.0.0.0:8080"));
    }

    #[test]
    fn lower_post_has_201_default() {
        let rest = make_rest("createUser", "post", "/", "bean:createUser");
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        // The default status (201 for POST) is injected as the LAST step
        // using if-absent semantics (spec §6.2/§7.1/§9) — it only applies
        // when no prior user step or error handler set the response code.
        let last = routes[0].steps.last().unwrap();
        match last {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.key, "CamelHttpResponseCode");
                assert_eq!(h.set_header.value, Some(serde_json::json!(201)));
            }
            other => panic!("expected SetHeaderIfAbsent as last step, got {other:?}"),
        }
    }

    #[test]
    fn lower_get_defaults_to_200() {
        let rest = make_rest("getUser", "get", "/{id}", "bean:userService");
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let last = routes[0].steps.last().unwrap();
        match last {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.value, Some(serde_json::json!(200)));
            }
            other => panic!("expected SetHeaderIfAbsent last, got {other:?}"),
        }
    }

    #[test]
    fn lower_delete_defaults_to_204() {
        let rest = make_rest("deleteUser", "delete", "/{id}", "bean:deleteUser");
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let last = routes[0].steps.last().unwrap();
        match last {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.value, Some(serde_json::json!(204)));
            }
            other => panic!("expected SetHeaderIfAbsent last, got {other:?}"),
        }
    }

    #[test]
    fn lower_default_status_is_last_step_if_absent() {
        let rest = make_rest("createUser", "post", "/", "bean:createUser");
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let last = routes[0].steps.last().unwrap();
        match last {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.key, "CamelHttpResponseCode");
                assert_eq!(h.set_header.value, Some(serde_json::json!(201)));
            }
            other => panic!("expected SetHeaderIfAbsent last, got {other:?}"),
        }
    }

    #[test]
    fn lower_if_absent_step_compiles_through_full_chain() {
        // Verify the SetHeaderIfAbsent variant survives the full lowering
        // → conversion → compile chain without errors.
        let rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();

        // Convert each route's steps through the YAML conversion path.
        for step in &routes[0].steps {
            let _ = crate::yaml::route_step_to_declarative_step(
                step.clone(),
                &crate::yaml::ScriptLoadContext::new(None),
            )
            .unwrap_or_else(|e| panic!("step {step:?} must convert to DeclarativeStep: {e}"));
        }

        // Verify the last step specifically converts to SetHeaderIfAbsent.
        let last = routes[0].steps.last().unwrap();
        let declarative = crate::yaml::route_step_to_declarative_step(
            last.clone(),
            &crate::yaml::ScriptLoadContext::new(None),
        )
        .unwrap();
        assert!(
            matches!(
                declarative,
                crate::model::DeclarativeStep::SetHeaderIfAbsent(_)
            ),
            "last step must convert to DeclarativeStep::SetHeaderIfAbsent"
        );
    }

    #[test]
    fn lower_user_set_status_preserved() {
        // Build a REST op that uses `steps` (not `to`) so we can inject a
        // user SetHeader in the middle of the step sequence. The default
        // status if-absent step must still come last.
        let rest = RouteDslRest {
            error_mapping: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: "/users".to_string(),
            security_policy: None,
            operations: vec![RouteDslRestOperation {
                method: "put".to_string(),
                path: "/{id}".to_string(),
                operation_id: Some("updateUser".to_string()),
                to: None,
                steps: vec![
                    RouteDslStep::SetHeader(SetHeaderStep {
                        set_header: SetHeaderData {
                            key: "CamelHttpResponseCode".to_string(),
                            value: Some(serde_json::json!(202)),
                            language: None,
                            source: None,
                            source_file: None,
                            simple: None,
                            rhai: None,
                            jsonpath: None,
                            xpath: None,
                        },
                    }),
                    RouteDslStep::To(ToStep {
                        to: "bean:updateUser".into(),
                        parameters: BTreeMap::new(),
                    }),
                ],
                consumes: "application/json".to_string(),
                produces: "application/json".to_string(),
                success_status: None,
                request_schema: None,
                response: None,
                description: None,
                parameters: BTreeMap::new(),
                binding: None,
            }],
        };
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        // User step in the middle, default-if-absent last.
        let last = routes[0].steps.last().unwrap();
        assert!(
            matches!(last, RouteDslStep::SetHeaderIfAbsent(_)),
            "expected SetHeaderIfAbsent last, got {last:?}"
        );
    }

    #[test]
    fn lower_rejects_to_and_steps_both() {
        let mut rest = make_rest("op", "get", "/", "bean:svc");
        rest.operations[0].steps = vec![RouteDslStep::To(crate::route_ast::ToStep {
            to: "bean:other".into(),
            parameters: BTreeMap::new(),
        })];
        let result = lower_all_rest_to_routes(&[rest]);
        assert!(result.is_err());
    }

    #[test]
    fn lower_rejects_non_json_consumes() {
        let mut rest = make_rest("op", "get", "/", "bean:svc");
        rest.operations[0].consumes = "application/xml".to_string();
        let result = lower_all_rest_to_routes(&[rest]);
        assert!(result.is_err());
    }

    // ── Path template parser tests (Task 4) ──

    #[test]
    fn parse_path_template_segments() {
        let segments =
            parse_path_template("/users/{id}/posts/{postId}").expect("template is well formed");
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0].parts, vec![SegmentPart::Lit("users".into())]);
        assert_eq!(segments[1].parts, vec![SegmentPart::Param("id".into())]);
        assert_eq!(segments[2].parts, vec![SegmentPart::Lit("posts".into())]);
        assert_eq!(segments[3].parts, vec![SegmentPart::Param("postId".into())]);
    }

    #[test]
    fn path_template_specificity_count() {
        let s1 = parse_path_template("/users/me").expect("template is well formed");
        let s2 = parse_path_template("/users/{id}").expect("template is well formed");
        assert!(template_specificity(&s1) > template_specificity(&s2));
    }

    #[test]
    fn parse_path_template_empty() {
        let segments = parse_path_template("").expect("empty template is well formed");
        assert!(segments.is_empty());
    }

    #[test]
    fn parse_path_template_root() {
        let segments = parse_path_template("/").expect("root template is well formed");
        assert!(segments.is_empty());
    }

    #[test]
    fn extract_param_names_from_template() {
        let segments =
            parse_path_template("/users/{id}/posts/{postId}").expect("template is well formed");
        let params = extract_param_names(&segments);
        assert_eq!(params, vec!["id", "postId"]);
    }

    #[test]
    fn extract_multi_param_segment_names() {
        let segments = parse_path_template("/{a}-{b}.json").expect("template is well formed");
        assert_eq!(extract_param_names(&segments), vec!["a", "b"]);
    }

    #[test]
    fn lower_rejects_duplicate_operation_ids() {
        // Two REST resources with same operation_id (reviewer fix M2)
        let rest1 = make_rest("getUser", "get", "/{id}", "bean:svc");
        let rest2 = make_rest("getUser", "get", "/{id}", "bean:other");
        // The shared check_duplicate_route_ids helper (now used by both the
        // YAML and JSON parsers — review I2) enforces uniqueness across the
        // full lowered route set.
        let mut all = lower_all_rest_to_routes(&[rest1]).unwrap();
        all.extend(lower_all_rest_to_routes(&[rest2]).unwrap());
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
        let operations = vec![RouteDslRestOperation {
            method: verb.to_string(),
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
            binding: None,
        }];
        RouteDslRest {
            error_mapping: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: base.to_string(),
            security_policy: None,
            operations,
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
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("empty base path must be rejected");
        assert!(err.to_string().contains("path"), "got: {err}");
    }

    #[test]
    fn lower_rejects_whitespace_only_base_path() {
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.path = "   ".to_string();
        assert!(lower_all_rest_to_routes(&[rest]).is_err());
    }

    #[test]
    fn lower_rejects_empty_operations() {
        let rest = RouteDslRest {
            error_mapping: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: "/users".to_string(),
            security_policy: None,
            operations: vec![],
        };
        let err = lower_all_rest_to_routes(&[rest])
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
        // lowering must inject Content-Type: application/json so the HTTP
        // finaliser advertises JSON (spec §8.1). The default-status
        // if-absent step is appended AFTER Content-Type, so we search for
        // the Content-Type step rather than asserting it is last.
        let rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let ct_step = routes[0]
            .steps
            .iter()
            .find(|s| matches!(s, RouteDslStep::SetHeader(h) if h.set_header.key == "Content-Type"))
            .expect("Content-Type step present");
        match ct_step {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.key, "Content-Type");
                assert_eq!(
                    h.set_header.value,
                    Some(serde_json::json!("application/json"))
                );
            }
            other => panic!("expected SetHeader(Content-Type) step, got {other:?}"),
        }
    }

    #[test]
    fn lower_content_type_is_after_marshal() {
        // The Content-Type header MUST come after marshal (which produces the
        // Body::Text wire form) so it overrides the text/plain inference.
        let rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        let steps = &lower_all_rest_to_routes(&[rest]).unwrap()[0].steps;
        let marshal_idx = steps
            .iter()
            .position(|s| matches!(s, RouteDslStep::ResponseMarshal(_)))
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
        let err = lower_all_rest_to_routes(&[rest]).err();
        assert!(err.is_some(), "unknown verb must be rejected");
        let msg = format!("{}", err.unwrap());
        assert!(msg.contains("unknown HTTP verb 'geet'"), "got: {msg}");
    }

    #[test]
    fn lower_accepts_uppercase_verb_key() {
        // Case-insensitive verb key; lowercase helpers must still pick the
        // right defaults (POST → 201).
        let rest = make_rest("createUser", "POST", "/", "bean:create");
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert!(routes[0].from.contains("httpMethod=POST"));
        let last = match routes[0].steps.last().unwrap() {
            RouteDslStep::SetHeaderIfAbsent(h) => h,
            other => panic!("expected SetHeaderIfAbsent last, got {other:?}"),
        };
        assert_eq!(last.set_header.value, Some(serde_json::json!(201)));
    }

    // ── Path-template param validation (review I1) ──

    #[test]
    fn lower_rejects_empty_path_param() {
        let rest = make_rest_with_base("op", "get", "/users", "/{}", "bean:svc");
        let err = lower_all_rest_to_routes(&[rest]).err();
        assert!(err.is_some(), "empty {{}} param must be rejected");
    }

    #[test]
    fn lower_rejects_duplicate_path_param() {
        let rest = make_rest_with_base("op", "get", "/users", "/{id}/{id}", "bean:svc");
        let err = lower_all_rest_to_routes(&[rest]).err();
        assert!(err.is_some(), "duplicate path param must be rejected");
    }

    #[test]
    fn validate_rejects_unclosed_placeholder_naming_path() {
        let err = validate_path_template("/files/{id.xml", "get")
            .expect_err("unclosed placeholder must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("/files/{id.xml"), "got: {msg}");
    }

    #[test]
    fn validate_rejects_stray_close_brace_naming_path() {
        let err = validate_path_template("/files/id}.xml", "get")
            .expect_err("stray close brace must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("/files/id}.xml"), "got: {msg}");
    }

    #[test]
    fn validate_rejects_empty_placeholder_name() {
        let err = validate_path_template("/files/{}.xml", "get")
            .expect_err("empty placeholder name must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("/files/{}.xml"), "got: {msg}");
        assert!(msg.contains("parameter names are required"), "got: {msg}");
    }

    #[test]
    fn validate_rejects_duplicate_names_across_mixed_segments() {
        let err = validate_path_template("/{id}.xml/{id}", "get")
            .expect_err("duplicate names across mixed segments must be rejected");
        assert!(err.to_string().contains("more than once"), "got: {err}");
    }

    #[test]
    fn validate_accepts_legacy_names() {
        assert!(validate_path_template("/users/{user-id}", "get").is_ok());
    }

    #[test]
    fn lowering_rejects_malformed_templates_naming_path() {
        for (op_path, needle, extra) in [
            ("/{id.xml", "/files/{id.xml", None),
            ("/id}.xml", "/files/id}.xml", None),
            (
                "/{}.xml",
                "/files/{}.xml",
                Some("parameter names are required"),
            ),
        ] {
            let rest = make_rest_with_base("getFile", "get", "/files", op_path, "bean:svc");
            let err = lower_all_rest_to_routes(&[rest])
                .err()
                .expect("malformed template must be rejected");
            assert!(matches!(err, CamelError::RouteError(_)), "got: {err:?}");
            let msg = err.to_string();
            assert!(msg.contains(needle), "got: {msg}");
            if let Some(extra) = extra {
                assert!(msg.contains(extra), "got: {msg}");
            }
        }
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
        let mut routes = lower_all_rest_to_routes(&[rest]).unwrap();
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
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
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
        rest.operations[0].request_schema = Some(schema.clone());
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
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
        rest.operations[0].request_schema = Some(serde_json::json!({ "type": "object" }));
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let has_unmarshal = routes[0]
            .steps
            .iter()
            .any(|s| matches!(s, RouteDslStep::Unmarshal(_)));
        assert!(!has_unmarshal, "GET must not emit unmarshal");
    }

    // ── Within-block duplicate & ambiguous detection (rc-y1xa Task 2) ──

    #[test]
    fn within_block_identical_verb_path_is_duplicate() {
        let rest = RouteDslRest {
            error_mapping: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: "/api".to_string(),
            security_policy: None,
            operations: vec![
                RouteDslRestOperation {
                    method: "GET".to_string(),
                    path: "/health".to_string(),
                    to: Some("direct:a".to_string()),
                    ..make_op("a")
                },
                RouteDslRestOperation {
                    method: "GET".to_string(),
                    path: "/health".to_string(),
                    to: Some("direct:b".to_string()),
                    ..make_op("b")
                },
            ],
        };
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("expected duplicate error");
        assert!(
            err.to_string().contains("duplicate"),
            "expected duplicate error, got: {err}"
        );
    }

    #[test]
    fn within_block_same_shape_distinct_param_is_ambiguous() {
        let rest = RouteDslRest {
            error_mapping: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: "/api".to_string(),
            security_policy: None,
            operations: vec![
                RouteDslRestOperation {
                    method: "GET".to_string(),
                    path: "/x/{id}".to_string(),
                    to: Some("direct:a".to_string()),
                    ..make_op("a")
                },
                RouteDslRestOperation {
                    method: "GET".to_string(),
                    path: "/x/{name}".to_string(),
                    to: Some("direct:b".to_string()),
                    ..make_op("b")
                },
            ],
        };
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("expected ambiguous error");
        assert!(
            err.to_string().contains("ambiguous"),
            "expected ambiguous error, got: {err}"
        );
    }

    #[test]
    fn empty_operations_list_is_rejected() {
        let rest = RouteDslRest {
            error_mapping: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: "/api".to_string(),
            security_policy: None,
            operations: vec![],
        };
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("expected error for empty operations");
        assert!(err.to_string().contains("operations"), "got: {err}");
    }

    #[test]
    fn multiple_ops_same_verb_preserve_declaration_order() {
        let rest = RouteDslRest {
            error_mapping: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: "/api".to_string(),
            security_policy: None,
            operations: vec![
                RouteDslRestOperation {
                    method: "GET".to_string(),
                    path: "/conteos".to_string(),
                    ..make_op("getConteos")
                },
                RouteDslRestOperation {
                    method: "GET".to_string(),
                    path: "/health".to_string(),
                    ..make_op("healthCheck")
                },
            ],
        };
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert_eq!(routes.len(), 2);
        // Declaration order, NOT alphabetical: conteos before health.
        assert!(
            routes[0].id.contains("getConteos"),
            "expected first id to contain 'getConteos', got: {}",
            routes[0].id
        );
        assert!(
            routes[1].id.contains("healthCheck"),
            "expected second id to contain 'healthCheck', got: {}",
            routes[1].id
        );
    }

    // ── Task 2.10: block security_policy declaration ──

    #[test]
    fn rest_block_policy_copies_to_lowered_routes() {
        let yaml = r#"
rest:
  - host: 127.0.0.1
    port: 18080
    path: /api/users
    security_policy:
      roles: ["admin"]
      provider: "idp-a"
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
      - method: POST
        operation_id: createUser
        to: direct:createUser
"#;
        // Named provider "idp-a" mirrors the fixture registries the kernel
        // tests use; lowering + conversion + compile run through the shared
        // `parse_yaml_with_threshold_and_security` path.
        let ctx = crate::model::SecurityCompileContext::default()
            .with_named_authenticator("idp-a", crate::test_support::test_authenticator("a-user"));
        let defs = crate::yaml::parse_yaml_with_threshold_and_security(yaml, 1024, ctx)
            .expect("rest block with roles policy must lower + compile");
        assert_eq!(defs.len(), 2, "both operations must lower to routes");

        for def in &defs {
            // Plan inputs per Task 1.8: a policy config on the definition
            // compiles to AccessMode::Authorized, and the declared provider
            // name becomes the plan's provider_ref. The Authorized +
            // provider_ref Some("idp-a") mapping itself is proven in
            // camel-core `route_compiler_ext_tests`
            // (`compilation_roles_named_provider_resolves_with_audience`);
            // the DSL layer owns producing these definition-level inputs.
            assert!(
                def.security_policy_config().is_some(),
                "route '{}' must carry the block policy (Authorized plan input)",
                def.route_id()
            );
            assert_eq!(
                def.security_provider(),
                Some("idp-a"),
                "route '{}' must carry provider_ref 'idp-a'",
                def.route_id()
            );
            assert!(
                def.security_authenticator().is_some(),
                "route '{}' must carry the resolved 'idp-a' authenticator",
                def.route_id()
            );
        }

        // Lowering level: every lowered RouteDslRoute carries the block
        // policy verbatim (the mcp.rs copy pattern).
        let parsed: crate::route_ast::RouteDslRoutes =
            noyalib::compat::serde_yaml::from_str(yaml).unwrap();
        let lowered = lower_all_rest_to_routes(&parsed.rest).unwrap();
        assert_eq!(lowered.len(), 2);
        for route in &lowered {
            let sp = route
                .security_policy
                .as_ref()
                .expect("every lowered route must carry the block policy");
            assert_eq!(sp.roles, Some(vec!["admin".to_string()]));
            assert_eq!(sp.provider.as_deref(), Some("idp-a"));
        }
    }

    #[test]
    fn bare_rest_block_lowers_public() {
        let yaml = r#"
rest:
  - host: 127.0.0.1
    port: 18081
    path: /api/items
    operations:
      - method: GET
        operation_id: listItems
        to: direct:listItems
"#;
        let parsed: crate::route_ast::RouteDslRoutes =
            noyalib::compat::serde_yaml::from_str(yaml).unwrap();
        let lowered = lower_all_rest_to_routes(&parsed.rest).unwrap();
        assert_eq!(lowered.len(), 1);
        assert!(
            lowered[0].security_policy.is_none(),
            "a rest block without security_policy must lower to Public routes"
        );

        // Through the full compile path the definition declares no security
        // either (kernel gate classifies it Public).
        let defs = crate::yaml::parse_yaml_with_threshold_and_security(
            yaml,
            1024,
            crate::model::SecurityCompileContext::default(),
        )
        .expect("bare rest block must compile without a security context");
        assert_eq!(defs.len(), 1);
        assert!(defs[0].security_policy_config().is_none());
        assert!(defs[0].security_provider().is_none());
    }

    #[test]
    fn media_helpers_accept_json_forms() {
        for media in [
            "application/json",
            "Application/JSON",
            "application/json; charset=utf-8",
            "application/problem+json",
            "text/json",
            " application/json ",
        ] {
            assert!(is_json_media_type(media), "expected JSON media: {media:?}");
        }
        for media in [
            "application/xml",
            "json",
            "application/jsonx",
            "",
            "png",
            "application/jsoné",
        ] {
            assert!(
                !is_json_media_type(media),
                "expected non-JSON media: {media:?}"
            );
        }
    }

    #[test]
    fn media_helpers_accept_valid_declarations() {
        for media in [
            "image/png",
            "text/plain; charset=utf-8",
            "application/octet-stream",
            " image/png ",
        ] {
            assert!(
                is_valid_media_declaration(media),
                "expected valid media declaration: {media:?}"
            );
        }
        for media in [
            "png",
            "/png",
            "png/",
            "image / png",
            "im age/png",
            "image/é",
            "",
            "   ",
        ] {
            assert!(
                !is_valid_media_declaration(media),
                "expected invalid media declaration: {media:?}"
            );
        }
    }

    // ── Task 1.3: binding-aware json-mode gate + trimmed produces CT ──

    /// Step variant names in declaration order — used to pin the exact step
    /// sequence the lowering produces (byte-identity regression pins).
    ///
    /// Derived from `Debug` instead of a hand-written match: the variant name
    /// is everything before the first `(` in the rendered output
    /// (e.g. `To(ToStep { .. })` → `"To"`).
    fn step_kind_names(steps: &[RouteDslStep]) -> Vec<String> {
        steps
            .iter()
            .map(|s| {
                format!("{s:?}")
                    .split('(')
                    .next()
                    .unwrap_or("<unknown>")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn json_mode_accepts_parameterized_json_media() {
        let mut rest = make_rest("createUser", "post", "/", "bean:create");
        rest.operations[0].consumes = "application/json; charset=utf-8".to_string();
        rest.operations[0].produces = "application/json; charset=utf-8".to_string();
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let ct = routes[0]
            .steps
            .iter()
            .find_map(|s| match s {
                RouteDslStep::SetHeader(h) if h.set_header.key == "Content-Type" => {
                    Some(h.set_header.value.clone())
                }
                _ => None,
            })
            .expect("Content-Type step present");
        assert_eq!(
            ct,
            Some(serde_json::json!("application/json; charset=utf-8")),
            "Content-Type must carry the declared produces after trim"
        );
    }

    #[test]
    fn json_mode_accepts_problem_plus_json() {
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.operations[0].produces = "application/problem+json".to_string();
        assert!(lower_all_rest_to_routes(&[rest]).is_ok());
    }

    #[test]
    fn json_mode_rejects_xml_with_precise_error() {
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.operations[0].consumes = "application/xml".to_string();
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("xml consumes with json binding must be rejected");
        let msg = err.to_string();
        for needle in ["getUser", "consumes", "application/xml", "raw"] {
            assert!(
                msg.contains(needle),
                "message must contain {needle:?}: {msg}"
            );
        }
    }

    #[test]
    fn json_mode_rejects_non_json_produces() {
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.operations[0].produces = "text/plain".to_string();
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("text/plain produces with json binding must be rejected");
        let msg = err.to_string();
        for needle in ["produces", "text/plain"] {
            assert!(
                msg.contains(needle),
                "message must contain {needle:?}: {msg}"
            );
        }
    }

    #[test]
    fn v1_route_lowers_with_negotiation_prefix() {
        let rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].from,
            "http://0.0.0.0:8080/users/{id}?httpMethod=GET"
        );
        assert_eq!(routes[0].id, "getUser");
        assert_eq!(
            step_kind_names(&routes[0].steps),
            vec![
                "ContentNegotiation",
                "To",
                "ResponseMarshal",
                "SetHeader",
                "SetHeaderIfAbsent"
            ]
        );
        // Positional: Content-Type injection is step idx 3 (negotiation gate
        // first, then the user To step and marshal; v1 pushes Marshal
        // UNCONDITIONALLY — only the unmarshal is verb-guarded).
        match &routes[0].steps[3] {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.key, "Content-Type");
                assert_eq!(
                    h.set_header.value,
                    Some(serde_json::json!("application/json"))
                );
            }
            other => panic!("expected SetHeader at idx 3, got {other:?}"),
        }
        match &routes[0].steps[4] {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.key, "CamelHttpResponseCode");
                assert_eq!(h.set_header.value, Some(serde_json::json!(200)));
            }
            other => panic!("expected SetHeaderIfAbsent at idx 4, got {other:?}"),
        }
    }

    #[test]
    fn v1_post_step_sequence_pinned() {
        let mut rest = make_rest("createUser", "post", "/", "bean:create");
        let schema = serde_json::json!({ "type": "object" });
        rest.operations[0].request_schema = Some(schema.clone());
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert_eq!(
            step_kind_names(&routes[0].steps),
            vec![
                "ContentNegotiation",
                "Unmarshal",
                "To",
                "ResponseMarshal",
                "SetHeader",
                "SetHeaderIfAbsent"
            ]
        );
        match &routes[0].steps[0] {
            RouteDslStep::ContentNegotiation(n) => {
                assert_eq!(n.consumes, "application/json");
                assert_eq!(n.produces, "application/json");
                assert!(n.check_content_type);
            }
            other => panic!("expected ContentNegotiation at idx 0, got {other:?}"),
        }
        match &routes[0].steps[1] {
            RouteDslStep::Unmarshal(u) => assert_eq!(u.schema, Some(schema)),
            other => panic!("expected Unmarshal at idx 1, got {other:?}"),
        }
        match &routes[0].steps[4] {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.key, "Content-Type");
                assert_eq!(
                    h.set_header.value,
                    Some(serde_json::json!("application/json"))
                );
            }
            other => panic!("expected SetHeader at idx 4, got {other:?}"),
        }
        match routes[0].steps.last().unwrap() {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.value, Some(serde_json::json!(201)));
            }
            other => panic!("expected SetHeaderIfAbsent last, got {other:?}"),
        }
    }

    #[test]
    fn json_mode_get_with_schema_has_no_unmarshal() {
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.operations[0].request_schema = Some(serde_json::json!({ "type": "object" }));
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert!(
            !step_kind_names(&routes[0].steps)
                .iter()
                .any(|name| name == "Unmarshal"),
            "GET must not emit unmarshal even with a request schema"
        );
    }

    #[test]
    fn response_binding_uses_response_marshal_step() {
        // The automatic response-encoding step must be tagged with the
        // internal ResponseMarshal variant so the compiler can pick the
        // response-side error wrapping (500 instead of 400).
        for binding in [
            RouteDslRestBinding::Json,
            RouteDslRestBinding::Form,
            RouteDslRestBinding::Xml,
        ] {
            let mut rest = make_rest("createUser", "post", "/", "bean:create");
            rest.operations[0].binding = Some(binding);
            let media = match binding {
                RouteDslRestBinding::Form => FORM_MEDIA,
                RouteDslRestBinding::Xml => XML_MEDIA,
                _ => "application/json",
            };
            rest.operations[0].consumes = media.to_string();
            rest.operations[0].produces = media.to_string();
            let routes = lower_all_rest_to_routes(&[rest]).unwrap();
            assert_eq!(
                step_kind_names(&routes[0].steps),
                vec![
                    "ContentNegotiation",
                    "Unmarshal",
                    "To",
                    "ResponseMarshal",
                    "SetHeader",
                    "SetHeaderIfAbsent"
                ],
                "binding {binding:?} must tag the response marshal step"
            );
            assert!(
                matches!(&routes[0].steps[3], RouteDslStep::ResponseMarshal(_)),
                "response marshal must not be the authored Marshal variant"
            );
        }
    }

    // ── Task 1.4: raw-mode lowering ──

    #[test]
    fn raw_mode_step_sequence_exact() {
        let mut rest = make_rest("getPhoto", "get", "/{id}", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        rest.operations[0].produces = "image/png".to_string();
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(
            step_kind_names(&routes[0].steps),
            vec!["ContentNegotiation", "To", "SetHeader", "SetHeaderIfAbsent"],
            "raw mode must not emit Unmarshal/Marshal binding steps"
        );
        // Positional: Content-Type injection follows the negotiation gate
        // and the user To step.
        match &routes[0].steps[2] {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.key, "Content-Type");
                assert_eq!(h.set_header.value, Some(serde_json::json!("image/png")));
            }
            other => panic!("expected SetHeader at idx 2, got {other:?}"),
        }
        match &routes[0].steps[3] {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.key, "CamelHttpResponseCode");
                assert_eq!(h.set_header.value, Some(serde_json::json!(200)));
            }
            other => panic!("expected SetHeaderIfAbsent at idx 3, got {other:?}"),
        }
        let names = step_kind_names(&routes[0].steps);
        assert!(
            !names.iter().any(|name| name == "Unmarshal"),
            "raw mode forbids Unmarshal"
        );
        assert!(
            !names.iter().any(|name| name == "Marshal"),
            "raw mode forbids Marshal"
        );
    }

    #[test]
    fn raw_mode_post_still_defaults_201_and_has_no_unmarshal() {
        let mut rest = make_rest("createDoc", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let names = step_kind_names(&routes[0].steps);
        assert!(
            !names.iter().any(|name| name == "Unmarshal"),
            "raw POST must not unmarshal"
        );
        assert!(
            !names.iter().any(|name| name == "Marshal"),
            "raw POST must not marshal"
        );
        match routes[0].steps.last().unwrap() {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.key, "CamelHttpResponseCode");
                assert_eq!(
                    h.set_header.value,
                    Some(serde_json::json!(201)),
                    "default-if-absent status is binding-independent"
                );
            }
            other => panic!("expected SetHeaderIfAbsent last, got {other:?}"),
        }
    }

    #[test]
    fn raw_mode_rejects_request_schema() {
        let mut rest = make_rest("createDoc", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        rest.operations[0].request_schema = Some(serde_json::json!({ "type": "object" }));
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("raw binding with request_schema must be rejected");
        let msg = err.to_string();
        for needle in ["createDoc", "request_schema"] {
            assert!(
                msg.contains(needle),
                "message must contain {needle:?}: {msg}"
            );
        }
    }

    #[test]
    fn raw_mode_rejects_response_schema() {
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        rest.operations[0].response = Some(RouteDslRestResponse {
            description: None,
            schema: Some(serde_json::json!({ "type": "object" })),
            headers: BTreeMap::new(),
        });
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("raw binding with response.schema must be rejected");
        let msg = err.to_string();
        for needle in ["getUser", "response.schema"] {
            assert!(
                msg.contains(needle),
                "message must contain {needle:?}: {msg}"
            );
        }
    }

    #[test]
    fn raw_mode_accepts_header_only_response() {
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        let mut headers = BTreeMap::new();
        headers.insert(
            "X-Rate-Limit".to_string(),
            serde_json::json!({"type":"integer"}),
        );
        rest.operations[0].response = Some(RouteDslRestResponse {
            description: None,
            schema: None,
            headers,
        });
        assert!(lower_all_rest_to_routes(&[rest]).is_ok());
    }

    #[test]
    fn raw_mode_rejects_missing_separator() {
        let mut rest = make_rest("getPhoto", "get", "/{id}", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        rest.operations[0].produces = "png".to_string();
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("'png' has no type/subtype separator and must be rejected");
        assert!(
            err.to_string().contains("png"),
            "message must name the offending value: {err}"
        );
    }

    #[test]
    fn raw_mode_rejects_empty_subtype() {
        let mut rest = make_rest("getPhoto", "get", "/{id}", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        rest.operations[0].produces = "/png".to_string();
        assert!(
            lower_all_rest_to_routes(&[rest]).is_err(),
            "'/png' has an empty subtype and must be rejected"
        );
    }

    #[test]
    fn raw_mode_rejects_whitespace_inside_base() {
        let mut rest = make_rest("getPhoto", "get", "/{id}", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        rest.operations[0].produces = "image / png".to_string();
        assert!(
            lower_all_rest_to_routes(&[rest]).is_err(),
            "'image / png' contains whitespace inside the type/subtype base and must be rejected"
        );
    }

    #[test]
    fn raw_mode_trims_outer_whitespace() {
        let mut rest = make_rest("getPhoto", "get", "/{id}", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        rest.operations[0].produces = " image/png ".to_string();
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let ct = routes[0]
            .steps
            .iter()
            .find_map(|s| match s {
                RouteDslStep::SetHeader(h) if h.set_header.key == "Content-Type" => {
                    Some(h.set_header.value.clone())
                }
                _ => None,
            })
            .expect("Content-Type step present");
        assert_eq!(ct, Some(serde_json::json!("image/png")));
    }

    #[test]
    fn raw_mode_accepts_parameterized_media() {
        let mut rest = make_rest("getReport", "get", "/{id}", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        rest.operations[0].produces = "text/plain; charset=utf-8".to_string();
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let ct = routes[0]
            .steps
            .iter()
            .find_map(|s| match s {
                RouteDslStep::SetHeader(h) if h.set_header.key == "Content-Type" => {
                    Some(h.set_header.value.clone())
                }
                _ => None,
            })
            .expect("Content-Type step present");
        assert_eq!(
            ct,
            Some(serde_json::json!("text/plain; charset=utf-8")),
            "raw mode carries the full trimmed declaration, parameters included"
        );
    }

    #[test]
    fn raw_mode_accepts_non_json_media() {
        let mut rest = make_rest("uploadBlob", "post", "/", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        rest.operations[0].consumes = "application/octet-stream".to_string();
        rest.operations[0].produces = "image/png".to_string();
        assert!(lower_all_rest_to_routes(&[rest]).is_ok());
    }

    // ── Task 2.1: form binding media acceptance ──

    const FORM_MEDIA: &str = "application/x-www-form-urlencoded";

    #[test]
    fn form_binding_exact_media_loads() {
        let mut rest = make_rest("createUser", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Form);
        rest.operations[0].consumes = FORM_MEDIA.to_string();
        rest.operations[0].produces = FORM_MEDIA.to_string();
        assert!(lower_all_rest_to_routes(&[rest]).is_ok());
    }

    #[test]
    fn form_binding_charset_utf8_accepted() {
        let mut rest = make_rest("createUser", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Form);
        rest.operations[0].consumes =
            "application/x-www-form-urlencoded; charset=utf-8".to_string();
        rest.operations[0].produces = FORM_MEDIA.to_string();
        assert!(lower_all_rest_to_routes(&[rest]).is_ok());
    }

    #[test]
    fn form_binding_non_form_media_fails() {
        let mut rest = make_rest("createUser", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Form);
        rest.operations[0].consumes = "application/json".to_string();
        rest.operations[0].produces = FORM_MEDIA.to_string();
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("non-form consumes with form binding must be rejected");
        assert!(matches!(err, CamelError::RouteError(_)), "got: {err:?}");
        let msg = err.to_string();
        for needle in [
            "createUser",
            "consumes",
            "application/json",
            "application/x-www-form-urlencoded",
        ] {
            assert!(
                msg.contains(needle),
                "message must contain {needle:?}: {msg}"
            );
        }
    }

    #[test]
    fn form_binding_non_utf8_charset_fails() {
        let mut rest = make_rest("createUser", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Form);
        rest.operations[0].consumes = FORM_MEDIA.to_string();
        rest.operations[0].produces =
            "application/x-www-form-urlencoded; charset=iso-8859-1".to_string();
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("non-UTF-8 charset with form binding must be rejected");
        assert!(matches!(err, CamelError::RouteError(_)), "got: {err:?}");
        let msg = err.to_string();
        for needle in [
            "createUser",
            "application/x-www-form-urlencoded; charset=iso-8859-1",
        ] {
            assert!(
                msg.contains(needle),
                "message must contain {needle:?}: {msg}"
            );
        }
    }

    #[test]
    fn form_binding_allows_request_and_response_schema() {
        let mut rest = make_rest("createUser", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Form);
        rest.operations[0].consumes = FORM_MEDIA.to_string();
        rest.operations[0].produces = FORM_MEDIA.to_string();
        rest.operations[0].request_schema = Some(serde_json::json!({ "type": "object" }));
        rest.operations[0].response = Some(RouteDslRestResponse {
            description: None,
            schema: Some(serde_json::json!({ "type": "object" })),
            headers: BTreeMap::new(),
        });
        assert!(
            lower_all_rest_to_routes(&[rest]).is_ok(),
            "form binding must allow request_schema and response.schema"
        );
    }

    // ── Task 2.2: form binding pipeline lowering ──

    #[test]
    fn form_post_step_sequence() {
        let mut rest = make_rest("createUser", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Form);
        rest.operations[0].consumes = FORM_MEDIA.to_string();
        rest.operations[0].produces = FORM_MEDIA.to_string();
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(
            step_kind_names(&routes[0].steps),
            vec![
                "ContentNegotiation",
                "Unmarshal",
                "To",
                "ResponseMarshal",
                "SetHeader",
                "SetHeaderIfAbsent"
            ],
            "form binding must mirror the json step sequence"
        );
        match &routes[0].steps[0] {
            RouteDslStep::ContentNegotiation(n) => {
                assert_eq!(n.consumes, FORM_MEDIA);
                assert_eq!(n.produces, FORM_MEDIA);
                assert!(n.check_content_type, "POST carries a body: 415 gate on");
            }
            other => panic!("expected ContentNegotiation at idx 0, got {other:?}"),
        }
        match &routes[0].steps[1] {
            RouteDslStep::Unmarshal(u) => {
                assert_eq!(u.unmarshal, "form");
                assert!(u.schema.is_none(), "no request_schema declared");
                assert!(u.config.is_none());
            }
            other => panic!("expected Unmarshal at idx 1, got {other:?}"),
        }
        match &routes[0].steps[2] {
            RouteDslStep::To(t) => assert_eq!(t.to, "bean:create"),
            other => panic!("expected To at idx 2, got {other:?}"),
        }
        match &routes[0].steps[3] {
            RouteDslStep::ResponseMarshal(m) => {
                assert_eq!(m.marshal, "form");
                assert!(m.config.is_none());
            }
            other => panic!("expected Marshal at idx 3, got {other:?}"),
        }
        match &routes[0].steps[4] {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.key, "Content-Type");
                assert_eq!(h.set_header.value, Some(serde_json::json!(FORM_MEDIA)));
            }
            other => panic!("expected SetHeader at idx 4, got {other:?}"),
        }
        match routes[0].steps.last().unwrap() {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.key, "CamelHttpResponseCode");
                assert_eq!(h.set_header.value, Some(serde_json::json!(201)));
            }
            other => panic!("expected SetHeaderIfAbsent last, got {other:?}"),
        }
    }

    #[test]
    fn form_get_no_unmarshal() {
        let mut rest = make_rest("getUser", "get", "/{id}", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Form);
        rest.operations[0].consumes = FORM_MEDIA.to_string();
        rest.operations[0].produces = FORM_MEDIA.to_string();
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let steps = &routes[0].steps;
        assert!(
            matches!(steps.first(), Some(RouteDslStep::ContentNegotiation(_))),
            "form GET must start with the negotiation step, got {steps:?}"
        );
        let marshal = steps.iter().find_map(|s| match s {
            RouteDslStep::ResponseMarshal(m) => Some(m),
            _ => None,
        });
        assert_eq!(
            marshal.map(|m| m.marshal.as_str()),
            Some("form"),
            "form GET must still marshal the response as form"
        );
        assert!(
            !steps
                .iter()
                .any(|s| matches!(s, RouteDslStep::Unmarshal(_))),
            "GET must not emit an unmarshal step"
        );
    }

    #[test]
    fn form_request_schema_carried() {
        let mut rest = make_rest("createUser", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Form);
        rest.operations[0].consumes = FORM_MEDIA.to_string();
        rest.operations[0].produces = FORM_MEDIA.to_string();
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        });
        rest.operations[0].request_schema = Some(schema.clone());
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let unmarshal = routes[0]
            .steps
            .iter()
            .find_map(|s| match s {
                RouteDslStep::Unmarshal(u) => Some(u),
                _ => None,
            })
            .expect("POST must inject unmarshal step");
        assert_eq!(unmarshal.unmarshal, "form");
        assert_eq!(unmarshal.schema, Some(schema));
    }

    // ── Task 1.2: xml binding media acceptance and lowering ──

    const XML_MEDIA: &str = "application/xml";

    #[test]
    fn xml_binding_exact_media_loads() {
        let mut rest = make_rest("createXml", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
        rest.operations[0].consumes = XML_MEDIA.to_string();
        rest.operations[0].produces = XML_MEDIA.to_string();
        assert!(lower_all_rest_to_routes(&[rest]).is_ok());
    }

    #[test]
    fn xml_binding_text_xml_loads() {
        let mut rest = make_rest("createXml", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
        rest.operations[0].consumes = "text/xml".to_string();
        rest.operations[0].produces = "text/xml".to_string();
        assert!(lower_all_rest_to_routes(&[rest]).is_ok());
    }

    #[test]
    fn xml_binding_structured_suffix_loads() {
        let mut rest = make_rest("createXml", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
        rest.operations[0].consumes = "application/atom+xml".to_string();
        rest.operations[0].produces = "application/atom+xml".to_string();
        assert!(lower_all_rest_to_routes(&[rest]).is_ok());
    }

    #[test]
    fn xml_binding_charset_utf8_accepted() {
        let mut rest = make_rest("createXml", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
        rest.operations[0].consumes = "application/xml; charset=utf-8".to_string();
        rest.operations[0].produces = XML_MEDIA.to_string();
        assert!(lower_all_rest_to_routes(&[rest]).is_ok());
    }

    #[test]
    fn xml_binding_non_xml_media_fails() {
        for offending in ["application/json", "text/plain"] {
            let mut rest = make_rest("createXml", "post", "/", "bean:create");
            rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
            rest.operations[0].consumes = offending.to_string();
            rest.operations[0].produces = XML_MEDIA.to_string();
            let err = lower_all_rest_to_routes(&[rest])
                .err()
                .expect("non-XML consumes with xml binding must be rejected");
            assert!(matches!(err, CamelError::RouteError(_)), "got: {err:?}");
            let msg = err.to_string();
            for needle in ["createXml", "consumes", offending, "application/xml"] {
                assert!(
                    msg.contains(needle),
                    "message must contain {needle:?}: {msg}"
                );
            }
        }
    }

    #[test]
    fn xml_binding_non_utf8_charset_fails() {
        let mut rest = make_rest("createXml", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
        rest.operations[0].consumes = XML_MEDIA.to_string();
        rest.operations[0].produces = "application/xml; charset=iso-8859-1".to_string();
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("non-UTF-8 charset with xml binding must be rejected");
        assert!(matches!(err, CamelError::RouteError(_)), "got: {err:?}");
        let msg = err.to_string();
        for needle in ["createXml", "application/xml; charset=iso-8859-1"] {
            assert!(
                msg.contains(needle),
                "message must contain {needle:?}: {msg}"
            );
        }
    }

    #[test]
    fn xml_binding_empty_prefix_suffix_fails() {
        let mut rest = make_rest("createXml", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
        rest.operations[0].consumes = "application/+xml".to_string();
        rest.operations[0].produces = XML_MEDIA.to_string();
        let err = lower_all_rest_to_routes(&[rest])
            .err()
            .expect("'+xml' with an empty prefix must be rejected");
        assert!(matches!(err, CamelError::RouteError(_)), "got: {err:?}");
        let msg = err.to_string();
        for needle in ["createXml", "application/+xml"] {
            assert!(
                msg.contains(needle),
                "message must contain {needle:?}: {msg}"
            );
        }
    }

    #[test]
    fn xml_binding_allows_request_and_response_schema() {
        let mut rest = make_rest("createXml", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
        rest.operations[0].consumes = XML_MEDIA.to_string();
        rest.operations[0].produces = XML_MEDIA.to_string();
        let schema = serde_json::json!({ "type": "object" });
        rest.operations[0].request_schema = Some(schema.clone());
        rest.operations[0].response = Some(RouteDslRestResponse {
            description: None,
            schema: Some(serde_json::json!({ "type": "object" })),
            headers: BTreeMap::new(),
        });
        let routes = lower_all_rest_to_routes(&[rest])
            .expect("xml binding must allow request_schema and response.schema");
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
    fn xml_post_step_sequence() {
        let mut rest = make_rest("createXml", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
        rest.operations[0].consumes = XML_MEDIA.to_string();
        rest.operations[0].produces = XML_MEDIA.to_string();
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(
            step_kind_names(&routes[0].steps),
            vec![
                "ContentNegotiation",
                "Unmarshal",
                "To",
                "ResponseMarshal",
                "SetHeader",
                "SetHeaderIfAbsent"
            ],
            "xml binding must mirror the form step sequence"
        );
        match &routes[0].steps[1] {
            RouteDslStep::Unmarshal(u) => assert_eq!(u.unmarshal, "xml"),
            other => panic!("expected Unmarshal(xml) at idx 1, got {other:?}"),
        }
        match &routes[0].steps[3] {
            RouteDslStep::ResponseMarshal(m) => assert_eq!(m.marshal, "xml"),
            other => panic!("expected Marshal(xml) at idx 3, got {other:?}"),
        }
    }

    #[test]
    fn xml_get_no_unmarshal() {
        let mut rest = make_rest("getXml", "get", "/{id}", "bean:svc");
        rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
        rest.operations[0].consumes = XML_MEDIA.to_string();
        rest.operations[0].produces = XML_MEDIA.to_string();
        rest.operations[0].request_schema = Some(serde_json::json!({ "type": "object" }));
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let steps = &routes[0].steps;
        assert!(
            matches!(steps.first(), Some(RouteDslStep::ContentNegotiation(_))),
            "xml GET must start with the negotiation step, got {steps:?}"
        );
        assert!(
            !steps
                .iter()
                .any(|s| matches!(s, RouteDslStep::Unmarshal(_))),
            "GET must not emit an unmarshal step"
        );
        let marshal = steps.iter().find_map(|s| match s {
            RouteDslStep::ResponseMarshal(m) => Some(m),
            _ => None,
        });
        assert_eq!(
            marshal.map(|m| m.marshal.as_str()),
            Some("xml"),
            "xml GET must still marshal the response as xml"
        );
    }

    #[test]
    fn xml_request_schema_carried() {
        let mut rest = make_rest("createXml", "post", "/", "bean:create");
        rest.operations[0].binding = Some(RouteDslRestBinding::Xml);
        rest.operations[0].consumes = XML_MEDIA.to_string();
        rest.operations[0].produces = XML_MEDIA.to_string();
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        });
        rest.operations[0].request_schema = Some(schema.clone());
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        let unmarshal = routes[0]
            .steps
            .iter()
            .find_map(|s| match s {
                RouteDslStep::Unmarshal(u) => Some(u),
                _ => None,
            })
            .expect("POST must inject unmarshal step");
        assert_eq!(unmarshal.unmarshal, "xml");
        assert_eq!(unmarshal.schema, Some(schema));
    }

    #[test]
    fn unknown_binding_still_fails_deserialize() {
        let yaml = r#"
rest:
  - path: /api
    operations:
      - method: GET
        to: direct:handler
        binding: multipart
"#;
        let err = noyalib::compat::serde_yaml::from_str::<crate::route_ast::RouteDslRoutes>(yaml)
            .err()
            .expect("unknown binding value must fail deserialization");
        let msg = err.to_string();
        assert!(msg.contains("unknown variant"), "got: {msg}");
        assert!(msg.contains("form"), "got: {msg}");
    }

    // ── Task 3.3: strict content-negotiation lowering ──

    #[test]
    fn lower_json_post_sequence_has_negotiation_first() {
        let rest = make_rest("op1", "post", "/x", "direct:t");
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(
            step_kind_names(&routes[0].steps),
            vec![
                "ContentNegotiation",
                "Unmarshal",
                "To",
                "ResponseMarshal",
                "SetHeader",
                "SetHeaderIfAbsent"
            ]
        );
        match &routes[0].steps[0] {
            RouteDslStep::ContentNegotiation(n) => {
                assert_eq!(n.consumes, "application/json");
                assert_eq!(n.produces, "application/json");
                assert!(
                    n.check_content_type,
                    "POST carries a body: the Content-Type (415) gate must be on"
                );
            }
            other => panic!("expected ContentNegotiation at idx 0, got {other:?}"),
        }
        match &routes[0].steps[1] {
            RouteDslStep::Unmarshal(u) => {
                assert_eq!(u.unmarshal, "json");
                assert!(u.schema.is_none());
            }
            other => panic!("expected Unmarshal(json) at idx 1, got {other:?}"),
        }
        match &routes[0].steps[2] {
            RouteDslStep::To(t) => assert_eq!(t.to, "direct:t"),
            other => panic!("expected To(direct:t) at idx 2, got {other:?}"),
        }
        match &routes[0].steps[3] {
            RouteDslStep::ResponseMarshal(m) => assert_eq!(m.marshal, "json"),
            other => panic!("expected Marshal(json) at idx 3, got {other:?}"),
        }
        match &routes[0].steps[4] {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.key, "Content-Type");
                assert_eq!(
                    h.set_header.value,
                    Some(serde_json::json!("application/json"))
                );
            }
            other => panic!("expected SetHeader(Content-Type) at idx 4, got {other:?}"),
        }
        match routes[0].steps.last().unwrap() {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.key, "CamelHttpResponseCode");
                assert_eq!(h.set_header.value, Some(serde_json::json!(201)));
            }
            other => panic!("expected SetHeaderIfAbsent(201) last, got {other:?}"),
        }
    }

    #[test]
    fn lower_raw_sequence_has_negotiation_first() {
        let mut rest = make_rest("op1", "post", "/x", "direct:t");
        rest.operations[0].binding = Some(RouteDslRestBinding::Raw);
        rest.operations[0].consumes = "application/octet-stream".to_string();
        rest.operations[0].produces = "image/png".to_string();
        let routes = lower_all_rest_to_routes(&[rest]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(
            step_kind_names(&routes[0].steps),
            vec!["ContentNegotiation", "To", "SetHeader", "SetHeaderIfAbsent"],
            "raw mode: negotiation gate first, no Unmarshal/Marshal"
        );
        match &routes[0].steps[0] {
            RouteDslStep::ContentNegotiation(n) => {
                assert_eq!(n.consumes, "application/octet-stream");
                assert_eq!(n.produces, "image/png");
                assert!(
                    n.check_content_type,
                    "POST carries a body: the Content-Type (415) gate must be on"
                );
            }
            other => panic!("expected ContentNegotiation at idx 0, got {other:?}"),
        }
        match &routes[0].steps[1] {
            RouteDslStep::To(t) => assert_eq!(t.to, "direct:t"),
            other => panic!("expected To(direct:t) at idx 1, got {other:?}"),
        }
        match &routes[0].steps[2] {
            RouteDslStep::SetHeader(h) => {
                assert_eq!(h.set_header.key, "Content-Type");
                assert_eq!(h.set_header.value, Some(serde_json::json!("image/png")));
            }
            other => panic!("expected SetHeader(Content-Type: image/png) at idx 2, got {other:?}"),
        }
        match routes[0].steps.last().unwrap() {
            RouteDslStep::SetHeaderIfAbsent(h) => {
                assert_eq!(h.set_header.key, "CamelHttpResponseCode");
                assert_eq!(
                    h.set_header.value,
                    Some(serde_json::json!(201)),
                    "POST default status is binding-independent"
                );
            }
            other => panic!("expected SetHeaderIfAbsent last, got {other:?}"),
        }
    }

    // ── Task 2.2: (L, C) specificity + overlap-based ambiguity ──

    fn segments_of(template: &str) -> Vec<SegmentPattern> {
        parse_path_template(template).expect("test template is well formed")
    }

    fn rest_block(base: &str, ops: Vec<RouteDslRestOperation>) -> RouteDslRest {
        RouteDslRest {
            error_mapping: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            path: base.to_string(),
            security_policy: None,
            operations: ops,
        }
    }

    fn op_on(operation_id: &str, verb: &str, path: &str) -> RouteDslRestOperation {
        RouteDslRestOperation {
            method: verb.to_string(),
            path: path.to_string(),
            ..make_op(operation_id)
        }
    }

    fn assert_names_both(err: &CamelError, needles: [&str; 4]) {
        let msg = err.to_string();
        for needle in needles {
            assert!(
                msg.contains(needle),
                "message must contain {needle:?}: {msg}"
            );
        }
    }

    #[test]
    fn ambiguous_equal_specificity_mixed_pair() {
        assert!(
            templates_ambiguous(&segments_of("/{id}.xml"), &segments_of("/{name}.xml")),
            "mixed templates of equal (L, C) that share a request must be ambiguous"
        );
    }

    #[test]
    fn ambiguous_despite_part_order() {
        assert!(
            templates_ambiguous(&segments_of("/ab{x}"), &segments_of("/a{y}b")),
            "both templates match /abb, so part order must not hide the overlap"
        );
    }

    #[test]
    fn ambiguous_prefix_suffix_forms() {
        assert!(
            templates_ambiguous(&segments_of("/a{x}"), &segments_of("/{y}a")),
            "both templates match /aa"
        );
    }

    #[test]
    fn ambiguous_mirrored_segment_literals() {
        assert!(
            templates_ambiguous(&segments_of("/x/{id}"), &segments_of("/{a}/y")),
            "both templates match /x/y"
        );
    }

    #[test]
    fn not_ambiguous_different_specificity() {
        assert!(
            !templates_ambiguous(&segments_of("/{p}y"), &segments_of("/{q}xy")),
            "specificity (1, 1) vs (1, 2): the longer literal wins"
        );
    }

    #[test]
    fn not_ambiguous_disjoint_literals() {
        assert!(
            !templates_ambiguous(&segments_of("/a{x}"), &segments_of("/b{y}")),
            "no request can match both"
        );
    }

    #[test]
    fn overlap_normalizes_empty_literal_parts() {
        let empty = SegmentPattern {
            parts: vec![SegmentPart::Lit(String::new())],
        };
        assert!(
            segment_patterns_overlap(&empty, &empty),
            "two empty-literal patterns both accept the empty string"
        );

        let with_holes = SegmentPattern {
            parts: vec![
                SegmentPart::Lit(String::new()),
                SegmentPart::Param("x".to_string()),
                SegmentPart::Lit(String::new()),
            ],
        };
        let bare_param = SegmentPattern {
            parts: vec![SegmentPart::Param("y".to_string())],
        };
        assert!(
            segment_patterns_overlap(&with_holes, &bare_param),
            "empty literals around a placeholder vanish, leaving one bare hole"
        );
    }

    #[test]
    fn lowering_rejects_cross_block_same_listener_ambiguity() {
        let a = make_rest_with_base("get_xml", "get", "/", "/{id}.xml", "bean:a");
        let b = make_rest_with_base("get_xml2", "get", "/", "/{name}.xml", "bean:b");
        let err = lower_all_rest_to_routes(&[a, b])
            .err()
            .expect("ambiguous templates across blocks must be rejected");
        assert_names_both(&err, ["/{id}.xml", "/{name}.xml", "get_xml2", "get_xml"]);
    }

    #[test]
    fn lowering_error_names_both_operations_same_block() {
        let block = rest_block(
            "/",
            vec![
                op_on("getAbx", "get", "/ab{x}"),
                op_on("getAyb", "get", "/a{y}b"),
            ],
        );
        let err = lower_all_rest_to_routes(&[block])
            .err()
            .expect("overlapping mixed templates with distinct op ids must be rejected");
        assert_names_both(&err, ["/ab{x}", "/a{y}b", "getAbx", "getAyb"]);
    }

    #[test]
    fn lowering_rejects_prefix_suffix_ambiguity() {
        let block = rest_block(
            "/",
            vec![
                op_on("getAx", "get", "/a{x}"),
                op_on("getYa", "get", "/{y}a"),
            ],
        );
        let err = lower_all_rest_to_routes(&[block])
            .err()
            .expect("prefix/suffix forms both matching /aa must be rejected");
        assert_names_both(&err, ["/a{x}", "/{y}a", "getAx", "getYa"]);
    }

    #[test]
    fn lowering_rejects_mirrored_segments_ambiguity() {
        let block = rest_block(
            "/",
            vec![
                op_on("getXId", "get", "/x/{id}"),
                op_on("getAY", "get", "/{a}/y"),
            ],
        );
        let err = lower_all_rest_to_routes(&[block])
            .err()
            .expect("mirrored segment literals both matching /x/y must be rejected");
        assert_names_both(&err, ["/x/{id}", "/{a}/y", "getXId", "getAY"]);
    }

    #[test]
    fn lowering_accepts_mixed_templates() {
        let block = rest_block(
            "/",
            vec![
                op_on("getFileXml", "get", "/files/{id}.xml"),
                op_on("postReportJson", "post", "/report-{date}.json"),
            ],
        );
        let routes = lower_all_rest_to_routes(&[block]).expect("valid mixed templates must load");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn lowering_allows_different_listener_and_method() {
        let a = make_rest_on_listener(
            "0.0.0.0",
            8080,
            "get_xml",
            "get",
            "/",
            "/{id}.xml",
            "bean:a",
        );
        let b = make_rest_on_listener(
            "0.0.0.0",
            9090,
            "get_xml2",
            "get",
            "/",
            "/{name}.xml",
            "bean:b",
        );
        let routes =
            lower_all_rest_to_routes(&[a, b]).expect("different listeners are independent");
        assert_eq!(routes.len(), 2);

        let get = make_rest_with_base("get_xml", "get", "/", "/{id}.xml", "bean:g");
        let post = make_rest_with_base("post_xml", "post", "/", "/{name}.xml", "bean:p");
        let routes = lower_all_rest_to_routes(&[get, post]).expect("ambiguity is per method");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn lowering_accepts_resolvable_overlap_pairs() {
        let block = rest_block(
            "/",
            vec![
                op_on("getAxy", "get", "/a/{x}/{y}"),
                op_on("getUbc", "get", "/{u}/b/c"),
            ],
        );
        let routes =
            lower_all_rest_to_routes(&[block]).expect("differing specificity resolves the overlap");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn lower_bodyless_verbs_carry_flag_false() {
        for (verb, op_id) in [("get", "opGet"), ("delete", "opDelete")] {
            let rest = make_rest(op_id, verb, "/x", "direct:t");
            let routes = lower_all_rest_to_routes(&[rest]).unwrap();
            assert_eq!(routes.len(), 1);
            match &routes[0].steps[0] {
                RouteDslStep::ContentNegotiation(n) => {
                    assert!(
                        !n.check_content_type,
                        "{verb} carries no body: the Content-Type (415) gate must be off"
                    );
                    assert_eq!(n.consumes, "application/json");
                    assert_eq!(n.produces, "application/json");
                }
                other => {
                    panic!("expected ContentNegotiation first for {verb}, got {other:?}")
                }
            }
            assert!(
                !step_kind_names(&routes[0].steps)
                    .iter()
                    .any(|name| name == "Unmarshal"),
                "{verb} must not emit Unmarshal"
            );
        }
    }
}
