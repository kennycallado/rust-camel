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
fn rschema_rest_form_document_is_silent() {
    // A `rest:`-block document is a valid DSL form (camel-dsl
    // `RouteDslRest`, lowered by `expand_rest_into`), but ROUTE_SCHEMA
    // does not model it yet. The bare-route normalisation used to wrap it
    // as `{routes: [{rest: ...}]}`, and `RouteDslRoute`'s
    // `additionalProperties: false` rejected the `rest` key — a false
    // positive on `examples/rest-crud/routes/secured.yaml` (rc-xmbi).
    // Until RestDsl defs land in ROUTE_SCHEMA, R-SCHEMA skips the rest
    // form entirely (same policy as scalar/null documents).
    let source = "\
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    security_policy:
      roles: [\"user\"]
      provider: native-demo
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "a rest-block document must not emit R-SCHEMA until the schema \
             models the form; got: {:?}",
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

#[test]
fn rschema_string_position_default_silent_with_info() {
    // rc-93wct rev 2: a string-typed field (`id` is strict "string" in
    // ROUTE_SCHEMA) carrying a whole-scalar `${env:X:-d}` token
    // validates against the substituted default — which the typing
    // mirror keeps a STRING — so zero Errors and exactly one Info note
    // anchored on the authored placeholder.
    let source = "\
id: ${env:MY_TITLE:-hello}
from: direct:start
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "string-position default must not be type-flagged; got: {errors:?}"
    );
    let infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        1,
        "expected exactly one Info note for the substituted default; got: {infos:?}"
    );
    let info = infos[0];
    assert!(
        info.message.contains("MY_TITLE") && info.message.contains(":-hello"),
        "Info message must name the variable and default; got: {}",
        info.message
    );
    assert!(
        slice(source, &info.span).contains("${env:MY_TITLE:-hello}"),
        "Info span must anchor on the placeholder value node; sliced: {:?}",
        slice(source, &info.span)
    );
}

#[test]
fn rschema_int_position_clean_default_no_diagnostic() {
    // Integer-position carve-out (typing mirror int arm): `max_requests`
    // wants an integer and the default `2` is a clean integer, so the
    // validation copy carries the NUMBER — exactly as the boot loader
    // coerces the leaf. No Error, no Info note, nothing.
    let source = "\
id: r1
from: direct:start
steps:
  - throttle:
      max_requests: ${env:MY_LIMIT:-2}
      period_secs: 1
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "int-position clean-integer default must produce no diagnostic; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_numeric_default_in_string_field_stays_string() {
    // Typing-mirror false-positive fix (rc-93wct rev 2): a
    // numeric-looking default substituted into a STRING field must not
    // be re-inferred as a number by the whole-text splice (the raw copy
    // `id: 8080` would type-error). The mirror forces the whole-scalar
    // leaf to the JSON string "8080" — clean pass, one Info note.
    let source = "\
id: ${env:RC93WCT_T:-8080}
from: direct:start
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "numeric default in a string field must stay a string; got: {errors:?}"
    );
    let infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        1,
        "expected exactly one Info note for the substituted default; got: {infos:?}"
    );
    assert!(
        infos[0].message.contains(":-8080"),
        "Info must report the substituted `:-8080` default; got: {}",
        infos[0].message
    );
}

#[test]
fn rschema_structure_changing_default_no_false_positive() {
    // Structure-changing-default ceiling: an unquoted flow-style default
    // (`[a,b]`) splices into a NON-scalar node in the validation copy, so
    // the scalar String-forcing arm never fires. The typing mirror must
    // not let the splice decide the type — boot keeps the substituted
    // leaf a STRING (the route loads), so this shape at a string position
    // must stay silent (zero Errors), never a false-positive type Error.
    let source = "\
id: ${env:RC_CEIL:-[a,b]}
from: direct:start
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "structure-changing default at a string position must not be \
             type-flagged (boot string typing); got: {errors:?}"
    );
}

#[test]
fn rschema_no_default_flagged_at_string_position() {
    // A whole-scalar `${env:X}` (no default) is an Error even where the
    // literal placeholder text is a valid string — boot hard-fails on
    // the unresolved variable, so lint must not stay silent. The
    // `$${env:X}` escape is exempt (boot emits the literal text).
    let source = "\
id: ${env:NO_DEF_xyz}
from: $${env:ESCAPED_FROM}
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one Error flagging the unresolved token; got: {errors:?}"
    );
    assert!(
        errors[0].message.contains("NO_DEF_xyz"),
        "Error message must name the unresolved variable; got: {}",
        errors[0].message
    );
    assert!(
        slice(source, &errors[0].span).contains("${env:NO_DEF_xyz}"),
        "Error must anchor on the token; sliced: {:?}",
        slice(source, &errors[0].span)
    );
    assert!(
        rschema.iter().all(|d| !d.message.contains("ESCAPED_FROM")),
        "escaped $${{env:...}} must produce no diagnostics; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_mixed_document_per_token() {
    // Per-token semantics under the typing canon + integer-position
    // carve-out: the string-position WITH_DEF token validates cleanly
    // with exactly one Info note, while the int-position ALSO_WITH_DEF
    // token (clean integer default) is carved out — no diagnostic at
    // all. Each token is judged at its own position. (A whole-document
    // Err→raw fallback would re-literal BOTH tokens and lose the
    // defaulted one's clean pass.)
    let source = "\
id: ${env:WITH_DEF:-hello}
from: direct:start
steps:
  - throttle:
      max_requests: ${env:ALSO_WITH_DEF:-2}
      period_secs: 1
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let with_def_infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info && d.message.contains("${env:WITH_DEF}"))
        .collect();
    assert_eq!(
        with_def_infos.len(),
        1,
        "expected exactly one Info note for WITH_DEF; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "int-position clean default must not be type-flagged; got: {:?}",
        errors
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    assert!(
        rschema.iter().all(|d| !d.message.contains("ALSO_WITH_DEF")),
        "carved-out int leaf must produce no diagnostic at all; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rschema_commented_tokens_no_diagnostics() {
    // Comments are not part of the parsed instance: tokens inside them
    // resolve no value-leaf span and are skipped entirely — no Info
    // note with a (0,0) miss span, no unresolved Error, nothing.
    let source = "\
# ${env:C_NOD} unresolved token in a comment
id: commented
from: direct:start
# ${env:C_DEF:-d} defaulted token in a comment
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.is_empty(),
        "comment tokens must produce no diagnostics; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}

// NOTE: the ambient-env hermeticity witness lives in
// `crates/camel-lint/tests/env_hermeticity.rs` — poisoning the process
// environment needs the standard-library env API, which the crate
// purity gate forbids anywhere under `src/`.

#[test]
fn rschema_no_default_int_position_still_error() {
    // A whole-scalar token with no default keeps its literal instance
    // value and is explicitly flagged by the typing-mirror walk (boot
    // hard-fails on the unresolved variable). At an int position the
    // schema type check fires on the same authored placeholder too —
    // the assertion holds for either source of the Error. The
    // integer-position carve-out never applies: there is no default to
    // coerce.
    let source = "\
id: r1
from: direct:start
steps:
  - throttle:
      max_requests: ${env:NO_DEFAULT_xyz}
      period_secs: 1
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    assert!(
        rschema.iter().any(|d| d.severity == Severity::Error
            && slice(source, &d.span).contains("${env:NO_DEFAULT_xyz}")),
        "expected an Error-severity diagnostic on the literal placeholder; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn int_position_notanumber_still_error() {
    // The carve-out needs a CLEAN integer default: `notanumber` stays on
    // the STRING validation copy → a schema type Error anchored on the
    // authored placeholder (boot parity: the route fails there too).
    let source = "\
id: r1
from: direct:start
steps:
  - throttle:
      max_requests: ${env:MY_LIMIT:-notanumber}
      period_secs: 1
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors
            .iter()
            .any(|d| slice(source, &d.span).contains("${env:MY_LIMIT:-notanumber}")),
        "expected a type Error anchored on the placeholder; got: {:?}",
        errors
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn int_position_leading_zero_still_error() {
    // Leading zeros are not a clean integer (YAML 1.1 octal ambiguity):
    // `007` keeps string typing → schema type Error, like boot.
    let source = "\
id: r1
from: direct:start
steps:
  - throttle:
      max_requests: ${env:MY_LIMIT:-007}
      period_secs: 1
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors
            .iter()
            .any(|d| slice(source, &d.span).contains("${env:MY_LIMIT:-007}")),
        "expected a type Error anchored on the placeholder; got: {:?}",
        errors
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn int_default_at_polymorphic_position_keeps_info() {
    // Guard-arm pin for the integer-position carve-out (Condition A):
    // `set_header.value` is polymorphic in ROUTE_SCHEMA (no `type`
    // constraint), so the STRING copy validates cleanly and the carve-out
    // must NOT fire — the numeric-looking default `${env:H:-123}` keeps
    // today's string-position behavior: exactly one Info note, zero
    // Errors. A refactor that carved unconditionally (dropping the
    // Condition A guard) would suppress the Info and fail this test.
    let source = "\
id: r1
from: direct:start
steps:
  - set_header:
      key: X-Custom
      value: ${env:H:-123}
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "polymorphic-position default must not be type-flagged; got: {errors:?}"
    );
    let infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        1,
        "expected exactly one Info note for the substituted default; got: {infos:?}"
    );
    assert!(
        infos[0].message.contains(":-123"),
        "Info must report the substituted `:-123` default; got: {}",
        infos[0].message
    );
    assert!(
        slice(source, &infos[0].span).contains("${env:H:-123}"),
        "Info span must anchor on the authored placeholder; sliced: {:?}",
        slice(source, &infos[0].span)
    );
}

#[test]
fn bool_position_placeholder_still_error() {
    // Guard-arm pin for the integer-position carve-out (Condition B):
    // `auto_startup` is a strict boolean field in ROUTE_SCHEMA. The
    // clean-integer default `1` makes the leaf an IntCandidate, but the
    // NUMBER copy still type-errors at the boolean position — Condition B
    // keeps the STRING copy, so the schema type Error anchored on the
    // authored placeholder survives (boot parity: the loader rejects the
    // coerced number there too).
    let source = "\
id: r1
from: direct:start
auto_startup: ${env:AS:-1}
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors
            .iter()
            .any(|d| slice(source, &d.span).contains("${env:AS:-1}")),
        "expected a type Error anchored on the bool-position placeholder; got: {:?}",
        errors
            .iter()
            .map(|d| (d.severity, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    // The Info note must ALSO survive: the candidate reached the carve-out
    // loop, Condition A fired (string copy errors), and Condition B kept
    // the STRING copy (the number copy errors too) — so the leaf is not
    // carved and the substituted-default note is not suppressed. A
    // refactor that let the number copy win at non-integer positions
    // would carve the leaf, drop this Info, and fail the assertion.
    let infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        1,
        "expected the substituted-default Info note to survive the bool \
             position; got: {infos:?}"
    );
}

#[test]
fn clean_integer_lexical_gate() {
    // SYNC gate pin: mirrors camel-dsl env_int_probe::clean_integer /
    // camel-config clean_i64 — lexical `-?(0|[1-9][0-9]*)` (no leading
    // zeros, no whitespace, no plus), then i64-or-u64 parse.
    let clean = [
        "0",
        "2",
        "-3",
        "9223372036854775807",  // i64::MAX
        "9223372036854775808",  // i64 overflow, u64 magnitude
        "18446744073709551615", // u64::MAX
    ];
    for s in clean {
        assert!(clean_integer(s).is_some(), "`{s}` must be a clean integer");
    }
    let not_clean = [
        "",
        "-",
        "+2",
        "007",
        "-007",
        "1e3",
        " 2",
        "2 ",
        "1_000",
        "2.0",
        "true",
        "99999999999999999999999", // > u64::MAX — overflow, boot rejects
    ];
    for s in not_clean {
        assert!(
            clean_integer(s).is_none(),
            "`{s}` must NOT be a clean integer"
        );
    }
}

#[test]
fn rschema_duplicate_tokens_anchor_distinct_spans() {
    // The SAME `${env:DUP_VAR:-same}` token in two string-typed fields
    // must emit one Info note PER authored occurrence, each anchored on
    // its own value node (occurrence queue — a first-match search
    // collapsed both notes onto the first route's `id`). String
    // positions keep the document Error-free under the typing canon.
    let source = "\
routes:
  - id: ${env:DUP_VAR:-same}
    from: direct:a
  - id: ${env:DUP_VAR:-same}
    from: direct:b
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "string-position duplicates must not be type-flagged; got: {errors:?}"
    );
    let infos: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        2,
        "expected two Info notes (one per authored occurrence); got: {infos:?}"
    );
    let (a, b) = (slice(source, &infos[0].span), slice(source, &infos[1].span));
    assert!(
        a.contains("${env:DUP_VAR:-same}") && b.contains("${env:DUP_VAR:-same}"),
        "both Info spans must slice to an authored occurrence of the token; \
             got: {:?} and {:?}",
        a,
        b
    );
    assert_ne!(
        infos[0].span, infos[1].span,
        "the two Info notes must anchor on DIFFERENT authored occurrences"
    );
}

#[test]
fn lint_accepts_remove_header_route() {
    // `remove_header` is a first-class RouteDslStep variant: a route using
    // it must validate with zero errors against ROUTE_SCHEMA. Direct
    // jsonschema validation (not the full Rule) isolates the schema shape
    // from rule-level span anchoring.
    let schema: serde_json::Value =
        serde_json::from_str(ROUTE_SCHEMA).expect("embedded route schema is valid JSON");
    let validator = jsonschema::validator_for(&schema).expect("embedded route schema must compile");
    let doc: serde_json::Value = serde_json::from_str(
            r#"{"routes": [{"id": "r1", "from": "direct://test", "steps": [{"remove_header": {"key": "CamelHttpPath"}}]}]}"#,
        )
        .expect("remove_header fixture must parse as JSON");
    let errors: Vec<_> = validator.iter_errors(&doc).collect();
    assert!(
        errors.is_empty(),
        "a remove_header step must validate cleanly; got: {:?}",
        errors
    );
}

#[test]
fn rschema_embedded_no_default_flagged() {
    // Boot-parity for EMBEDDED no-default tokens: `svc-${env:EMBED_UNDEF}`
    // is not a whole-scalar token, but the boot tree-walk still
    // Unresolved-fails on it — the mirror walk's embedded scan must flag
    // it with exactly one explicit Error naming the variable, anchored on
    // the token inside the scalar. The `$${env:...}` escape stays silent.
    let source = "\
id: svc-${env:EMBED_UNDEF}
from: $${env:ESCAPED_FROM}
steps:
  - to: log:out
";
    let diags = analyze(source);
    let rschema = rschema_only(&diags);
    let errors: Vec<_> = rschema
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one Error for the embedded no-default token; got: {errors:?}"
    );
    assert!(
        errors[0].message.contains("EMBED_UNDEF"),
        "Error must name the unresolved variable; got: {}",
        errors[0].message
    );
    assert!(
        slice(source, &errors[0].span).contains("${env:EMBED_UNDEF}"),
        "Error must anchor on the embedded token; sliced: {:?}",
        slice(source, &errors[0].span)
    );
    assert!(
        rschema.iter().all(|d| !d.message.contains("ESCAPED_FROM")),
        "escaped $${{env:...}} must produce no diagnostics; got: {:?}",
        rschema
            .iter()
            .map(|d| (d.severity, d.message.as_str()))
            .collect::<Vec<_>>()
    );
}
