//! R-URI-known rule — scheme + option validation against the catalog.
//!
//! For each endpoint URI, splits the scheme (text before the first `:`) and
//! consults [`ComponentMetadataCatalog`]:
//!
//! - **Absent scheme** → one informational `UnverifiedScheme` note on the
//!   scheme token; no option diagnostics (the catalog cannot verify options
//!   for a scheme it does not know).
//! - **Cross-source duplicate key** (an option key in both the URI query
//!   string and a `parameters:` map, or in config and step `parameters:`
//!   maps) → one `DuplicateKey` error on the redundant occurrence, before
//!   any catalog lookup — the collision fails lowering regardless of scheme
//!   knowledge.
//! - **Known-but-minimal scheme** (registered, no `uri_options`) → silent.
//! - **Known scheme with `uri_options`** → each provided option is resolved
//!   against `name`/`aliases`: an unresolved key yields an `UnknownOption`
//!   error on the key; a resolved `Bool` option given a non-boolean value
//!   yields a `KindMismatch` error on the value; any catalog `UriOption`
//!   declared `required` that is absent yields a `MissingRequiredOption`
//!   error on the URI.
//!
//! v1 validates `OptionKind::Bool` only; other kinds (String/Int/Float/
//! Duration/Enum/List) are deferred — see bd follow-up. The `#[non_exhaustive]`
//! attribute additionally requires `matches!` (not an exhaustive match) so
//! future kinds stay non-erroring.

use camel_api::component_metadata::{ComponentMetadataCatalog, OptionKind};

use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity, Span, UriKnownSubCode};
use crate::document::Document;
use crate::route_view::{Endpoint, OptionOrigin};
use crate::rule::Rule;

/// R-URI-known: validates endpoint schemes and options against the catalog.
pub struct RUriKnownRule;

impl Rule for RUriKnownRule {
    fn analyze(&self, doc: &Document, catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic> {
        // R-URI-known cannot run on a document that failed to parse — R-SYN
        // owns that case.
        if doc.parse_failure.is_some() {
            return Vec::new();
        }

        let mut diagnostics = Vec::new();
        for ep in doc.route_view.endpoints() {
            analyze_endpoint(&ep, catalog, &mut diagnostics);
        }
        diagnostics
    }

    fn code(&self) -> DiagnosticCode {
        DiagnosticCode::RUriKnown(UriKnownSubCode::UnverifiedScheme)
    }
}

/// Flag option keys declared in more than one source origin for `ep`.
///
/// One error per colliding key, on the span of the FIRST non-Query
/// occurrence: options arrive ordered query-first, then step-level
/// parameters, then config parameters, so that span is the step-level key
/// when both parameter sides collide and the parameter key for a
/// query/parameters overlap — the side the lowering's duplicate-key errors
/// name. Raw keys only; alias resolution plays no part. Repeated keys
/// within the raw query alone are legal (the lowering preserves them).
fn flag_cross_source_duplicates(ep: &Endpoint, diagnostics: &mut Vec<Diagnostic>) {
    struct Seen {
        query: bool,
        step: bool,
        config: bool,
        first_non_query: Option<Span>,
    }
    let mut by_key: std::collections::BTreeMap<&str, Seen> = std::collections::BTreeMap::new();
    for opt in &ep.options {
        let seen = by_key.entry(opt.key.value.as_str()).or_insert(Seen {
            query: false,
            step: false,
            config: false,
            first_non_query: None,
        });
        match opt.origin {
            OptionOrigin::Query => seen.query = true,
            OptionOrigin::StepParameters => {
                seen.step = true;
                if seen.first_non_query.is_none() {
                    seen.first_non_query = Some(opt.key.span.clone());
                }
            }
            OptionOrigin::ConfigParameters => {
                seen.config = true;
                if seen.first_non_query.is_none() {
                    seen.first_non_query = Some(opt.key.span.clone());
                }
            }
        }
    }
    for (key, seen) in by_key {
        // Fixed source vocabulary for the message, in lowering-check order.
        let mut sources: Vec<&str> = Vec::new();
        if seen.query {
            sources.push("the URI query string");
        }
        if seen.step {
            sources.push("step parameters");
        }
        if seen.config {
            sources.push("config parameters");
        }
        if sources.len() < 2 {
            continue;
        }
        // ≥2 sources always include a parameters-side occurrence, which
        // recorded its key span above; kept total rather than `expect` to
        // honor lint-unwrap on non-test code.
        let Some(span) = seen.first_non_query else {
            continue;
        };
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::RUriKnown(UriKnownSubCode::DuplicateKey),
            severity: Severity::Error,
            span,
            message: format!(
                "duplicate option key `{key}`: declared in {}",
                sources.join(" and ")
            ),
            fix: None,
        });
    }
}

/// Analyze a single endpoint against the catalog, appending diagnostics.
fn analyze_endpoint(
    ep: &Endpoint,
    catalog: &dyn ComponentMetadataCatalog,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // Cross-source duplicate keys FIRST: the collision fails lowering
    // regardless of catalog knowledge or a parseable scheme, so this pass
    // runs before the scheme split and the catalog early-returns. It mirrors
    // the lowering's two fail-closed paths (`EndpointUriError::DuplicateKey`
    // for query/parameters overlap; `combine_params` for config/step
    // parameters overlap). Raw keys only — alias resolution plays no part,
    // and repeated keys within the raw query alone stay legal (the lowering
    // preserves them in order).
    flag_cross_source_duplicates(ep, diagnostics);

    let uri = &ep.uri.value;
    // Split the scheme: text before the first `:`. A URI without a colon has
    // no parseable scheme; skip it (structural issues are R-SCHEMA's domain).
    let Some(colon) = uri.find(':') else {
        return;
    };
    let scheme = &uri[..colon];
    let scheme_span = Span::new(ep.uri.span.start, ep.uri.span.start + scheme.len());

    let Some(meta) = catalog.get_metadata(scheme) else {
        // Absent scheme: one informational note on the scheme token, and NO
        // option diagnostics for this endpoint.
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::RUriKnown(UriKnownSubCode::UnverifiedScheme),
            severity: Severity::Info,
            span: scheme_span,
            message: "scheme not registered in catalog; cannot verify options".to_string(),
            fix: None,
        });
        return;
    };

    // Known-but-minimal scheme: nothing to validate.
    if meta.uri_options.is_empty() {
        return;
    }

    // Validate each provided option against the catalog's uri_options.
    for opt in &ep.options {
        let Some(canon) = crate::route_view::resolve_option(opt, &meta.uri_options) else {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::RUriKnown(UriKnownSubCode::UnknownOption),
                severity: Severity::Error,
                span: opt.key.span.clone(),
                message: format!("unknown option `{}` for scheme `{}`", opt.key.value, scheme),
                fix: None,
            });
            continue;
        };
        // v1 validates `OptionKind::Bool` only; other kinds (String/Int/Float/
        // Duration/Enum/List) are deferred — see bd follow-up.
        if matches!(canon.kind, OptionKind::Bool)
            && let Some(val) = &opt.value
        {
            let v = val.value.to_ascii_lowercase();
            if v != "true" && v != "false" {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch),
                    severity: Severity::Error,
                    span: val.span.clone(),
                    message: format!(
                        "option `{}` expects a boolean value (true/false)",
                        canon.name
                    ),
                    fix: None,
                });
            }
        }
    }

    // Report required catalog options that are absent from the endpoint.
    for canon in &meta.uri_options {
        if canon.required
            && !crate::route_view::option_present(&canon.name, &canon.aliases, &ep.options)
        {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::RUriKnown(UriKnownSubCode::MissingRequiredOption),
                severity: Severity::Error,
                span: ep.uri.span.clone(),
                message: format!("missing required option `{}`", canon.name),
                fix: None,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
                        UriOption::new("period", "period", OptionKind::Duration)
                            .with_alias("interval"),
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
}
