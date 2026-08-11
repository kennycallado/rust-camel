//! LintEngine — stateless analysis engine.
//!
//! Holds a component metadata catalog and a list of rules. Runs all rules
//! over a parsed `Document` and returns the concatenated diagnostics.

use std::sync::Arc;

use camel_api::component_metadata::ComponentMetadataCatalog;

use crate::completion::CompletionItem;
use crate::diagnostic::Diagnostic;
use crate::document::Document;
use crate::hover::HoverInfo;
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

    /// Return completion items for the cursor position in `doc`.
    ///
    /// Finds the endpoint whose URI span contains `offset`, classifies the
    /// cursor context (scheme or option-key position), and returns matching
    /// completions from the catalog.
    pub fn complete_at(&self, doc: &Document, offset: usize) -> Vec<CompletionItem> {
        if offset > doc.raw.len() {
            return vec![];
        }

        let endpoints = doc.route_view.endpoints();
        let Some(ep) = endpoints
            .iter()
            .find(|e| e.uri.span.start <= offset && offset <= e.uri.span.end)
        else {
            return vec![];
        };

        let uri_str = &doc.raw[ep.uri.span.start..ep.uri.span.end];
        let rel = offset - ep.uri.span.start;
        let colon = uri_str.find(':');

        // Scheme position: offset at or before the first ':' (or no ':').
        let is_scheme = match colon {
            Some(c) => rel <= c,
            None => true,
        };

        if is_scheme {
            return self
                .catalog
                .schemes()
                .into_iter()
                .map(|s| CompletionItem {
                    label: s,
                    detail: None,
                })
                .collect();
        }

        // Option-key position: after '?' or '&', before '=' (or no '=' yet).
        if let Some(qs) = uri_str.find('?')
            && rel > qs
        {
            let abs_qs = qs + 1;
            let rel_in_query = rel.saturating_sub(abs_qs);
            let query = &uri_str[abs_qs..];

            // Scan &-delimited segments to find the one containing cursor.
            let mut seg_start = 0usize;
            while seg_start < query.len() {
                let seg_end = query[seg_start..]
                    .find('&')
                    .map_or(query.len(), |p| seg_start + p);
                let seg = &query[seg_start..seg_end];

                if rel_in_query >= seg_start && rel_in_query <= seg_end {
                    let eq_pos = seg.find('=');
                    let is_key_pos = match eq_pos {
                        Some(eq) => rel_in_query < seg_start + eq,
                        None => true,
                    };
                    if is_key_pos
                        && !seg.is_empty()
                        && let Some(col) = colon
                    {
                        let scheme = &uri_str[..col];
                        if let Some(meta) = self.catalog.get_metadata(scheme)
                            && !meta.uri_options.is_empty()
                        {
                            let mut items = Vec::new();
                            for opt in &meta.uri_options {
                                let detail = if opt.description.is_empty() {
                                    None
                                } else {
                                    Some(opt.description.clone())
                                };
                                items.push(CompletionItem {
                                    label: opt.name.clone(),
                                    detail: detail.clone(),
                                });
                                for alias in &opt.aliases {
                                    items.push(CompletionItem {
                                        label: alias.clone(),
                                        detail: detail.clone(),
                                    });
                                }
                            }
                            return items;
                        }
                    }
                    break;
                }
                seg_start = seg_end + 1;
            }
        }

        vec![]
    }

    /// Return hover info for the option key at `offset` in `doc`.
    ///
    /// Finds the endpoint whose URI span contains `offset`, extracts the
    /// option key at the cursor position, and resolves its metadata from
    /// the catalog. Returns `None` when the cursor is outside an option-key
    /// position or the option is unknown.
    pub fn hover_at(&self, doc: &Document, offset: usize) -> Option<HoverInfo> {
        if offset > doc.raw.len() {
            return None;
        }

        let endpoints = doc.route_view.endpoints();
        let ep = endpoints
            .iter()
            .find(|e| e.uri.span.start <= offset && offset <= e.uri.span.end)?;

        let uri_str = &doc.raw[ep.uri.span.start..ep.uri.span.end];
        let rel = offset - ep.uri.span.start;

        // Must be in query portion (after '?').
        let qs = uri_str.find('?')?;
        if rel <= qs {
            return None;
        }

        let abs_qs = qs + 1;
        let rel_in_query = rel.saturating_sub(abs_qs);
        let query = &uri_str[abs_qs..];

        // Scan &-delimited segments to find the one containing cursor.
        let mut seg_start = 0usize;
        while seg_start < query.len() {
            let seg_end = query[seg_start..]
                .find('&')
                .map_or(query.len(), |p| seg_start + p);
            let seg = &query[seg_start..seg_end];

            if rel_in_query >= seg_start && rel_in_query <= seg_end {
                let eq_pos = seg.find('=');
                let is_key_pos = match eq_pos {
                    Some(eq) => rel_in_query < seg_start + eq,
                    None => true,
                };
                if is_key_pos && !seg.is_empty() {
                    let key_text = match eq_pos {
                        Some(eq) => &seg[..eq],
                        None => seg,
                    };
                    let colon = uri_str.find(':')?;
                    let scheme = &uri_str[..colon];
                    let meta = self.catalog.get_metadata(scheme)?;
                    let opt = meta.uri_options.iter().find(|uo| {
                        uo.name == key_text || uo.aliases.iter().any(|a| a == key_text)
                    })?;
                    return Some(HoverInfo {
                        description: if opt.description.is_empty() {
                            None
                        } else {
                            Some(opt.description.clone())
                        },
                        deprecated: opt.deprecated.clone(),
                        secret: opt.secret,
                    });
                }
                break;
            }
            seg_start = seg_end + 1;
        }

        None
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

    // ---- complete_at tests ----

    #[test]
    fn complete_at_scheme_position() {
        // "from: tim" — no colon, any offset within "tim" is scheme position.
        // Byte layout: "from: " = 0..6, "tim" = 6..9.
        let catalog = StubCatalog::empty()
            .with("timer", ComponentMetadata::minimal("timer"))
            .with("log", ComponentMetadata::minimal("log"))
            .with("direct", ComponentMetadata::minimal("direct"));
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: tim";
        let doc = Document::parse(source);
        // offset 7 = inside 'i' of "tim"
        let items = engine.complete_at(&doc, 7);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"timer"),
            "expected 'timer' in scheme completions; got: {labels:?}"
        );
        assert!(
            labels.contains(&"log"),
            "expected 'log' in scheme completions; got: {labels:?}"
        );
        assert!(
            labels.contains(&"direct"),
            "expected 'direct' in scheme completions; got: {labels:?}"
        );
        assert_eq!(labels.len(), 3);
    }

    #[test]
    fn complete_at_option_key_position() {
        // "from: timer:tick?per" — cursor inside "per" (option key).
        // Byte layout: "from: " = 0..6, "timer:tick?per" = 6..20.
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![UriOption::new(
                "period",
                "Interval between ticks",
                OptionKind::Duration,
            )]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:tick?per";
        let doc = Document::parse(source);
        // offset 18 = inside 'e' of "per"
        let items = engine.complete_at(&doc, 18);
        assert_eq!(items.len(), 1, "expected one option completion");
        assert_eq!(items[0].label, "period");
        assert_eq!(items[0].detail, Some("Interval between ticks".to_string()));
    }

    #[test]
    fn complete_at_minimal_scheme_returns_empty() {
        // "from: redis:cache?op" — cursor in option-key position but redis
        // has no uri_options.
        let catalog = StubCatalog::empty().with("redis", ComponentMetadata::minimal("redis"));
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: redis:cache?op";
        let doc = Document::parse(source);
        // "from: " = 0..6, "redis:cache?op" = 6..20, '?' at rel 11 (byte 17).
        // offset 18 = inside 'o' of "op", after '?'.
        let items = engine.complete_at(&doc, 18);
        assert!(
            items.is_empty(),
            "minimal scheme should return empty vec; got: {items:?}"
        );
    }

    #[test]
    fn complete_at_outside_uri_returns_empty() {
        // Cursor on YAML key "id" outside any endpoint URI span.
        let catalog = StubCatalog::empty().with("timer", ComponentMetadata::minimal("timer"));
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "id: r1\nfrom: timer:foo\n";
        let doc = Document::parse(source);
        // offset 1 = inside 'd' of "id", which is NOT inside any URI span.
        let items = engine.complete_at(&doc, 1);
        assert!(
            items.is_empty(),
            "cursor outside URI span should return empty; got: {items:?}"
        );
    }

    #[test]
    fn complete_at_offset_beyond_source_returns_empty() {
        let catalog = StubCatalog::empty().with("timer", ComponentMetadata::minimal("timer"));
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:foo\n"; // 16 bytes
        let doc = Document::parse(source);
        let items = engine.complete_at(&doc, 50);
        assert!(
            items.is_empty(),
            "offset beyond source should return empty; got: {items:?}"
        );
    }

    // ---- hover_at tests ----

    #[test]
    fn hover_at_documented_option_returns_description() {
        // "from: timer:tick?period=1s"
        // 0         1         2
        // 012345678901234567890123456
        // period starts at byte 18
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![UriOption::new(
                "period",
                "Tick interval",
                OptionKind::Duration,
            )]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:tick?period=1s";
        let doc = Document::parse(source);
        // offset 19 = inside 'e' of "period"
        let info = engine.hover_at(&doc, 19);
        assert_eq!(
            info,
            Some(HoverInfo {
                description: Some("Tick interval".to_string()),
                deprecated: None,
                secret: false,
            })
        );
    }

    #[test]
    fn hover_at_deprecated_option_returns_reason() {
        // "from: timer:tick?oldFreq=1s"
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![
                UriOption::new("oldFreq", "Old frequency", OptionKind::Duration)
                    .deprecated("use period instead"),
            ]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:tick?oldFreq=1s";
        let doc = Document::parse(source);
        // offset 19 = inside 'd' of "oldFreq"
        let info = engine.hover_at(&doc, 19);
        assert_eq!(
            info,
            Some(HoverInfo {
                description: Some("Old frequency".to_string()),
                deprecated: Some("use period instead".to_string()),
                secret: false,
            })
        );
    }

    #[test]
    fn hover_at_secret_option_returns_flag() {
        // "from: timer:tick?password=s3cret"
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![
                UriOption::new("password", "Auth password", OptionKind::String)
                    .with_alias("pwd")
                    .secret(),
            ]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:tick?password=s3cret";
        let doc = Document::parse(source);
        // offset 21 = inside 'w' of "password"
        let info = engine.hover_at(&doc, 21);
        assert_eq!(
            info,
            Some(HoverInfo {
                description: Some("Auth password".into()),
                deprecated: None,
                secret: true,
            })
        );
    }

    #[test]
    fn hover_at_outside_option_returns_none() {
        // Cursor in scheme position ("timer") — no hover info.
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![UriOption::new(
                "period",
                "Tick interval",
                OptionKind::Duration,
            )]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:tick?period=1s";
        let doc = Document::parse(source);
        // offset 7 = inside 'i' of "timer" (scheme position)
        let info = engine.hover_at(&doc, 7);
        assert_eq!(info, None, "scheme position should return None");
    }
}
