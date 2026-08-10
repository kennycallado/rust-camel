//! LintEngine — stateless analysis engine.
//!
//! Holds a component metadata catalog and a list of rules. Runs all rules
//! over a parsed `Document` and returns the concatenated diagnostics.

use std::sync::Arc;

use camel_api::component_metadata::ComponentMetadataCatalog;

use crate::diagnostic::Diagnostic;
use crate::document::Document;
use crate::rule::Rule;

/// The lint analysis engine.
pub struct LintEngine {
    catalog: Arc<dyn ComponentMetadataCatalog>,
    rules: Vec<Box<dyn Rule>>,
}

impl LintEngine {
    /// Create a new engine with an empty rule set.
    pub fn new(catalog: Arc<dyn ComponentMetadataCatalog>) -> Self {
        Self {
            catalog,
            rules: Vec::new(),
        }
    }

    /// Add a rule to the engine (builder pattern).
    pub fn with_rule(mut self, rule: Box<dyn Rule>) -> Self {
        self.rules.push(rule);
        self
    }

    /// Register the default rule set.
    ///
    /// Returns `self` with all implemented rules registered.
    /// Rules are registered in Tasks 2.1–2.5 as they land.
    pub fn with_default_rules(self) -> Self {
        self.with_rule(Box::new(crate::rules::rsyn::RSynRule))
            .with_rule(Box::new(crate::rules::rschema::RSchemaRule))
            .with_rule(Box::new(crate::rules::ruriknown::RUriKnownRule))
            .with_rule(Box::new(crate::rules::rsecret::RSecretRule))
            .with_rule(Box::new(crate::rules::rdeprecated::RDeprecatedRule))
    }

    /// Run all rules over `source` and return the concatenated diagnostics.
    ///
    /// Returns `Vec<Diagnostic>` directly — parse failures are NOT engine
    /// errors; they flow through `Document.parse_failure` to the R-SYN rule
    /// (Task 2.1).
    pub fn lint(&self, source: &str) -> Vec<Diagnostic> {
        let doc = Document::parse(source);
        let mut diagnostics = Vec::new();

        for rule in &self.rules {
            diagnostics.extend(rule.analyze(&doc, &*self.catalog));
        }

        diagnostics
    }

    /// Number of registered rules (test-only visibility).
    #[cfg(test)]
    pub(crate) fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::StubCatalog;
    use camel_api::component_metadata::{ComponentMetadata, OptionKind, UriOption};
    use std::sync::Arc;

    #[test]
    fn all_five_rules_registered() {
        let catalog = Arc::new(StubCatalog::empty());
        let engine = LintEngine::new(catalog).with_default_rules();
        assert_eq!(engine.rule_count(), 5);
    }

    #[test]
    fn all_five_rules_silent_on_valid_doc() {
        // Stub catalog with timer/log/direct metadata; clean fixture using
        // only valid options. With all five rules registered, lint must
        // return zero diagnostics.
        let catalog = StubCatalog::empty()
            .with("direct", ComponentMetadata::minimal("direct"))
            .with("log", ComponentMetadata::minimal("log"))
            .with(
                "timer",
                ComponentMetadata::minimal("timer").with_uri_options(vec![UriOption::new(
                    "period",
                    "period",
                    OptionKind::Duration,
                )]),
            );
        let catalog: Arc<dyn ComponentMetadataCatalog + 'static> = Arc::new(catalog);

        let engine = LintEngine::new(catalog).with_default_rules();
        let source = "id: r1\nfrom: timer:foo?period=1s\nsteps:\n  - to: log:bar\n";
        let diags = engine.lint(source);

        assert!(
            diags.is_empty(),
            "expected no diagnostics for a valid doc; got: {:?}",
            diags
        );
    }
}
