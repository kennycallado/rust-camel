//! R-SCHEMA rule — JSON Schema validation of the route against `ROUTE_SCHEMA`.
//!
//! Validates the parsed document against `ROUTE_SCHEMA` (whose root is the
//! `{routes: [...]}` envelope) and reports one [`DiagnosticCode::RSchema`]
//! diagnostic per violation, anchored by keyword.
//!
//! The document is normalised to the envelope form first — corpus files
//! arrive as an envelope (`{routes: [...]}`), a legacy array (`[...]`), or a
//! bare single route (`{from, steps}`). Each form strips a different number of
//! leading instance-path segments (`routes`, index) when mapping validator
//! paths back onto the document's CST, so span anchoring stays exact for every
//! form.
//!
//! Validation targets an INTERPOLATED copy of the source (rc-93wct):
//! `${env:X:-d}` tokens resolve to their defaults (default-only lookup,
//! never the process environment), and a whole-scalar token validates as
//! the STRING default — the typing mirror of the boot path's tree-walk
//! (see [`enforce_typing_mirror`]). Whole-scalar no-default tokens are
//! explicit Errors; comment tokens produce nothing.
//!
//! - Most keywords (type/enum/pattern/const/format/minimum/`exclusiveMinimum`/
//!   anyOf/oneOf/minItems/maxItems/required) anchor on the JSON-pointer
//!   instance node: jsonschema already points `required` at the parent
//!   object, so the default anchoring is correct.
//! - `additionalProperties` anchors on each offending KEY, extracted from
//!   [`ValidationErrorKind::AdditionalProperties { unexpected }`].
//!
//! The compiled validator is cached in a process-wide [`OnceLock`].

use std::collections::HashMap;
use std::sync::OnceLock;

use camel_api::component_metadata::ComponentMetadataCatalog;
use jsonschema::{Validator, error::ValidationErrorKind};
use noyalib::cst;

use crate::ROUTE_SCHEMA;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity, Span};
use crate::document::Document;
use crate::env_interpolation::{
    WholeScalarEnvToken, env_regex, interpolated_validation_copy, sanitize_env_value,
    whole_scalar_env_token,
};
use crate::rule::Rule;

/// Compiled route-schema validator (built once per process).
static VALIDATOR: OnceLock<Validator> = OnceLock::new();

/// R-SCHEMA: validates the route against the embedded JSON Schema.
pub struct RSchemaRule;

impl Rule for RSchemaRule {
    fn analyze(&self, doc: &Document, _catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic> {
        // R-SCHEMA cannot run on a document that failed to parse — R-SYN owns
        // that case.
        if doc.parse_failure.is_some() {
            return Vec::new();
        }

        // Build the interpolated validation copy FIRST (rc-93wct):
        // default-only `${env:X:-d}` resolution, never the process
        // environment, per-token — no-default tokens stay literal so their
        // authored placeholder keeps flagging the genuinely undefined
        // variables.
        let (validation_raw, substituted) = interpolated_validation_copy(&doc.raw);

        // Convert the interpolated source to a JSON value. An unconvertible
        // document yields no diagnostics (no panic, no abort).
        let Some(mut value) = raw_to_json_value(&validation_raw) else {
            return Vec::new();
        };

        // ROUTE_SCHEMA's root is the `{routes: [...]}` envelope. Real corpus
        // files arrive in three forms; detect which and normalise to the
        // envelope so validation targets the real document structure. Wrapping
        // an envelope again previously produced `{routes: [{routes: [...]}]}`,
        // surfacing dozens of false "required"/"unexpected" errors.
        //
        // `envelope_depth` counts how many leading instance-path segments
        // (`routes`, then the index) belong to the wrapper rather than the raw
        // document, so span resolution maps validator paths back onto `doc.raw`
        // for every form:
        //   - array              -> legacy array form -> {routes: <array>}, depth 1
        //   - object with routes -> envelope form     -> as-is,            depth 0
        //   - object with rest   -> rest-block form   -> R-SCHEMA skips (see below)
        //   - any other object   -> bare single route -> {routes: [value]}, depth 2
        //   - scalar/null        -> R-SCHEMA cannot validate; R-SYN owns it.
        //
        // A `{rest: [...]}` document is a valid DSL form (camel-dsl
        // `RouteDslRest`, lowered by `expand_rest_into`), but ROUTE_SCHEMA
        // does not model it; validating it as a bare route yields bogus
        // `required`/`additionalProperties` errors (rc-xmbi). Skip the form
        // until RestDsl defs land in the schema.
        let envelope_depth = match &value {
            serde_json::Value::Array(_) => 1,
            serde_json::Value::Object(map) if map.contains_key("routes") => 0,
            serde_json::Value::Object(map) if map.contains_key("rest") => {
                return Vec::new();
            }
            serde_json::Value::Object(_) => 2,
            _ => return Vec::new(),
        };
        // Parse the ORIGINAL CST (from `doc.raw`, not the interpolated copy)
        // once so span resolution reuses it across all errors — diagnostics
        // land on authored text.
        let Ok(parsed) = cst::parse_document(&doc.raw) else {
            return Vec::new();
        };

        // Typing mirror (rc-93wct rev 2): force whole-scalar substituted
        // tokens to JSON STRINGS (the whole-text splice let YAML re-infer
        // numeric/boolean types the boot tree-walk never produces) and
        // collect whole-scalar no-default tokens for explicit Errors.
        let mut unresolved: Vec<UnresolvedPlaceholder> = Vec::new();
        enforce_typing_mirror(&parsed, &mut value, "", &doc.raw, &mut unresolved);

        // Info spans resolve against the interpolated (pre-envelope) `value`
        // tree BEFORE it is moved into the wrapped `instance`: leaf paths in
        // `value`-coordinates map 1:1 onto the original CST for every
        // document form (the envelope wrapper is added around `value`,
        // never inside it).
        //
        // The SAME `${env:V:-d}` token can appear in several fields. Each
        // note must anchor on its OWN authored occurrence, so the matching
        // leaves are collected per token in walk order and each occurrence
        // consumes the next one — a first-match search would collapse every
        // duplicate note onto the first matching leaf.
        let mut matches_by_token: HashMap<String, Vec<Span>> = HashMap::new();
        let info_spans: Vec<Option<Span>> = substituted
            .iter()
            .map(|sub| {
                let token = format!("${{env:{}:-{}}}", sub.var, sub.default);
                let matches = matches_by_token.entry(token.clone()).or_insert_with(|| {
                    let mut spans = Vec::new();
                    collect_placeholder_spans(&parsed, &value, "", &token, &doc.raw, &mut spans);
                    spans
                });
                // A token with no resolvable value-leaf span (comment,
                // mapping key) yields NO note — comments are not part of
                // the parsed instance.
                (!matches.is_empty()).then(|| matches.remove(0))
            })
            .collect();

        let instance = match envelope_depth {
            0 => value,
            1 => serde_json::json!({ "routes": value }),
            _ => serde_json::json!({ "routes": [value] }),
        };

        let validator = VALIDATOR.get_or_init(compile_validator);

        let mut diagnostics = Vec::new();
        for err in validator.iter_errors(&instance) {
            let instance_path = err.instance_path().as_str();
            match err.kind() {
                // The offending key is NOT in instance_path (which points at
                // the parent object): resolve each key's span by appending it
                // to the parent's path.
                ValidationErrorKind::AdditionalProperties { unexpected } => {
                    let parent = instance_path_to_noyalib(instance_path, envelope_depth);
                    for key in unexpected {
                        let key_path = if parent.is_empty() {
                            key.clone()
                        } else {
                            format!("{parent}.{key}")
                        };
                        let span = crate::document::key_span_for(&parsed, &key_path);
                        diagnostics.push(diagnostic_for(span, err.to_string()));
                    }
                }
                // Every other keyword anchors on the resolved instance node.
                _ => {
                    let noya_path = instance_path_to_noyalib(instance_path, envelope_depth);
                    let span = crate::document::value_span_for(&parsed, &noya_path);
                    diagnostics.push(diagnostic_for(span, err.to_string()));
                }
            }
        }

        // Whole-scalar no-default tokens: boot hard-fails on the
        // unresolved variable, so lint must not stay silent even where the
        // literal placeholder text is a valid string. `$${env:...}`
        // escapes never reach this list (their authored `$$` breaks the
        // whole-scalar exact match).
        //
        // NOTE: at an int/bool schema position the SAME authored
        // placeholder also fails the schema `type` keyword (the literal
        // token text is a string, the position wants a number), so one
        // no-default token there yields TWO Errors — the schema type Error
        // and this explicit unresolved Error. Both are intentional: one
        // reports the type defect, the other names the unresolved variable
        // (boot-parity hard failure). Do not "deduplicate" them.
        for u in unresolved {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::RSchema,
                severity: Severity::Error,
                span: u.span,
                message: format!(
                    "unresolved ${{env:{}}} placeholder (no default): route loading \
                     would fail on this placeholder",
                    u.var
                ),
                fix: None,
            });
        }

        // One Info note per substituted default with a resolvable value-leaf
        // span, explaining why the field validated cleanly. The note lands
        // on the authored placeholder; tokens without a value-leaf span
        // (comments, mapping keys) are skipped above.
        for (sub, span) in substituted.iter().zip(info_spans) {
            let Some(span) = span else {
                continue;
            };
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::RSchema,
                severity: Severity::Info,
                span,
                message: format!(
                    "validated against substituted default for ${{env:{}}} (:-{})",
                    sub.var, sub.default
                ),
                fix: None,
            });
        }
        diagnostics
    }

    fn code(&self) -> DiagnosticCode {
        DiagnosticCode::RSchema
    }
}

/// Build a [`Diagnostic`] for an R-SCHEMA violation.
fn diagnostic_for(span: Span, message: String) -> Diagnostic {
    Diagnostic {
        code: DiagnosticCode::RSchema,
        severity: Severity::Error,
        span,
        message,
        fix: None,
    }
}

/// Compile the embedded [`ROUTE_SCHEMA`] into a [`Validator`].
///
/// The schema is trusted-valid (committed, byte-checked by the xtask
/// `schema --check` gate), so compilation failure is a build-time invariant.
fn compile_validator() -> Validator {
    let schema: serde_json::Value =
        serde_json::from_str(ROUTE_SCHEMA).expect("embedded route schema is valid JSON"); // allow-unwrap
    jsonschema::validator_for(&schema).expect("embedded route schema must compile") // allow-unwrap
}

/// Convert raw source text (YAML or JSON) to a [`serde_json::Value`].
///
/// Deserializes via noyalib's serde compat shim; on ANY conversion error
/// returns `None` (R-SCHEMA then returns no diagnostics — no panic).
fn raw_to_json_value(raw: &str) -> Option<serde_json::Value> {
    let value: serde_json::Value = noyalib::compat::serde_yaml::from_str(raw).ok()?;
    Some(value)
}

/// Collect every leaf span whose authored slice contains `token`, in CST walk
/// order, resolving each through the existing
/// [`crate::document::value_span_for`] path.
///
/// Walks the interpolated instance's leaf paths (in `value`-coordinates,
/// which map 1:1 onto the original CST for every document form — the
/// envelope wrapper is added around `value`, never inside it) and resolves
/// each against the ORIGINAL CST; every leaf whose authored slice contains
/// the token is a placeholder value node. The Nth substituted occurrence of
/// a token anchors on the Nth collected span, so duplicate `${env:V:-d}`
/// tokens in different fields each keep their own note.
///
/// A leaf whose slice holds the token `k` times contributes `k` copies of
/// its span: a scalar repeating the same placeholder still anchors every
/// note on its (single) value node. Only UNESCAPED occurrences count —
/// ENV_RE consumes `$${env:...}` / `$$` escapes atomically, so the token
/// text inside a `$${env:...}` escape never contributes (a naive
/// `matches(token)` count would let the Info note anchor on the escape).
/// A leaf with no match (including an unresolvable zero span) contributes
/// nothing — the caller keeps the miss path (`Span::new(0, 0)`).
fn collect_placeholder_spans(
    parsed: &cst::Document,
    value: &serde_json::Value,
    path: &str,
    token: &str,
    raw: &str,
    out: &mut Vec<Span>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                collect_placeholder_spans(parsed, v, &child, token, raw, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                let child = format!("{path}[{i}]");
                collect_placeholder_spans(parsed, v, &child, token, raw, out);
            }
        }
        _ => {
            let span = crate::document::value_span_for(parsed, path);
            // Count only unescaped occurrences: ENV_RE consumes
            // `$${env:...}` / `$$` escapes atomically, so a bare-match arm
            // whose text equals the token is a real authored placeholder.
            let count = env_regex()
                .captures_iter(&raw[span.start..span.end])
                .filter(|caps| {
                    caps.get(1).is_none()
                        && caps.get(2).is_none()
                        && caps.get(3).is_some_and(|m| m.as_str() == token)
                })
                .count();
            out.extend(std::iter::repeat_n(span, count));
        }
    }
}

/// A whole-scalar `${env:VAR}` token (no default) found at a value
/// position — reported as an Error (boot hard-fails on it).
struct UnresolvedPlaceholder {
    var: String,
    span: Span,
}

/// Typing-mirror walk over the interpolated instance (rc-93wct rev 2).
///
/// The validation copy is a whole-text splice, so YAML re-infers
/// numeric/boolean types from substituted defaults (`max_requests: 2` →
/// JSON number). The boot path's tree-walk keeps STRING typing for every
/// scalar that carried a token; this walk replicates that canon:
///
/// - a leaf whose AUTHORED scalar (original CST slice from `doc.raw`,
///   quotes/whitespace trimmed) is EXACTLY one substituted `${env:X:-d}`
///   token is forced to the JSON STRING `"d"` — int/bool positions then
///   type-error against the string, string positions pass cleanly;
/// - a whole-scalar `${env:X}` (no default, unescaped) keeps its literal
///   instance value and is collected for an explicit Error — even at a
///   string position, where the literal placeholder validates as an
///   ordinary string;
/// - `$${env:...}` escapes never match (their authored `$$` breaks the
///   exact match), and tokens inside comments are never visited (comments
///   have no value leaf in the instance).
///
/// Paths are in `value`-coordinates (pre-envelope), which map 1:1 onto the
/// original CST for every document form. An unresolvable path keeps the
/// `value_span_for` miss span (`Span::new(0, 0)`), whose empty slice never
/// matches a token.
/// Boot-parity restore for STRUCTURE-CHANGING defaults (rc-93wct ceiling).
///
/// An unquoted flow-style default (`${env:X:-[a,b]}`) splices `[a,b]`
/// into the whole-text validation copy, where YAML re-parses it as a
/// sequence/mapping — the node becomes NON-scalar and the scalar
/// String-forcing arm in [`enforce_typing_mirror`] never sees it, leaving
/// the re-inferred shape to false-positive against scalar-typed schema
/// positions. The boot tree-walk keeps the substituted leaf a STRING no
/// matter the default's shape, so the mirror restores that typing by
/// replacing the spliced node with the string default.
///
/// Known ceiling (degrade-safe, like the miss-span path): a default
/// containing `}` makes the token regex stop at the first brace, so the
/// token no longer matches whole-scalar and the partial splice can break
/// the validation copy — R-SCHEMA then stays silent (no diagnostics).
/// Both shapes are silence-or-Info, never a false positive.
fn structure_changing_default(parsed: &cst::Document, path: &str, raw: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let span = crate::document::value_span_for(parsed, path);
    if span.end <= span.start {
        return None;
    }
    let authored = &raw[span.start..span.end];
    if !authored.contains("${") {
        return None;
    }
    match whole_scalar_env_token(authored) {
        Some(WholeScalarEnvToken::WithDefault { default }) => Some(sanitize_env_value(&default)),
        // No-default tokens never substitute, so they cannot change the
        // validation copy's structure; the scalar arm owns them.
        _ => None,
    }
}

fn enforce_typing_mirror(
    parsed: &cst::Document,
    value: &mut serde_json::Value,
    path: &str,
    raw: &str,
    unresolved: &mut Vec<UnresolvedPlaceholder>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(default) = structure_changing_default(parsed, path, raw) {
                *value = serde_json::Value::String(default);
                return;
            }
            for (k, v) in map.iter_mut() {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                enforce_typing_mirror(parsed, v, &child, raw, unresolved);
            }
        }
        serde_json::Value::Array(items) => {
            if let Some(default) = structure_changing_default(parsed, path, raw) {
                *value = serde_json::Value::String(default);
                return;
            }
            for (i, v) in items.iter_mut().enumerate() {
                enforce_typing_mirror(parsed, v, &format!("{path}[{i}]"), raw, unresolved);
            }
        }
        _ => {
            // Cheap pre-filter: only scalars whose authored slice contains
            // a `${` can be whole-scalar tokens.
            let span = crate::document::value_span_for(parsed, path);
            let authored = &raw[span.start..span.end];
            if !authored.contains("${") {
                return;
            }
            match whole_scalar_env_token(authored) {
                Some(WholeScalarEnvToken::WithDefault { default }) => {
                    *value = serde_json::Value::String(sanitize_env_value(&default));
                }
                Some(WholeScalarEnvToken::NoDefault { var }) => {
                    unresolved.push(UnresolvedPlaceholder { var, span });
                }
                None => {
                    // Embedded-token parity (rc-93wct): the boot tree-walk
                    // Unresolved-fails on EVERY unescaped no-default token,
                    // including ones inside a larger scalar
                    // (`id: svc-${env:HOST}`). The whole-scalar arms above
                    // cannot see them, so scan the authored slice with
                    // ENV_RE — it consumes `$${env:...}` / `$$` escapes
                    // atomically — and flag each bare no-default match,
                    // span-anchored at its offset inside the leaf. Value
                    // leaves only: comments and mapping keys are never
                    // visited by this walk.
                    for caps in env_regex().captures_iter(authored) {
                        let (Some(whole), Some(var)) = (caps.get(3), caps.get(4)) else {
                            // `$${env:...}` / `$$` escape arm — never unresolved.
                            continue;
                        };
                        if caps.get(5).is_some() {
                            // Has a default: substituted validation + Info
                            // note, not unresolved.
                            continue;
                        }
                        unresolved.push(UnresolvedPlaceholder {
                            var: var.as_str().to_string(),
                            span: Span::new(span.start + whole.start(), span.start + whole.end()),
                        });
                    }
                }
            }
        }
    }
}

/// Convert a JSON-pointer instance path to a noyalib CST query path.
///
/// Drops the leading `envelope_depth` segments that belong to the
/// `{routes: [...]}` wrapper but not to the raw document's CST, so the
/// remainder maps onto `doc.raw`:
/// - `0` (envelope form): keep `routes` + index (the CST root IS the envelope);
/// - `1` (legacy array form): drop `routes`, keep the index (CST root is the
///   array);
/// - `2` (bare single route): drop `routes` + index (CST root is the route).
///
/// Remaining array indices become `[i]`; property names are dot-joined.
fn instance_path_to_noyalib(instance_path: &str, envelope_depth: usize) -> String {
    let mut segments: Vec<&str> = instance_path.split('/').filter(|s| !s.is_empty()).collect();
    // Drop the wrapper segments belonging to `instance` but not `doc.raw`.
    for _ in 0..envelope_depth.min(segments.len()) {
        segments.remove(0);
    }

    let mut out = String::new();
    for seg in &segments {
        let unescaped = seg.replace("~1", "/").replace("~0", "~");
        if unescaped.parse::<usize>().is_ok() {
            out.push('[');
            out.push_str(&unescaped);
            out.push(']');
        } else if out.is_empty() {
            out.push_str(&unescaped);
        } else {
            out.push('.');
            out.push_str(&unescaped);
        }
    }
    out
}

#[cfg(test)]
mod tests;
