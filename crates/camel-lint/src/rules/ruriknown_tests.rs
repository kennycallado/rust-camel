// Extracted from ruriknown.rs to keep the module under 1k lines.
// Wired via `#[cfg(test)] #[path = "ruriknown_tests.rs"] mod tests;` at the
// bottom of ruriknown.rs (same pattern as camel-api metrics_tests.rs).
// NOTE: string-literal continuation lines (`\`-ended) are content — their
// indentation is YAML, not Rust; never re-indent them.

use super::*;
use crate::diagnostic::Span;
use crate::route_view::{LintOption, OptionOrigin, Spanned, resolve_option};
use crate::test_support::StubCatalog;
use camel_api::component_metadata::{
    ComponentCapabilities, ComponentMetadata, OptionKind, UriOption,
};

/// Build a catalog entry for `scheme` with the given uri_options.
fn meta_with_options(scheme: &str, opts: Vec<UriOption>) -> ComponentMetadata {
    ComponentMetadata {
        scheme: scheme.to_string(),
        capabilities: ComponentCapabilities::default(),
        uri_options: opts,
        ..ComponentMetadata::minimal(scheme)
    }
}

fn analyze(source: &str, catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic> {
    let doc = Document::parse(source);
    assert!(
        doc.parse_failure.is_none(),
        "test fixtures must parse cleanly (got: {:?})",
        doc.parse_failure
    );
    RUriKnownRule.analyze(&doc, catalog)
}

fn slice<'a>(raw: &'a str, span: &Span) -> &'a str {
    &raw[span.start..span.end]
}

/// Keep only R-URI-known diagnostics.
fn ruriknown_only(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags
        .iter()
        .filter(|d| matches!(d.code, DiagnosticCode::RUriKnown(_)))
        .collect()
}

fn count_subcode(diags: &[Diagnostic], sub: UriKnownSubCode) -> usize {
    diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::RUriKnown(sub.clone()))
        .count()
}

#[test]
fn unverified_scheme_for_absent_metadata() {
    // `kafka` is absent from the catalog. The route-level `from` is the
    // single endpoint, so exactly one UnverifiedScheme note is emitted on
    // the `kafka` token, and zero option diagnostics.
    let source = "id: r1\nfrom: kafka:topic\n";
    let diags = analyze(source, &StubCatalog::empty());
    let kept = ruriknown_only(&diags);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::UnverifiedScheme),
        1,
        "expected exactly one UnverifiedScheme; got: {:?}",
        kept.iter()
            .map(|d| (&d.code, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    let note = kept
        .iter()
        .find(|d| {
            matches!(
                d.code,
                DiagnosticCode::RUriKnown(UriKnownSubCode::UnverifiedScheme)
            )
        })
        .expect("UnverifiedScheme diagnostic present");
    assert_eq!(slice(source, &note.span), "kafka");
    assert_eq!(note.severity, Severity::Info);
    // No option diagnostics for an unverified scheme.
    assert_eq!(count_subcode(&diags, UriKnownSubCode::UnknownOption), 0);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::MissingRequiredOption),
        0
    );
}

#[test]
fn unverified_scheme_span_exact_for_quoted_uri() {
    // Regression (rc-bsx4t): the scheme span used to start at the YAML
    // opening quote (`"jm` for from: "jms:queue"). It must slice the
    // exact scheme token for quoted scalars.
    let source = "id: r1\nfrom: \"kafka:topic\"\n";
    let diags = analyze(source, &StubCatalog::empty());
    let note = diags
        .iter()
        .find(|d| {
            matches!(
                d.code,
                DiagnosticCode::RUriKnown(UriKnownSubCode::UnverifiedScheme)
            )
        })
        .expect("UnverifiedScheme diagnostic present");
    assert_eq!(slice(source, &note.span), "kafka");
}

#[test]
fn unknown_option_span_exact_for_quoted_uri() {
    // Query-option spans derive from `uri.span.start` too: on a quoted
    // URI the key span must slice the exact key token.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("period", "period", OptionKind::Duration)],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: \"timer:foo?frequency=1s\"\n";
    let diags = analyze(source, &catalog);
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::UnknownOption))
        .expect("UnknownOption diagnostic present");
    assert_eq!(slice(source, &d.span), "frequency");
}

#[test]
fn minimal_known_scheme_is_silent() {
    // `redis` is registered with minimal metadata (no uri_options): known
    // but nothing to validate → no UnverifiedScheme, no option diagnostics.
    let catalog = StubCatalog::empty().with("redis", ComponentMetadata::minimal("redis"));
    let source = "id: r1\nfrom: redis://x\n";
    let diags = analyze(source, &catalog);
    assert!(
        ruriknown_only(&diags).is_empty(),
        "minimal known scheme must be silent; got: {:?}",
        diags
    );
}

#[test]
fn unknown_option_for_known_scheme() {
    // `timer` lists only `period` (no `frequency` alias). `frequency` is
    // unknown → one UnknownOption Error on the `frequency` key span.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("period", "period", OptionKind::Duration)],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?frequency=1s\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::UnknownOption),
        1,
        "expected one UnknownOption; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::UnknownOption))
        .unwrap();
    assert_eq!(slice(source, &d.span), "frequency");
    assert_eq!(d.severity, Severity::Error);
}

#[test]
fn unknown_option_in_rest_operation_to() {
    // A rest operation's `to:` is an endpoint URI (rc-p86s): ROUTE_SCHEMA
    // models `rest`/`operations` as containers (CONTAINER_KEYS derives
    // from the embedded schema), so the CST walk reaches the operation's
    // `to:` and R-URI-known validates its options like any step `to:`.
    let catalog = StubCatalog::empty().with(
        "timer",
        meta_with_options(
            "timer",
            vec![UriOption::new("period", "period", OptionKind::Duration)],
        ),
    );
    let source = "\
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api
    operations:
      - method: GET
        to: timer:foo?frequency=1s
";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::UnknownOption),
        1,
        "expected one UnknownOption on the rest `to:` URI; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::UnknownOption))
        .unwrap();
    assert_eq!(slice(source, &d.span), "frequency");
    assert_eq!(d.severity, Severity::Error);
}

#[test]
fn mcp_resource_uri_not_validated_as_endpoint() {
    // The mirror-image of the rest `to:` pin (rc-6pikg): the mcp block
    // authors no endpoint URIs — its consumer from-URIs are fabricated
    // by the lowering at parse time. `resources[].uri` is an MCP
    // resource URI (operator config, arbitrary scheme — `crm://...`),
    // and `uri`'s URI_KEYS membership exists for EnrichConfig.uri. The
    // walk skips the mcp subtree, so R-URI-known must stay silent on it
    // against ANY catalog (an empty one maximally would flag `crm` as
    // an unknown component scheme).
    let catalog = StubCatalog::empty();
    let source = "\
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
    tools:
      - name: lookup
        input_schema:
          type: object
    resources:
      - name: customers
        uri: crm://customers
";
    let diags = analyze(source, &catalog);
    assert!(
        ruriknown_only(&diags).is_empty(),
        "mcp resource uri must not be validated as an endpoint URI; got: {:?}",
        ruriknown_only(&diags)
    );
}

#[test]
fn missing_required_option() {
    // `timer` declares `period` as required; the step omits it → one
    // MissingRequiredOption Error on the URI span.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("period", "period", OptionKind::Duration).required()],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::MissingRequiredOption),
        1,
        "expected one MissingRequiredOption; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::MissingRequiredOption))
        .unwrap();
    assert_eq!(slice(source, &d.span), "timer:foo");
    assert_eq!(d.severity, Severity::Error);
}

#[test]
fn accepted_alias_silent() {
    // `period` has alias `interval`; providing `interval=1s` matches and the
    // Duration kind is non-erroring → no diagnostic for that option.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![
                    UriOption::new("period", "period", OptionKind::Duration).with_alias("interval"),
                ],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?interval=1s\n";
    let diags = analyze(source, &catalog);
    assert!(
        ruriknown_only(&diags).is_empty(),
        "an accepted alias must be silent; got: {:?}",
        diags
    );
}

#[test]
fn kind_mismatch_reported() {
    // `enabled` is a Bool option; `maybe` is not boolean → one
    // KindMismatch Error on the value span.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("enabled", "enabled", OptionKind::Bool)],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?enabled=maybe\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "expected one KindMismatch; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
        .unwrap();
    assert_eq!(slice(source, &d.span), "maybe");
    assert_eq!(d.severity, Severity::Error);
}

#[test]
fn bool_kind_runtime_vocab_silent() {
    // `parse_bool_param` (camel-endpoint) accepts 1/0/yes/no in any
    // case; the lint must accept the same vocabulary or it false-
    // positives on values the runtime takes.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("enabled", "enabled", OptionKind::Bool)],
            ),
        );
    for value in ["1", "0", "yes", "no", "YES", "True", "FALSE"] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?enabled={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Bool value `{value}` is runtime-legal; got: {:?}",
            ruriknown_only(&diags)
        );
    }
}

#[test]
fn int_kind_mismatch_reported() {
    // `repeatCount` is an Int option; `many` does not parse → one
    // KindMismatch Error on the value span, message naming integers.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("repeatCount", "repeats", OptionKind::Int)],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?repeatCount=many\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "expected one KindMismatch; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
        .unwrap();
    assert_eq!(slice(source, &d.span), "many");
    assert_eq!(d.severity, Severity::Error);
    assert!(
        d.message.contains("integer"),
        "message should name the expected kind; got: {}",
        d.message
    );
}

#[test]
fn int_kind_valid_values_silent() {
    // OptionKind::Int does not carry signedness or width; every value
    // the i64 or u64 FromStr grammars accept (including negatives and
    // values beyond i64 range) must stay silent.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("repeatCount", "repeats", OptionKind::Int)],
            ),
        );
    for value in ["25", "0", "-3", "18446744073709551615"] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?repeatCount={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Int value `{value}` must be silent; got: {:?}",
            ruriknown_only(&diags)
        );
    }
}

#[test]
fn float_kind_mismatch_reported() {
    // `ratio` is a Float option; `fast` does not parse → one
    // KindMismatch Error on the value span.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("ratio", "backoff ratio", OptionKind::Float)],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?ratio=fast\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "expected one KindMismatch; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
        .unwrap();
    assert_eq!(slice(source, &d.span), "fast");
    assert!(
        d.message.contains("floating-point"),
        "message should name the expected kind; got: {}",
        d.message
    );
}

#[test]
fn float_kind_valid_values_silent() {
    // Rust's f64 FromStr grammar: decimals, exponent forms, bare
    // integers, inf/NaN — the runtime parses with the same grammar.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("ratio", "backoff ratio", OptionKind::Float)],
            ),
        );
    for value in ["1.5", "2", "1e3", "-0.25"] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?ratio={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Float value `{value}` must be silent; got: {:?}",
            ruriknown_only(&diags)
        );
    }
}

#[test]
fn duration_kind_mismatch_reported() {
    // `period` is a Duration option; `fast` is not a duration → one
    // KindMismatch Error on the value span, message naming durations.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new(
                    "period",
                    "tick interval",
                    OptionKind::Duration,
                )],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?period=fast\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "expected one KindMismatch; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
        .unwrap();
    assert_eq!(slice(source, &d.span), "fast");
    assert!(
        d.message.contains("duration"),
        "message should name the expected kind; got: {}",
        d.message
    );
}

#[test]
fn duration_kind_rejects_bare_integer() {
    // humantime requires a unit suffix: a bare `2000` is NOT a
    // Duration value. URI options that count milliseconds declare
    // OptionKind::Int (the UriConfig `_ms` companion convention), so
    // bare integers must mismatch the Duration kind.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new(
                    "period",
                    "tick interval",
                    OptionKind::Duration,
                )],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?period=2000\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "bare integer is not a humantime duration; got: {:?}",
        ruriknown_only(&diags)
    );
}

#[test]
fn duration_kind_valid_values_silent() {
    // humantime grammar: single units and concatenated groups.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new(
                    "period",
                    "tick interval",
                    OptionKind::Duration,
                )],
            ),
        );
    for value in ["1s", "500ms", "2h30m"] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?period={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Duration value `{value}` must be silent; got: {:?}",
            ruriknown_only(&diags)
        );
    }
}

#[test]
fn enum_kind_mismatch_reported() {
    // `format` is an Enum option (json/csv); `xml` is not in the
    // allowed set → one KindMismatch Error on the value span, message
    // listing the allowed values.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new(
                    "format",
                    "output format",
                    OptionKind::Enum(vec!["json".to_string(), "csv".to_string()]),
                )],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?format=xml\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "expected one KindMismatch; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
        .unwrap();
    assert_eq!(slice(source, &d.span), "xml");
    assert!(
        d.message.contains("json") && d.message.contains("csv"),
        "message should list the allowed values; got: {}",
        d.message
    );
}

#[test]
fn enum_kind_case_insensitive_match_silent() {
    // Enum membership compares ASCII-case-insensitively: known FromStr
    // impls are case-lenient (camel-log's level accepts any case), so
    // exact-case comparison would false-positive on `INFO` vs `info`.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new(
                    "format",
                    "output format",
                    OptionKind::Enum(vec!["json".to_string(), "csv".to_string()]),
                )],
            ),
        );
    for value in ["JSON", "Json", "csv"] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?format={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Enum value `{value}` must be silent (case-insensitive); got: {:?}",
            ruriknown_only(&diags)
        );
    }
}

#[test]
fn enum_kind_underscore_normalization_silent() {
    // seda's FromStr impls lowercase AND strip underscores
    // (`if_reply_expected` → IfReplyExpected, `in_only` → InOnly);
    // membership must apply the same normalization or runtime-legal
    // values false-positive.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new(
                    "waitForTaskToComplete",
                    "when to wait",
                    OptionKind::Enum(vec![
                        "Never".to_string(),
                        "IfReplyExpected".to_string(),
                        "Always".to_string(),
                    ]),
                )],
            ),
        );
    for value in ["if_reply_expected", "IF_REPLY_EXPECTED", "ifreplyexpected"] {
        let source = format!(
            "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?waitForTaskToComplete={value}\n"
        );
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Enum value `{value}` must be silent (underscore normalization); got: {:?}",
            ruriknown_only(&diags)
        );
    }
    // A value outside the set under BOTH normalizations still flags.
    let source =
        "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?waitForTaskToComplete=sometimes\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "unlisted value must mismatch; got: {:?}",
        ruriknown_only(&diags)
    );
}

#[test]
fn list_kind_mismatch_reported() {
    // `ids` is a List(Int) option; element `foo` does not parse → one
    // KindMismatch Error on the whole value span, message naming the
    // element kind.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new(
                    "ids",
                    "allowed ids",
                    OptionKind::List(Box::new(OptionKind::Int)),
                )],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?ids=1,foo,3\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "expected one KindMismatch; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
        .unwrap();
    assert_eq!(slice(source, &d.span), "1,foo,3");
    assert!(
        d.message.contains("integer"),
        "message should name the element kind; got: {}",
        d.message
    );
}

#[test]
fn list_kind_valid_values_silent() {
    // Comma-separated integers and the empty list (bare `ids=`) are
    // valid List(Int) values.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new(
                    "ids",
                    "allowed ids",
                    OptionKind::List(Box::new(OptionKind::Int)),
                )],
            ),
        );
    for value in ["1,2,3", "42", ""] {
        let source = format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?ids={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "List value `{value}` must be silent; got: {:?}",
            ruriknown_only(&diags)
        );
    }
}

#[test]
fn string_kind_never_mismatches() {
    // String accepts any text; even garbage-looking values stay silent.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("fileName", "file name", OptionKind::String)],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?fileName=%2fnot-a-number\n";
    let diags = analyze(source, &catalog);
    assert!(
        ruriknown_only(&diags).is_empty(),
        "String kind never mismatches; got: {:?}",
        ruriknown_only(&diags)
    );
}

#[test]
fn interpolated_value_skips_kind_validation() {
    // Interpolation the lint cannot resolve stays exempt: a no-default
    // `${env:...}` token resolves to the (unknowable) process
    // environment at boot, `${arg:...}` tokens and `{{...}}` placeholders
    // resolve outside the lint's knowledge, so kind validation skips
    // them (mirrors R-SECRET's reference treatment). A whole-scalar
    // `${env:X:-d}` token is NOT in this list — its default is
    // validated (see the env-default tests below).
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![
                    UriOption::new("repeatCount", "repeats", OptionKind::Int),
                    UriOption::new(
                        "format",
                        "output format",
                        OptionKind::Enum(vec!["json".to_string()]),
                    ),
                ],
            ),
        );
    for query in [
        "repeatCount=${env:RETRIES}",
        "repeatCount=${arg:RETRIES}",
        "format={{ config.format }}",
    ] {
        let source = format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?{query}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "interpolated value `{query}` must skip kind validation; got: {:?}",
            ruriknown_only(&diags)
        );
    }
}

// -----------------------------------------------------------------------
// Whole-scalar env-default kind validation (rc-w4otz) tests
// -----------------------------------------------------------------------

#[test]
fn env_default_int_kind_mismatch_reported() {
    // `repeatCount` and `period` are Int options; each whole-scalar env
    // token's default fails the i64/u64 parse (`many` is not a number;
    // `1s` is the bd's motivating defect — timer's real `period` is u64
    // MILLIS, so the `1s` default fails at boot). The default is the
    // concrete boot-time fallback → one KindMismatch Error on the WHOLE
    // value span, message naming integers.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![
                    UriOption::new("repeatCount", "repeats", OptionKind::Int),
                    // timer's real `period` is u64 millis (UriConfig
                    // `period_ms` companion convention) — Int kind.
                    UriOption::new("period", "tick interval millis", OptionKind::Int),
                ],
            ),
        );
    for (query, value) in [
        ("repeatCount=${env:RETRIES:-many}", "${env:RETRIES:-many}"),
        ("period=${env:POLL:-1s}", "${env:POLL:-1s}"),
    ] {
        let source = format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?{query}\n");
        let diags = analyze(&source, &catalog);
        assert_eq!(
            count_subcode(&diags, UriKnownSubCode::KindMismatch),
            1,
            "expected one KindMismatch for `{query}`; got: {:?}",
            ruriknown_only(&diags)
        );
        let d = diags
            .iter()
            .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
            .unwrap();
        assert_eq!(slice(&source, &d.span), value);
        assert_eq!(d.severity, Severity::Error);
        assert!(
            d.message.contains("integer"),
            "message should name the expected kind; got: {}",
            d.message
        );
    }
}

#[test]
fn env_default_int_kind_valid_defaults_silent() {
    // Defaults the i64/u64 grammars accept are silent — the
    // unset-variable boot path parses them fine. In `${env:RETRIES:--3}`
    // the first `-` after `:` is the separator, so the default is `-3`.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("repeatCount", "repeats", OptionKind::Int)],
            ),
        );
    for value in [
        "${env:RETRIES:-25}",
        "${env:RETRIES:--3}",
        "${env:RETRIES:-18446744073709551615}",
    ] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?repeatCount={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Int env default `{value}` must be silent; got: {:?}",
            ruriknown_only(&diags)
        );
    }
}

#[test]
fn env_default_bool_kind_valid_and_invalid() {
    // Bool env defaults use the runtime vocabulary (`parse_bool_param`:
    // true/false/1/0/yes/no, any case); `maybe` is reported on the whole
    // token span with the boolean expectation.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("enabled", "enabled", OptionKind::Bool)],
            ),
        );
    for value in ["${env:FLAG:-true}", "${env:FLAG:-1}", "${env:FLAG:-YES}"] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?enabled={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Bool env default `{value}` is runtime-legal; got: {:?}",
            ruriknown_only(&diags)
        );
    }
    let source =
        "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?enabled=${env:FLAG:-maybe}\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "expected one KindMismatch; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
        .unwrap();
    assert_eq!(slice(source, &d.span), "${env:FLAG:-maybe}");
    assert_eq!(d.severity, Severity::Error);
    assert!(
        d.message.contains("boolean"),
        "message should name the expected kind; got: {}",
        d.message
    );
}

#[test]
fn env_default_float_kind_valid_and_invalid() {
    // `${env:RATIO:-1.5}` parses as f64 → silent; the `fast` default is
    // reported naming floating-point.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("ratio", "ratio", OptionKind::Float)],
            ),
        );
    for value in ["${env:RATIO:-1.5}", "${env:RATIO:--0.25}"] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?ratio={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Float env default `{value}` must be silent; got: {:?}",
            ruriknown_only(&diags)
        );
    }
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?ratio=${env:RATIO:-fast}\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "expected one KindMismatch; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
        .unwrap();
    assert_eq!(slice(source, &d.span), "${env:RATIO:-fast}");
    assert!(
        d.message.contains("floating-point"),
        "message should name the expected kind; got: {}",
        d.message
    );
}

#[test]
fn env_default_duration_kind_valid_and_invalid() {
    // humantime grammar: `500ms`/`2h30m` defaults are silent; `soon` and
    // the bare-integer `100` (an Int-kind value, mirroring
    // `duration_kind_rejects_bare_integer`) are reported naming
    // durations.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("delay", "delay", OptionKind::Duration)],
            ),
        );
    for value in ["${env:POLL:-500ms}", "${env:POLL:-2h30m}"] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?delay={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Duration env default `{value}` must be silent; got: {:?}",
            ruriknown_only(&diags)
        );
    }
    for value in ["${env:POLL:-soon}", "${env:POLL:-100}"] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?delay={value}\n");
        let diags = analyze(&source, &catalog);
        assert_eq!(
            count_subcode(&diags, UriKnownSubCode::KindMismatch),
            1,
            "expected one KindMismatch for `{value}`; got: {:?}",
            ruriknown_only(&diags)
        );
        let d = diags
            .iter()
            .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
            .unwrap();
        assert_eq!(slice(&source, &d.span), value);
        assert!(
            d.message.contains("duration"),
            "message should name the expected kind; got: {}",
            d.message
        );
    }
}

#[test]
fn env_default_enum_kind_valid_and_invalid() {
    // Enum membership applies to the default under the same case-fold
    // normalization as literals (`XML` matches `xml`); `yaml` is outside
    // the allowed set → reported with the allowed-value list.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new(
                    "format",
                    "output format",
                    OptionKind::Enum(vec!["json".to_string(), "xml".to_string()]),
                )],
            ),
        );
    for value in ["${env:FMT:-json}", "${env:FMT:-XML}"] {
        let source =
            format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?format={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "Enum env default `{value}` must be silent; got: {:?}",
            ruriknown_only(&diags)
        );
    }
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?format=${env:FMT:-yaml}\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "expected one KindMismatch; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
        .unwrap();
    assert_eq!(slice(source, &d.span), "${env:FMT:-yaml}");
    assert!(
        d.message.contains("one of"),
        "message should list the allowed values; got: {}",
        d.message
    );
}

#[test]
fn env_default_list_kind_valid_and_invalid() {
    // Each comma-separated ELEMENT of the default is validated against
    // the element kind; the empty default (`${env:IDS:-}`) is the empty
    // list — silent. `1,b` carries a non-integer element → reported.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new(
                    "ids",
                    "allowed ids",
                    OptionKind::List(Box::new(OptionKind::Int)),
                )],
            ),
        );
    for value in ["${env:IDS:-1,2,3}", "${env:IDS:-42}", "${env:IDS:-}"] {
        let source = format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?ids={value}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "List env default `{value}` must be silent; got: {:?}",
            ruriknown_only(&diags)
        );
    }
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?ids=${env:IDS:-1,b}\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::KindMismatch),
        1,
        "expected one KindMismatch; got: {:?}",
        ruriknown_only(&diags)
    );
    let d = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch))
        .unwrap();
    assert_eq!(slice(source, &d.span), "${env:IDS:-1,b}");
    assert!(
        d.message.contains("comma-separated list"),
        "message should name the list convention; got: {}",
        d.message
    );
}

#[test]
fn env_default_string_kind_any_default_silent() {
    // String accepts any text; even a garbage-looking default on a
    // whole-scalar token stays silent (the carve-out cannot
    // false-positive on String options).
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("fileName", "file name", OptionKind::String)],
            ),
        );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?fileName=${env:NAME:-%2fdev%2fnull}\n";
    let diags = analyze(source, &catalog);
    assert!(
        ruriknown_only(&diags).is_empty(),
        "String kind never mismatches; got: {:?}",
        ruriknown_only(&diags)
    );
}

#[test]
fn env_default_no_default_token_stays_exempt() {
    // `${env:RETRIES}` has no default: the boot value is the process
    // environment, unknowable at lint time — exempt even for an Int
    // option, mirroring R-SECRET's reference treatment.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![UriOption::new("repeatCount", "repeats", OptionKind::Int)],
            ),
        );
    let source =
        "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?repeatCount=${env:RETRIES}\n";
    let diags = analyze(source, &catalog);
    assert!(
        ruriknown_only(&diags).is_empty(),
        "no-default token must stay exempt; got: {:?}",
        ruriknown_only(&diags)
    );
}

#[test]
fn env_default_non_whole_scalar_tokens_stay_exempt() {
    // The carve-out is whole-scalar ONLY: kind-invalid defaults inside
    // tokens that are not one whole-scalar env token stay exempt —
    // mid-string embedding, token concatenation, `$${...}` escape,
    // `${arg:...}` (env-only scope), and `{{...}}` placeholders all
    // resolve outside lint knowledge.
    let catalog = StubCatalog::empty()
        .with("direct", ComponentMetadata::minimal("direct"))
        .with(
            "timer",
            meta_with_options(
                "timer",
                vec![
                    UriOption::new("repeatCount", "repeats", OptionKind::Int),
                    UriOption::new("fileName", "file name", OptionKind::String),
                ],
            ),
        );
    for query in [
        "repeatCount=${env:A:-many}/b",
        "repeatCount=pre-${env:A:-many}",
        "repeatCount=${env:A:-many}${env:B:-many}",
        "repeatCount=$${env:A:-many}",
        "repeatCount=${arg:RETRIES:-many}",
        "repeatCount={{ cfg.count }}",
        "fileName=${env:NAME:-garbage}",
    ] {
        let source = format!("id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?{query}\n");
        let diags = analyze(&source, &catalog);
        assert!(
            ruriknown_only(&diags).is_empty(),
            "non-whole-scalar `{query}` must stay exempt; got: {:?}",
            ruriknown_only(&diags)
        );
    }
}

// -----------------------------------------------------------------------
// Pattern prefix resolution tests (open-namespace URI options)
// -----------------------------------------------------------------------

/// Helper: build a `LintOption` with a bare key (no value) at a dummy span.
/// Origin defaults to `Query`; tests that need another origin build the
/// literal directly.
fn lint_option(key: &str) -> LintOption {
    LintOption {
        key: Spanned {
            value: key.to_string(),
            span: Span::new(0, key.len()),
        },
        value: None,
        origin: OptionOrigin::Query,
    }
}

#[test]
fn pattern_prefix_resolves_non_empty_suffix() {
    let uri_options =
        vec![UriOption::new("param", "namespace", OptionKind::String).pattern_prefix("param.")];
    let opt = lint_option("param.foo");
    let result = resolve_option(&opt, &uri_options);
    assert!(
        result.is_some(),
        "param.foo should match pattern_prefix(\"param.\")"
    );
    assert_eq!(result.unwrap().name, "param");
}

#[test]
fn pattern_prefix_rejects_empty_suffix() {
    let uri_options =
        vec![UriOption::new("param", "namespace", OptionKind::String).pattern_prefix("param.")];
    let opt = lint_option("param.");
    let result = resolve_option(&opt, &uri_options);
    assert!(
        result.is_none(),
        "param. should NOT match pattern_prefix(\"param.\") — empty suffix"
    );
}

#[test]
fn pattern_prefix_rejects_unrelated_key() {
    let uri_options =
        vec![UriOption::new("param", "namespace", OptionKind::String).pattern_prefix("param.")];
    let opt = lint_option("direction");
    let result = resolve_option(&opt, &uri_options);
    assert!(
        result.is_none(),
        "direction should NOT match pattern_prefix(\"param.\")"
    );
}

#[test]
fn discrete_option_wins_over_pattern_on_name_collision() {
    let uri_options = vec![
        UriOption::new("param.foo", "discrete", OptionKind::String),
        UriOption::new("param", "namespace", OptionKind::String).pattern_prefix("param."),
    ];
    let opt = lint_option("param.foo");
    let result = resolve_option(&opt, &uri_options);
    assert!(
        result.is_some(),
        "param.foo should resolve to the discrete option"
    );
    let hit = result.unwrap();
    assert_eq!(hit.name, "param.foo");
    assert!(
        hit.pattern.is_none(),
        "should be the discrete option, not the patterned one"
    );
}

#[test]
fn discrete_option_wins_when_pattern_derived_name_collides() {
    // Both options have name == "param"; one has pattern, one doesn't.
    // The pattern option's derived name should NOT participate in Phase-1 matching.
    let uri_options = vec![
        UriOption::new("param", "discrete", OptionKind::String),
        UriOption::new("param", "namespace", OptionKind::String).pattern_prefix("param."),
    ];
    let opt = lint_option("param");
    let result = resolve_option(&opt, &uri_options);
    assert!(
        result.is_some(),
        "param (no suffix) should resolve to the discrete option"
    );
    let hit = result.unwrap();
    assert_eq!(hit.description, "discrete");
    assert!(
        hit.pattern.is_none(),
        "should be the discrete option, not the patterned one"
    );
}

#[test]
fn longest_pattern_separator_wins() {
    let uri_options = vec![
        UriOption::new("param", "short", OptionKind::String).pattern_prefix("param."),
        UriOption::new("param.foo", "long", OptionKind::String).pattern_prefix("param.foo."),
    ];
    let opt = lint_option("param.foo.bar");
    let result = resolve_option(&opt, &uri_options);
    assert!(
        result.is_some(),
        "param.foo.bar should match (longer separator wins)"
    );
    let hit = result.unwrap();
    assert_eq!(hit.description, "long");
}

#[test]
fn shorter_pattern_wins_when_longer_does_not_match() {
    let uri_options = vec![
        UriOption::new("param", "short", OptionKind::String).pattern_prefix("param."),
        UriOption::new("param.foo", "long", OptionKind::String).pattern_prefix("param.foo."),
    ];
    let opt = lint_option("param.baz");
    let result = resolve_option(&opt, &uri_options);
    assert!(
        result.is_some(),
        "param.baz should match the short pattern_prefix(\"param.\")"
    );
    let hit = result.unwrap();
    assert_eq!(hit.description, "short");
}

#[test]
fn alias_match_skipped_for_pattern_options() {
    let uri_options = vec![
        UriOption::new("param", "namespace", OptionKind::String)
            .with_alias("legacy")
            .pattern_prefix("param."),
    ];
    let opt = lint_option("legacy");
    let result = resolve_option(&opt, &uri_options);
    assert!(
        result.is_none(),
        "legacy alias should NOT match a patterned option — aliases do not participate in Phase 1 for pattern options"
    );
}

#[test]
fn pattern_option_covers_multiple_distinct_suffixes() {
    let uri_options =
        vec![UriOption::new("param", "namespace", OptionKind::String).pattern_prefix("param.")];

    let result_a = resolve_option(&lint_option("param.a"), &uri_options);
    let result_b = resolve_option(&lint_option("param.b"), &uri_options);
    let result_long = resolve_option(&lint_option("param.longName"), &uri_options);

    let hit_a = result_a.expect("param.a should resolve");
    let hit_b = result_b.expect("param.b should resolve");
    let hit_long = result_long.expect("param.longName should resolve");

    // All three distinct suffixes resolve to the SAME option.
    assert!(std::ptr::eq(hit_a, hit_b));
    assert!(std::ptr::eq(hit_a, hit_long));
}

// -----------------------------------------------------------------------
// Cross-source duplicate key tests (Task 2.1)
// -----------------------------------------------------------------------

#[test]
fn duplicate_key_display_string() {
    assert_eq!(
        DiagnosticCode::RUriKnown(UriKnownSubCode::DuplicateKey).to_string(),
        "R-URI-known:duplicate-key"
    );
}

#[test]
fn query_and_step_parameters_overlap_flagged() {
    // Per the spec scenario: the catalog KNOWS `timer` with option
    // `period` (non-Bool kind → silent), so the duplicate fires while
    // the per-occurrence validation loop is active (coexistence path).
    let catalog = StubCatalog::empty().with(
        "timer",
        meta_with_options(
            "timer",
            vec![UriOption::new(
                "period",
                "tick interval",
                OptionKind::String,
            )],
        ),
    );
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?period=1000\n    parameters:\n      period: \"2500\"\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::DuplicateKey),
        1,
        "expected exactly one DuplicateKey; got: {:?}",
        ruriknown_only(&diags)
            .iter()
            .map(|d| (&d.code, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    let dup = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::DuplicateKey))
        .unwrap();
    // Span lands on the `period` key inside the parameters map — the
    // SECOND `period` occurrence in the source.
    assert_eq!(slice(source, &dup.span), "period");
    assert_eq!(
        dup.span.start,
        source.rfind("period").expect("parameters-side key present")
    );
    assert_eq!(dup.severity, Severity::Error);
}

#[test]
fn config_and_step_parameters_overlap_flagged() {
    let catalog = StubCatalog::empty().with("db", meta_with_options("db", Vec::new()));
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - enrich:\n      uri: db:query\n      parameters:\n        timeout: \"1\"\n    parameters:\n      timeout: \"2\"\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::DuplicateKey),
        1,
        "expected exactly one DuplicateKey; got: {:?}",
        ruriknown_only(&diags)
            .iter()
            .map(|d| (&d.code, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    let dup = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::DuplicateKey))
        .unwrap();
    // Span on the step-level `timeout` key — the second occurrence.
    assert_eq!(slice(source, &dup.span), "timeout");
    assert_eq!(
        dup.span.start,
        source.rfind("timeout").expect("step-level key present")
    );
}

#[test]
fn repeated_query_keys_not_flagged() {
    let catalog = StubCatalog::empty().with("timer", meta_with_options("timer", Vec::new()));
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?period=1&period=2\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::DuplicateKey),
        0,
        "repeated keys within the raw query alone are legal; got: {:?}",
        ruriknown_only(&diags)
    );
}

#[test]
fn overlap_flagged_for_unregistered_scheme() {
    // `kafka` absent from the catalog: the duplicate check still fires,
    // alongside the informational unverified-scheme note. `direct` IS
    // registered (known-but-minimal → silent) so the only unverified
    // note is kafka's.
    let catalog = StubCatalog::empty().with("direct", meta_with_options("direct", Vec::new()));
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: kafka:orders?brokers=h1\n    parameters:\n      brokers: \"h2\"\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::DuplicateKey),
        1,
        "duplicate fires regardless of catalog knowledge; got: {:?}",
        ruriknown_only(&diags)
            .iter()
            .map(|d| (&d.code, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    let dup = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::DuplicateKey))
        .unwrap();
    assert_eq!(slice(source, &dup.span), "brokers");
    assert_eq!(
        dup.span.start,
        source
            .rfind("brokers")
            .expect("parameters-side key present")
    );
    assert_eq!(count_subcode(&diags, UriKnownSubCode::UnverifiedScheme), 1);
}

#[test]
fn route_level_from_overlap_flagged() {
    let catalog = StubCatalog::empty().with("timer", meta_with_options("timer", Vec::new()));
    let source = "id: r1\nfrom: timer:tick?period=1s\nparameters:\n  period: \"2500\"\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::DuplicateKey),
        1,
        "route-level from overlap must be flagged; got: {:?}",
        ruriknown_only(&diags)
            .iter()
            .map(|d| (&d.code, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    let dup = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::DuplicateKey))
        .unwrap();
    assert_eq!(slice(source, &dup.span), "period");
    assert_eq!(
        dup.span.start,
        source.rfind("period").expect("route-level key present")
    );
}

#[test]
fn all_three_sources_single_diagnostic() {
    let catalog = StubCatalog::empty().with("timer", meta_with_options("timer", Vec::new()));
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to:\n      uri: timer:foo?period=1s\n      parameters:\n        period: \"2\"\n    parameters:\n      period: \"3\"\n";
    let diags = analyze(source, &catalog);
    assert_eq!(
        count_subcode(&diags, UriKnownSubCode::DuplicateKey),
        1,
        "one diagnostic per colliding key per endpoint, even across three sources; got: {:?}",
        ruriknown_only(&diags)
            .iter()
            .map(|d| (&d.code, slice(source, &d.span)))
            .collect::<Vec<_>>()
    );
    // Pin the span side: options arrive [query, step-inherited,
    // config-local], so the first non-Query occurrence is the STEP-level
    // key (value "3") — the last `period` in the source. A future reorder
    // of the walk's `inherited ++ local` chain turns this red.
    let dup = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::DuplicateKey))
        .unwrap();
    assert_eq!(slice(source, &dup.span), "period");
    assert_eq!(
        dup.span.start,
        source.rfind("period").expect("step-level key present")
    );
}
