//! R-URI-known rule — scheme + option validation against the catalog.
//!
//! For each endpoint URI, splits the scheme (text before the first `:`) and
//! consults [`ComponentMetadataCatalog`]:
//!
//! - **Absent scheme** → one informational `UnverifiedScheme` note on the
//!   scheme token; no option diagnostics (the catalog cannot verify options
//!   for a scheme it does not know).
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
use crate::route_view::Endpoint;
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

/// Analyze a single endpoint against the catalog, appending diagnostics.
fn analyze_endpoint(
    ep: &Endpoint,
    catalog: &dyn ComponentMetadataCatalog,
    diagnostics: &mut Vec<Diagnostic>,
) {
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
}
