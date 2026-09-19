//! Ratchet lint for blocking/async sleep calls in test function bodies
//! (`lint-test-sleep`, bd rc-c9r6w).
//!
//! Finds `tokio::time::sleep` and `std::thread::sleep` calls that execute
//! directly in `#[test]` / `#[tokio::test]` function bodies (at any module
//! depth) and resolves short forms through module-level `use` declarations,
//! including grouped and aliased imports.
//!
//! Out of scope by design: sleeps inside closures, nested `fn` items, and
//! associated `fn`s in `impl`/`trait` blocks are legitimate (simulate-work
//! inside route closures); `sleep_until` and other
//! timer APIs are not sleeps; a locally bound symbol (including `if let` /
//! `while let` patterns) shadows a same-named
//! import (conservative fn-wide rule, no name-resolution engine).
//!
//! Enforcement is a soft count ratchet, NOT a build break for existing
//! code (mirror of `lint-cancel-tokens`, bd rc-pu2s / mission 128):
//! `scripts/xtask/ratchet-test-sleep.max` holds the maximum allowed count
//! of unadjudicated findings. The number may only decrease (monotone).
//! Lowering it is the ratchet action; raising it is a review-visible
//! regression signal.
//!
//! Escape hatch: append `// allow-test-sleep: <reason>` (non-empty reason)
//! to any source line spanned by the sleep call. Marked sites are
//! suppressed at scan time and never count toward the ceiling.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Item, ItemFn};

/// A reported sleep call in a test function body.
#[derive(Debug)]
pub struct Finding {
    pub line: usize,
}

/// Scanner failure. An unreadable or unparsable file makes the ratchet
/// report untrustworthy, so callers must surface the file path.
#[derive(Debug)]
pub enum ScanError {
    Parse { file: PathBuf, err: syn::Error },
    Read { file: PathBuf, err: std::io::Error },
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::Parse { file, err } => {
                write!(f, "{}: parse error: {err}", file.display())
            }
            ScanError::Read { file, err } => {
                write!(f, "{}: read error: {err}", file.display())
            }
        }
    }
}

impl std::error::Error for ScanError {}

/// Parse `source` and report sleep calls in test function bodies.
pub fn scan_source(source: &str, file: &Path) -> Result<Vec<Finding>, ScanError> {
    let ast = syn::parse_file(source).map_err(|err| ScanError::Parse {
        file: file.to_path_buf(),
        err,
    })?;
    let lines: Vec<&str> = source.lines().collect();
    let mut root_imports = Imports::new();
    collect_imports(&ast.items, &mut root_imports);
    let mut findings = Vec::new();
    scan_items(&ast.items, &[root_imports], &lines, &mut findings);
    Ok(findings)
}

/// Member roots scanned by [`run`], relative to the workspace root.
const MEMBER_ROOTS: [&str; 5] = ["crates", "scripts", "examples", "benchmarks", "fuzz"];

/// Name of the ratchet file inside `scripts/xtask/`.
pub const RATCHET_FILE: &str = "ratchet-test-sleep.max";

/// Read the ratchet ceiling from `scripts/xtask/ratchet-test-sleep.max`.
/// First non-empty, non-`#` line must be a non-negative integer. A missing
/// file is an error: an accidentally deleted ratchet must not silently pass
/// (same fail-closed rule as `lint_cancel_tokens::read_ratchet_max`).
fn read_ratchet_max(workspace_root: &Path) -> Result<usize, String> {
    let path = workspace_root
        .join("scripts")
        .join("xtask")
        .join(RATCHET_FILE);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(|l| l.parse::<usize>().ok())
        .ok_or_else(|| format!("{} must contain one integer line", path.display()))
}

/// Aggregated ratchet result for a workspace scan.
#[derive(Debug)]
pub struct Report {
    /// Sleep findings with their source-file paths.
    pub findings: Vec<(PathBuf, Finding)>,
    /// Ceiling read from the ratchet file.
    pub max: usize,
}

/// Read the ratchet ceiling, then scan every member root under `root`
/// (`crates/`, `scripts/`, `examples/`, `benchmarks/`, `fuzz/`) for
/// test-body sleeps. Member roots that do not exist are skipped and
/// `target`, `.worktrees`, `node_modules`, and `archive` directories are
/// pruned. A read or parse failure aborts the scan with a path-qualified
/// [`ScanError`]. A missing or malformed ratchet file is an error before
/// any scanning (fail-closed).
pub fn run(root: &Path) -> Result<Report, Box<dyn std::error::Error>> {
    let max = read_ratchet_max(root)?;
    let mut findings = Vec::new();
    for member in MEMBER_ROOTS {
        let member_root = root.join(member);
        if !member_root.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&member_root)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                !matches!(
                    e.file_name().to_str(),
                    Some("target") | Some(".worktrees") | Some("node_modules") | Some("archive")
                )
            })
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension() != Some(OsStr::new("rs")) {
                continue;
            }
            let source = std::fs::read_to_string(path).map_err(|err| ScanError::Read {
                file: path.to_path_buf(),
                err,
            })?;
            for finding in scan_source(&source, path)? {
                findings.push((path.to_path_buf(), finding));
            }
        }
    }
    Ok(Report { findings, max })
}

/// Visible-name -> fully-qualified target path (e.g. `sleep` ->
/// `tokio::time::sleep`).
type Imports = HashMap<String, String>;

const SLEEP_TARGETS: [&str; 2] = ["tokio::time::sleep", "std::thread::sleep"];

fn is_sleep_target(path: &str) -> bool {
    SLEEP_TARGETS.contains(&path)
}

/// True when `f` is annotated `#[test]` or `#[tokio::test]` (with or without
/// attribute arguments such as `flavor = "multi_thread"`).
fn is_test_fn(f: &ItemFn) -> bool {
    f.attrs.iter().any(|attr| {
        let names: Vec<String> = attr
            .path()
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        matches!(names.as_slice(), ["test"] | ["tokio", "test"])
    })
}

/// Register every binding a `use` declaration introduces (plain, grouped,
/// aliased). Glob imports are not resolved.
fn collect_imports(items: &[Item], map: &mut Imports) {
    for item in items {
        if let Item::Use(use_item) = item {
            collect_use_tree(&use_item.tree, String::new(), map);
        }
    }
}

fn collect_use_tree(tree: &syn::UseTree, prefix: String, map: &mut Imports) {
    match tree {
        syn::UseTree::Path(p) => {
            let next = if prefix.is_empty() {
                p.ident.to_string()
            } else {
                format!("{prefix}::{}", p.ident)
            };
            collect_use_tree(&p.tree, next, map);
        }
        syn::UseTree::Name(n) => {
            let target = if prefix.is_empty() {
                n.ident.to_string()
            } else {
                format!("{prefix}::{}", n.ident)
            };
            map.insert(n.ident.to_string(), target);
        }
        syn::UseTree::Rename(r) => {
            let target = if prefix.is_empty() {
                r.ident.to_string()
            } else {
                format!("{prefix}::{}", r.ident)
            };
            map.insert(r.rename.to_string(), target);
        }
        syn::UseTree::Group(g) => {
            for inner in &g.items {
                collect_use_tree(inner, prefix.clone(), map);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

/// Recursively walk items, descending into inline modules with a per-module
/// import chain (root module first, innermost last).
fn scan_items(items: &[Item], chain: &[Imports], lines: &[&str], findings: &mut Vec<Finding>) {
    for item in items {
        match item {
            Item::Fn(f) if is_test_fn(f) => scan_test_fn(f, chain, lines, findings),
            Item::Mod(m) => {
                if let Some(content) = &m.content {
                    let mut map = Imports::new();
                    collect_imports(&content.1, &mut map);
                    let mut sub_chain = chain.to_vec();
                    sub_chain.push(map);
                    scan_items(&content.1, &sub_chain, lines, findings);
                }
            }
            _ => {}
        }
    }
}

fn scan_test_fn(f: &ItemFn, chain: &[Imports], lines: &[&str], findings: &mut Vec<Finding>) {
    // Pass 1: collect every name bound anywhere in the test fn (inner fn
    // items, let bindings, closure parameters) — conservative fn-wide rule:
    // any such binding shadows a same-named import.
    let mut shadow = HashSet::new();
    ShadowCollector { names: &mut shadow }.visit_block(f.block.as_ref());

    // Pass 2: report sleep calls reachable in the fn body, skipping
    // closures and nested fn items; async blocks are direct body.
    let mut finder = SleepFinder {
        chain,
        shadow: &shadow,
        lines,
        findings: Vec::new(),
    };
    finder.visit_block(f.block.as_ref());
    findings.append(&mut finder.findings);
}

/// Collect binding names from a pattern (ident, tuple/struct destructuring,
/// reference, type-annotated, ...).
struct PatIdents<'a> {
    names: &'a mut HashSet<String>,
}

impl Visit<'_> for PatIdents<'_> {
    fn visit_pat(&mut self, pat: &syn::Pat) {
        if let syn::Pat::Ident(pi) = pat {
            self.names.insert(pi.ident.to_string());
        }
        visit::visit_pat(self, pat);
    }
}

struct ShadowCollector<'a> {
    names: &'a mut HashSet<String>,
}

impl Visit<'_> for ShadowCollector<'_> {
    fn visit_local(&mut self, local: &syn::Local) {
        PatIdents { names: self.names }.visit_pat(&local.pat);
        visit::visit_local(self, local);
    }

    fn visit_expr_closure(&mut self, c: &syn::ExprClosure) {
        for arg in &c.inputs {
            PatIdents { names: self.names }.visit_pat(arg);
        }
        visit::visit_expr_closure(self, c);
    }

    fn visit_expr_for_loop(&mut self, f: &syn::ExprForLoop) {
        PatIdents { names: self.names }.visit_pat(&f.pat);
        visit::visit_expr_for_loop(self, f);
    }

    fn visit_expr_let(&mut self, e: &syn::ExprLet) {
        PatIdents { names: self.names }.visit_pat(&e.pat);
        visit::visit_expr_let(self, e);
    }

    fn visit_arm(&mut self, arm: &syn::Arm) {
        PatIdents { names: self.names }.visit_pat(&arm.pat);
        visit::visit_arm(self, arm);
    }

    fn visit_item_fn(&mut self, f: &ItemFn) {
        self.names.insert(f.sig.ident.to_string());
        visit::visit_item_fn(self, f);
    }
}

struct SleepFinder<'a> {
    chain: &'a [Imports],
    shadow: &'a HashSet<String>,
    lines: &'a [&'a str],
    findings: Vec<Finding>,
}

impl SleepFinder<'_> {
    /// True when the callee path is a fully-qualified sleep call (leading
    /// `::` tolerated) or a single segment resolving through the enclosing
    /// module chain to a sleep target, unless a local binding shadows it.
    fn is_sleep_call(&self, path: &syn::Path) -> bool {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        // A leading `::` lives in `path.leading_colon`, not in the segments,
        // so joining reconstructs the qualified path either way.
        let qualified = segments.join("::");
        if segments.len() == 1 {
            let name = segments[0].as_str();
            !self.shadow.contains(name)
                && self
                    .resolve(name)
                    .is_some_and(|target| is_sleep_target(target))
        } else {
            is_sleep_target(&qualified)
        }
    }

    /// Innermost module wins: walk the chain from the last map backwards.
    fn resolve(&self, name: &str) -> Option<&String> {
        self.chain.iter().rev().find_map(|m| m.get(name))
    }

    /// Suppress when any source line spanned by the call carries
    /// `// allow-test-sleep:` with non-whitespace reason text. The range
    /// covers start line through end line inclusive (proc-macro2 span
    /// locations), so a marker on the closing line of a multi-line call
    /// still applies.
    fn suppressed(&self, start: usize, end: usize) -> bool {
        (start..=end).any(|line| {
            self.lines.get(line.saturating_sub(1)).is_some_and(|text| {
                text.find("// allow-test-sleep:").is_some_and(|idx| {
                    !text[idx + "// allow-test-sleep:".len()..].trim().is_empty()
                })
            })
        })
    }
}

impl Visit<'_> for SleepFinder<'_> {
    fn visit_expr_closure(&mut self, _c: &syn::ExprClosure) {
        // Sleeps inside closures are legitimate (simulate-work in route
        // builders); the closure body is not part of the test fn body.
    }

    fn visit_item_fn(&mut self, _f: &ItemFn) {
        // Nested fn items are separate functions, out of scope.
    }

    fn visit_impl_item_fn(&mut self, _f: &syn::ImplItemFn) {
        // Associated fns in impl blocks are separate functions, out of scope.
    }

    fn visit_trait_item_fn(&mut self, _f: &syn::TraitItemFn) {
        // Associated fns in trait definitions are separate functions, out
        // of scope.
    }

    fn visit_expr_call(&mut self, call: &syn::ExprCall) {
        if let syn::Expr::Path(pe) = call.func.as_ref()
            && self.is_sleep_call(&pe.path)
        {
            let span = call.span();
            let line = span.start().line;
            let end = span.end().line;
            if !self.suppressed(line, end) {
                self.findings.push(Finding { line });
            }
        }
        visit::visit_expr_call(self, call);
    }
}

#[cfg(test)]
mod tests {
    use super::{RATCHET_FILE, ScanError, run, scan_source};
    use std::path::Path;

    /// A minimal file containing one reportable test-body sleep.
    const TEST_SLEEP_SRC: &str = "#[test]\nfn t() {\n    std::thread::sleep(d);\n}\n";

    /// Write a ratchet ceiling file into a throwaway workspace root
    /// (mirrors `tmp_workspace_tokens` in lint_cancel_tokens.rs).
    fn seed_ratchet(root: &Path, max: usize) {
        let xtask = root.join("scripts").join("xtask");
        std::fs::create_dir_all(&xtask).unwrap(); // allow-unwrap
        std::fs::write(xtask.join(RATCHET_FILE), format!("# ratchet\n{max}\n")).unwrap(); // allow-unwrap
    }

    #[test]
    fn blocking_sleep_in_plain_test_reported() {
        let src =
            "#[test]\nfn t() {\n    std::thread::sleep(std::time::Duration::from_millis(1));\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 3);
    }

    #[test]
    fn async_sleep_in_tokio_test_reported() {
        let src = "#[tokio::test(flavor = \"multi_thread\")]\nasync fn t() {\n    let d = std::time::Duration::from_millis(1);\n    tokio::time::sleep(d).await;\n    ::tokio::time::sleep(d).await;\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].line, 4);
        assert_eq!(findings[1].line, 5);
    }

    #[test]
    fn nested_mod_test_with_mod_level_use_reported() {
        let src = "#[cfg(test)]\nmod tests {\n    use tokio::time::sleep;\n\n    #[tokio::test]\n    async fn t() {\n        let d = std::time::Duration::from_millis(1);\n        sleep(d).await;\n    }\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn sleep_in_closure_not_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let d = std::time::Duration::from_millis(1);\n    builder\n        .process(|ex| async move {\n            tokio::time::sleep(d).await;\n            Ok(ex)\n        });\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn sleep_in_async_block_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let d = std::time::Duration::from_millis(1);\n    let f = async {\n        tokio::time::sleep(d).await;\n    };\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn sleep_in_nested_fn_not_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let d = std::time::Duration::from_millis(1);\n    fn helper() {\n        std::thread::sleep(d);\n    }\n    helper();\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn impl_assoc_fn_in_test_body_not_reported() {
        let src = "#[test]\nfn t() {\n    struct S;\n    impl S {\n        fn helper() {\n            std::thread::sleep(std::time::Duration::from_millis(1));\n        }\n    }\n    S::helper();\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn sleep_in_non_test_fn_ignored() {
        let src = "use tokio::time::sleep;\nuse std::thread::sleep as tsleep;\n\nfn not_a_test() {\n    sleep(d1);\n    tsleep(d2);\n}\n\n#[cfg(test)]\nmod tests {\n    fn also_not_a_test() {\n        tokio::time::sleep(d3);\n        std::thread::sleep(d4);\n    }\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn short_form_via_use_reported() {
        let src = "use tokio::time::sleep;\n\n#[tokio::test]\nasync fn t() {\n    let d = std::time::Duration::from_millis(1);\n    sleep(d).await;\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn aliased_import_reported() {
        let src = "use tokio::time::sleep as pause;\n\n#[tokio::test]\nasync fn t() {\n    let d = std::time::Duration::from_millis(1);\n    pause(d).await;\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn shadowed_local_symbol_not_reported() {
        let src = "use tokio::time::sleep;\n\n#[tokio::test]\nasync fn t() {\n    fn sleep(_: std::time::Duration) {}\n    let d = std::time::Duration::from_millis(1);\n    sleep(d);\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn loop_binding_shadows_not_reported() {
        let src = "use tokio::time::sleep;\n\n#[tokio::test]\nasync fn t() {\n    let d = std::time::Duration::from_millis(1);\n    let v = vec![d];\n    for sleep in v {\n        sleep(d).await;\n    }\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn if_let_binding_shadows_not_reported() {
        let src = "use tokio::time::sleep;\n\n#[tokio::test]\nasync fn t() {\n    let d = std::time::Duration::from_millis(1);\n    if let Some(sleep) = Some(1) {\n        sleep(d).await;\n    }\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn sleep_until_not_flagged() {
        let src = "#[tokio::test]\nasync fn t() {\n    tokio::time::sleep_until(d).await;\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn allow_marker_suppresses() {
        let src = "#[test]\nfn t() {\n    std::thread::sleep(d); // allow-test-sleep: simulates slow consumer\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn empty_allow_marker_does_not_suppress() {
        let src = "#[test]\nfn t() {\n    std::thread::sleep(d); // allow-test-sleep:\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn multiline_call_marker_on_last_line_suppresses() {
        let src = "#[tokio::test]\nasync fn t() {\n    let d = std::time::Duration::from_millis(1);\n    tokio::time::sleep(\n        d,\n    ).await; // allow-test-sleep: simulated wait\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_error_propagates() {
        let err = match scan_source("fn broken( {", Path::new("fixture.rs")) {
            Err(e @ ScanError::Parse { .. }) => e,
            other => panic!("expected parse error, got {other:?}"),
        };
        assert!(err.to_string().contains("fixture.rs"));
    }

    #[test]
    fn run_walks_member_trees_and_skips_target() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 1);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, TEST_SLEEP_SRC).unwrap(); // allow-unwrap
        let gen_rs = root.path().join("crates/x/target/gen.rs");
        std::fs::create_dir_all(gen_rs.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&gen_rs, TEST_SLEEP_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        // Only a.rs is scanned: target/ is pruned.
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].0, a);
        assert_eq!(report.findings[0].1.line, 3);
    }

    #[test]
    fn run_prunes_sibling_excluded_dirs() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 1);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, TEST_SLEEP_SRC).unwrap(); // allow-unwrap
        let excluded = [
            "crates/x/src/.worktrees/b.rs",
            "crates/x/src/node_modules/c.rs",
            "crates/x/src/archive/d.rs",
        ];
        for rel in excluded {
            let f = root.path().join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap(); // allow-unwrap
            std::fs::write(&f, TEST_SLEEP_SRC).unwrap(); // allow-unwrap
        }

        let report = run(root.path()).unwrap(); // allow-unwrap

        // Excluded sibling dirs contribute no findings; only a.rs does.
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].0, a);
        assert_eq!(report.findings[0].1.line, 3);
    }

    #[test]
    fn run_skips_absent_member_roots() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 1);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, TEST_SLEEP_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert_eq!(report.findings.len(), 1);
    }

    #[test]
    fn run_reports_path_qualified_parse_error() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 1);
        let broken = root.path().join("crates/x/src/broken.rs");
        std::fs::create_dir_all(broken.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&broken, "fn broken( {").unwrap(); // allow-unwrap

        let err = run(root.path()).unwrap_err();

        assert!(err.to_string().contains("broken.rs"));
    }

    #[test]
    fn run_reports_read_error_for_directory_named_rs() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 0);
        let dir = root.path().join("crates/x/src/dir.rs");
        std::fs::create_dir_all(&dir).unwrap(); // allow-unwrap

        let err = run(root.path()).unwrap_err();

        assert!(err.to_string().contains("dir.rs"));
    }

    #[test]
    fn run_ignores_non_member_roots() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 0);
        let note = root.path().join("docs/note.rs");
        std::fs::create_dir_all(note.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&note, TEST_SLEEP_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert!(report.findings.is_empty());
        // Zero findings against a zero ceiling: at baseline, pass.
        assert_eq!(report.findings.len(), report.max);
    }

    // ---- ratchet semantics (mirror of lint_cancel_tokens) ----

    /// The FAIL condition's inputs: more findings than the ceiling allows.
    /// The command layer (main.rs) names every offender on this branch.
    #[test]
    fn ratchet_exceeded_when_findings_over_max() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 0);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, TEST_SLEEP_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert_eq!(report.findings.len(), 1);
        assert!(report.findings.len() > report.max);
    }

    /// The PASS condition at exactly the ceiling: a new unadjudicated sleep
    /// pushes the count over (previous test); the seeded count passes.
    #[test]
    fn ratchet_passes_at_exact_baseline() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 1);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, TEST_SLEEP_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings.len(), report.max);
    }

    /// Under the ceiling: green with a headroom hint (lower-to-N data).
    #[test]
    fn ratchet_headroom_when_findings_under_max() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 4);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, TEST_SLEEP_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert!(report.findings.len() < report.max);
    }

    /// Allowlist exemption: a `// allow-test-sleep:`-marked site is
    /// suppressed at scan time and therefore never counts toward the
    /// ceiling — the ratchet sees only unadjudicated findings.
    #[test]
    fn allow_marker_sites_do_not_count_toward_ratchet() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 1);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(
            &a,
            "#[test]\nfn t() {\n    std::thread::sleep(d); // allow-test-sleep: timed drain\n}\n",
        )
        .unwrap(); // allow-unwrap
        let b = root.path().join("crates/x/src/b.rs");
        std::fs::create_dir_all(b.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&b, TEST_SLEEP_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        // Only the unmarked site in b.rs is a finding; the marked one in
        // a.rs is exempt. At baseline (1 = 1): pass.
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].0, b);
        assert_eq!(report.findings.len(), report.max);
    }

    /// An accidentally deleted ratchet file must not silently pass
    /// (fail-closed, same rule as lint_cancel_tokens).
    #[test]
    fn missing_ratchet_file_is_error() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, TEST_SLEEP_SRC).unwrap(); // allow-unwrap

        let err = run(root.path()).unwrap_err();

        assert!(err.to_string().contains(RATCHET_FILE));
    }

    /// A ratchet file without a parsable integer line is an error.
    #[test]
    fn malformed_ratchet_file_is_error() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        let xtask = root.path().join("scripts").join("xtask");
        std::fs::create_dir_all(&xtask).unwrap(); // allow-unwrap
        std::fs::write(xtask.join(RATCHET_FILE), "# only comments\n").unwrap(); // allow-unwrap
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, TEST_SLEEP_SRC).unwrap(); // allow-unwrap

        let err = run(root.path()).unwrap_err();

        assert!(err.to_string().contains(RATCHET_FILE));
    }
}
