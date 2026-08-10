//! R-SECRET rule — flags secret options set to literal values.
//!
//! For each option whose catalog [`UriOption::secret`] is `true`, checks
//! whether the provided value is a reference: a value containing `${` (env
//! interpolation) or `{{` (placeholder interpolation) is acceptable; a
//! literal value emits a warning.
//!
//! R-SECRET does NOT emit for absent secret options (that is
//! R-URI-known's `MissingRequiredOption` when the option is also `required`).

use camel_api::component_metadata::ComponentMetadataCatalog;

use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity};
use crate::document::Document;
use crate::rule::Rule;

/// R-SECRET: warns when a secret option is set to a literal value.
pub struct RSecretRule;

impl Rule for RSecretRule {
    fn analyze(&self, doc: &Document, catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic> {
        if doc.parse_failure.is_some() {
            return Vec::new();
        }

        let mut diagnostics = Vec::new();
        let endpoints = doc.route_view.endpoints();
        for (ep, meta) in crate::route_view::known_endpoints(&endpoints, catalog) {
            for opt in &ep.options {
                let Some(canon) = crate::route_view::resolve_option(opt, &meta.uri_options) else {
                    // Unknown option — not R-SECRET's domain.
                    continue;
                };

                if !canon.secret {
                    continue;
                }

                let Some(value) = &opt.value else {
                    // No value present — nothing to check for literal secret.
                    continue;
                };

                // A reference contains at least one interpolation marker.
                let is_reference = value.value.contains("${") || value.value.contains("{{");
                if is_reference {
                    continue;
                }

                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::RSecret,
                    severity: Severity::Warning,
                    span: value.span.clone(),
                    message: format!(
                        "secret option `{}` set to a literal value; use an interpolation reference",
                        canon.name
                    ),
                    fix: None,
                });
            }
        }
        diagnostics
    }

    fn code(&self) -> DiagnosticCode {
        DiagnosticCode::RSecret
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
    use camel_api::component_metadata::UriOption;
    use camel_api::component_metadata::{ComponentCapabilities, ComponentMetadata, OptionKind};

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
        RSecretRule.analyze(&doc, catalog)
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
    fn literal_secret_warned() {
        // `password` is a secret option; the source sets `password=hunter2`
        // literally → one RSecret Warning on the `hunter2` value span.
        let catalog = StubCatalog::empty()
            .with("direct", ComponentMetadata::minimal("direct"))
            .with(
                "db",
                meta_with_options(
                    "db",
                    vec![UriOption::new("password", "password", OptionKind::String).secret()],
                ),
            );
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: db:conn?password=hunter2\n";
        let diags = analyze(source, &catalog);

        assert_eq!(
            diags.len(),
            1,
            "expected exactly one RSecret diagnostic; got: {:?}",
            diags
        );
        let d = &diags[0];
        assert_eq!(d.code, DiagnosticCode::RSecret);
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(slice(source, &d.span), "hunter2");
        assert!(
            d.message.contains("password"),
            "message should name the secret option; got: {}",
            d.message
        );
    }

    #[test]
    fn interpolated_secret_silent() {
        // `password` is secret; the source sets it via placeholder
        // interpolation → no RSecret diagnostic.
        let catalog = StubCatalog::empty()
            .with("direct", ComponentMetadata::minimal("direct"))
            .with(
                "db",
                meta_with_options(
                    "db",
                    vec![UriOption::new("password", "password", OptionKind::String).secret()],
                ),
            );
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: db:conn?password={{ secrets.db.password }}\n";
        let diags = analyze(source, &catalog);
        assert!(
            diags.is_empty(),
            "expected no RSecret diagnostic; got: {:?}",
            diags
        );
    }
}
