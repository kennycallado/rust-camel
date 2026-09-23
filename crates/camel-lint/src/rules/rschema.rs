//! R-SCHEMA rule — JSON Schema validation of the route against `ROUTE_SCHEMA`.
//!
//! Validates the parsed document against `ROUTE_SCHEMA` (whose root is the
//! `{routes: [...]}` envelope) and reports one [`DiagnosticCode::RSchema`]
//! diagnostic per violation, anchored by keyword.
//!
//! The document is normalised to the envelope form first — corpus files
//! arrive as an envelope (`{routes: [...]}`, `{rest: [...]}`,
//! `{mcp: [...]}`, or a mix of them), a legacy array (`[...]`), or a bare
//! single route (`{from, steps}`). Each form strips a different number of
//! leading instance-path segments (`routes`, index) when mapping validator
//! paths back onto the document's CST, so span anchoring stays exact for
//! every form.
//!
//! Validation targets an INTERPOLATED copy of the source (rc-93wct):
//! `${env:X:-d}` tokens resolve to their defaults (default-only lookup,
//! never the process environment), and a whole-scalar token validates as
//! the STRING default — the typing mirror of the boot path's tree-walk
//! (see `enforce_typing_mirror`). One carve-out mirrors the boot
//! numeric-knob repair: a whole-scalar token whose default is a clean
//! integer (i64-or-u64 parse; SYNC with camel-dsl `env_int_probe` and
//! camel-config `clean_i64`) at an integer-typed schema position is
//! validated as the NUMBER instead, so such leaves produce no diagnostic.
//! Whole-scalar no-default tokens are explicit Errors; comment tokens
//! produce nothing.
//!
//! - Most keywords (type/enum/pattern/const/format/minimum/`exclusiveMinimum`/
//!   anyOf/oneOf/minItems/maxItems/required) anchor on the JSON-pointer
//!   instance node: jsonschema already points `required` at the parent
//!   object, so the default anchoring is correct.
//! - `additionalProperties` anchors on each offending KEY, extracted from
//!   [`ValidationErrorKind::AdditionalProperties { unexpected }`].
//! - A collapsed anyOf burying an exactly-one permission value-source
//!   oneOf failure (`security_policy.permission` `resource`/`action`)
//!   de-collapses into ONE targeted diagnostic per field, anchored on
//!   the value-spec mapping (see the `AnyOf` arm in [`RSchemaRule`]).
//! - A collapsed anyOf burying pattern violations (the MCP TLS path
//!   fields) de-collapses into one diagnostic per pattern leaf, and a
//!   non-pattern defect co-located in the SAME failed anyOf also
//!   surfaces as its own sibling leaf diagnostic (the null-branch
//!   whole-node type mismatch is excluded as branch noise); with no
//!   nested pattern, the collapsed diagnostic keeps its shape.
//!
//! The compiled validator is cached in a process-wide [`OnceLock`].

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use camel_api::component_metadata::ComponentMetadataCatalog;
use jsonschema::{Validator, error::ValidationError, error::ValidationErrorKind};
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
        //   - array                            -> legacy array form -> {routes: <array>}, depth 1
        //   - object with routes, rest, or mcp -> envelope form     -> as-is,         depth 0
        //   - any other object                 -> bare single route -> {routes: [value]}, depth 2
        //   - scalar/null                      -> R-SCHEMA cannot validate; R-SYN owns it.
        //
        // A `{rest: [...]}` document is a valid DSL form (camel-dsl
        // `RouteDslRest`, lowered by `expand_rest_into`). ROUTE_SCHEMA models
        // the rest block in its envelope (rc-p86s), so the document validates
        // as-is at depth 0 — the same treatment as a `{routes: [...]}` file.
        // (rc-xmbi previously skipped the form entirely because the schema had
        // no RestDsl defs; the skip died with the schema gap.)
        //
        // A `{mcp: [...]}` document is the same story (rc-6pikg): camel-dsl
        // `RouteDslMcp`, lowered by `expand_mcp_into`; ROUTE_SCHEMA models the
        // mcp block in its envelope, so the document validates at depth 0
        // instead of false-positive wrapping as a bare route.
        let envelope_depth = match &value {
            serde_json::Value::Array(_) => 1,
            serde_json::Value::Object(map)
                if map.contains_key("routes")
                    || map.contains_key("rest")
                    || map.contains_key("mcp") =>
            {
                0
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
        let mut int_candidates: Vec<IntCandidate> = Vec::new();
        enforce_typing_mirror(
            &parsed,
            &mut value,
            "",
            &doc.raw,
            &mut unresolved,
            &mut int_candidates,
        );

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

        let validator = VALIDATOR.get_or_init(compile_validator);

        // Integer-position carve-out (typing mirror int arm): for every
        // clean-integer whole-scalar default, also validate a NUMBER
        // copy; when the STRING copy flags the leaf's chain but the
        // NUMBER copy is clean there, the NUMBER copy is the honest boot
        // equivalent and becomes the validation instance. The leaf's
        // Info note is dropped with it (the default was not kept as a
        // string).
        let (instance, carved_spans) =
            integer_carve_out_instance(validator, &parsed, value, envelope_depth, &int_candidates);

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
                        diagnostics.push(diagnostic_for(span, diagnostic_message(&err)));
                    }
                }
                // A failed anyOf surfaces as ONE collapsed error at the
                // branch node, burying the offending leaf (pre-existing
                // collapse limitation). Two de-collapse passes run, in
                // order:
                //
                // 1. TARGETED exactly-one permission value-source
                //    diagnostics: a `security_policy.permission`
                //    `resource`/`action` value spec with zero or several
                //    non-null sources among `literal`/`header`/`property`
                //    fails the exactly-one oneOf buried under THREE
                //    Option-wrapper anyOf levels (security_policy ->
                //    permission -> resource/action), collapsing into the
                //    generic message at this node. The walker
                //    (`collect_permission_oneof_paths`) recovers each
                //    buried oneOf failure and re-anchors ONE diagnostic
                //    per field on the value-spec mapping, naming the
                //    field and the found set.
                //
                //    KNOWN LIMITATION: once a targeted diagnostic fires
                //    for this error, any
                //    SIBLING defect inside the same collapsed anyOf
                //    (e.g. an unknown value-spec key next to a zero
                //    source) is subsumed — first-error-wins, like serde's
                //    stop-at-first deserialization error; fix the flagged
                //    source set and re-lint to surface the sibling.
                //
                // 2. PATTERN de-collapse — the schema's only pattern
                //    keywords are the two MCP TLS path fields (rc-n3t73)
                //    — reporting each nested, strictly-deeper pattern
                //    error on its own leaf.
                //
                // When a pattern violation co-occurs with a
                // non-pattern defect in the SAME failed anyOf (e.g. a
                // blank cert_path plus an unknown tls key), the
                // sibling defect also surfaces as its own leaf
                // diagnostic (unknown keys anchor on the key, other
                // kinds on their value node) instead of being subsumed
                // by the collapsed diagnostic; the null-branch
                // whole-node type mismatch is excluded as branch
                // noise. When NO nested pattern surfaces, the
                // collapsed anyOf diagnostic keeps today's shape
                // byte-identically.
                ValidationErrorKind::AnyOf { context } => {
                    // Pass 1: targeted permission value-source exactly-one
                    // diagnostics. Branches retry the same subschema
                    // shapes, so the same oneOf failure can surface more
                    // than once; dedup preserving first-occurrence order
                    // (a two-field route yields two SIBLING matches under
                    // this one collapsed anyOf — both reported).
                    let mut permission_paths: Vec<String> = Vec::new();
                    collect_permission_oneof_paths(&err, &mut permission_paths);
                    let mut seen_paths = HashSet::new();
                    let mut emitted_targeted = false;
                    for path in &permission_paths {
                        if !seen_paths.insert(path.clone()) {
                            continue;
                        }
                        // A tail that is neither `resource` nor `action`
                        // is a non-match: skip it (defensive — the marker
                        // already scopes the def to its only two
                        // ref-sites).
                        let Some(field) = permission_value_source_field(path) else {
                            continue;
                        };
                        // The instance path IS a JSON pointer; a miss
                        // (defensive) reports `none set`.
                        let value_at = instance.pointer(path).unwrap_or(&serde_json::Value::Null);
                        // Exactly one non-null recognized source means
                        // the oneOf failed on the value's TYPE (e.g.
                        // `literal: 123`; serde's deserializer rejects
                        // the same shape with `expected string`), not
                        // on the cardinality — a "must specify exactly
                        // one" diagnostic would contradict the authored
                        // shape. Treat as non-match: the error falls
                        // through to the generic collapsed-anyOf form.
                        if non_null_source_keys(value_at).len() == 1 {
                            continue;
                        }
                        let noya_path = instance_path_to_noyalib(path, envelope_depth);
                        let span = crate::document::value_span_for(&parsed, &noya_path);
                        diagnostics.push(diagnostic_for(
                            span,
                            format!(
                                "security_policy permission {field} must specify exactly one \
                                 of: literal, header, or property (set: {})",
                                found_sources(value_at)
                            ),
                        ));
                        emitted_targeted = true;
                    }
                    if emitted_targeted {
                        // Remaining nested non-permission errors of this
                        // same collapsed anyOf are intentionally subsumed
                        // (first-error-wins, see the arm comment).
                        continue;
                    }
                    // Pass 2: pattern de-collapse.
                    let own_depth = segment_depth(instance_path);
                    let mut seen = HashSet::new();
                    let mut pattern_errors = Vec::new();
                    for branch in context {
                        for nested in branch {
                            let ValidationErrorKind::Pattern { pattern } = nested.kind() else {
                                continue;
                            };
                            let nested_path = nested.instance_path().as_str();
                            let nested_depth = segment_depth(nested_path);
                            if nested_depth <= own_depth {
                                continue;
                            }
                            // Branches retry the same subschema shapes, so
                            // the same (path, pattern) defect can surface
                            // more than once; report each one once.
                            if seen.insert((nested_path.to_string(), pattern.clone())) {
                                pattern_errors.push(nested);
                            }
                        }
                    }
                    if pattern_errors.is_empty() {
                        // No leaf-anchored pattern violation inside the
                        // collapse: keep today's collapsed anyOf
                        // diagnostic at the anyOf node.
                        let noya_path = instance_path_to_noyalib(instance_path, envelope_depth);
                        let span = crate::document::value_span_for(&parsed, &noya_path);
                        diagnostics.push(diagnostic_for(span, diagnostic_message(&err)));
                    } else {
                        // Co-located non-pattern defects of the same
                        // collapsed anyOf surface next to the pattern
                        // leaves (rc-lys6a) — collected only here, so
                        // the empty-pattern collapsed branch above
                        // stays untouched.
                        let mut sibling_errors = Vec::new();
                        collect_sibling_errors(&err, own_depth, &mut sibling_errors);
                        for nested in pattern_errors {
                            let noya_path = instance_path_to_noyalib(
                                nested.instance_path().as_str(),
                                envelope_depth,
                            );
                            let span = crate::document::value_span_for(&parsed, &noya_path);
                            diagnostics.push(diagnostic_for(span, diagnostic_message(nested)));
                        }
                        for nested in sibling_errors {
                            if let ValidationErrorKind::AdditionalProperties { unexpected } =
                                nested.kind()
                            {
                                // Mirror the top-level arm: the
                                // offending key is NOT in instance_path
                                // (which points at the parent object);
                                // resolve each key's span by appending
                                // it to the parent's path.
                                let parent = instance_path_to_noyalib(
                                    nested.instance_path().as_str(),
                                    envelope_depth,
                                );
                                for key in unexpected {
                                    let key_path = if parent.is_empty() {
                                        key.clone()
                                    } else {
                                        format!("{parent}.{key}")
                                    };
                                    let span = crate::document::key_span_for(&parsed, &key_path);
                                    diagnostics
                                        .push(diagnostic_for(span, diagnostic_message(nested)));
                                }
                            } else {
                                let noya_path = instance_path_to_noyalib(
                                    nested.instance_path().as_str(),
                                    envelope_depth,
                                );
                                let span = crate::document::value_span_for(&parsed, &noya_path);
                                diagnostics.push(diagnostic_for(span, diagnostic_message(nested)));
                            }
                        }
                    }
                }
                // Every other keyword anchors on the resolved instance node.
                _ => {
                    let noya_path = instance_path_to_noyalib(instance_path, envelope_depth);
                    let span = crate::document::value_span_for(&parsed, &noya_path);
                    diagnostics.push(diagnostic_for(span, diagnostic_message(&err)));
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
            // `arg:` tokens fail the boot tree walk regardless of a
            // fallback suffix (the arg grammar rejects `:-fallback`), so
            // the "(no default)" qualifier only applies to `env:`.
            let qualifier = if u.namespace == "env" {
                " (no default)"
            } else {
                ""
            };
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::RSchema,
                severity: Severity::Error,
                span: u.span,
                message: format!(
                    "unresolved ${{{}:{}}} placeholder{}: route loading \
                     would fail on this placeholder",
                    u.namespace, u.var, qualifier
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
            if carved_spans.contains(&span) {
                // Carved-out leaf: the default was not kept as a string,
                // so the substituted-string Info note would be a lie.
                continue;
            }
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

/// Serialize a JSON value with recursively sorted object keys.
///
/// jsonschema's `Display` echoes the instance via `serde_json`, whose
/// map ordering depends on the `preserve_order` feature. Workspace-wide
/// builds unify that feature ON through unrelated crates (the siumai
/// stack), which echoes instances in authored key order and makes the
/// lint diagnostics depend on the build graph. Canonical sorting keeps
/// every diagnostic byte-identical across build configurations; the
/// pinned byte-exact regression tests fail loudly if this ever drifts.
fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::Value::String(k.clone()),
                        canonical_json(&map[k])
                    )
                })
                .collect();
            format!("{{{}}}", body.join(","))
        }
        serde_json::Value::Array(items) => {
            let body: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", body.join(","))
        }
        // Primitives echo identically to `serde_json::to_string`.
        prim => prim.to_string(),
    }
}

/// Message for a validation error, with a build-independent instance
/// echo for the collapsed `anyOf`/`oneOf` shapes — the only kinds whose
/// `Display` embeds the whole instance. Every other kind keeps
/// jsonschema's `Display` verbatim (its echoes are scalar or
/// single-keyed and cannot reorder).
fn diagnostic_message(err: &ValidationError<'_>) -> String {
    match err.kind() {
        ValidationErrorKind::AnyOf { .. } => format!(
            "{} is not valid under any of the schemas listed in the 'anyOf' keyword",
            canonical_json(err.instance())
        ),
        ValidationErrorKind::OneOfNotValid { .. } => format!(
            "{} is not valid under any of the schemas listed in the 'oneOf' keyword",
            canonical_json(err.instance())
        ),
        _ => err.to_string(),
    }
}

/// Schema-path substring identifying the exactly-one oneOf failure of
/// `RouteDslPermissionValueSource` (probe-confirmed against jsonschema
/// 0.52.1 with the injected oneOf: the nested `OneOfNotValid` error's
/// schema path is `/$defs/RouteDslPermissionValueSource/oneOf`). The
/// targeted tests fail loudly if a future version changes the form.
const PERMISSION_VALUE_SOURCE_ONEOF_MARKER: &str = "RouteDslPermissionValueSource/oneOf";

/// The `RouteDslPermissionPolicy` child key a validator instance path
/// ends with (`resource` or `action`), if any — the field context for a
/// targeted exactly-one permission value-source diagnostic.
///
/// Splits the raw validator instance path on `/` (dropping the empty
/// first segment) and matches the LAST segment. Only
/// `RouteDslPermissionPolicy` carries these child keys, so the tail IS
/// the field context regardless of the prefix (envelope depth, route
/// index, rest/mcp nesting).
fn permission_value_source_field(instance_path: &str) -> Option<&'static str> {
    let mut last = "";
    for segment in instance_path.split('/').filter(|s| !s.is_empty()) {
        last = segment;
    }
    match last {
        "resource" => Some("resource"),
        "action" => Some("action"),
        _ => None,
    }
}

/// Render the found-set of a permission value-source instance: the keys
/// among `literal`, `header`, `property` whose value is non-null, joined
/// in that canonical order (regardless of authored order); `none set`
/// when the collection is empty (a non-object or missing instance
/// reports `none set` too — `Value::get` yields `None` there).
fn found_sources(value: &serde_json::Value) -> String {
    let found = non_null_source_keys(value);
    if found.is_empty() {
        "none set".to_string()
    } else {
        found.join(", ")
    }
}

/// The keys among `literal`, `header`, `property` whose value is
/// non-null in a permission value-source instance, in that canonical
/// order (regardless of authored order). The count drives the
/// targeted-diagnostic gate: exactly one means the oneOf failed on the
/// value's TYPE, zero or several on the cardinality.
fn non_null_source_keys(value: &serde_json::Value) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = Vec::new();
    for key in ["literal", "header", "property"] {
        if value.get(key).is_some_and(|v| !v.is_null()) {
            found.push(key);
        }
    }
    found
}

/// Recursively collect the instance paths of every exactly-one oneOf
/// failure of `RouteDslPermissionValueSource` buried inside a collapsed
/// error tree (jsonschema 0.52 nests branch errors as owned
/// `ValidationError<'static>` inside the `AnyOf`/`OneOfNotValid`
/// contexts, so owned paths avoid borrowed-error gymnastics).
///
/// - a `OneOfNotValid` whose schema path contains
///   [`PERMISSION_VALUE_SOURCE_ONEOF_MARKER`] is a match: push its
///   instance path and stop descending (the branch errors below it are
///   per-branch `required` noise);
/// - any other `AnyOf`/`OneOfNotValid` may still bury a match deeper:
///   recurse into every nested error of every branch;
/// - every other kind cannot bury a permission oneOf failure: skip.
fn collect_permission_oneof_paths(err: &ValidationError<'_>, out: &mut Vec<String>) {
    match err.kind() {
        ValidationErrorKind::OneOfNotValid { .. }
            if err
                .schema_path()
                .as_str()
                .contains(PERMISSION_VALUE_SOURCE_ONEOF_MARKER) =>
        {
            out.push(err.instance_path().as_str().to_owned());
        }
        ValidationErrorKind::AnyOf { context } | ValidationErrorKind::OneOfNotValid { context } => {
            for branch in context {
                for nested in branch {
                    collect_permission_oneof_paths(nested, out);
                }
            }
        }
        _ => {}
    }
}

/// Count the non-empty `/` segments of a validator instance path —
/// the nesting depth used by the anyOf de-collapse passes to separate
/// leaf-level errors from errors reported at the collapsed node.
fn segment_depth(path: &str) -> usize {
    path.split('/').filter(|s| !s.is_empty()).count()
}

/// Collect the NON-pattern sibling defects co-located in a collapsed
/// AnyOf error whose pattern leaves were already collected (rc-lys6a):
/// the defects the old replace-when-present pass silently dropped.
///
/// Walks the same `context` branches as the pattern pass:
/// - nested `Pattern` errors are skipped (owned by the pattern walk);
/// - a nested `Type` error whose instance path equals the collapsed
///   error's own path is branch noise, not a defect — the
///   `{"type": "null"}` sibling branch failing because the instance is
///   an object of the intended shape;
/// - nested errors strictly SHALLOWER than the collapsed node are
///   skipped (defensive; jsonschema reports nested branch errors with
///   absolute deeper-or-equal paths);
/// - dedup by `(instance_path, message)` preserving first-occurrence
///   order (branches retry the same subschema shapes, so the same
///   defect can surface more than once).
fn collect_sibling_errors<'a>(
    err: &'a ValidationError<'_>,
    own_depth: usize,
    out: &mut Vec<&'a ValidationError<'a>>,
) {
    let ValidationErrorKind::AnyOf { context } = err.kind() else {
        return;
    };
    let own_path = err.instance_path().as_str();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for branch in context {
        for nested in branch {
            if matches!(nested.kind(), ValidationErrorKind::Pattern { .. }) {
                continue;
            }
            let nested_path = nested.instance_path().as_str();
            if matches!(nested.kind(), ValidationErrorKind::Type { .. }) && nested_path == own_path
            {
                continue;
            }
            if segment_depth(nested_path) < own_depth {
                continue;
            }
            if seen.insert((nested_path.to_string(), diagnostic_message(nested))) {
                out.push(nested);
            }
        }
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
/// position — reported as an Error (boot hard-fails on it). The
/// namespace is `env` or `arg`; `arg:` tokens hard-fail the boot tree
/// walk with or without a fallback suffix (jobargs Task 2.1).
struct UnresolvedPlaceholder {
    namespace: &'static str,
    var: String,
    span: Span,
}

/// A whole-scalar `${env:VAR:-d}` token whose default is a clean integer
/// (i64-or-u64 magnitude) — a candidate for the integer-position
/// carve-out.
struct IntCandidate {
    /// Leaf path in `value` coordinates (noyalib form, pre-envelope).
    path: String,
    /// Authored span of the leaf (== the Info-note span for this leaf).
    span: Span,
    /// The default as a JSON number (i64 or u64 magnitude).
    number: serde_json::Number,
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
///   type-error against the string, string positions pass cleanly; a
///   clean-integer default (`-?(0|[1-9][0-9]*)`, i64-or-u64 parse) is
///   ALSO recorded as an [`IntCandidate`] so `analyze` can validate a
///   NUMBER copy of that leaf (integer-position carve-out);
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
/// String-forcing arm in `enforce_typing_mirror` never sees it, leaving
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
    int_candidates: &mut Vec<IntCandidate>,
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
                enforce_typing_mirror(parsed, v, &child, raw, unresolved, int_candidates);
            }
        }
        serde_json::Value::Array(items) => {
            if let Some(default) = structure_changing_default(parsed, path, raw) {
                *value = serde_json::Value::String(default);
                return;
            }
            for (i, v) in items.iter_mut().enumerate() {
                enforce_typing_mirror(
                    parsed,
                    v,
                    &format!("{path}[{i}]"),
                    raw,
                    unresolved,
                    int_candidates,
                );
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
                    // Integer-position carve-out candidate: a clean-integer
                    // default also gets a NUMBER validation copy in
                    // `analyze` (the boot loader coerces such a leaf when
                    // the schema position wants an integer).
                    if let Some(number) = clean_integer(&default) {
                        int_candidates.push(IntCandidate {
                            path: path.to_string(),
                            span,
                            number,
                        });
                    }
                }
                Some(WholeScalarEnvToken::NoDefault { var }) => {
                    unresolved.push(UnresolvedPlaceholder {
                        namespace: "env",
                        var,
                        span,
                    });
                }
                None => {
                    // Embedded-token parity (rc-93wct): the boot tree-walk
                    // Unresolved-fails on EVERY unescaped no-default token,
                    // including ones inside a larger scalar
                    // (`id: svc-${env:HOST}`). The whole-scalar arms above
                    // cannot see them, so scan the authored slice with
                    // ENV_RE — it consumes `$${env:...}` / `$${arg:...}` /
                    // `$$` escapes atomically — and flag each bare
                    // no-default match, span-anchored at its offset inside
                    // the leaf. Value leaves only: comments and mapping
                    // keys are never visited by this walk.
                    for caps in env_regex().captures_iter(authored) {
                        let (Some(whole), Some(namespace), Some(var)) =
                            (caps.get(3), caps.get(4), caps.get(5))
                        else {
                            // `$${env:...}` / `$${arg:...}` / `$$` escape
                            // arm — never unresolved.
                            continue;
                        };
                        // Boot-tree parity (jobargs Task 2.1): the tree
                        // walk has no argument context, so EVERY unescaped
                        // `arg:` token Unresolved-fails it — including the
                        // `:-fallback` form, which the arg grammar
                        // rejects. Env tokens only fail without a default
                        // (a default substitutes cleanly).
                        if namespace.as_str() != "arg" && caps.get(6).is_some() {
                            continue;
                        }
                        unresolved.push(UnresolvedPlaceholder {
                            namespace: if namespace.as_str() == "arg" {
                                "arg"
                            } else {
                                "env"
                            },
                            var: var.as_str().to_string(),
                            span: Span::new(span.start + whole.start(), span.start + whole.end()),
                        });
                    }
                }
            }
        }
    }
}

/// SYNC: lexical+parse gate mirroring camel-dsl's
/// `env_int_probe::clean_integer` (i64-or-u64 magnitude) and
/// camel-config's `clean_i64` (i64-only); crate purity forbids the
/// dependencies. Update all three together.
///
/// Lexical form `-?(0|[1-9][0-9]*)` — ASCII digits, optional leading `-`,
/// no leading zeros (YAML 1.1 octal ambiguity), no whitespace, no plus —
/// then a successful i64 or u64 parse; ROUTE_SCHEMA enforces the target
/// field's bounds. Returns the default as a JSON number for the
/// integer-position carve-out validation copy.
fn clean_integer(s: &str) -> Option<serde_json::Number> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    let lexically_clean = match digits.as_bytes() {
        // `0` alone — no leading zeros allowed.
        [b'0'] => true,
        // `[1-9]` followed by ASCII digits only.
        [first, rest @ ..] if first.is_ascii_digit() && *first != b'0' => {
            rest.iter().all(|b| b.is_ascii_digit())
        }
        _ => false,
    };
    if !lexically_clean {
        return None;
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(serde_json::Number::from(n));
    }
    s.parse::<u64>().ok().map(serde_json::Number::from)
}

/// Wrap a `value`-coordinates tree into the `{routes: [...]}` envelope
/// form implied by `envelope_depth` (see `analyze` for the depth table).
fn wrap_envelope(value: serde_json::Value, envelope_depth: usize) -> serde_json::Value {
    match envelope_depth {
        0 => value,
        1 => serde_json::json!({ "routes": value }),
        _ => serde_json::json!({ "routes": [value] }),
    }
}

/// Whether a leaf- or ancestor-anchored span covers `inner`. The STRING
/// copy's schema type violation for a whole-scalar token may be reported
/// by jsonschema directly at the leaf (`type` keyword) or collapsed into
/// an anyOf/oneOf Error at an ancestor node (branch failures surface at
/// the nearest non-matching schema node, e.g. the step object) — either
/// way the error's resolved span covers the leaf's authored span.
fn span_covers(outer: Span, inner: &Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// Integer-position carve-out (typing mirror int arm).
///
/// For every [`IntCandidate`] leaf (whole-scalar `${env:X:-d}` token with
/// a clean-integer default), validates two copies against ROUTE_SCHEMA:
/// the STRING copy the typing mirror produces, and a variant where that
/// leaf is the NUMBER. The NUMBER copy wins exactly when the STRING copy
/// produces an Error anchored at the leaf's chain (its span covers the
/// leaf) AND the NUMBER copy produces no error there — the boot loader
/// coerces such a leaf, so flagging it would be a false positive. The
/// returned instance is then the accumulated NUMBER copy (identical to
/// the STRING copy everywhere else: changing one scalar leaf can only
/// affect errors anchored at that leaf or its ancestors). Any other
/// outcome keeps today's STRING-copy behavior (Info note for valid string
/// positions; Error for bool positions, non-integer defaults, no-default
/// tokens). The carved leaves' spans come back for Info-note suppression.
fn integer_carve_out_instance(
    validator: &Validator,
    parsed: &cst::Document,
    value: serde_json::Value,
    envelope_depth: usize,
    candidates: &[IntCandidate],
) -> (serde_json::Value, Vec<Span>) {
    if candidates.is_empty() {
        return (wrap_envelope(value, envelope_depth), Vec::new());
    }
    let string_instance = wrap_envelope(value.clone(), envelope_depth);
    let string_errors: Vec<_> = validator.iter_errors(&string_instance).collect();
    let mut working = value;
    let mut carved_spans = Vec::new();
    for cand in candidates {
        // Condition A: the STRING copy flags the leaf's chain (error span
        // covers the leaf — leaf-anchored or ancestor-collapsed).
        let string_flags_leaf = string_errors.iter().any(|err| {
            let noya_path = instance_path_to_noyalib(err.instance_path().as_str(), envelope_depth);
            span_covers(
                crate::document::value_span_for(parsed, &noya_path),
                &cand.span,
            )
        });
        if !string_flags_leaf {
            continue;
        }
        // Condition B: applying the number to the accumulated tree clears
        // the leaf's chain. Any remaining error there (minimum/maximum/
        // enum on the coerced number) keeps today's behavior — boot
        // rejects those too.
        let mut probe = working.clone();
        if !set_leaf_at(
            &mut probe,
            &cand.path,
            serde_json::Value::Number(cand.number.clone()),
        ) {
            continue;
        }
        let probe_instance = wrap_envelope(probe.clone(), envelope_depth);
        let probe_flags_leaf = validator.iter_errors(&probe_instance).any(|err| {
            let noya_path = instance_path_to_noyalib(err.instance_path().as_str(), envelope_depth);
            span_covers(
                crate::document::value_span_for(parsed, &noya_path),
                &cand.span,
            )
        });
        if probe_flags_leaf {
            continue;
        }
        working = probe;
        carved_spans.push(cand.span.clone());
    }
    (wrap_envelope(working, envelope_depth), carved_spans)
}

/// Overwrite the leaf at a noyalib-style `value`-coordinates path
/// (`a.b[0].c`, possibly rooted at `[0]`) with `new`. Returns `false`
/// when a segment is missing or mistyped (defensive: the typing mirror
/// walked the same tree to collect the candidate).
fn set_leaf_at(root: &mut serde_json::Value, path: &str, new: serde_json::Value) -> bool {
    let mut cur = root;
    for part in path.split('.') {
        let (key, indices) = split_index_suffix(part);
        if !key.is_empty() {
            let serde_json::Value::Object(map) = cur else {
                return false;
            };
            let Some(child) = map.get_mut(key) else {
                return false;
            };
            cur = child;
        }
        for idx in indices {
            let serde_json::Value::Array(items) = cur else {
                return false;
            };
            let Some(child) = items.get_mut(idx) else {
                return false;
            };
            cur = child;
        }
    }
    *cur = new;
    true
}

/// Split `name[0][1]` into `("name", [0, 1])`; a part that starts with an
/// index (`[0]`, the legacy-array root form) yields `("", [0])`.
fn split_index_suffix(part: &str) -> (&str, Vec<usize>) {
    let Some((key, rest)) = part.split_once('[') else {
        return (part, Vec::new());
    };
    let trimmed = rest.strip_suffix(']').unwrap_or(rest);
    let indices = trimmed
        .split("][")
        .filter_map(|s| s.parse::<usize>().ok())
        .collect();
    (key, indices)
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
