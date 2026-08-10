//! R-SYN rule — reports syntax errors with byte-exact spans.
//!
//! Reads `Document::parse_failure` and produces a single diagnostic when the
//! parser encountered a syntax error. The parser's message is carried verbatim.

use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity};
use crate::document::Document;
use crate::rule::Rule;
use camel_api::component_metadata::ComponentMetadataCatalog;

/// Reports a single `DiagnosticCode::RSyn` diagnostic when
/// `Document::parse_failure` is set.
pub struct RSynRule;

impl Rule for RSynRule {
    fn analyze(&self, doc: &Document, _catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic> {
        match &doc.parse_failure {
            Some(failure) => vec![Diagnostic {
                code: DiagnosticCode::RSyn,
                severity: Severity::Error,
                span: failure.span.clone(),
                message: failure.message.clone(),
                fix: None,
            }],
            None => vec![],
        }
    }

    fn code(&self) -> DiagnosticCode {
        DiagnosticCode::RSyn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::StubCatalog;
    use std::sync::Arc;

    #[test]
    fn rsyn_reports_at_parser_error_location() {
        let source = "steps:\n  - to: timer:foo\n  bad: [";
        let doc = Document::parse(source);
        assert!(
            doc.parse_failure.is_some(),
            "malformed input must set parse_failure"
        );

        let stub = StubCatalog::empty();
        let diags = RSynRule.analyze(&doc, &stub);

        assert_eq!(diags.len(), 1, "exactly one diagnostic expected");
        let d = &diags[0];
        assert_eq!(d.code, DiagnosticCode::RSyn);
        assert_eq!(d.severity, Severity::Error);
        assert!(!d.message.is_empty(), "message must be non-empty");
        // noyalib reports this syntax error at byte 27 (the `b` of `bad:`) — the
        // start of the offending construct. ErrorKind::Syntax has no sub-kind to
        // identify the unclosed `[` specifically, so the span is the parser's
        // reported location. Pinned: if noyalib changes its error location, this
        // fails loudly and we update the pin.
        assert_eq!(d.span.start, 27, "span start = noyalib parser location");
        assert_eq!(d.span.end, 28, "single-byte span");
        assert_eq!(&source[d.span.start..d.span.end], "b");
    }

    #[test]
    fn rsyn_silent_on_valid_doc() {
        let source = "from: direct:start\nsteps:\n  - to: log:out\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");

        let stub = StubCatalog::empty();
        let diags = RSynRule.analyze(&doc, &stub);

        assert!(diags.is_empty(), "valid doc must produce no diagnostics");
    }

    #[test]
    fn rsyn_emits_and_others_skip_on_broken_doc() {
        use crate::engine::LintEngine;

        let source = "steps:\n  - to: timer:foo\n  bad: [";
        let engine = LintEngine::new(Arc::new(StubCatalog::empty())).with_default_rules();
        let diags = engine.lint(source);

        assert_eq!(diags.len(), 1, "exactly one diagnostic expected");
        assert_eq!(diags[0].code, DiagnosticCode::RSyn);
        // All 4 non-RSYN rules (R-SCHEMA, R-URI-known, R-SECRET,
        // R-DEPRECATED) skip a parse-failed document. Asserting
        // `len()==1` with `code==RSyn` here confirms they all do.
        assert!(
            diags
                .iter()
                .all(|d| !matches!(d.code, DiagnosticCode::RSchema)),
            "R-SCHEMA must skip a parse-failed document"
        );
    }
}
