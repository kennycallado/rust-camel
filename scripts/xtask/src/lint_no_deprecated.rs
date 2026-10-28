//! Enforce ADR-0090: the pre-1.0 policy is delete, not hide. No
//! `#[deprecated]` attribute — bare or nested in `#[cfg_attr(.., deprecated)]`
//! — may appear anywhere in workspace Rust sources. Test files are in
//! scope: a hidden API is a hidden API regardless of where it is declared.
//!
//! The scan is a `syn` AST walk: an attribute counts only when its path is
//! exactly `deprecated`, or when a `cfg_attr` carries `deprecated` as a
//! top-level nested attribute (an item after the first comma). Mentions in
//! comments, doc prose, string literals, and `#[allow(deprecated)]` are not
//! attribute paths and therefore never match.

use crate::Violation;
use std::path::Component;
use std::path::Path;
use syn::spanned::Spanned;
use walkdir::WalkDir;

/// True when a `cfg_attr` attribute nests `deprecated` as a top-level
/// attribute — i.e. a comma-separated item after the first (condition)
/// item. The tokens are parsed as comma-separated `Meta` values, so a
/// `deprecated` sitting inside `allow(...)` or another group is not
/// matched, and `deprecated(since = "..")` (a `Meta::List`) is.
fn cfg_attr_has_deprecated(attr: &syn::Attribute) -> bool {
    use syn::parse::Parser;
    let syn::Meta::List(list) = &attr.meta else {
        return false;
    };
    let Ok(items) = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
        .parse2(list.tokens.clone())
    else {
        return false;
    };
    items
        .iter()
        .skip(1)
        .any(|meta| meta.path().is_ident("deprecated"))
}

/// Visitor flagging every forbidden `deprecated` attribute.
struct DeprecatedVisitor<'a> {
    file_path: &'a str,
    lines: Vec<&'a str>,
    violations: Vec<Violation>,
}

impl syn::visit::Visit<'_> for DeprecatedVisitor<'_> {
    fn visit_attribute(&mut self, attr: &syn::Attribute) {
        let bare = attr.path().is_ident("deprecated");
        let nested = attr.path().is_ident("cfg_attr") && cfg_attr_has_deprecated(attr);
        if bare || nested {
            let line = attr.span().start().line;
            let snippet = self
                .lines
                .get(line.saturating_sub(1))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            self.violations.push(Violation {
                file: self.file_path.to_string(),
                line,
                snippet,
            });
        }
        syn::visit::visit_attribute(self, attr);
    }
}

/// Scan one source file (unit-testable core). Parse failures are hard
/// errors: a file the lint cannot parse must not silently pass.
pub fn lint_no_deprecated_src(src: &str, file_path: &str) -> Result<Vec<Violation>, String> {
    let file = syn::parse_file(src).map_err(|e| format!("Cannot parse {file_path}: {e}"))?;
    let mut vis = DeprecatedVisitor {
        file_path,
        lines: src.lines().collect(),
        violations: Vec::new(),
    };
    syn::visit::visit_file(&mut vis, &file);
    Ok(vis.violations)
}

/// Walk `crates/` (always) and `bridges/` (when present) for `.rs` files and
/// aggregate violations. Excluded directories: `target/`, `.worktrees/`,
/// `node_modules/`, `archive/`.
pub fn lint_no_deprecated(workspace_root: &Path) -> Result<Vec<Violation>, String> {
    let mut violations = Vec::new();
    let crates_dir = workspace_root.join("crates");
    let bridges_dir = workspace_root.join("bridges");
    let mut roots = vec![crates_dir];
    if bridges_dir.is_dir() {
        roots.push(bridges_dir);
    }
    for root in roots {
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_excluded_dir(e, workspace_root))
        {
            let entry = entry.map_err(|e| {
                let affected = e
                    .path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| root.display().to_string());
                format!("cannot walk {affected}: {e}")
            })?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
            let rel = path
                .strip_prefix(workspace_root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            violations.extend(lint_no_deprecated_src(&content, &rel)?);
        }
    }
    Ok(violations)
}

/// Prune excluded directories at the `WalkDir` level (relative-path
/// comparison so a worktree root living under an excluded-named path is not
/// vacuously skipped).
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

    #[test]
    fn deprecated_bare_is_flagged() {
        let src = "#[deprecated]\nfn old() {}\n";
        let v = lint_no_deprecated_src(src, "synthetic.rs").expect("snippet parses");
        assert_eq!(v.len(), 1, "bare deprecated must be flagged: {v:?}");
        assert!(
            v[0].snippet.contains("deprecated"),
            "snippet must contain the attribute: {:?}",
            v[0].snippet
        );
    }

    #[test]
    fn deprecated_with_args_is_flagged() {
        let src = "#[deprecated(since = \"0.1.0\", note = \"use X\")]\nfn old() {}\n";
        let v = lint_no_deprecated_src(src, "synthetic.rs").expect("snippet parses");
        assert_eq!(v.len(), 1, "deprecated with args must be flagged: {v:?}");
    }

    #[test]
    fn cfg_attr_bare_deprecated_is_flagged() {
        let src = "#[cfg_attr(feature = \"x\", deprecated)]\nfn old() {}\n";
        let v = lint_no_deprecated_src(src, "synthetic.rs").expect("snippet parses");
        assert_eq!(
            v.len(),
            1,
            "cfg_attr bare deprecated must be flagged: {v:?}"
        );
    }

    #[test]
    fn cfg_attr_deprecated_with_args_is_flagged() {
        let src = "#[cfg_attr(feature = \"x\", deprecated(since = \"0.1.0\"))]\nfn old() {}\n";
        let v = lint_no_deprecated_src(src, "synthetic.rs").expect("snippet parses");
        assert_eq!(v.len(), 1, "cfg_attr deprecated with args: {v:?}");
    }

    #[test]
    fn allow_deprecated_passes() {
        let src = "#[allow(deprecated)]\nfn current() {}\n";
        let v = lint_no_deprecated_src(src, "synthetic.rs").expect("snippet parses");
        assert!(v.is_empty(), "allow(deprecated) must pass: {v:?}");
    }

    #[test]
    fn cfg_attr_allow_deprecated_passes() {
        let src = "#[cfg_attr(feature = \"x\", allow(deprecated))]\nfn current() {}\n";
        let v = lint_no_deprecated_src(src, "synthetic.rs").expect("snippet parses");
        assert!(v.is_empty(), "cfg_attr allow(deprecated) must pass: {v:?}");
    }

    #[test]
    fn deprecated_in_comment_and_string_passes() {
        let src = r##"
// deprecated: legacy
/// Deprecated since forever.
fn current() {
    let _s = "#[deprecated]";
}
"##;
        let v = lint_no_deprecated_src(src, "synthetic.rs").expect("snippet parses");
        assert!(
            v.is_empty(),
            "comments, doc prose, and strings must pass: {v:?}"
        );
    }

    #[test]
    fn non_exhaustive_passes() {
        let src = "#[non_exhaustive]\npub enum E { A }\n";
        let v = lint_no_deprecated_src(src, "synthetic.rs").expect("snippet parses");
        assert!(v.is_empty(), "non_exhaustive must pass: {v:?}");
    }

    #[test]
    fn deprecated_note_ident_is_not_matched() {
        let src = "#[deprecated_note = \"x\"]\nfn f() {}\n";
        let v = lint_no_deprecated_src(src, "synthetic.rs").expect("snippet parses");
        assert!(v.is_empty(), "deprecated_note is a different path: {v:?}");
    }

    #[test]
    fn file_line_reported() {
        let src = "fn a() {}\nfn b() {}\n#[deprecated]\nfn c() {}\n";
        let v = lint_no_deprecated_src(src, "synthetic.rs").expect("snippet parses");
        assert_eq!(v.len(), 1, "exactly one violation: {v:?}");
        assert_eq!(v[0].line, 3, "attribute is on line 3: {v:?}");
        assert_eq!(v[0].file, "synthetic.rs", "file must be reported: {v:?}");
    }

    #[test]
    fn walk_error_is_error_not_ok() {
        let root = tempfile::tempdir().expect("temporary dir");
        let err = lint_no_deprecated(root.path())
            .expect_err("missing crates dir must fail the walk, not scan clean");
        assert!(
            err.contains("cannot walk"),
            "error must name the walk failure: {err}"
        );
    }

    #[test]
    fn parse_failure_is_error() {
        assert!(
            lint_no_deprecated_src("fn f( {", "synthetic.rs").is_err(),
            "unparsable source must be a hard error"
        );
    }
}
