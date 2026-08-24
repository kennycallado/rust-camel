//! R-MOCK-IN-PRODUCTION rule — warns when a `mock:` endpoint is used as a
//! send target in a production route.
//!
//! `mock:` endpoints are test doubles. Sending to one inline in a production
//! route is a mistake; intercepts (`skipTo` / `divertCopyTo`) belong in a
//! `*.test.yaml` instead. The rule fires on endpoints whose origin key is
//! `to` or `endpoints` (the send positions) and whose URI scheme is `mock:`.

use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity};
use crate::document::Document;
use crate::rule::Rule;
use camel_api::component_metadata::ComponentMetadataCatalog;

/// The migration guidance attached to every R-MOCK-IN-PRODUCTION diagnostic.
const MIGRATION_MSG: &str = "inline mock: send in production route; declare intercepts (skipTo/divertCopyTo) in a *.test.yaml instead - see the testing guide (docs/src/testing)";

/// R-MOCK-IN-PRODUCTION: flags inline `mock:` sends in production routes.
pub struct RMockRule;

impl Rule for RMockRule {
    fn analyze(&self, doc: &Document, _catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic> {
        if doc.parse_failure.is_some() {
            return Vec::new();
        }

        let mut diagnostics = Vec::new();
        for ep in doc.route_view.endpoints() {
            let is_send = ep.key.value == "to" || ep.key.value == "endpoints";
            if is_send && ep.uri.value.starts_with("mock:") {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::RMock,
                    severity: Severity::Warning,
                    span: ep.uri.span.clone(),
                    message: MIGRATION_MSG.to_string(),
                    fix: None,
                });
            }
        }
        diagnostics
    }

    fn code(&self) -> DiagnosticCode {
        DiagnosticCode::RMock
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
    use std::sync::Arc;

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
        RMockRule.analyze(&doc, &StubCatalog::empty())
    }

    fn rmock_diags(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
        diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::RMock)
            .collect()
    }

    #[test]
    fn to_mock_warns_once_with_migration_message() {
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: mock:out\n";
        let diags = analyze(source);
        let rmock = rmock_diags(&diags);
        assert_eq!(
            rmock.len(),
            1,
            "expected exactly one R-MOCK-IN-PRODUCTION; got: {diags:?}"
        );
        let d = rmock[0];
        assert_eq!(d.code, DiagnosticCode::RMock);
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(slice(source, &d.span), "mock:out");
        assert!(
            d.message.contains("intercepts"),
            "message must mention intercepts; got: {}",
            d.message
        );
        assert!(
            d.message.contains("skipTo"),
            "message must mention skipTo; got: {}",
            d.message
        );
    }

    #[test]
    fn endpoints_recipient_list_warns_per_occurrence() {
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - scatter_gather:\n      endpoints:\n        - mock:a\n        - mock:b\n";
        let diags = analyze(source);
        let rmock = rmock_diags(&diags);
        assert_eq!(
            rmock.len(),
            2,
            "expected two R-MOCK-IN-PRODUCTION (one per endpoint); got: {diags:?}"
        );
        assert_ne!(rmock[0].span, rmock[1].span, "spans must be distinct");
        assert_eq!(slice(source, &rmock[0].span), "mock:a");
        assert_eq!(slice(source, &rmock[1].span), "mock:b");
    }

    #[test]
    fn mock_with_query_params_warns() {
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: mock:out?count=2\n";
        let diags = analyze(source);
        let rmock = rmock_diags(&diags);
        assert_eq!(
            rmock.len(),
            1,
            "expected one R-MOCK-IN-PRODUCTION; got: {diags:?}"
        );
        assert_eq!(slice(source, &rmock[0].span), "mock:out?count=2");
    }

    #[test]
    fn two_to_mock_steps_warn_twice() {
        let source =
            "id: r1\nfrom: direct:start\nsteps:\n  - to: mock:first\n  - to: mock:second\n";
        let diags = analyze(source);
        let rmock = rmock_diags(&diags);
        assert_eq!(
            rmock.len(),
            2,
            "expected two R-MOCK-IN-PRODUCTION; got: {diags:?}"
        );
        assert_ne!(rmock[0].span, rmock[1].span, "spans must be distinct");
        assert_eq!(slice(source, &rmock[0].span), "mock:first");
        assert_eq!(slice(source, &rmock[1].span), "mock:second");
    }

    #[test]
    fn non_mock_send_silent() {
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: \"kafka:orders\"\n";
        let diags = analyze(source);
        let rmock = rmock_diags(&diags);
        assert!(
            rmock.is_empty(),
            "non-mock send must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn non_interceptable_origins_silent() {
        // wire_tap, enrich (shorthand + object form), poll_enrich, a route-2+
        // `from`, and dead_letter_channel are NOT send positions — none may
        // trigger R-MOCK-IN-PRODUCTION even though every URI is `mock:`.
        let source = "\
routes:
  - id: a
    from: direct:start
    steps:
      - wire_tap: \"mock:tap\"
      - enrich: \"mock:enr\"
      - poll_enrich: \"mock:poll\"
      - enrich:
          uri: \"mock:uri\"
    error_handler:
      dead_letter_channel: \"mock:dlq\"
  - id: b
    from: \"mock:src\"
    steps:
      - to: \"direct:ok\"
";
        let diags = analyze(source);
        let rmock = rmock_diags(&diags);
        assert!(
            rmock.is_empty(),
            "non-send origins must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn parse_failure_skips_rule() {
        use crate::diagnostic::DiagnosticCode;
        use crate::engine::LintEngine;

        let source = "steps:\n  - to: timer:foo\n  bad: [";
        let engine = LintEngine::new(Arc::new(StubCatalog::empty())).with_default_rules();
        let diags = engine.lint(source);

        assert_eq!(diags.len(), 1, "exactly one diagnostic expected");
        assert_eq!(diags[0].code, DiagnosticCode::RSyn);
        assert!(
            rmock_diags(&diags).is_empty(),
            "R-MOCK-IN-PRODUCTION must skip a parse-failed document"
        );
    }
}
