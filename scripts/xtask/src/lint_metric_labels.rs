//! Lint metric emission call sites for closed label sets (ADR-0041
//! principle; dashboard-observability D6, ruling N8).
//!
//! Walks `crates/**/src/**/*.rs` for calls to `record_counter`,
//! `record_histogram`, `record_component_operation`, and
//! `increment_retry_attempt`. Every string-valued argument — the metric
//! name (first parameter, it becomes a series name), the label keys and
//! values inside the literal labels array, or the scheme/operation /
//! component/operation/outcome parameters — must be:
//!
//! (a) a string literal, or
//! (b) BEST-EFFORT recognized enum-variant-derived expressions
//!     (`Enum::Variant` paths and `.as_str()`/`.to_string()` calls on
//!     them — the variant list bounds the value set), or
//! (c) the call annotated `// allow-open-label <bd-ref>` on the
//!     preceding line or the same line.
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

/// Visitor collecting closed-set violations for target method calls.
struct MetricLabelVisitor<'a> {
    file_path: &'a str,
    lines: Vec<&'a str>,
    violations: Vec<Violation>,
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
                self.push(
                    line,
                    format!("{ALLOW_MARKER} annotation is missing a bd reference"),
                );
                return false;
            }
        }
        false
    }

    /// Check one string-position argument; push a violation when the
    /// value is not provably closed.
    fn check_str_position(&mut self, line: usize, e: &syn::Expr) {
        let inner = strip_outer(e);
        let ok = matches!(inner, syn::Expr::Lit(l) if matches!(l.lit, syn::Lit::Str(_)))
            || is_enum_derived(inner);
        if !ok {
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
    fn visit_expr_method_call(&mut self, m: &syn::ExprMethodCall) {
        if TARGET_FNS.contains(&m.method.to_string().as_str()) {
            let line = m.span().start().line;
            if self.annotation_allows(line) {
                return;
            }
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
}
