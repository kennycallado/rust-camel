//! Lint metric emission call sites for closed label sets (ADR-0041
//! principle; dashboard-observability D6, ruling N8).
//!
//! Walks `crates/**/src/**/*.rs` for calls to `record_counter`,
//! `record_histogram`, `record_component_operation`,
//! `increment_retry_attempt`, and `increment_errors`. Every string-valued
//! argument — the metric name (first parameter, it becomes a series name),
//! the label keys and values inside the literal labels array, or the
//! scheme/operation / component/operation/outcome parameters — must be:
//!
//! (a) a string literal, or
//! (b) BEST-EFFORT recognized enum-variant-derived expressions
//!     (`Enum::Variant` paths and `.as_str()`/`.to_string()` calls on
//!     them — the variant list bounds the value set), or
//! (c) the call annotated `// allow-open-label <bd-ref>` on the
//!     preceding line or the same line.
//!
//! INDEPENDENT of (a)-(c): EVERY string literal in walked code carrying
//! the `b-prime:` prefix — in any call, including helper-forwarding
//! arguments — must match the ADR-0012 b-prime grammar
//! `b-prime:<component>:<site>` (the b-prime branch of the canonical
//! `LABEL_REGEX` in `main.rs`; component min one char, site min two). A
//! malformed b-prime literal is a violation with no annotation escape:
//! `allow-open-label` suppresses only the closed-set denial above, never
//! the shape check.
//!
//! For `increment_errors` only the second argument (error_type) is the
//! ADR-0012 label position; the first argument (route_id) is a runtime
//! dimension and is never checked.
//!
//! Default-deny: undecidable expressions are violations. Calls inside
//! `impl MetricsCollector` / `impl ComponentMetrics` blocks are SKIPPED —
//! those are the collector transport (parameter pass-through by
//! definition), not emission sites; the values they forward were already
//! checked at their origin. Test files and `#[cfg(test)]` modules are
//! out of scope.

use crate::Violation;
use quote::ToTokens;
use std::path::{Component, Path};
use syn::spanned::Spanned;
use walkdir::WalkDir;

/// Method names whose call sites are emission points for label values.
const TARGET_FNS: &[&str] = &[
    "record_counter",
    "record_histogram",
    "record_component_operation",
    "increment_retry_attempt",
    "increment_errors",
];

/// Trait names whose impl blocks are collector transport, not emission.
const COLLECTOR_TRAITS: &[&str] = &["MetricsCollector", "ComponentMetrics"];

/// Annotation marker that suppresses the closed-set check for one call.
const ALLOW_MARKER: &str = "allow-open-label";

/// True when `s` carries an explicit tracker reference token
/// (`bd-12`, `rc-xl5k`) — the marker word itself (`allow-open-label`)
/// and plain hyphenated prose must NOT satisfy this.
fn has_bd_ref(s: &str) -> bool {
    s.split_whitespace().any(|w| {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        (w.starts_with("bd-") || w.starts_with("rc-"))
            && w.len() > 3
            && w[3..].bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// True when the file path looks like a test file (mirrors the
/// `lint-unwrap` predicate; duplicated locally so this module does not
/// depend on `crate::main` helpers, following the `lint_context_citations`
/// precedent).
fn is_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    path.components()
        .any(|c| c == Component::Normal("tests".as_ref()))
        || name.starts_with("test_")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || name == "tests.rs"
        || name == "build.rs"
}

/// True when `attrs` contains `#[cfg(test)]` (also `#[cfg(all(test, ...))]`).
fn has_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        attr.to_token_stream().to_string().contains("test")
    })
}

/// True when `attrs` contains a bare `#[test]` attribute.
fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("test"))
}

/// Strip parenthesization and shared borrows (`&expr`), returning the
/// inner expression. `&[...]` slice borrows over arrays strip one layer;
/// callers handle arrays explicitly.
fn strip_outer(e: &syn::Expr) -> &syn::Expr {
    match e {
        syn::Expr::Paren(p) => strip_outer(&p.expr),
        syn::Expr::Reference(r) => strip_outer(&r.expr),
        other => other,
    }
}

/// True when `ident_text` starts with an uppercase letter (Rust enum
/// variant / type naming convention).
fn is_camel_case(ident_text: &str) -> bool {
    ident_text
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
}

/// True when `path` is a multi-segment path whose segments are all
/// CamelCase — the shape of an enum variant path (`Mode::Async`). The
/// variant list of the enum bounds the value set, which is the closure
/// argument of ADR-0041's `OptionKind::Enum` closed sets.
fn is_enum_variant_path(path: &syn::Path) -> bool {
    path.segments.len() >= 2
        && path
            .segments
            .iter()
            .all(|s| s.arguments.is_empty() && is_camel_case(&s.ident.to_string()))
}

/// True when `seg` matches `[a-z][a-z0-9-]*` — the ADR-0012 component
/// segment: lowercase start, then lowercase letters, digits, or hyphens;
/// a single character is legal (the `*` of LABEL_REGEX).
fn is_component_segment(seg: &str) -> bool {
    let mut chars = seg.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// True when `seg` matches `[a-z][a-z0-9-]+` — the ADR-0012 site segment:
/// the component character classes but at least two characters (the `+`
/// of LABEL_REGEX demands one continuation char).
fn is_site_segment(seg: &str) -> bool {
    is_component_segment(seg) && seg.len() >= 2
}

/// True when `label` matches the ADR-0012 b-prime grammar
/// `^b-prime:[a-z][a-z0-9-]*:[a-z][a-z0-9-]+$` — the b-prime branch of the
/// canonical `LABEL_REGEX` (`main.rs`), narrowed to the only family this
/// lint shape-checks; the character classes are reused verbatim and pinned
/// against the regex by `b_prime_predicate_matches_label_regex`.
fn is_well_formed_b_prime(label: &str) -> bool {
    let mut segs = label.split(':');
    segs.next() == Some("b-prime")
        && segs.next().is_some_and(is_component_segment)
        && segs.next().is_some_and(is_site_segment)
        && segs.next().is_none()
}

/// BEST-EFFORT recognition of enum-derived label values: a bare
/// `Enum::Variant` path, or `.as_str()` / `.to_string()` / `.to_str()`
/// on one. Everything else — identifiers, `format!` results, field
/// accesses — is undecidable and default-denied.
fn is_enum_derived(e: &syn::Expr) -> bool {
    match strip_outer(e) {
        syn::Expr::Path(p) => is_enum_variant_path(&p.path),
        syn::Expr::MethodCall(mc) => {
            let m = mc.method.to_string();
            matches!(m.as_str(), "as_str" | "to_string" | "to_str")
                && matches!(strip_outer(&mc.receiver), syn::Expr::Path(p) if is_enum_variant_path(&p.path))
        }
        _ => false,
    }
}

/// Visitor collecting closed-set violations for target method calls and
/// b-prime shape violations for every string literal.
struct MetricLabelVisitor<'a> {
    file_path: &'a str,
    lines: Vec<&'a str>,
    violations: Vec<Violation>,
    /// Line a missing-bd-ref violation was already pushed for, so one call
    /// with several open string positions reports the annotation defect once.
    missing_ref_reported: Option<usize>,
}

impl MetricLabelVisitor<'_> {
    /// Push a violation for the call starting at 1-based `line`.
    fn push(&mut self, line: usize, snippet: String) {
        self.violations.push(Violation {
            file: self.file_path.to_string(),
            line,
            snippet,
        });
    }

    /// True when the call at 1-based `line` carries a valid
    /// `// allow-open-label <bd-ref>` annotation on the preceding line
    /// or the same line. An annotation marker WITHOUT a bd reference
    /// pushes its own violation.
    fn annotation_allows(&mut self, line: usize) -> bool {
        let idx = line.saturating_sub(1);
        let candidates: [String; 2] = [
            self.lines.get(idx).map(|s| s.to_string()),
            self.lines.get(idx.wrapping_sub(1)).map(|s| s.to_string()),
        ]
        .map(|opt| opt.unwrap_or_default());
        for cand in &candidates {
            if cand.contains(ALLOW_MARKER) {
                if has_bd_ref(cand) {
                    return true;
                }
                if self.missing_ref_reported != Some(line) {
                    self.missing_ref_reported = Some(line);
                    self.push(
                        line,
                        format!("{ALLOW_MARKER} annotation is missing a bd reference"),
                    );
                }
                return false;
            }
        }
        false
    }

    /// Check one string-position argument; push a violation when the
    /// value is not provably closed. A valid `// allow-open-label
    /// <bd-ref>` annotation suppresses exactly this closed-set denial.
    fn check_str_position(&mut self, line: usize, e: &syn::Expr) {
        let inner = strip_outer(e);
        let ok = matches!(inner, syn::Expr::Lit(l) if matches!(l.lit, syn::Lit::Str(_)))
            || is_enum_derived(inner);
        if !ok && !self.annotation_allows(line) {
            let text = inner.to_token_stream().to_string();
            self.push(line, format!("label value not provably closed ({text})"));
        }
    }

    /// Check the labels argument (third parameter of record_counter /
    /// record_histogram): it must be a literal array, and every string
    /// position of every tuple element must itself be closed.
    fn check_labels_arg(&mut self, line: usize, e: &syn::Expr) {
        let inner = strip_outer(e);
        let syn::Expr::Array(array) = inner else {
            if self.annotation_allows(line) {
                return;
            }
            let text = inner.to_token_stream().to_string();
            self.push(
                line,
                format!("label value not provably closed (labels argument must be a literal array: {text})"),
            );
            return;
        };
        for elem in &array.elems {
            match strip_outer(elem) {
                syn::Expr::Tuple(t) => {
                    for part in &t.elems {
                        self.check_str_position(line, part);
                    }
                }
                other => self.check_str_position(line, other),
            }
        }
    }

    /// Dispatch the per-function argument shape.
    fn check_call(&mut self, m: &syn::ExprMethodCall) {
        let line = m.span().start().line;
        let args: Vec<&syn::Expr> = m.args.iter().collect();
        let name = m.method.to_string();
        match name.as_str() {
            "record_counter" | "record_histogram" => {
                if args.len() != 3 {
                    return; // does not compile against the trait; ignore
                }
                self.check_str_position(line, args[0]);
                self.check_labels_arg(line, args[2]);
            }
            "increment_retry_attempt" => {
                if args.len() != 2 {
                    return;
                }
                for a in &args[..2] {
                    self.check_str_position(line, a);
                }
            }
            "increment_errors" => {
                if args.len() != 2 {
                    return; // does not compile against the trait; ignore
                }
                // arg0 (route_id) is a runtime dimension, not a closed
                // label; arg1 (error_type) is the ADR-0012 label position.
                // The b-prime SHAPE of a literal error_type is checked
                // globally in visit_expr_lit — never suppressible here.
                self.check_str_position(line, args[1]);
            }
            "record_component_operation" => {
                if args.len() != 3 {
                    return;
                }
                for a in &args[..3] {
                    self.check_str_position(line, a);
                }
            }
            _ => {}
        }
    }
}

impl syn::visit::Visit<'_> for MetricLabelVisitor<'_> {
    /// GLOBAL b-prime shape check: EVERY string literal in walked
    /// (non-test, non-collector-transport) code that carries the
    /// `b-prime:` prefix must match the ADR-0012 grammar, regardless of
    /// which call it sits in. This closes the helper-forwarding gap and is
    /// deliberately NOT annotation-suppressible — the annotation escapes
    /// closed-set denials only.
    fn visit_expr_lit(&mut self, l: &syn::ExprLit) {
        if let syn::Lit::Str(s) = &l.lit {
            let value = s.value();
            if value.starts_with("b-prime:") && !is_well_formed_b_prime(&value) {
                let line = l.span().start().line;
                self.push(
                    line,
                    format!(
                        "b-prime label malformed: expected b-prime:<component>:<site> ({value})"
                    ),
                );
            }
        }
        syn::visit::visit_expr_lit(self, l);
    }

    fn visit_expr_method_call(&mut self, m: &syn::ExprMethodCall) {
        if TARGET_FNS.contains(&m.method.to_string().as_str()) {
            // Annotation consultation lives inside the closed-set checkers:
            // it suppresses only "not provably closed" denials, never the
            // global b-prime shape check above.
            self.missing_ref_reported = None;
            self.check_call(m);
        }
        // Descend so target calls NESTED in other calls' arguments
        // (spawn blocks, iterators, helpers) are still visited.
        syn::visit::visit_expr_method_call(self, m);
    }
}

/// Recursively visit items, skipping `#[cfg(test)]` modules, test fns,
/// and collector-trait impl blocks (transport, not emission sites).
fn visit_clean_items(vis: &mut MetricLabelVisitor<'_>, items: &[syn::Item]) {
    for item in items {
        match item {
            syn::Item::Mod(m) => {
                if has_cfg_test(&m.attrs) {
                    continue;
                }
                if let Some((_, content)) = &m.content {
                    visit_clean_items(vis, content);
                }
            }
            syn::Item::Fn(f) => {
                if has_cfg_test(&f.attrs) || has_test_attr(&f.attrs) {
                    continue;
                }
                syn::visit::visit_item_fn(vis, f);
            }
            syn::Item::Impl(imp) => {
                let is_collector = imp
                    .trait_
                    .as_ref()
                    .and_then(|(_, path, _)| path.segments.last())
                    .is_some_and(|seg| COLLECTOR_TRAITS.contains(&seg.ident.to_string().as_str()));
                if !is_collector {
                    syn::visit::visit_item_impl(vis, imp);
                }
            }
            other => syn::visit::visit_item(vis, other),
        }
    }
}

/// Scan one source file (unit-testable core). Parse failures are hard
/// errors: a file the lint cannot parse must not silently pass.
pub fn lint_metric_labels_src(src: &str, file_path: &str) -> Result<Vec<Violation>, String> {
    let file = syn::parse_file(src).map_err(|e| format!("Cannot parse {file_path}: {e}"))?;
    let mut vis = MetricLabelVisitor {
        file_path,
        lines: src.lines().collect(),
        violations: Vec::new(),
        missing_ref_reported: None,
    };
    visit_clean_items(&mut vis, &file.items);
    Ok(vis.violations)
}

/// Walk `crates/**/src/**/*.rs` and aggregate violations. Excluded:
/// test files, `target/`, `.worktrees/`, `node_modules/`, `archive/`.
pub fn lint_metric_labels(workspace_root: &Path) -> Result<Vec<Violation>, String> {
    let crates_dir = workspace_root.join("crates");
    let mut violations = Vec::new();
    for entry in WalkDir::new(&crates_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_excluded_dir(e, workspace_root))
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if is_test_file(path) {
            continue;
        }
        if !path
            .components()
            .any(|c| c == Component::Normal("src".as_ref()))
        {
            continue;
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        violations.extend(lint_metric_labels_src(&content, &rel)?);
    }
    Ok(violations)
}

/// Prune excluded directories at the `WalkDir` level (relative-path
/// comparison so a worktree root living under an excluded-named path is
/// not vacuously skipped).
fn is_excluded_dir(entry: &walkdir::DirEntry, workspace_root: &Path) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    const EXCLUDED: &[&str] = &["target", ".worktrees", "node_modules", "archive"];
    let rel = entry
        .path()
        .strip_prefix(workspace_root)
        .unwrap_or(entry.path());
    rel.components()
        .any(|c| matches!(c, Component::Normal(s) if EXCLUDED.iter().any(|e| s == *e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift guard (rc-otxh): the hand-rolled b-prime predicate must agree
    /// with the canonical `LABEL_REGEX` (b-prime branch) so the two
    /// encodings of ADR-0012 cannot diverge silently.
    #[test]
    fn b_prime_predicate_matches_label_regex() {
        let re = regex::Regex::new(crate::LABEL_REGEX).expect("valid label regex"); // allow-unwrap
        let corpus = [
            "b-prime:cxf:response-marshalling",
            "b-prime:direct:send-and-wait",
            "b-prime:cxf:site-", // trailing dash: allowed by both
            "b-prime:a:bc",      // single-char component: legal (`*`), two-char site (`+`)
            "b-prime:cxf:",      // empty site
            "b-prime::x",        // empty component
            "b-prime:x",         // 2 segments
            "b-prime:a:b",       // single-char site: regex rejects (`+`)
            "b-prime:ab:b",      // single-char site, legal component
            "b-prime:a:b:c",     // 4 segments
            "b-prime:Cxf:site",  // uppercase component
            "b-prime:cxf:Site",  // uppercase site
            "b-prime:cxf:site!", // illegal char
        ];
        for label in corpus {
            let regex_ok = re.is_match(label) && label.starts_with("b-prime:");
            assert_eq!(
                is_well_formed_b_prime(label),
                regex_ok,
                "predicate/regex disagree on {label:?}"
            );
        }
    }

    /// Spec scenario pair (`component-metrics-emission` — "Label values
    /// are closed sets"): a `format!`-built label value is reported; a
    /// literal passes; an annotated site passes. Synthetic snippet
    /// strings, not repo files.
    #[test]
    fn lint_catches_raw_label() {
        // RED case: format!-built label value -> violation reported.
        let raw = r#"
fn emit(m: &dyn MetricsCollector, id: &str) {
    m.record_counter("spawns_total", 1.0, &[("route", &format!("r-{id}"))]);
}
"#;
        let v = lint_metric_labels_src(raw, "synthetic.rs").expect("snippet parses");
        assert_eq!(
            v.len(),
            1,
            "format!-built label value must be reported, got: {v:?}"
        );
        assert!(
            v[0].snippet.contains("not provably closed"),
            "violation must say 'not provably closed': {:?}",
            v[0].snippet
        );

        // GREEN case: literal label value -> clean.
        let lit = r#"
fn emit(m: &dyn MetricsCollector) {
    m.record_counter("spawns_total", 1.0, &[("route", "r1")]);
}
"#;
        let v = lint_metric_labels_src(lit, "synthetic.rs").expect("snippet parses");
        assert!(v.is_empty(), "literal labels must be clean, got: {v:?}");

        // Annotation case: same-line allow-open-label with a bd ref -> clean.
        let annotated = r#"
fn emit(m: &dyn MetricsCollector, id: &str) {
    m.record_counter("spawns_total", 1.0, &[("route", id)]); // allow-open-label rc-xl5k
}
"#;
        let v = lint_metric_labels_src(annotated, "synthetic.rs").expect("snippet parses");
        assert!(v.is_empty(), "annotated site must be clean, got: {v:?}");
    }

    /// F2 fix: marker without a bd reference is itself a violation.
    ///
    /// rc-otxh: the global b-prime shape check inherits the clean-item
    /// scope — literals inside `#[cfg(test)]` modules and collector
    /// transport impls must NOT fire.
    #[test]
    fn global_shape_check_respects_scope_exclusions() {
        let raw = r#"
#[cfg(test)]
mod tests {
    fn emit(m: &dyn MetricsCollector) {
        m.increment_errors("route-1", "b-prime:a:b");
    }
}

impl MetricsCollector for MyCollector {
    fn increment_errors(&self, route_id: &str, error_type: &str) {
        let _ = (route_id, error_type);
        let typo = "b-prime:a:b";
        let _ = typo;
    }
}
"#;
        let v = lint_metric_labels_src(raw, "synthetic.rs").expect("snippet parses");
        assert!(
            v.is_empty(),
            "test-module and collector-impl literals are out of scope: {v:?}"
        );
    }

    /// rc-otxh: a bare marker on a call with several dynamic positions
    /// reports each denial plus exactly ONE missing-bd-ref violation.
    #[test]
    fn bare_marker_multi_denial_reports_one_missing_ref() {
        let raw = r#"
fn emit(m: &dyn MetricsCollector, a: &str, b: &str, c: &str) {
    // allow-open-label pass-through helper
    m.record_component_operation(a, b, c);
}
"#;
        let v = lint_metric_labels_src(raw, "synthetic.rs").expect("snippet parses");
        assert_eq!(v.len(), 4, "3 dynamic denials + 1 missing-ref, got {v:?}");
        assert_eq!(
            v.iter()
                .filter(|x| x.snippet.contains("missing a bd reference"))
                .count(),
            1,
            "missing-ref must be deduped per call: {v:?}"
        );
    }

    #[test]
    fn marker_without_bd_ref_is_violation() {
        let raw = r#"
fn emit(m: &dyn MetricsCollector, id: &str) {
    // allow-open-label pass-through helper
    m.record_counter("spawns_total", 1.0, &[("route", id)]);
}
"#;
        let v = lint_metric_labels_src(raw, "synthetic.rs").expect("snippet parses");
        assert_eq!(
            v.len(),
            2,
            "bare marker: label violation + missing-ref violation, got {v:?}"
        );
        assert!(
            v.iter()
                .any(|x| x.snippet.contains("missing a bd reference"))
        );
        let ok = r#"
fn emit(m: &dyn MetricsCollector, id: &str) {
    // allow-open-label rc-gm6s caller-bounded
    m.record_counter("spawns_total", 1.0, &[("route", id)]);
}
"#;
        assert_eq!(
            lint_metric_labels_src(ok, "synthetic.rs")
                .expect("snippet parses")
                .len(),
            0,
            "marker with rc- ref suppresses"
        );
    }

    /// F1 fix: target call nested inside another method call's
    /// arguments is still visited.
    #[test]
    fn nested_target_call_is_visited() {
        let raw = r#"
fn emit(m: &Metrics, id: &str) {
    self.spawn(async { m.record_counter("spawns_total", 1.0, &[("route", id)]) });
}
"#;
        assert_eq!(
            lint_metric_labels_src(raw, "synthetic.rs")
                .expect("snippet parses")
                .len(),
            1,
            "nested call must not be invisible"
        );
    }

    /// rc-otxh: `increment_errors` arg0 (route_id) is a runtime dimension,
    /// never checked; arg1 (error_type) is the ADR-0012 label position.
    /// A literal carrying the `b-prime:` prefix must additionally match the
    /// ADR-0012 grammar `b-prime:<component>:<site>`.
    #[test]
    fn increment_errors_b_prime_literal_passes() {
        let lit = r#"
fn emit(m: &dyn MetricsCollector) {
    m.increment_errors("route-1", "b-prime:cxf:response-marshalling");
}
"#;
        let v = lint_metric_labels_src(lit, "synthetic.rs").expect("snippet parses");
        assert!(v.is_empty(), "well-formed b-prime literal must pass: {v:?}");
    }

    /// rc-otxh: a b-prime literal whose site (or component) segment breaks
    /// the ADR-0012 grammar is flagged with a message naming the shape.
    #[test]
    fn increment_errors_malformed_b_prime_flagged() {
        let empty_site = r#"
fn emit(m: &dyn MetricsCollector) {
    m.increment_errors("route-1", "b-prime:cxf:");
}
"#;
        let v = lint_metric_labels_src(empty_site, "synthetic.rs").expect("snippet parses");
        assert_eq!(v.len(), 1, "malformed b-prime site must be flagged: {v:?}");
        assert!(
            v[0].snippet.contains("b-prime label malformed"),
            "violation must name the problem: {:?}",
            v[0].snippet
        );
        assert!(
            v[0].snippet.contains("b-prime:<component>:<site>"),
            "violation must show the expected shape: {:?}",
            v[0].snippet
        );

        // Grammar segmentation at the call path: empty component,
        // 2-segment, and 4-segment shapes are all malformed; a trailing
        // dash in the site is legal (pins non-over-rejection).
        for (bad, why) in [
            ("b-prime::x", "empty component"),
            ("b-prime:x", "2 segments"),
            ("b-prime:a:b:c", "4 segments"),
        ] {
            let src = format!(
                "fn emit(m: &dyn MetricsCollector) {{\n    m.increment_errors(\"route-1\", \"{bad}\");\n}}\n"
            );
            let v = lint_metric_labels_src(&src, "synthetic.rs").expect("snippet parses");
            assert_eq!(v.len(), 1, "{why} must be flagged: {v:?}");
        }
        let trailing_dash = r#"
fn emit(m: &dyn MetricsCollector) {
    m.increment_errors("route-1", "b-prime:cxf:site-");
}
"#;
        let v = lint_metric_labels_src(trailing_dash, "synthetic.rs").expect("snippet parses");
        assert!(v.is_empty(), "trailing dash is ADR-legal: {v:?}");

        // Grammar character classes: an uppercase component start is
        // malformed too (ADR-0012 segments are `[a-z][a-z0-9-]*`).
        let upper = r#"
fn emit(m: &dyn MetricsCollector) {
    m.increment_errors("route-1", "b-prime:Cxf:site");
}
"#;
        assert_eq!(
            lint_metric_labels_src(upper, "synthetic.rs")
                .expect("snippet parses")
                .len(),
            1,
            "uppercase component must be flagged"
        );
    }

    /// rc-otxh: a dynamic (format!-built) error_type is undecidable and
    /// default-denied, same as the other target fns.
    #[test]
    fn increment_errors_dynamic_error_type_flagged() {
        let raw = r#"
fn emit(m: &dyn MetricsCollector, site: &str) {
    m.increment_errors("route-1", &format!("b-prime:cxf:{site}"));
}
"#;
        let v = lint_metric_labels_src(raw, "synthetic.rs").expect("snippet parses");
        assert_eq!(
            v.len(),
            1,
            "format!-built error_type must be flagged: {v:?}"
        );
        assert!(
            v[0].snippet.contains("not provably closed"),
            "violation must say 'not provably closed': {:?}",
            v[0].snippet
        );
    }

    /// rc-otxh: the annotation escape hatch applies to increment_errors.
    #[test]
    fn increment_errors_annotated_dynamic_passes() {
        let annotated = r#"
fn emit(m: &dyn MetricsCollector, label: &str) {
    m.increment_errors("route-1", label); // allow-open-label rc-otxh
}
"#;
        let v = lint_metric_labels_src(annotated, "synthetic.rs").expect("snippet parses");
        assert!(v.is_empty(), "annotated dynamic site must pass: {v:?}");
    }

    /// rc-otxh: non-b-prime literal error_type passes the closed-set rule
    /// (the collector-transport otel impl is skipped at walk level), and
    /// arg0 stays unchecked even when it is a dynamic expression.
    #[test]
    fn increment_errors_non_b_prime_literal_passes() {
        let lit = r#"
fn emit(m: &dyn MetricsCollector, route_id: &str) {
    m.increment_errors(route_id, "timeout");
}
"#;
        let v = lint_metric_labels_src(lit, "synthetic.rs").expect("snippet parses");
        assert!(
            v.is_empty(),
            "plain literal error_type with dynamic route_id must pass: {v:?}"
        );
    }

    /// rc-otxh review: the ADR-0012 segment grammar is position-dependent —
    /// component `[a-z][a-z0-9-]*` (single char legal), site
    /// `[a-z][a-z0-9-]+` (min two chars). Corpus pinned at arbitrary
    /// (non-target) call sites so the global literal walk is exercised.
    #[test]
    fn b_prime_grammar_segment_positions() {
        // Single-char component + two-char site: legal per LABEL_REGEX.
        let ok = r#"
fn helper(label: &str) {}
fn emit() {
    helper("b-prime:a:bc");
}
"#;
        assert_eq!(
            lint_metric_labels_src(ok, "synthetic.rs")
                .expect("snippet parses")
                .len(),
            0,
            "single-char component with two-char site is legal"
        );

        // Single-char site: rejected by LABEL_REGEX (`+`), must be flagged.
        for label in ["b-prime:a:b", "b-prime:ab:b"] {
            let raw = format!(
                r#"
fn helper(label: &str) {{}}
fn emit() {{
    helper("{label}");
}}
"#
            );
            let v = lint_metric_labels_src(&raw, "synthetic.rs").expect("snippet parses");
            assert_eq!(v.len(), 1, "{label} must be flagged: {v:?}");
            assert!(v[0].snippet.contains("b-prime label malformed"));
        }
    }

    /// rc-otxh review: call-path pin — the same single-char-site drift is
    /// flagged on the increment_errors path with the grammar message.
    #[test]
    fn b_prime_single_char_site_flagged_at_call() {
        let raw = r#"
fn emit(m: &dyn MetricsCollector) {
    m.increment_errors("route-1", "b-prime:a:b");
}
"#;
        let v = lint_metric_labels_src(raw, "synthetic.rs").expect("snippet parses");
        assert_eq!(v.len(), 1, "single-char site at the call path: {v:?}");
        assert!(
            v[0].snippet.contains("b-prime label malformed"),
            "violation must name the grammar: {:?}",
            v[0].snippet
        );
    }

    /// rc-otxh review finding 2a: the b-prime shape check is GLOBAL — a
    /// malformed b-prime literal is flagged no matter which call (or plain
    /// expression position) it sits in, closing the helper-forwarding gap.
    #[test]
    fn global_shape_check_flags_non_target_sites() {
        let raw = r#"
fn helper(label: &str) {}
fn emit() {
    helper("b-prime:cxf:");
}
"#;
        let v = lint_metric_labels_src(raw, "synthetic.rs").expect("snippet parses");
        assert_eq!(v.len(), 1, "malformed literal at a helper call: {v:?}");
        assert!(
            v[0].snippet.contains("b-prime label malformed"),
            "violation must name the grammar: {:?}",
            v[0].snippet
        );
    }

    /// rc-otxh review finding 2b: the annotation suppresses ONLY the
    /// closed-set denial — the b-prime shape check is not suppressible.
    #[test]
    fn shape_check_not_suppressible_by_annotation() {
        let raw = r#"
fn helper(label: &str) {}
fn emit() {
    // allow-open-label rc-otxh
    helper("b-prime:cxf:");
}
"#;
        let v = lint_metric_labels_src(raw, "synthetic.rs").expect("snippet parses");
        assert_eq!(
            v.len(),
            1,
            "annotated malformed literal must STILL be flagged: {v:?}"
        );
        assert!(v[0].snippet.contains("b-prime label malformed"));
    }

    /// rc-otxh review: camel-sql-style forwarding — a helper takes a dynamic
    /// label (annotation suppresses the closed-set denial on the identifier)
    /// while its call sites pass well-formed b-prime literals that the
    /// global shape check validates silently.
    #[test]
    fn forwarding_b_prime_literals_pass() {
        let raw = r#"
fn forward(m: &dyn MetricsCollector, label: &str) {
    // allow-open-label rc-otxh
    m.increment_errors("route", label);
}
fn emit(m: &dyn MetricsCollector) {
    forward(m, "b-prime:sql:on-consume");
}
"#;
        let v = lint_metric_labels_src(raw, "synthetic.rs").expect("snippet parses");
        assert!(
            v.is_empty(),
            "well-formed forwarding literals must pass: {v:?}"
        );
    }
}
