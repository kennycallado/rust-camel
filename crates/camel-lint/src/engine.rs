//! LintEngine — stateless analysis engine.
//!
//! Holds a component metadata catalog and a list of rules. Runs all rules
//! over a parsed `Document` and returns the concatenated diagnostics.

use std::sync::Arc;

use camel_api::component_metadata::{ComponentMetadataCatalog, OptionKind};

use crate::completion::CompletionItem;
use crate::diagnostic::{Diagnostic, Span};
use crate::document::Document;
use crate::hover::HoverInfo;
use crate::route_view::{Endpoint, LintOption, option_present, resolve_option};
use crate::rule::Rule;

/// Whether `offset` lies within `span` (inclusive at both ends).
fn span_contains(span: &Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

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
            .with_rule(Box::new(crate::rules::rmock::RMockRule))
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
    /// completions from the catalog. When the cursor sits inside a
    /// `parameters:` entry span (which lives outside the URI span), key and
    /// value completions come from the endpoint's catalog metadata instead.
    pub fn complete_at(&self, doc: &Document, offset: usize) -> Vec<CompletionItem> {
        if offset > doc.raw.len() {
            return vec![];
        }

        let endpoints = doc.route_view.endpoints();
        if let Some(ep) = endpoints
            .iter()
            .find(|e| span_contains(&e.uri.span, offset))
        {
            return self.complete_in_uri(doc, ep, offset);
        }

        self.complete_in_parameters(&endpoints, offset)
    }

    /// URI-span completion: scheme position or query-string option key.
    fn complete_in_uri(&self, doc: &Document, ep: &Endpoint, offset: usize) -> Vec<CompletionItem> {
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

    /// `parameters:` entry completion (cursor inside an entry key or value
    /// span, outside the URI span).
    ///
    /// Key positions complete option names and aliases from the catalog,
    /// minus keys already declared in the query string — the DSL lowering
    /// rejects the same key in both places (fail-closed), so suggesting the
    /// overlap again would propose an invalid route. Value positions hint
    /// from the resolved option's kind: enum variants, bool literals, or the
    /// default value.
    fn complete_in_parameters(&self, endpoints: &[Endpoint], offset: usize) -> Vec<CompletionItem> {
        for ep in endpoints {
            // The owning entry is the one whose key/value span contains the
            // cursor and whose spans sit outside the URI span (query-string
            // options live inside it and are handled by complete_in_uri).
            let Some(opt) = ep.options.iter().find(|o| {
                let outside_uri = !(span_contains(&ep.uri.span, o.key.span.start)
                    && span_contains(&ep.uri.span, o.key.span.end));
                outside_uri
                    && (span_contains(&o.key.span, offset)
                        || o.value
                            .as_ref()
                            .is_some_and(|v| span_contains(&v.span, offset)))
            }) else {
                continue;
            };

            let Some(colon) = ep.uri.value.find(':') else {
                return vec![];
            };
            let scheme = &ep.uri.value[..colon];
            let Some(meta) = self.catalog.get_metadata(scheme) else {
                return vec![];
            };
            if meta.uri_options.is_empty() {
                return vec![];
            }

            if span_contains(&opt.key.span, offset) {
                let query_options: Vec<LintOption> = ep
                    .options
                    .iter()
                    .filter(|o| {
                        span_contains(&ep.uri.span, o.key.span.start)
                            && span_contains(&ep.uri.span, o.key.span.end)
                    })
                    .cloned()
                    .collect();
                let mut items = Vec::new();
                for uo in &meta.uri_options {
                    if option_present(&uo.name, &uo.aliases, &query_options) {
                        continue;
                    }
                    let detail = if uo.description.is_empty() {
                        None
                    } else {
                        Some(uo.description.clone())
                    };
                    items.push(CompletionItem {
                        label: uo.name.clone(),
                        detail: detail.clone(),
                    });
                    for alias in &uo.aliases {
                        items.push(CompletionItem {
                            label: alias.clone(),
                            detail: detail.clone(),
                        });
                    }
                }
                return items;
            }

            // Value position: hint from the resolved option's kind.
            let Some(uo) = resolve_option(opt, &meta.uri_options) else {
                return vec![];
            };
            return match &uo.kind {
                OptionKind::Enum(variants) => variants
                    .iter()
                    .map(|v| CompletionItem {
                        label: v.clone(),
                        detail: None,
                    })
                    .collect(),
                OptionKind::Bool => vec![
                    CompletionItem {
                        label: "true".to_string(),
                        detail: None,
                    },
                    CompletionItem {
                        label: "false".to_string(),
                        detail: None,
                    },
                ],
                _ => uo
                    .default_value
                    .as_ref()
                    .map(|d| CompletionItem {
                        label: d.clone(),
                        detail: Some("default value".to_string()),
                    })
                    .into_iter()
                    .collect(),
            };
        }

        vec![]
    }

    /// Return hover info for the option key at `offset` in `doc`.
    ///
    /// Finds the endpoint whose URI span contains `offset`, extracts the
    /// option key at the cursor position, and resolves its metadata from
    /// the catalog. A cursor on a `parameters:` entry key resolves through
    /// the owning endpoint's scheme the same way. Returns `None` when the
    /// cursor is outside an option-key position or the option is unknown.
    pub fn hover_at(&self, doc: &Document, offset: usize) -> Option<HoverInfo> {
        if offset > doc.raw.len() {
            return None;
        }

        let endpoints = doc.route_view.endpoints();
        if let Some(ep) = endpoints
            .iter()
            .find(|e| span_contains(&e.uri.span, offset))
        {
            return self.hover_in_uri_query(doc, ep, offset);
        }

        self.hover_in_parameters(&endpoints, offset)
    }

    /// Query-string hover: option key positions after `?` in the URI.
    fn hover_in_uri_query(
        &self,
        doc: &Document,
        ep: &Endpoint,
        offset: usize,
    ) -> Option<HoverInfo> {
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

    /// `parameters:` entry hover: cursor inside an entry KEY span (value
    /// spans mirror the query-string behavior and return `None`).
    fn hover_in_parameters(&self, endpoints: &[Endpoint], offset: usize) -> Option<HoverInfo> {
        for ep in endpoints {
            let Some(opt) = ep.options.iter().find(|o| {
                let outside_uri = !(span_contains(&ep.uri.span, o.key.span.start)
                    && span_contains(&ep.uri.span, o.key.span.end));
                outside_uri && span_contains(&o.key.span, offset)
            }) else {
                continue;
            };

            let colon = ep.uri.value.find(':')?;
            let scheme = &ep.uri.value[..colon];
            let meta = self.catalog.get_metadata(scheme)?;
            let uo = resolve_option(opt, &meta.uri_options)?;
            return Some(HoverInfo {
                description: if uo.description.is_empty() {
                    None
                } else {
                    Some(uo.description.clone())
                },
                deprecated: uo.deprecated.clone(),
                secret: uo.secret,
            });
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
    fn all_six_rules_registered() {
        let catalog = Arc::new(StubCatalog::empty());
        let engine = LintEngine::new(catalog).with_default_rules();
        assert_eq!(engine.rule_count(), 6);
    }

    #[test]
    fn all_six_rules_silent_on_valid_doc() {
        // Stub catalog with timer/log/direct metadata; clean fixture using
        // only valid options. With all six rules registered, lint must
        // return zero diagnostics.
        //
        // OWNERSHIP: this mock-free fixture is the owner of the MODIFIED
        // "Valid document yields no diagnostics" scenario — it must stay
        // free of `mock:` sends so R-MOCK-IN-PRODUCTION stays silent here.
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

    #[test]
    fn complete_at_parameters_key_position_suggests_options() {
        // Step-level parameters; cursor inside the partial key "per".
        // Byte layout: "  - to: timer:foo" = 33..50, "    parameters:" =
        // 51..66, "      per: 1s" = 67..80 with key "per" at 73..76.
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![
                UriOption::new("period", "Interval between ticks", OptionKind::Duration),
                UriOption::new("delay", "Initial delay", OptionKind::Duration),
            ]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo\n    parameters:\n      per: 1s\n";
        let doc = Document::parse(source);
        // offset 74 = inside 'e' of "per"
        let items = engine.complete_at(&doc, 74);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"period"),
            "expected 'period' in parameters completions; got: {labels:?}"
        );
        assert!(
            labels.contains(&"delay"),
            "expected 'delay' in parameters completions; got: {labels:?}"
        );
        let period = items
            .iter()
            .find(|i| i.label == "period")
            .expect("period item must exist");
        assert_eq!(period.detail, Some("Interval between ticks".to_string()));
    }

    #[test]
    fn complete_at_parameters_key_excludes_query_declared() {
        // `delay` is declared in the query string; the DSL lowering rejects
        // the same key in query string and parameters map, so completion for
        // the parameters key must not suggest `delay` again.
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![
                UriOption::new("period", "Interval between ticks", OptionKind::Duration),
                UriOption::new("delay", "Initial delay", OptionKind::Duration),
            ]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:tick?delay=2s\nparameters:\n  period: 1s\n";
        let doc = Document::parse(source);
        // "  period: 1s" = 38..50, key "period" at 40..46; offset 41 = 'e'.
        let items = engine.complete_at(&doc, 41);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["period"],
            "query-declared `delay` must be excluded; got: {labels:?}"
        );
    }

    #[test]
    fn complete_at_parameters_value_enum_suggests_variants() {
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![UriOption::new(
                "output",
                "Output format",
                OptionKind::Enum(vec![
                    "json".to_string(),
                    "csv".to_string(),
                    "xml".to_string(),
                ]),
            )]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:tick\nparameters:\n  output: json\n";
        let doc = Document::parse(source);
        // value "json" at 39..43; offset 40 = 's'.
        let items = engine.complete_at(&doc, 40);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["json", "csv", "xml"]);
    }

    #[test]
    fn complete_at_parameters_value_bool_suggests_literals() {
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![UriOption::new(
                "daemon",
                "Daemonize",
                OptionKind::Bool,
            )]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:tick\nparameters:\n  daemon: \"true\"\n";
        let doc = Document::parse(source);
        // value `"true"` at 39..45 (quoted string); offset 41 = 'r'.
        let items = engine.complete_at(&doc, 41);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["true", "false"]);
    }

    #[test]
    fn complete_at_parameters_value_default_hint() {
        let mut opt = UriOption::new("retries", "Retry count", OptionKind::Int);
        opt.default_value = Some("3".to_string());
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![opt]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:tick\nparameters:\n  retries: \"4\"\n";
        let doc = Document::parse(source);
        // value `"4"` at 40..42 (quoted string); offset 41 = '4'.
        let items = engine.complete_at(&doc, 41);
        assert_eq!(
            items.len(),
            1,
            "expected single default-value hint; got: {items:?}"
        );
        assert_eq!(items[0].label, "3");
        assert_eq!(items[0].detail, Some("default value".to_string()));
    }

    #[test]
    fn complete_at_parameters_unknown_scheme_returns_empty() {
        // Empty catalog: the `timer` scheme is unknown, so the parameters
        // key position must yield no completions.
        let engine = LintEngine::new(Arc::new(StubCatalog::empty()));

        let source = "from: timer:tick\nparameters:\n  period: 1s\n";
        let doc = Document::parse(source);
        // key "period" at 31..37; offset 32 = 'e'.
        let items = engine.complete_at(&doc, 32);
        assert!(
            items.is_empty(),
            "unknown scheme should return empty; got: {items:?}"
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

    #[test]
    fn hover_at_parameters_key_returns_info() {
        // Route-level parameters entry "password": hover on the key must
        // resolve through the from endpoint's scheme and honor the secret
        // flag, mirroring the query-string hover behavior.
        let catalog = StubCatalog::empty().with(
            "http",
            ComponentMetadata::minimal("http").with_uri_options(vec![
                UriOption::new("password", "Auth password", OptionKind::String).secret(),
            ]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: http:srv\nparameters:\n  password: x\n";
        let doc = Document::parse(source);
        // key "password" at 29..37; offset 30 = 'a'.
        let info = engine.hover_at(&doc, 30);
        assert_eq!(
            info,
            Some(HoverInfo {
                description: Some("Auth password".to_string()),
                deprecated: None,
                secret: true,
            })
        );
    }

    #[test]
    fn hover_at_parameters_value_returns_none() {
        // Hover is key-position only (mirrors query-string hover); a cursor
        // inside a parameters VALUE span must return None.
        let catalog = StubCatalog::empty().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![UriOption::new(
                "period",
                "Tick interval",
                OptionKind::Duration,
            )]),
        );
        let engine = LintEngine::new(Arc::new(catalog));

        let source = "from: timer:tick\nparameters:\n  period: 1s\n";
        let doc = Document::parse(source);
        // value "1s" at 39..41; offset 40 = 's'.
        let info = engine.hover_at(&doc, 40);
        assert_eq!(info, None, "parameters value position should return None");
    }
}
