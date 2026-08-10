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
//! - Most keywords (type/enum/pattern/const/format/minimum/`exclusiveMinimum`/
//!   anyOf/oneOf/minItems/maxItems/required) anchor on the JSON-pointer
//!   instance node: jsonschema already points `required` at the parent
//!   object, so the default anchoring is correct.
//! - `additionalProperties` anchors on each offending KEY, extracted from
//!   [`ValidationErrorKind::AdditionalProperties { unexpected }`].
//!
//! The compiled validator is cached in a process-wide [`OnceLock`].

use std::sync::OnceLock;

use camel_api::component_metadata::ComponentMetadataCatalog;
use jsonschema::{Validator, error::ValidationErrorKind};
use noyalib::cst;

use crate::ROUTE_SCHEMA;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity, Span};
use crate::document::Document;
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

        // Convert the raw source to a JSON value. An unconvertible document
        // yields no diagnostics (no panic, no abort).
        let Some(value) = raw_to_json_value(doc) else {
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
        //   - any other object   -> bare single route -> {routes: [value]}, depth 2
        //   - scalar/null        -> R-SCHEMA cannot validate; R-SYN owns it.
        let envelope_depth = match &value {
            serde_json::Value::Array(_) => 1,
            serde_json::Value::Object(map) if map.contains_key("routes") => 0,
            serde_json::Value::Object(_) => 2,
            _ => return Vec::new(),
        };
        let instance = match envelope_depth {
            0 => value,
            1 => serde_json::json!({ "routes": value }),
            _ => serde_json::json!({ "routes": [value] }),
        };

        // Parse the CST once so span resolution reuses it across all errors.
        let Ok(parsed) = cst::parse_document(&doc.raw) else {
            return Vec::new();
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

/// Convert `doc.raw` (YAML or JSON) to a [`serde_json::Value`].
///
/// Deserializes via noyalib's serde compat shim; on ANY conversion error
/// returns `None` (R-SCHEMA then returns no diagnostics — no panic).
fn raw_to_json_value(doc: &Document) -> Option<serde_json::Value> {
    let value: serde_json::Value = noyalib::compat::serde_yaml::from_str(&doc.raw).ok()?;
    Some(value)
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::StubCatalog;

    fn slice<'a>(raw: &'a str, span: &Span) -> &'a str {
        &raw[span.start..span.end]
    }

    fn analyze(source: &str) -> Vec<Diagnostic> {
        let doc = Document::parse(source);
        assert!(
            doc.parse_failure.is_none(),
            "test fixtures must parse cleanly (got: {:?})",
            doc.parse_failure
        );
        RSchemaRule.analyze(&doc, &StubCatalog::empty())
    }

    fn rschema_only(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
        diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::RSchema)
            .collect()
    }

    #[test]
    fn rschema_wrong_type_reports_value() {
        // `steps` must be an array; a string violates the `type` keyword.
        let source = "id: r1\nfrom: direct:start\nsteps: notanarray\n";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            rschema
                .iter()
                .any(|d| slice(source, &d.span) == "notanarray"),
            "expected a RSchema diagnostic on the `steps` string value; got spans: {:?}",
            rschema
                .iter()
                .map(|d| slice(source, &d.span))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn rschema_missing_required_reports_parent() {
        // `RouteDslRoute` requires both `id` and `from`; omitting `from`
        // yields a `required` error whose instance_path is the route object
        // itself (the missing key has no node). The span anchors on that
        // parent mapping — the whole route, which contains `id`.
        let source = "id: r1\n";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            !rschema.is_empty(),
            "expected at least one RSchema diagnostic for the missing `from`"
        );
        // The parent-object span must be non-empty and cover the route body.
        let covers_parent = rschema
            .iter()
            .any(|d| d.span.start == 0 && source[d.span.start..d.span.end].contains("id"));
        assert!(
            covers_parent,
            "expected the diagnostic to anchor on the parent route object"
        );
    }

    #[test]
    fn rschema_minimum_reports_numeric_value() {
        // `concurrent` is a direct `RouteDslRoute` property with `minimum: 0`
        // (not wrapped in anyOf, so the keyword fires at the leaf). `-1`
        // violates it; the diagnostic must anchor on the offending value.
        let source = "id: r1\nfrom: direct:start\nconcurrent: -1\n";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            rschema.iter().any(|d| slice(source, &d.span) == "-1"),
            "expected a RSchema diagnostic on `-1`; got spans: {:?}",
            rschema
                .iter()
                .map(|d| slice(source, &d.span))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn rschema_anyof_failure_reports_value() {
        // `circuit_breaker` is `anyOf: [RouteDslCircuitBreaker, null]`; an
        // integer matches neither branch. The diagnostic anchors on the value.
        let source = "id: r1\nfrom: direct:start\ncircuit_breaker: 123\n";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            rschema.iter().any(|d| slice(source, &d.span) == "123"),
            "expected a RSchema diagnostic on `123`; got spans: {:?}",
            rschema
                .iter()
                .map(|d| slice(source, &d.span))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn rschema_additional_properties_reports_key() {
        // `RouteDslRoute` has `additionalProperties: false`; `bogus` is not
        // allowed. The diagnostic must anchor on the offending KEY.
        let source = "id: r1\nfrom: direct:start\nbogus: 1\n";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            rschema.iter().any(|d| slice(source, &d.span) == "bogus"),
            "expected a RSchema diagnostic on the `bogus` key; got spans: {:?}",
            rschema
                .iter()
                .map(|d| slice(source, &d.span))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn rschema_items_violation_reports_array() {
        // `RouteDslRoute.steps` has `items: { $ref: "#/$defs/RouteDslStep" }`
        // and `type: "array"`. A numeric element violates the `items`
        // subschema (it does not match any `anyOf` branch of RouteDslStep).
        // jsonschema points the instance_path at the offending element, so the
        // diagnostic must anchor on the `123` element, not the whole array.
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - 123\n";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            rschema.iter().any(|d| slice(source, &d.span) == "123"),
            "expected an RSchema diagnostic anchoring on the offending array element `123`; got spans: {:?}",
            rschema
                .iter()
                .map(|d| slice(source, &d.span))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn rschema_skips_when_parse_failure() {
        let source = "steps:\n  - to: timer:foo\n  bad: [";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_some(), "fixture must fail to parse");
        let diags = RSchemaRule.analyze(&doc, &StubCatalog::empty());
        assert!(
            diags.is_empty(),
            "R-SCHEMA must skip a parse-failed document"
        );
    }

    #[test]
    fn rschema_envelope_form_valid_is_silent() {
        // Regression for the false-positive cluster: a valid multi-route
        // envelope `{routes: [...]}` must be validated AS-IS (not re-wrapped).
        // Re-wrapping produced `{routes: [{routes: [...]}]}` and dozens of
        // bogus "id/from required" + "'routes' was unexpected" errors.
        let source = "\
routes:
  - id: r1
    from: direct:start
    steps:
      - to: log:info
  - id: r2
    from: timer:tick?period=1000
    steps:
      - to: log:info
";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            rschema.is_empty(),
            "a valid multi-route envelope must produce no R-SCHEMA errors; got: {:?}",
            rschema
        );
    }

    #[test]
    fn rschema_envelope_form_reports_real_defect() {
        // Envelope form must still catch a genuine defect in route N>0; the
        // span-strip must handle any route index (instance_path /routes/1/...).
        let source = "\
routes:
  - id: r1
    from: direct:start
  - id: r2
    from: direct:other
    steps: notanarray
";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            !rschema.is_empty(),
            "expected an R-SCHEMA error for the malformed `steps` in route 2"
        );
    }

    #[test]
    fn rschema_legacy_array_form_is_silent_when_valid() {
        // Legacy array form `[ {...}, {...} ]` is normalised to
        // `{routes: <array>}`; valid bare routes must pass.
        let source = "\
- id: r1
  from: direct:start
  steps:
    - to: log:info
";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            rschema.is_empty(),
            "a valid legacy-array route must produce no R-SCHEMA errors; got: {:?}",
            rschema
        );
    }

    #[test]
    fn rschema_bare_route_valid_is_silent() {
        // Bare single-route form (depth 2) with a clean document must produce
        // no R-SCHEMA diagnostics — proves the depth-2 normalisation path
        // validates cleanly, not just defects.
        let source = "\
id: r1
from: direct:start
steps:
  - to: log:out
";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            rschema.is_empty(),
            "a valid bare single route must produce no R-SCHEMA errors; got: {:?}",
            rschema
        );
    }

    #[test]
    fn rschema_legacy_array_defect_anchors_element() {
        // Legacy array form (depth 1) with a defect: `steps` is a string
        // instead of an array. The diagnostic must anchor on the offending
        // value (`notanarray`), proving span resolution works through the
        // envelope_depth=1 wrapper.
        let source = "\
- id: r1
  from: direct:start
  steps: notanarray
";
        let diags = analyze(source);
        let rschema = rschema_only(&diags);
        assert!(
            !rschema.is_empty(),
            "expected at least one RSchema diagnostic for the malformed `steps`"
        );
        assert!(
            rschema
                .iter()
                .any(|d| slice(source, &d.span) == "notanarray"),
            "expected the diagnostic to anchor on `notanarray`; got spans: {:?}",
            rschema
                .iter()
                .map(|d| slice(source, &d.span))
                .collect::<Vec<_>>()
        );
    }
}
