//! Ratchet lint for free-port probe functions and their call sites
//! (`lint-no-free-port`, bd rc-d70up, mission 401).
//!
//! Finds Rust identifiers that are `fn` definitions or `fn` call
//! expressions whose whole name matches the port-probe vocabulary
//! (`free_port`, `find_free_port`, `prealloc_port`, `reserve_local_port`,
//! `closed_port`, `unused_port`, `pick_free_port`, ...). These helpers
//! bind an ephemeral port, read it, and drop the listener before the
//! real consumer binds — the ADR-0070 port-toctou class: a sibling test
//! can steal the port in the window, so the test flakes under parallel
//! execution.
//!
//! Matching is whole-identifier anchored: `free_import` and
//! `reserve_transport` never match. Calls resolve by the final path
//! segment, so `support::free_port()` and `crate::find_free_port()` are
//! both reported.
//!
//! Enforcement is a soft count ratchet, NOT a build break for existing
//! code (mirror of `lint-test-sleep`, bd rc-c9r6w):
//! `scripts/xtask/ratchet-no-free-port.max` holds the maximum allowed
//! count of unadjudicated findings. The number may only decrease
//! (monotone). Lowering it is the ratchet action; raising it is a
//! review-visible regression signal.
//!
//! Escape hatch: put `// allow-free-port: <reason>` (non-empty reason) on
//! the line IMMEDIATELY ABOVE a definition or call site. The marker
//! suppresses that site only — a marked definition does NOT suppress its
//! call sites; every site is adjudicated individually.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{Expr, ItemFn};

/// A reported free-port definition or call site.
#[derive(Debug)]
pub struct Finding {
    pub line: usize,
    pub ident: String,
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

/// Marker prefix accepted on the line above a site.
const ALLOW_MARKER: &str = "// allow-free-port:";

/// Whole-identifier port-probe vocabulary. Anchored at both ends so
/// `free_import`, `reserve_transport`, and `sentinel_master_port` never
/// match. Verb set covers the synonyms a future helper author would
/// plausibly reach for (stage-4 review, mission 401): probe/alloc
/// helpers named `ephemeral_port`, `random_port`, `next_port`,
/// `open_port`, or `grab_*_port` must not escape the ratchet.
const PORT_PROBE_PATTERN: &str = "^(get_)?(free|find|prealloc|reserve|closed|unused|pick|ephemeral|random|next|open|grab)_([a-z0-9_]+_)?port$";

/// True when `name` is a port-probe identifier (whole-name anchored).
pub fn is_port_probe_name(name: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = PATTERN.get_or_init(|| {
        regex::Regex::new(PORT_PROBE_PATTERN).expect("port-probe regex is valid") // allow-unwrap
    });
    re.is_match(name)
}

/// Parse `source` and report free-port definitions and call sites.
pub fn scan_source(source: &str, file: &Path) -> Result<Vec<Finding>, ScanError> {
    let ast = syn::parse_file(source).map_err(|err| ScanError::Parse {
        file: file.to_path_buf(),
        err,
    })?;
    let lines: Vec<&str> = source.lines().collect();
    let mut finder = FreePortFinder {
        lines: &lines,
        findings: Vec::new(),
    };
    finder.visit_file(&ast);
    let mut findings = finder.findings;
    findings.sort_by_key(|f| f.line);
    Ok(findings)
}

/// Member roots scanned by [`run`], relative to the workspace root.
const MEMBER_ROOTS: [&str; 5] = ["crates", "scripts", "examples", "benchmarks", "fuzz"];

/// Name of the ratchet file inside `scripts/xtask/`.
pub const RATCHET_FILE: &str = "ratchet-no-free-port.max";

/// Read the ratchet ceiling from `scripts/xtask/ratchet-no-free-port.max`.
/// First non-empty, non-`#` line must be a non-negative integer. A missing
/// file is an error: an accidentally deleted ratchet must not silently pass
/// (same fail-closed rule as `lint_test_sleep::read_ratchet_max`).
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
    /// Findings with their source-file paths.
    pub findings: Vec<(PathBuf, Finding)>,
    /// Ceiling read from the ratchet file.
    pub max: usize,
}

/// Read the ratchet ceiling, then scan every member root under `root`
/// (`crates/`, `scripts/`, `examples/`, `benchmarks/`, `fuzz/`) for
/// free-port definitions and call sites. Member roots that do not exist
/// are skipped and `target`, `.worktrees`, `node_modules`, and `archive`
/// directories are pruned. A read or parse failure aborts the scan with a
/// path-qualified [`ScanError`]. A missing or malformed ratchet file is an
/// error before any scanning (fail-closed).
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

struct FreePortFinder<'a> {
    lines: &'a [&'a str],
    findings: Vec<Finding>,
}

impl FreePortFinder<'_> {
    /// Suppress when the line immediately above `line` carries
    /// `// allow-free-port:` with non-whitespace reason text. The marker
    /// applies to that site only: a definition's marker never suppresses
    /// its call sites.
    fn suppressed(&self, line: usize) -> bool {
        if line < 2 {
            return false;
        }
        self.lines
            .get(line - 2)
            .is_some_and(|text| marker_is_valid(text))
    }

    fn record(&mut self, ident: &syn::Ident) {
        let name = ident.to_string();
        if !is_port_probe_name(&name) {
            return;
        }
        let line = ident.span().start().line;
        if !self.suppressed(line) {
            self.findings.push(Finding { line, ident: name });
        }
    }
}

fn marker_is_valid(text: &str) -> bool {
    text.find(ALLOW_MARKER)
        .is_some_and(|idx| !text[idx + ALLOW_MARKER.len()..].trim().is_empty())
}

impl<'ast> Visit<'ast> for FreePortFinder<'_> {
    fn visit_item_fn(&mut self, f: &'ast ItemFn) {
        self.record(&f.sig.ident);
        visit::visit_item_fn(self, f);
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.record(&f.sig.ident);
        visit::visit_impl_item_fn(self, f);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Expr::Path(pe) = call.func.as_ref()
            && let Some(segment) = pe.path.segments.last()
        {
            self.record(&segment.ident);
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        self.record(&call.method);
        visit::visit_expr_method_call(self, call);
    }
}

#[cfg(test)]
mod tests {
    use super::{RATCHET_FILE, ScanError, is_port_probe_name, run, scan_source};
    use std::path::Path;

    /// A minimal file containing one reportable free-port definition.
    const FREE_PORT_SRC: &str = "fn free_port() -> u16 {\n    0\n}\n";

    fn seed_ratchet(root: &Path, max: usize) {
        let xtask = root.join("scripts").join("xtask");
        std::fs::create_dir_all(&xtask).unwrap(); // allow-unwrap
        std::fs::write(xtask.join(RATCHET_FILE), format!("# ratchet\n{max}\n")).unwrap(); // allow-unwrap
    }

    #[test]
    fn anchored_pattern_accepts_port_probes() {
        for name in [
            "free_port",
            "find_free_port",
            "prealloc_port",
            "reserve_local_port",
            "reserve_free_port",
            "closed_port",
            "unused_port",
            "pick_unused_port",
            "get_free_port",
            "ephemeral_port",
            "random_port",
            "next_port",
            "open_port",
            "grab_free_port",
        ] {
            assert!(is_port_probe_name(name), "{name} must match");
        }
    }

    #[test]
    fn anchored_pattern_rejects_lookalikes() {
        for name in [
            "free_import",
            "reserve_transport",
            "sentinel_master_port",
            "free_ports",
            "my_free_port",
            "free_port_extra",
            "port",
        ] {
            assert!(!is_port_probe_name(name), "{name} must not match");
        }
    }

    #[test]
    fn def_reported() {
        let findings = scan_source(FREE_PORT_SRC, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 1);
        assert_eq!(findings[0].ident, "free_port");
    }

    #[test]
    fn call_reported() {
        let src = "fn t() {\n    let p = support::find_free_port();\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 2);
        assert_eq!(findings[0].ident, "find_free_port");
    }

    #[test]
    fn non_matching_identifiers_not_reported() {
        let src = "fn free_import() {}\nfn reserve_transport() {}\nfn t() {\n    free_import();\n    let sentinel_master_port = 0;\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn marker_above_definition_suppresses_it() {
        let src = "// allow-free-port: external JVM consumer\nfn free_port() -> u16 {\n    0\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert!(findings.is_empty());
    }

    #[test]
    fn empty_marker_does_not_suppress() {
        let src = "// allow-free-port:\nfn free_port() -> u16 {\n    0\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn def_marker_does_not_suppress_callsite() {
        let src = "// allow-free-port: external consumer\nfn free_port() -> u16 {\n    0\n}\nfn t() {\n    let p = free_port();\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 6);
    }

    #[test]
    fn callsite_marker_suppresses_only_that_site() {
        let src = "fn free_port() -> u16 {\n    0\n}\nfn t() {\n    // allow-free-port: adjudicated\n    let a = free_port();\n    let b = free_port();\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        // Definition + the second call remain; the first call is marked.
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].line, 1);
        assert_eq!(findings[1].line, 7);
    }

    #[test]
    fn marker_not_immediately_above_does_not_suppress() {
        let src = "// allow-free-port: not adjacent\n\nfn free_port() -> u16 {\n    0\n}\n";
        let findings = scan_source(src, Path::new("fixture.rs")).expect("parse ok");
        assert_eq!(findings.len(), 1);
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
        std::fs::write(&a, FREE_PORT_SRC).unwrap(); // allow-unwrap
        let gen_rs = root.path().join("crates/x/target/gen.rs");
        std::fs::create_dir_all(gen_rs.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&gen_rs, FREE_PORT_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].0, a);
        assert_eq!(report.findings[0].1.line, 1);
    }

    #[test]
    fn run_prunes_sibling_excluded_dirs() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 1);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, FREE_PORT_SRC).unwrap(); // allow-unwrap
        for rel in [
            "crates/x/src/.worktrees/b.rs",
            "crates/x/src/node_modules/c.rs",
            "crates/x/src/archive/d.rs",
        ] {
            let f = root.path().join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap(); // allow-unwrap
            std::fs::write(&f, FREE_PORT_SRC).unwrap(); // allow-unwrap
        }

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].0, a);
    }

    #[test]
    fn ratchet_exceeded_when_findings_over_max() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 0);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, FREE_PORT_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert_eq!(report.findings.len(), 1);
        assert!(report.findings.len() > report.max);
    }

    #[test]
    fn ratchet_passes_at_exact_baseline() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 1);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, FREE_PORT_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings.len(), report.max);
    }

    #[test]
    fn allow_marker_sites_do_not_count_toward_ratchet() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 1);
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(
            &a,
            "// allow-free-port: external consumer\nfn free_port() -> u16 {\n    0\n}\n",
        )
        .unwrap(); // allow-unwrap
        let b = root.path().join("crates/x/src/b.rs");
        std::fs::create_dir_all(b.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&b, FREE_PORT_SRC).unwrap(); // allow-unwrap

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].0, b);
        assert_eq!(report.findings.len(), report.max);
    }

    #[test]
    fn missing_ratchet_file_is_error() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, FREE_PORT_SRC).unwrap(); // allow-unwrap

        let err = run(root.path()).unwrap_err();

        assert!(err.to_string().contains(RATCHET_FILE));
    }

    #[test]
    fn malformed_ratchet_file_is_error() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        let xtask = root.path().join("scripts").join("xtask");
        std::fs::create_dir_all(&xtask).unwrap(); // allow-unwrap
        std::fs::write(xtask.join(RATCHET_FILE), "# only comments\n").unwrap(); // allow-unwrap
        let a = root.path().join("crates/x/src/a.rs");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(&a, FREE_PORT_SRC).unwrap(); // allow-unwrap

        let err = run(root.path()).unwrap_err();

        assert!(err.to_string().contains(RATCHET_FILE));
    }
}
