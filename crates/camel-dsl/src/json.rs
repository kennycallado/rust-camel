//! JSON route definition parser.
//!
//! Reuses the same AST types as the YAML parser ([`crate::yaml::RouteDslRoutes`],
//! [`crate::yaml::RouteDslRoute`], [`crate::yaml::RouteDslStep`])
//! since JSON is a subset of YAML's data model. The only difference is the deserializer.
//!
//! ## Stability note
//!
//! The parser uses [`RouteDslRoutes`] directly (no separate JSON-specific AST types).
//! The `RouteDsl*` AST types are **not a stable SDK contract**. SDKs and external
//! consumers should target [`CanonicalRouteSpec`] for forward compatibility.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use camel_api::{CamelError, CanonicalRouteSpec};
use camel_core::route::RouteDefinition;

use crate::compile::{
    compile_declarative_route, compile_declarative_route_to_canonical,
    compile_declarative_route_with_stream_cache_threshold,
};
use crate::env_interpolation::ProvenancePath;
use crate::input_format::{InputFormat, annotate_format};
use crate::model::SecurityCompileContext;
use crate::yaml::{
    RouteDslRoutes, RoutesEnvError, ScriptLoadContext, route_dsl_to_declarative_route,
};

/// Parse a JSON string into declarative route models.
pub fn parse_json_to_declarative(
    json: &str,
) -> Result<Vec<crate::model::DeclarativeRoute>, CamelError> {
    annotate_format(
        InputFormat::Json,
        parse_json_to_declarative_inner(json, &ScriptLoadContext::new(None)),
    )
}

/// Context-aware twin of [`parse_json_to_declarative`] for the template
/// materializer: materialized route JSON lowers `source_file` against the
/// TEMPLATE document's directory instead of CWD.
pub(crate) fn parse_json_to_declarative_with_script_ctx(
    json: &str,
    script_ctx: &ScriptLoadContext,
) -> Result<Vec<crate::model::DeclarativeRoute>, CamelError> {
    annotate_format(
        InputFormat::Json,
        parse_json_to_declarative_inner(json, script_ctx),
    )
}

fn parse_json_to_declarative_inner(
    json: &str,
    script_ctx: &ScriptLoadContext,
) -> Result<Vec<crate::model::DeclarativeRoute>, CamelError> {
    // Unknown keys inside `transacted:` blocks must be named, which the
    // untagged `RouteDslStep` enum cannot do — same raw pre-scan as the YAML
    // loader. JSON is a YAML 1.2 subset, so the `serde_yml::Value` walk
    // applies unchanged; malformed JSON falls through (the pre-scan returns
    // `Ok`) to the typed parse, which owns the error.
    crate::route_ast::reject_unknown_transacted_keys(json).map_err(CamelError::RouteError)?;

    let mut dsl: RouteDslRoutes = serde_json::from_str(json)
        .map_err(|e| CamelError::RouteError(format!("JSON parse error: {e}")))?;

    // Expand REST blocks (review I2): the JSON authoring path previously
    // dropped `rest:` silently. JSON is a full-DSL authoring format
    // (ADR-0026), so it lowers `rest:` exactly like the YAML path — via the
    // shared helper that also runs cross-block validation.
    crate::rest::expand_rest_into(&mut dsl.routes, &dsl.rest)?;
    crate::mcp::expand_mcp_into(&mut dsl.routes, &dsl.mcp)?;
    crate::rest::check_duplicate_route_ids(&dsl.routes)?;

    dsl.routes
        .into_iter()
        .map(|route| route_dsl_to_declarative_route(route, script_ctx))
        .collect()
}

/// Parse a JSON string into compiled [`RouteDefinition`]s.
pub fn parse_json(json: &str) -> Result<Vec<RouteDefinition>, CamelError> {
    annotate_format(
        InputFormat::Json,
        parse_json_inner(json, &ScriptLoadContext::new(None)),
    )
}

fn parse_json_inner(
    json: &str,
    script_ctx: &ScriptLoadContext,
) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_json_to_declarative_inner(json, script_ctx)?
        .into_iter()
        .map(compile_declarative_route)
        .collect()
}

/// Parse a JSON string with a custom stream-cache threshold.
pub fn parse_json_with_threshold(
    json: &str,
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, CamelError> {
    annotate_format(
        InputFormat::Json,
        parse_json_with_threshold_inner(
            json,
            stream_cache_threshold,
            &ScriptLoadContext::new(None),
        ),
    )
}

fn parse_json_with_threshold_inner(
    json: &str,
    stream_cache_threshold: usize,
    script_ctx: &ScriptLoadContext,
) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_json_with_threshold_and_security_inner(
        json,
        stream_cache_threshold,
        SecurityCompileContext::default(),
        script_ctx,
    )
}

pub fn parse_json_with_threshold_and_security(
    json: &str,
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_json_with_threshold_and_security_with_script_ctx(
        json,
        stream_cache_threshold,
        security_ctx,
        &ScriptLoadContext::new(None),
    )
}

/// Context-aware twin of [`parse_json_with_threshold_and_security`] for
/// discovery, which owns the [`ScriptLoadContext`] whose recorded scripts it
/// folds into the document source hash. The public wrapper parses with CWD
/// semantics.
pub(crate) fn parse_json_with_threshold_and_security_with_script_ctx(
    json: &str,
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
    script_ctx: &ScriptLoadContext,
) -> Result<Vec<RouteDefinition>, CamelError> {
    annotate_format(
        InputFormat::Json,
        parse_json_with_threshold_and_security_inner(
            json,
            stream_cache_threshold,
            security_ctx,
            script_ctx,
        ),
    )
}

fn parse_json_with_threshold_and_security_inner(
    json: &str,
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
    script_ctx: &ScriptLoadContext,
) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_json_to_declarative_inner(json, script_ctx)?
        .into_iter()
        .map(|route| {
            compile_declarative_route_with_stream_cache_threshold(
                route,
                stream_cache_threshold,
                security_ctx.clone(),
            )
        })
        .collect()
}

/// Quiet twin of [`parse_json_with_threshold_and_security`]: the same staged
/// deserialize + threshold/security-aware lowering, sharing ONLY
/// [`parse_json_to_declarative_inner`] with the logging parser. It exists for
/// the loader-layer typed probe (`env_int_probe`), whose speculative attempts
/// are expected failures: [`crate::env_int_probe::QuietCompileGuard`] drops
/// every `warn!`/`error!` the step compilers would otherwise repeat once per
/// candidate subset. Final failures are the CALLER's to surface (with the
/// original first-pass error, never this one).
pub(crate) fn parse_json_with_threshold_and_security_quiet(
    json: &str,
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
    script_ctx: &ScriptLoadContext,
) -> Result<Vec<RouteDefinition>, CamelError> {
    let _quiet = crate::env_int_probe::QuietCompileGuard::new();
    parse_json_to_declarative_inner(json, script_ctx)?
        .into_iter()
        .map(|route| {
            compile_declarative_route_with_stream_cache_threshold(
                route,
                stream_cache_threshold,
                security_ctx.clone(),
            )
        })
        .collect()
}

/// Whether any JSON object literal in `text` declares the same key twice at
/// the same nesting level. Keys are compared DECODED (`"a"` and `"\u0061"`
/// are the same key), matching `serde_json`'s map-insert behavior; equal keys
/// in distinct objects are not duplicates.
///
/// `serde_json`'s deserializer silently collapses duplicate keys, so the
/// repair gate needs its own raw-text scan: a compact recursive scanner with
/// in-string/in-escape state tracking (precedent: the raw pre-scan shape of
/// [`crate::route_ast::reject_unknown_transacted_keys`]). Malformed text
/// yields `false` — the caller's syntax gate owns that verdict, and the
/// typed parse owns the error text.
pub(crate) fn json_has_duplicate_keys(text: &str) -> bool {
    let mut pos = 0;
    skip_json_whitespace(text, &mut pos);
    scan_json_value(text, &mut pos).unwrap_or(false)
}

fn skip_json_whitespace(text: &str, pos: &mut usize) {
    while let Some(byte) = text.as_bytes().get(*pos) {
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
            *pos += 1;
        } else {
            break;
        }
    }
}

/// Scan one JSON value; `Some(has_duplicate_keys)` on success, `None` when
/// the text is not a well-formed value at `pos`.
fn scan_json_value(text: &str, pos: &mut usize) -> Option<bool> {
    skip_json_whitespace(text, pos);
    match *text.as_bytes().get(*pos)? {
        b'{' => scan_json_object(text, pos),
        b'[' => scan_json_array(text, pos),
        b'"' => {
            scan_json_string(text, pos)?;
            Some(false)
        }
        b't' | b'f' | b'n' | b'-' | b'0'..=b'9' => {
            skip_json_literal(text, pos);
            Some(false)
        }
        _ => None,
    }
}

fn scan_json_object(text: &str, pos: &mut usize) -> Option<bool> {
    *pos += 1; // consume '{'
    let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut duplicates = false;
    skip_json_whitespace(text, pos);
    if text.as_bytes().get(*pos) == Some(&b'}') {
        *pos += 1;
        return Some(false);
    }
    loop {
        skip_json_whitespace(text, pos);
        let key = scan_json_string(text, pos)?;
        if !keys.insert(key) {
            duplicates = true;
        }
        skip_json_whitespace(text, pos);
        if text.as_bytes().get(*pos) != Some(&b':') {
            return None;
        }
        *pos += 1;
        duplicates |= scan_json_value(text, pos)?;
        skip_json_whitespace(text, pos);
        match text.as_bytes().get(*pos) {
            Some(&b',') => *pos += 1,
            Some(&b'}') => {
                *pos += 1;
                return Some(duplicates);
            }
            _ => return None,
        }
    }
}

fn scan_json_array(text: &str, pos: &mut usize) -> Option<bool> {
    *pos += 1; // consume '['
    skip_json_whitespace(text, pos);
    if text.as_bytes().get(*pos) == Some(&b']') {
        *pos += 1;
        return Some(false);
    }
    let mut duplicates = false;
    loop {
        duplicates |= scan_json_value(text, pos)?;
        skip_json_whitespace(text, pos);
        match text.as_bytes().get(*pos) {
            Some(&b',') => *pos += 1,
            Some(&b']') => {
                *pos += 1;
                return Some(duplicates);
            }
            _ => return None,
        }
    }
}

/// Scan a JSON string literal starting at `pos` and return its DECODED
/// content (escapes resolved, surrogate pairs combined).
fn scan_json_string(text: &str, pos: &mut usize) -> Option<String> {
    if text.as_bytes().get(*pos) != Some(&b'"') {
        return None;
    }
    *pos += 1;
    let mut decoded = String::new();
    loop {
        let ch = text.get(*pos..)?.chars().next()?;
        match ch {
            '"' => {
                *pos += 1;
                return Some(decoded);
            }
            '\\' => {
                *pos += 1;
                let escape = text.get(*pos..)?.chars().next()?;
                *pos += escape.len_utf8();
                match escape {
                    '"' => decoded.push('"'),
                    '\\' => decoded.push('\\'),
                    '/' => decoded.push('/'),
                    'b' => decoded.push('\u{8}'),
                    'f' => decoded.push('\u{c}'),
                    'n' => decoded.push('\n'),
                    'r' => decoded.push('\r'),
                    't' => decoded.push('\t'),
                    'u' => {
                        let unit = scan_json_hex4(text, pos)?;
                        let scalar = if (0xD800..=0xDBFF).contains(&unit)
                            && text.as_bytes().get(*pos) == Some(&b'\\')
                            && text.as_bytes().get(*pos + 1) == Some(&b'u')
                        {
                            *pos += 2;
                            let low = scan_json_hex4(text, pos)?;
                            if (0xDC00..=0xDFFF).contains(&low) {
                                0x1_0000
                                    + ((u32::from(unit) - 0xD800) << 10)
                                    + (u32::from(low) - 0xDC00)
                            } else {
                                u32::from(unit)
                            }
                        } else {
                            u32::from(unit)
                        };
                        decoded.push(char::from_u32(scalar).unwrap_or(char::REPLACEMENT_CHARACTER));
                    }
                    _ => return None,
                }
            }
            _ => {
                decoded.push(ch);
                *pos += ch.len_utf8();
            }
        }
    }
}

fn scan_json_hex4(text: &str, pos: &mut usize) -> Option<u16> {
    let mut value: u16 = 0;
    for _ in 0..4 {
        let digit = text.get(*pos..)?.chars().next()?.to_digit(16)?;
        value = value * 16 + u16::try_from(digit).ok()?;
        *pos += 1;
    }
    Some(value)
}

/// Skip a JSON literal (`true`/`false`/`null`) or number token.
fn skip_json_literal(text: &str, pos: &mut usize) {
    while let Some(byte) = text.as_bytes().get(*pos) {
        if matches!(byte, b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r') {
            break;
        }
        *pos += 1;
    }
}

/// Probe closure shape for [`repair_json_with_probe`]: given the
/// splice-substituted text and the tree-walk provenance, run the bounded
/// coercion search with the caller's quiet oracle. Callers own the search
/// ([`crate::env_int_probe::parse_json_with_probe`] for routes,
/// [`crate::env_int_probe::parse_json_with_probe_with_text`] when the winning
/// document text is also needed).
pub(crate) type RepairProbe<'a, R> = dyn Fn(&str, &[ProvenancePath]) -> Result<R, CamelError> + 'a;

/// ORACLE-GENERIC JSON repair pass shared by every JSON document arm (the
/// string seam here, discovery, and the OpenAPI REST extract): the SAME gate
/// sequence runs for all of them, so no arm can repair a document another
/// refuses.
///
/// Gates, in order (any failure means "no repair applies" — the caller keeps
/// its first-pass error):
/// 1. the splice-substituted text parses as JSON and carries no duplicate
///    object keys ([`json_has_duplicate_keys`]); duplicate-key documents are
///    never repaired, because key-sorted `serde_json` maps cannot preserve
///    authored overwrite order;
/// 2. the tree-walk document exists (`Some` provenance — a fallback document
///    has no baseline) and its parsed tree EQUALS the splice-parsed tree
///    (`serde_json::Value` equality): escape sanitization or any other
///    normalization drift means no repair, so a repair never silently changes
///    a substituted value;
/// 3. at least one coercion candidate exists
///    ([`crate::env_int_probe::json_has_probe_candidates`]): a zero-candidate
///    repair pass re-parses nothing.
///
/// Then the bounded smallest-first probe runs through the caller's QUIET
/// `probe` closure, which receives the spliced text AND the tree-walk
/// provenance: the closure owns the search (the JSON route seam's plain
/// [`crate::env_int_probe::parse_json_with_probe`], discovery's
/// text-returning twin), so one gate sequence serves every arm and no arm
/// forks its own probe loop. `Ok(Some(result))` when a coercion subset makes
/// the probe succeed; `Ok(None)` when no repair applies or every subset
/// fails; `Err(variable)` only when the tree walk hits an unresolved
/// placeholder — the caller maps it onto its own unresolved-variable error
/// type.
pub(crate) fn repair_json_with_probe<R>(
    spliced: &str,
    raw: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    probe: &RepairProbe<'_, R>,
) -> Result<Option<R>, String> {
    // Gate 1: legacy semantic baseline (syntax + duplicate keys).
    let Ok(spliced_tree) = serde_json::from_str::<serde_json::Value>(spliced) else {
        return Ok(None);
    };
    if json_has_duplicate_keys(spliced) {
        return Ok(None);
    }
    // Gate 2: the tree walk must reproduce the splice tree value-for-value.
    let (walk_text, Some(provenance)) =
        crate::env_interpolation::interpolate_json_source_with_provenance(raw, lookup)?
    else {
        return Ok(None);
    };
    let Ok(walk_tree) = serde_json::from_str::<serde_json::Value>(&walk_text) else {
        return Ok(None);
    };
    if spliced_tree != walk_tree {
        return Ok(None);
    }
    // Gate 3: a repair pass with zero candidates re-parses nothing.
    if !crate::env_int_probe::json_has_probe_candidates(spliced, &provenance) {
        return Ok(None);
    }
    match probe(spliced, &provenance) {
        Ok(result) => Ok(Some(result)),
        Err(_) => Ok(None),
    }
}

/// `${env:}`-aware JSON route-document seam: the JSON sibling of
/// [`crate::yaml::parse_routes_with_env`], with the same typed error surface.
///
/// PASS 1 is the legacy whole-text splice
/// ([`crate::env_interpolation::interpolate_env_with`]) followed by today's
/// threshold/security parse — values, error text, and warnings are unchanged
/// for every document that loads today (the splice and the tree walk disagree
/// on escape decoding, error positions, and syntax-breaking defaults, so the
/// legacy pass stays first).
///
/// REPAIR runs only after pass 1 fails, through the oracle-generic
/// `repair_json_with_probe` helper (the SAME gates and candidate rules as
/// discovery and the OpenAPI extract) with the quiet
/// `parse_json_with_threshold_and_security_quiet` oracle. A winning subset
/// loads; otherwise the ORIGINAL pass-1 error is returned — no replay: the
/// JSON chain has no logging deserializer, so pass 1 already emitted today's
/// output and the speculative compiles are silenced by the quiet guard.
///
/// # Unresolved-variable naming
///
/// Pass 1 splices the RAW source text, while the repair walk reads DECODED
/// values, so the two passes can see different placeholder spellings (an
/// escape-hidden `\u0024{env:...}` is invisible to the splice but resolved by
/// the walk). Both use the same scanner and error shape, so a variable either
/// path reports is named identically. A pass-1 unresolved variable returns
/// before repair starts, so that name always wins; the walk's name surfaces
/// only when pass 1 resolved every placeholder it could see.
pub fn parse_json_routes_with_env(
    raw: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<RouteDefinition>, RoutesEnvError> {
    let spliced = crate::env_interpolation::interpolate_env_with(raw, lookup)
        .map_err(RoutesEnvError::Unresolved)?;
    let script_ctx = ScriptLoadContext::new(None);
    let first_error = match parse_json_with_threshold_and_security_with_script_ctx(
        &spliced,
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
        &script_ctx,
    ) {
        Ok(routes) => return Ok(routes),
        Err(error) => error,
    };
    let probe = |probe_text: &str, provenance: &[ProvenancePath]| {
        crate::env_int_probe::parse_json_with_probe(probe_text, Some(provenance), |oracle_text| {
            parse_json_with_threshold_and_security_quiet(
                oracle_text,
                camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
                SecurityCompileContext::default(),
                &script_ctx,
            )
        })
    };
    match repair_json_with_probe(&spliced, raw, lookup, &probe) {
        Ok(Some(routes)) => Ok(routes),
        Ok(None) => Err(RoutesEnvError::Parse(first_error)),
        Err(variable) => Err(RoutesEnvError::Unresolved(variable)),
    }
}

/// Parse a JSON string into canonical route specs.
pub fn parse_json_to_canonical(json: &str) -> Result<Vec<CanonicalRouteSpec>, CamelError> {
    annotate_format(
        InputFormat::Json,
        parse_json_to_canonical_inner(json, &ScriptLoadContext::new(None)),
    )
}

fn parse_json_to_canonical_inner(
    json: &str,
    script_ctx: &ScriptLoadContext,
) -> Result<Vec<CanonicalRouteSpec>, CamelError> {
    let routes = parse_json_to_declarative_inner(json, script_ctx)?;
    for route in &routes {
        if route.security_policy.is_some() {
            return Err(CamelError::RouteError(
                "routes with security_policy cannot use the canonical/hot-reload path (not yet supported)".into(),
            ));
        }
    }
    routes
        .into_iter()
        .map(|r| compile_declarative_route_to_canonical(r, false).map(|(spec, _)| spec))
        .collect()
}

use crate::util::read_route_file_capped;

/// Load a JSON route definition from a file.
///
/// Reads the file contents and parses them as JSON. No environment variable interpolation is performed.
/// Script `source_file` references resolve against the file's directory.
pub fn load_json_from_file(path: &Path) -> Result<Vec<RouteDefinition>, CamelError> {
    let content = read_route_file_capped(path)?;
    // Script `source_file` references resolve against the route file's
    // directory (string-based parse entry points keep CWD semantics).
    let script_ctx = ScriptLoadContext::new(path.parent().map(Path::to_path_buf));
    let annotated = annotate_format(InputFormat::Json, parse_json_inner(&content, &script_ctx));
    let routes = annotated.map_err(|e| match e {
        CamelError::RouteError(msg) => {
            CamelError::RouteError(format!("{msg} (in {})", path.display()))
        }
        other => other,
    })?;
    // File-backed loads have no discovery/reload pass to publish later, so
    // their script dependencies are committed directly.
    let script_paths: BTreeSet<PathBuf> = script_ctx.into_scripts().into_keys().collect();
    let registry = crate::script_watch::ScriptWatchRegistry::global();
    for route in &routes {
        registry.register_direct(route.route_id(), script_paths.clone());
    }
    Ok(routes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeclarativeStep;
    use std::fs::File;

    /// Basic route: parse JSON to declarative model.
    #[test]
    fn test_basic_route_to_declarative() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "test-route",
                    "from": "timer:tick?period=1000",
                    "steps": [
                        {"set_header": {"key": "source", "value": "timer"}},
                        {"to": "log:info"}
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id, "test-route");
        assert_eq!(routes[0].from, "timer:tick?period=1000");
        assert_eq!(routes[0].steps.len(), 2);
    }

    /// Route metadata: auto_startup and startup_order round-trip correctly.
    #[test]
    fn test_route_metadata_auto_startup_and_startup_order() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "meta-route",
                    "from": "direct:start",
                    "auto_startup": false,
                    "startup_order": 42
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        assert_eq!(routes.len(), 1);
        assert!(!routes[0].auto_startup);
        assert_eq!(routes[0].startup_order, 42);
    }

    /// Error handler with on_exceptions including handled_by.
    #[test]
    fn test_error_handler_with_handled_by() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "eh-route",
                    "from": "direct:start",
                    "error_handler": {
                        "dead_letter_channel": "log:dlc",
                        "on_exceptions": [
                            {
                                "kind": "Io",
                                "handled_by": "log:io",
                                "retry": {
                                    "max_attempts": 3
                                }
                            }
                        ]
                    }
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        let eh = routes[0]
            .error_handler
            .as_ref()
            .expect("error handler should be present");
        let clauses = eh
            .on_exceptions
            .as_ref()
            .expect("on_exceptions should be present");
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].kind.as_deref(), Some("Io"));
        let retry = clauses[0].retry.as_ref().expect("retry should be present");
        assert_eq!(retry.max_attempts, 3);
        assert_eq!(clauses[0].handled_by.as_deref(), Some("log:io"));
    }

    /// Empty route id must be rejected.
    #[test]
    fn test_empty_id_fails() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "",
                    "from": "timer:tick"
                }
            ]
        }"#;
        let result = parse_json_to_declarative(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("route 'id' must not be empty"),
            "unexpected error: {err}"
        );
    }

    /// Invalid JSON must produce an error containing "JSON".
    #[test]
    fn test_invalid_json_error_says_json() {
        let json = "{ not valid json }}}";
        let result = parse_json_to_declarative(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("JSON DSL error: JSON parse error:"),
            "expected 'JSON DSL error: JSON parse error:' in error, got: {err}"
        );
    }

    /// Compiled route via parse_json produces a RouteDefinition.
    #[test]
    fn test_compiled_route_via_parse_json() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "compiled-route",
                    "from": "timer:tick",
                    "steps": [
                        {"to": "log:info"}
                    ]
                }
            ]
        }"#;
        let defs = parse_json(json).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "compiled-route");
        assert_eq!(defs[0].from_uri(), "timer:tick");
    }

    /// parse_json_with_threshold compiles routes with custom stream-cache threshold.
    #[test]
    fn test_threshold_parse() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "threshold-route",
                    "from": "timer:tick",
                    "steps": [
                        {"stream_cache": true},
                        {"to": "log:info"}
                    ]
                }
            ]
        }"#;
        let defs = parse_json_with_threshold(json, 8192).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "threshold-route");
    }

    /// Canonical conversion via parse_json_to_canonical.
    #[test]
    fn test_canonical_conversion() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "canonical-v1",
                    "from": "direct:start",
                    "steps": [
                        {"to": "mock:out"},
                        {"log": {"message": "hello"}},
                        {"stop": true}
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_canonical(json).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id, "canonical-v1");
        assert_eq!(routes[0].from, "direct:start");
        assert_eq!(routes[0].version, 2);
        assert_eq!(routes[0].steps.len(), 3);
    }

    /// The "loop" step JSON key works.
    #[test]
    fn test_loop_step_json_key() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "loop-route",
                    "from": "direct:start",
                    "steps": [
                        {"loop": 3}
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        assert_eq!(routes.len(), 1);
        match &routes[0].steps[0] {
            DeclarativeStep::Loop(def) => {
                assert_eq!(def.count, Some(3));
            }
            other => panic!("expected Loop step, got {:?}", other),
        }
    }

    /// Multiple routes in a single JSON document.
    #[test]
    fn test_multiple_routes() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "route-a",
                    "from": "timer:tick",
                    "steps": [{"to": "log:info"}]
                },
                {
                    "id": "route-b",
                    "from": "timer:tock",
                    "auto_startup": false,
                    "startup_order": 10
                }
            ]
        }"#;
        let defs = parse_json(json).unwrap();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].route_id(), "route-a");
        assert_eq!(defs[1].route_id(), "route-b");
    }

    /// Defaults: auto_startup=true, startup_order=1000 when omitted.
    #[test]
    fn test_defaults() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "default-route",
                    "from": "timer:tick"
                }
            ]
        }"#;
        let defs = parse_json(json).unwrap();
        assert!(defs[0].auto_startup());
        assert_eq!(defs[0].startup_order(), 1000);
    }

    /// File loading via load_json_from_file.
    #[test]
    fn test_file_loading() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();

        let json_content = r#"
        {
            "routes": [
                {
                    "id": "file-route",
                    "from": "timer:tick",
                    "steps": [{"to": "log:info"}]
                }
            ]
        }"#;

        file.write_all(json_content.as_bytes()).unwrap();

        let defs = load_json_from_file(file.path()).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "file-route");
    }

    /// Missing file must return an error.
    #[test]
    fn test_missing_file_error() {
        let result = load_json_from_file(Path::new("/nonexistent/path/routes.json"));
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("Failed to read"),
            "expected file read error, got: {err}"
        );
    }

    #[test]
    fn test_function_step_json_parses_and_compiles() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "fn-json-route",
                    "from": "direct:start",
                    "steps": [
                        {
                            "function": {
                                "runtime": "deno",
                                "source": "return { body: 'ok' };",
                                "timeout_ms": 3000
                            }
                        }
                    ]
                }
            ]
        }"#;
        let defs = parse_json(json).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "fn-json-route");
    }

    #[test]
    fn test_function_step_json_compiles_to_declarative_function() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "fn-decl",
                    "from": "direct:start",
                    "steps": [
                        {
                            "function": {
                                "runtime": "deno",
                                "source": "return { body: 1 };"
                            }
                        }
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        match &routes[0].steps[0] {
            DeclarativeStep::Function(def) => {
                assert_eq!(def.runtime, "deno");
                assert_eq!(def.source, "return { body: 1 };");
                assert_eq!(def.timeout_ms, None);
            }
            other => panic!("expected Function, got {other:?}"),
        }
    }

    #[test]
    fn test_function_step_rejected_by_canonical_json() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "fn-canonical-reject",
                    "from": "direct:start",
                    "steps": [
                        {
                            "function": {
                                "runtime": "deno",
                                "source": "return {};",
                                "timeout_ms": 1000
                            }
                        }
                    ]
                }
            ]
        }"#;
        let err = parse_json_to_canonical(json).unwrap_err().to_string();
        assert!(
            err.contains("canonical v2 does not support step `function`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_set_property_step_json_parses_to_declarative() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "property-json-route",
                    "from": "direct:start",
                    "steps": [
                        {"set_property": {"name": "traceId", "value": "abc-123"}}
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        match &routes[0].steps[0] {
            DeclarativeStep::SetProperty(def) => {
                assert_eq!(def.key, "traceId");
                assert_eq!(
                    def.value,
                    crate::model::ValueSourceDef::Literal(serde_json::Value::String(
                        "abc-123".into()
                    ))
                );
            }
            other => panic!("expected SetProperty, got {other:?}"),
        }
    }

    #[test]
    fn dollar_schema_key_is_accepted() {
        let json = r#"{
            "$schema": "https://example.com/route-schema.json",
            "routes": [{"id": "r1", "from": "direct:start", "steps": [{"to": "direct:end"}]}]
        }"#;
        let parsed = parse_json_to_declarative(json).expect("$schema key must be accepted");
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn parse_json_to_canonical_rejects_security_policy() {
        let json = r#"{
            "routes": [{
                "id": "r-sec",
                "from": "direct:start",
                "security_policy": { "roles": ["admin"] },
                "steps": [{ "to": "log:info" }]
            }]
        }"#;
        let result = parse_json_to_canonical(json);
        assert!(result.is_err());
    }

    #[test]
    fn semantic_error_carries_json_format_prefix() {
        let json = r#"{"routes": [{"id": "", "from": "timer:tick"}]}"#;
        let result = parse_json_to_declarative(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("JSON DSL error:"),
            "expected 'JSON DSL error:' prefix, got: {err}"
        );
        assert!(
            err.contains("route 'id' must not be empty"),
            "expected underlying semantic message, got: {err}"
        );
    }

    /// Promoted fuzz regression (rc-m5ah): seq-shaped JSON documents
    /// (`[]`) must be rejected — a route document is a mapping per
    /// `schemas/dsl/route-schema.json` (`type: object`), never a
    /// positional sequence. The YAML front-end already rejects them.
    #[test]
    fn test_seq_shaped_document_rejected() {
        let result = parse_json_to_declarative("[]");
        assert!(result.is_err(), "JSON front-end must reject `[]`");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("invalid type: sequence"),
            "expected seq-type rejection, got: {err}"
        );
    }

    #[test]
    fn json_parse_error_carries_format_prefix() {
        let json = "{ not valid json }}}";
        let result = parse_json_to_declarative(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("JSON DSL error: JSON parse error:"),
            "expected double prefix for parse errors, got: {err}"
        );
    }

    /// Review I2: the JSON authoring path must lower `rest:` blocks, not
    /// silently drop them. JSON is a full-DSL authoring format (ADR-0026),
    /// so a `rest:` block expands exactly like the YAML path.
    #[test]
    fn json_rest_block_expands_into_routes() {
        let json = r#"{
            "rest": [
                {
                    "host": "0.0.0.0",
                    "port": 8080,
                    "path": "/users",
                    "operations": [
                        { "method": "GET", "path": "/{id}", "operation_id": "getUser", "to": "bean:svc" },
                        { "method": "POST", "path": "/", "operation_id": "createUser", "to": "bean:create" }
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        // Two operations → two lowered routes.
        assert_eq!(routes.len(), 2);
        let ids: Vec<&str> = routes.iter().map(|r| r.route_id.as_str()).collect();
        assert!(ids.contains(&"getUser"));
        assert!(ids.contains(&"createUser"));
        // Each route's from-URI carries the templated path + httpMethod.
        let get_route = routes.iter().find(|r| r.route_id == "getUser").unwrap();
        assert!(get_route.from.contains("/users/{id}"));
        assert!(get_route.from.contains("httpMethod=GET"));
    }

    /// Review I2 + C3: the JSON path runs the same cross-block validation
    /// as YAML — duplicate (method, path) tuples must be rejected.
    #[test]
    fn json_rest_rejects_duplicate_method_path() {
        let json = r#"{
            "rest": [
                {
                    "path": "/users",
                    "operations": [
                        { "method": "GET", "path": "/{id}", "operation_id": "a", "to": "bean:x" }
                    ]
                },
                {
                    "path": "/users",
                    "operations": [
                        { "method": "GET", "path": "/{id}", "operation_id": "b", "to": "bean:y" }
                    ]
                }
            ]
        }"#;
        let result = parse_json_to_declarative(json);
        assert!(
            result.is_err(),
            "JSON rest must reject duplicate (method, path)"
        );
    }

    #[test]
    fn load_json_from_file_rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.json");
        std::fs::write(&path, "{}").unwrap();
        // Extend the file past the 16 MiB cap (sparse file, no actual disk usage)
        let file = File::create(&path).unwrap();
        file.set_len(16 * 1024 * 1024 + 1).unwrap();
        drop(file);

        let err = match load_json_from_file(&path) {
            Ok(_) => panic!("expected oversized file error"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("exceeds max"),
            "expected size cap error, got: {err}"
        );
    }

    /// Legacy layout (`handled_by` nested inside `retry`) is a hard parse
    /// error: the field moved to the clause level and `deny_unknown_fields`
    /// rejects it at parse time. JSON shares the converter with YAML, so the
    /// error surfaces identically.
    #[test]
    fn test_json_legacy_retry_handled_by_is_hard_error() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "legacy-handled-by",
                    "from": "direct:start",
                    "error_handler": {
                        "on_exceptions": [
                            {
                                "kind": "Io",
                                "retry": {
                                    "max_attempts": 1,
                                    "handled_by": "log:io"
                                }
                            }
                        ]
                    }
                }
            ]
        }"#;
        let err = parse_json_to_declarative(json)
            .expect_err("legacy retry.handled_by must be a hard parse error");
        let msg = format!("{err}");
        assert!(
            msg.contains("handled_by"),
            "expected 'handled_by' in error, got: {msg}"
        );
        assert!(
            msg.contains("unknown field"),
            "expected 'unknown field' in error, got: {msg}"
        );
    }

    // ── env-typed JSON route documents (envtyped-routes 1.4) ─────────────

    use crate::test_support::EventCounts;
    use crate::test_support::event_counting_subscriber;
    use camel_core::route::BuilderStep;

    const DEFAULT_THRESHOLD: usize = camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD;

    const JSON_REST_PORT_DOC: &str = r#"{
      "rest": [
        {
          "host": "127.0.0.1",
          "port": "${env:RC_J_PORT:-8080}",
          "path": "/api",
          "operations": [
            { "method": "GET", "operation_id": "listUsers", "to": "direct:listUsers" }
          ]
        }
      ]
    }"#;

    /// JSON seam with a defaults-only lookup (variables unset).
    fn json_env_defaults(raw: &str) -> Result<Vec<RouteDefinition>, RoutesEnvError> {
        parse_json_routes_with_env(raw, &|_| None)
    }

    /// Run `f` under a WARN/ERROR counting subscriber; the counts stay valid
    /// after the capture guard drops.
    fn measured<T>(f: impl FnOnce() -> T) -> (T, EventCounts) {
        let (counts, _guard) = event_counting_subscriber();
        let value = f();
        (value, counts)
    }

    /// Pass-1 splice text for `raw` under a defaults-only lookup.
    fn spliced(raw: &str) -> String {
        crate::env_interpolation::interpolate_env_with(raw, &|_| None)
            .expect("the fixture's placeholders all carry defaults")
    }

    /// Today's error text for a JSON document (the legacy first-pass class).
    fn today_error_text(json: &str) -> String {
        format!(
            "{}",
            expect_parse_error(parse_json(json), "fixture fails today")
        )
    }

    /// `RouteDefinition` is not `Debug`, so `Result::expect_err` is unavailable
    /// on the parse seams.
    fn expect_seam_error(
        result: Result<Vec<RouteDefinition>, RoutesEnvError>,
        context: &str,
    ) -> RoutesEnvError {
        match result {
            Ok(_) => panic!("{context}: expected an error, got routes"),
            Err(error) => error,
        }
    }

    /// Same for the plain [`CamelError`] parse surface.
    fn expect_parse_error(
        result: Result<Vec<RouteDefinition>, CamelError>,
        context: &str,
    ) -> CamelError {
        match result {
            Ok(_) => panic!("{context}: expected an error, got routes"),
            Err(error) => error,
        }
    }

    fn assert_rest_uri(routes: &[RouteDefinition], expected: &str) {
        assert_eq!(routes.len(), 1, "one REST block lowers to one route");
        let uri = routes[0].from_uri();
        assert!(
            uri.contains(expected),
            "expected the lowered REST URI to contain {expected:?}, got: {uri}"
        );
    }

    #[test]
    fn json_routes_env_fixes_rest_port_default() {
        let routes = json_env_defaults(JSON_REST_PORT_DOC)
            .expect("the repair probe must coerce the rest port default");
        assert_rest_uri(&routes, "http://127.0.0.1:8080/api");
    }

    #[test]
    fn json_routes_env_fixes_rest_port_from_lookup() {
        let lookup = |name: &str| (name == "RC_J_PORT").then(|| "9090".to_string());
        let routes = parse_json_routes_with_env(JSON_REST_PORT_DOC, &lookup)
            .expect("the repair probe must coerce the looked-up rest port");
        assert_rest_uri(&routes, ":9090/api");
    }

    #[test]
    fn json_routes_env_fixes_throttle_under_routes() {
        let raw = r#"{
          "routes": [{
            "id": "j-throttle",
            "from": "direct:start",
            "steps": [
              { "throttle": { "max_requests": "${env:RC_J_MAX:-2}", "period_secs": 1 } }
            ]
          }]
        }"#;
        let routes =
            json_env_defaults(raw).expect("the repair probe must coerce the throttle position");
        match &routes[0].steps()[0] {
            BuilderStep::Throttle { config, .. } => {
                assert_eq!(config.max_requests, 2, "max_requests must be the typed 2");
            }
            other => panic!("expected a throttle step, got: {other:?}"),
        }
    }

    #[test]
    fn json_repair_tie_uses_sorted_keys() {
        // Authored `b` BEFORE `a`. Default `serde_json` maps are key-sorted, so
        // the walk would already see `a` first — but `camel-language-rhai`
        // enables `preserve_order`, and cargo feature unification then hands
        // this crate an authored-order map. The explicit sort in the JSON
        // walker makes the tie-break a guarantee of the walk in BOTH feature
        // configurations, never a map-iteration artifact.
        let raw = r#"{"b":"${env:RC_TIE_B:-1}","a":"${env:RC_TIE_A:-1}"}"#;
        let spliced = spliced(raw);

        // Test oracle (acceptance: "a test oracle accepts either single
        // coercion and rejects both strings"): rejects both strings, accepts
        // EITHER single coercion, rejects two coercions. That is the exact tie
        // the minimum-cardinality search must break by provenance order.
        fn oracle(text: &str) -> Result<serde_json::Value, CamelError> {
            let tree: serde_json::Value = serde_json::from_str(text)
                .map_err(|error| CamelError::RouteError(format!("invalid JSON: {error}")))?;
            let coerced = ["a", "b"]
                .iter()
                .filter(|key| tree.get(**key).is_some_and(serde_json::Value::is_number))
                .count();
            if coerced == 1 {
                Ok(tree)
            } else {
                Err(CamelError::RouteError(format!(
                    "the oracle accepts exactly one coercion, saw {coerced}: {text}"
                )))
            }
        }

        // Every oracle call in order, as `(a coerced, b coerced)`.
        let attempts: std::cell::RefCell<Vec<(bool, bool)>> = std::cell::RefCell::new(Vec::new());
        let probe =
            |text: &str, provenance: &[ProvenancePath]| -> Result<serde_json::Value, CamelError> {
                crate::env_int_probe::parse_json_with_probe::<serde_json::Value>(
                    text,
                    Some(provenance),
                    |candidate| {
                        let tree = serde_json::from_str::<serde_json::Value>(candidate).map_err(
                            |error| CamelError::RouteError(format!("invalid JSON: {error}")),
                        )?;
                        attempts.borrow_mut().push((
                            tree.get("a").is_some_and(serde_json::Value::is_number),
                            tree.get("b").is_some_and(serde_json::Value::is_number),
                        ));
                        oracle(candidate)
                    },
                )
            };
        let winning = repair_json_with_probe(&spliced, raw, &|_| None, &probe)
            .expect("the gates pass and one single coercion repairs the document")
            .expect("the tie has a winner");

        assert_eq!(
            attempts.borrow().as_slice(),
            &[(false, false), (true, false)],
            "the unmodified text is tried first, then the sorted-first `a` subset"
        );
        assert_eq!(
            winning.get("a").and_then(serde_json::Value::as_i64),
            Some(1),
            "ONLY the sorted-first `a` path is coerced, got: {winning}"
        );
        assert_eq!(
            winning.get("b").and_then(serde_json::Value::as_str),
            Some("1"),
            "the `b` path stays the spliced string, got: {winning}"
        );

        // Non-vacuity: the `b` subset would ALSO satisfy the oracle, so the
        // winner is decided purely by provenance order.
        assert!(
            oracle(r#"{"a":"1","b":1}"#).is_ok(),
            "coercing only `b` must be an accepted repair"
        );
        assert!(
            oracle(r#"{"a":"1","b":"1"}"#).is_err(),
            "both strings must be rejected"
        );
        assert!(
            oracle(r#"{"a":1,"b":1}"#).is_err(),
            "both coercions must be rejected"
        );
    }

    #[test]
    fn json_first_pass_preserves_escape_value_byte_identical() {
        // The JSON source carries the two-character escape `\n`; the splice
        // places it inside the JSON string, so the parsed value carries a
        // REAL newline — the tree-walk repair pass never runs and never
        // sanitizes it to "a b".
        let raw = r#"{"routes":[{"id":"j-esc","from":"direct:start","steps":[{"set_header":{"key":"k","value":"${env:RC_J_ESC:-a\nb}"}}]}]}"#;
        let routes = json_env_defaults(raw).expect("the splice pass loads the escape default");
        match &routes[0].steps()[0] {
            BuilderStep::DeclarativeSetHeader { key, value } => {
                assert_eq!(key, "k");
                assert_eq!(
                    value,
                    &crate::model::ValueSourceDef::Literal(serde_json::Value::String(
                        "a\nb".into()
                    )),
                    "splice semantics keep the real newline byte-identically"
                );
            }
            other => panic!("expected a set_header step, got: {other:?}"),
        }
    }

    #[test]
    fn json_unescaped_quote_no_repair_first_pass_error() {
        // A raw quote inside the default breaks JSON syntax once spliced:
        // gate 1 refuses the repair and today's splice syntax error stands.
        let raw = r#"{"rest":[{"host":"127.0.0.1","port":"${env:RC_J_QUOTE:-8"080}","path":"/api","operations":[{"method":"GET","to":"direct:q"}]}]}"#;
        let error = expect_seam_error(
            json_env_defaults(raw),
            "broken spliced syntax must not repair",
        );
        let text = format!("{error}");
        assert!(
            text.contains("JSON parse error"),
            "expected today's splice syntax error, got: {text}"
        );
    }

    #[test]
    fn json_escape_default_plus_repairable_port_no_repair() {
        // The escape-bearing default decodes to a newline in the splice pass
        // but is sanitized to a space by the tree walk: the trees differ, so
        // no partial repair may silently change the default.
        let raw = r#"{
          "rest": [{
            "host": "127.0.0.1",
            "port": "${env:RC_J_EP:-8080}",
            "path": "/api",
            "operations": [{ "method": "GET", "to": "direct:e" }]
          }],
          "routes": [{
            "id": "j-esc-pair",
            "from": "direct:start",
            "steps": [
              { "set_header": { "key": "k", "value": "${env:RC_J_ESC2:-a\nb}" } }
            ]
          }]
        }"#;
        let error = expect_seam_error(
            json_env_defaults(raw),
            "baseline inequality keeps the first-pass error",
        );
        let text = format!("{error}");
        assert!(
            text.contains("expected u16"),
            "expected the pass-1 typed port error, got: {text}"
        );
    }

    #[test]
    fn json_duplicate_key_document_never_repaired() {
        // A duplicate object key anywhere refuses the repair outright: the
        // probe must never run (zero suppressed events) and the first-pass
        // error stands.
        let raw = r#"{"rest":[{"host":"127.0.0.1","host":"0.0.0.0","port":"${env:RC_J_DUP:-8080}","path":"/api","operations":[{"method":"GET","to":"direct:d"}]}]}"#;
        // Pass 1 already rejects the duplicate field; the gate must keep that
        // error and never reach the probe.
        let expected = today_error_text(&spliced(raw));
        let suppressed_before = crate::env_int_probe::suppressed_event_count();
        let error = expect_seam_error(
            json_env_defaults(raw),
            "duplicate-key documents are never repaired",
        );
        assert_eq!(
            format!("{error}"),
            expected,
            "the first-pass duplicate-field error stands"
        );
        assert_eq!(
            crate::env_int_probe::suppressed_event_count(),
            suppressed_before,
            "no probe attempt may run for a duplicate-key document"
        );
    }

    #[test]
    fn json_unresolved_names_variable() {
        let raw = r#"{"rest":[{"host":"${env:RC_J_MISSING}","port":8080,"path":"/api","operations":[{"method":"GET","to":"direct:m"}]}]}"#;
        match parse_json_routes_with_env(raw, &|_| None) {
            Err(RoutesEnvError::Unresolved(variable)) => assert_eq!(variable, "RC_J_MISSING"),
            Err(other) => panic!("expected the named-variable error, got: {other}"),
            Ok(_) => panic!("an unresolved placeholder must not load"),
        }
    }

    /// Named quietness fixture (spec: `json_repair_quiet_idempotent_warning`):
    /// an `idempotent_consumer` step that legitimately warns at compile time,
    /// a `set_body` candidate whose value compiles both as the spliced string
    /// and as the coerced scalar (so retries reach the compile stage and the
    /// warning site), then a trailing unknown-format `unmarshal` whose LATE
    /// compile failure keeps the document failing. Zero-throttle is NOT used:
    /// it fails at AST lowering, before the warning site.
    const QUIET_FIXTURE: &str = r#"{
      "routes": [{
        "id": "j-quiet",
        "from": "direct:start",
        "steps": [
          {
            "idempotent_consumer": {
              "repository": "memory",
              "expression": "${header.messageId}",
              "eager": true,
              "remove_on_failure": false,
              "steps": [{ "to": "direct:next" }]
            }
          },
          { "set_body": "${env:RC_J_BODY:-__BODY__}" },
          { "unmarshal": "no-such-format" }
        ]
      }]
    }"#;

    fn quiet_fixture(body_default: &str) -> String {
        QUIET_FIXTURE.replace("__BODY__", body_default)
    }

    fn assert_quiet_repair(body_default: &str) {
        let raw = quiet_fixture(body_default);
        let first_pass_text = spliced(&raw);

        // Baseline: today's direct splice parse of the same document emits
        // exactly one warning (the idempotent_consumer contract) and no
        // error-level event.
        let (baseline, baseline_counts) = measured(|| {
            parse_json_with_threshold_and_security(
                &first_pass_text,
                DEFAULT_THRESHOLD,
                SecurityCompileContext::default(),
            )
        });
        let baseline_error = format!(
            "{}",
            expect_parse_error(
                baseline,
                "the trailing unmarshal keeps the document failing"
            )
        );
        assert_eq!(
            baseline_counts.warn(),
            1,
            "baseline: exactly one idempotent_consumer warning"
        );
        assert_eq!(baseline_counts.error(), 0, "baseline: no error events");

        // Control: the SAME document through the quiet compiler proves the
        // suppression counter observes the warning the baseline emitted, so
        // the count delta below cannot be vacuous.
        let direct_before = crate::env_int_probe::suppressed_event_count();
        let direct = parse_json_with_threshold_and_security_quiet(
            &first_pass_text,
            DEFAULT_THRESHOLD,
            SecurityCompileContext::default(),
            &ScriptLoadContext::new(None),
        );
        assert!(
            direct.is_err(),
            "the trailing unmarshal keeps the quiet compile failing"
        );
        assert!(
            crate::env_int_probe::suppressed_event_count() > direct_before,
            "the quiet guard must observe the idempotent_consumer warning"
        );

        // The seam adds nothing: pass 1 emits the same single warning and the
        // repair attempts are silenced, returning the ORIGINAL error.
        let suppressed_before = crate::env_int_probe::suppressed_event_count();
        let (seam_result, seam_counts) = measured(|| parse_json_routes_with_env(&raw, &|_| None));
        let seam_error = match seam_result {
            Err(RoutesEnvError::Parse(error)) => format!("{error}"),
            Err(other) => panic!("expected the first-pass parse error, got: {other}"),
            Ok(_) => panic!("expected the first-pass parse error, got routes"),
        };
        assert_eq!(seam_error, baseline_error, "the original first-pass error");
        assert_eq!(
            seam_counts.warn(),
            1,
            "repair attempts must add zero warnings"
        );
        assert_eq!(
            seam_counts.error(),
            0,
            "repair attempts must add zero error-level events"
        );
        // Pass 1 compiles WITHOUT the guard, so any increase here is a repair
        // attempt. `>= 1` proves the retries really compiled: the fixture's
        // candidate is compile-valid both ways and the late unmarshal failure
        // is what keeps the document failing.
        assert!(
            crate::env_int_probe::suppressed_event_count() - suppressed_before >= 1,
            "at least one repair attempt must have compiled under the quiet guard"
        );
    }

    #[test]
    fn json_repair_unresolved_matches_pass1_variable() {
        // The pass-1 splice scans RAW source text, the repair walk scans
        // DECODED values. An escape-hidden token (`\u0024{env:...}`) is
        // invisible to the splice but resolved by the walk, so this document
        // reaches the repair gate with an unresolved variable — and that name
        // must be the same one pass 1 reports for the literal spelling.
        let escaped = r#"{"rest":[{"host":"127.0.0.1","port":"\u0024{env:RC_J_MISSING}","path":"/api","operations":[{"method":"GET","to":"direct:m"}]}]}"#;
        let suppressed_before = crate::env_int_probe::suppressed_event_count();
        let repair_variable = match parse_json_routes_with_env(escaped, &|_| None) {
            Err(RoutesEnvError::Unresolved(variable)) => variable,
            Err(other) => panic!("expected the repair gate's named error, got: {other}"),
            Ok(_) => panic!("an unresolved placeholder must not load"),
        };
        assert_eq!(
            crate::env_int_probe::suppressed_event_count(),
            suppressed_before,
            "the unresolved gate fires before any probe attempt"
        );

        // The same document spelled literally: pass 1 itself names it (the
        // splice fails unresolved before the repair pass starts).
        let literal = r#"{"rest":[{"host":"127.0.0.1","port":"${env:RC_J_MISSING}","path":"/api","operations":[{"method":"GET","to":"direct:m"}]}]}"#;
        let pass1_variable = match parse_json_routes_with_env(literal, &|_| None) {
            Err(RoutesEnvError::Unresolved(variable)) => variable,
            Err(other) => panic!("expected the pass-1 named error, got: {other}"),
            Ok(_) => panic!("an unresolved placeholder must not load"),
        };

        assert_eq!(
            pass1_variable, "RC_J_MISSING",
            "pass 1 must name the missing variable"
        );
        assert_eq!(
            repair_variable, pass1_variable,
            "the repair walk must name the SAME variable pass 1 reports"
        );
    }

    #[test]
    fn json_repair_quiet_idempotent_warning_string_body() {
        // `true` at `set_body`: the spliced value is the STRING "true" and
        // the coerced value is the boolean `true` — both compile.
        assert_quiet_repair("true");
    }

    #[test]
    fn json_repair_quiet_idempotent_warning_numeric_body() {
        // `42` at `set_body`: numeric-coercing candidate; both forms compile.
        assert_quiet_repair("42");
    }

    #[test]
    fn json_routes_env_malformed_json_first_pass_unchanged() {
        let raw = "{ not valid json }}}";
        let first_pass_text = today_error_text(raw);
        let suppressed_before = crate::env_int_probe::suppressed_event_count();
        let error = expect_seam_error(json_env_defaults(raw), "malformed JSON must not load");
        match error {
            RoutesEnvError::Parse(parse_error) => assert_eq!(
                format!("{parse_error}"),
                first_pass_text,
                "identical to today's parse_json error text"
            ),
            other => panic!("expected the parse error class, got: {other}"),
        }
        assert_eq!(
            crate::env_int_probe::suppressed_event_count(),
            suppressed_before,
            "no probe attempt may run for malformed JSON"
        );
    }

    #[test]
    fn json_embedded_token_and_notanumber_keep_first_pass_error() {
        // (a) Embedded token at a typed position: not a whole-scalar token,
        // so no candidate — and no probe (zero suppressed events).
        let embedded = r#"{"rest":[{"host":"127.0.0.1","port":"p-${env:RC_J_E:-8080}","path":"/api","operations":[{"method":"GET","to":"direct:e"}]}]}"#;
        let embedded_expected = today_error_text(&spliced(embedded));
        let suppressed_before = crate::env_int_probe::suppressed_event_count();
        let embedded_error = expect_seam_error(
            json_env_defaults(embedded),
            "an embedded token keeps the first-pass error",
        );
        assert_eq!(
            embedded_error.to_string(),
            embedded_expected,
            "embedded token: today's error text"
        );
        assert_eq!(
            crate::env_int_probe::suppressed_event_count(),
            suppressed_before,
            "embedded token: no probe attempt may run"
        );

        // (b) Non-integer default: the leaf is not a clean candidate.
        let notanumber = r#"{"rest":[{"host":"127.0.0.1","port":"${env:RC_J_NAN:-notanumber}","path":"/api","operations":[{"method":"GET","to":"direct:n"}]}]}"#;
        let notanumber_expected = today_error_text(&spliced(notanumber));
        let suppressed_before = crate::env_int_probe::suppressed_event_count();
        let notanumber_error = expect_seam_error(
            json_env_defaults(notanumber),
            "a non-integer default keeps the error",
        );
        assert_eq!(
            format!("{notanumber_error}"),
            notanumber_expected,
            "notanumber: today's error text"
        );
        assert_eq!(
            crate::env_int_probe::suppressed_event_count(),
            suppressed_before,
            "notanumber: no probe attempt may run"
        );
    }

    #[test]
    fn dup_keys_escape_decoded_dup_detected() {
        // `"\u0061"` decodes to "a": the two keys collide at the same level
        // even though their authored text differs.
        let text = r#"{"a": 1, "\u0061": 2}"#;
        assert!(
            json_has_duplicate_keys(text),
            "an escape-decoded key must collide with its literal spelling"
        );
    }

    #[test]
    fn dup_keys_string_value_containing_json_not_flagged() {
        // The JSON-looking text lives inside a string VALUE: the scanner must
        // stay in string state and never forge object keys from it.
        let text = r#"{"outer": "{\"k\":1,\"k\":2}"}"#;
        assert!(
            !json_has_duplicate_keys(text),
            "JSON text inside a string value is not an object key"
        );
    }

    #[test]
    fn dup_keys_same_key_distinct_nesting_ok() {
        // Each object level has its own key set: `k` at the top and `k` under
        // `nested` is not a duplicate.
        let text = r#"{"k": 1, "nested": {"k": 2}}"#;
        assert!(
            !json_has_duplicate_keys(text),
            "keys in distinct objects are independent"
        );
    }

    #[test]
    fn dup_keys_nested_dup_detected() {
        // The collision sits inside the nested object; the clean top level
        // must not mask it.
        let text = r#"{"outer": {"k": 1, "k": 2}}"#;
        assert!(
            json_has_duplicate_keys(text),
            "a duplicate key must be detected at any nesting depth"
        );
    }
}
