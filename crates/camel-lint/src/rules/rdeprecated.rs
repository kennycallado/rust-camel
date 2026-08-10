//! R-DEPRECATED rule — warns when a deprecated URI option is used.
//!
//! For each endpoint with a known scheme, resolves each provided option
//! (including aliases) against the catalog's `uri_options`. When the
//! canonical [`UriOption::deprecated`] is `Some(msg)`, emits a warning
//! bearing the deprecation message on the option key span.

use camel_api::component_metadata::ComponentMetadataCatalog;

use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity};
use crate::document::Document;
use crate::rule::Rule;

/// R-DEPRECATED: warns when a deprecated option is used.
pub struct RDeprecatedRule;

impl Rule for RDeprecatedRule {
    fn analyze(&self, doc: &Document, catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic> {
        if doc.parse_failure.is_some() {
            return Vec::new();
        }

        let mut diagnostics = Vec::new();
        let endpoints = doc.route_view.endpoints();
        for (ep, meta) in crate::route_view::known_endpoints(&endpoints, catalog) {
            for opt in &ep.options {
                let Some(canon) = crate::route_view::resolve_option(opt, &meta.uri_options) else {
                    continue;
                };
                if let Some(msg) = &canon.deprecated {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::RDeprecated,
                        severity: Severity::Warning,
                        span: opt.key.span.clone(),
                        message: msg.clone(),
                        fix: None,
                    });
                }
            }
        }
        diagnostics
    }

    fn code(&self) -> DiagnosticCode {
        DiagnosticCode::RDeprecated
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Span;
    use crate::test_support::StubCatalog;
    use camel_api::component_metadata::{
        ComponentCapabilities, ComponentMetadata, OptionKind, UriOption,
    };

    fn slice<'a>(raw: &'a str, span: &Span) -> &'a str {
        &raw[span.start..span.end]
    }

    fn analyze(source: &str, catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic> {
        let doc = Document::parse(source);
        assert!(
            doc.parse_failure.is_none(),
            "test fixtures must parse cleanly (got: {:?})",
            doc.parse_failure
        );
        RDeprecatedRule.analyze(&doc, catalog)
    }

    fn meta_with_options(scheme: &str, opts: Vec<UriOption>) -> ComponentMetadata {
        ComponentMetadata {
            scheme: scheme.to_string(),
            capabilities: ComponentCapabilities::default(),
            uri_options: opts,
            ..ComponentMetadata::minimal(scheme)
        }
    }

    #[test]
    fn deprecated_option_reported_with_message() {
        let catalog = StubCatalog::empty()
            .with("direct", ComponentMetadata::minimal("direct"))
            .with(
                "timer",
                meta_with_options(
                    "timer",
                    vec![
                        UriOption::new("oldFreq", "old frequency", OptionKind::Duration)
                            .deprecated("use `period` instead"),
                    ],
                ),
            );
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?oldFreq=1s\n";
        let diags = analyze(source, &catalog);

        assert_eq!(
            diags.len(),
            1,
            "expected exactly one RDeprecated diagnostic; got: {:?}",
            diags
        );
        let d = &diags[0];
        assert_eq!(d.code, DiagnosticCode::RDeprecated);
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(slice(source, &d.span), "oldFreq");
        assert_eq!(d.message, "use `period` instead");
    }
}
