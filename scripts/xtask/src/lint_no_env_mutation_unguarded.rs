//! Ratchet lint for unguarded process-env mutation in test scope
//! (`lint-no-env-mutation-unguarded`, mission 403 task 7.1).
//!
//! `std::env::set_var` / `remove_var` mutate process-global state and are
//! `unsafe` in edition 2024 because a concurrent reader races the write.
//! Tests must either go through the canonical
//! `camel_component_api::test_support::EnvGuard` (which serializes writers
//! and restores the prior value) or carry an explicit adjudication marker.
//!
//! Scope: Rust files under the workspace member roots that are test-scope —
//! the path contains `/tests/`, the file name is `*_tests.rs` / `test_*.rs`,
//! or the source contains `#[cfg(test)]`. The canonical guard
//! implementation (`test_support/env.rs` and `env_tests.rs`) is exempt: it
//! owns the raw calls by design.
//!
//! A raw site is adjudicated by a line comment `// env-guard-exception:
//! <reason>` before it. The token must appear at a token boundary in a `//`
//! comment (doc comments `///` / `//!` never count). Each marker authorizes
//! EXACTLY ONE site — the nearest following raw site. A second raw site
//! before another marker is a violation even when the marker sits above the
//! pair.
//!
//! Enforcement is a per-file count ledger at
//! `scripts/xtask/lint-no-env-mutation-unguarded.ledger`, one line per
//! residual file: `<repo-relative-file> <count> <reason summary>`. The
//! marked-site count must equal the seeded count exactly: above means a
//! regression, below means the entry was not lowered in the same change.
//! Entries never increase without review. A missing ledger file is an
//! error (fail-closed).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Expr, ExprCall};

/// Marker token accepted in a preceding `//` comment.
pub const MARKER: &str = "env-guard-exception:";

/// Canonical guard files that own raw env mutation by design.
pub const EXEMPT_FILES: [&str; 2] = [
    "crates/components/camel-component-api/src/test_support/env.rs",
    "crates/components/camel-component-api/src/test_support/env_tests.rs",
];

/// Ledger file path relative to the workspace root.
pub const LEDGER_FILE: &str = "scripts/xtask/lint-no-env-mutation-unguarded.ledger";

/// Member roots scanned by [`run`], relative to the workspace root.
const MEMBER_ROOTS: [&str; 5] = ["crates", "scripts", "examples", "benchmarks", "fuzz"];

/// Why a raw site was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// No `env-guard-exception:` marker precedes the site.
    MissingMarker,
    /// A marker precedes the site but its reason is empty.
    EmptyReason,
    /// The preceding marker already authorized an earlier site.
    MarkerAlreadyUsed,
}

impl ViolationKind {
    /// Human-readable remedy for the command output.
    pub fn remedy(self) -> &'static str {
        match self {
            ViolationKind::MissingMarker => {
                "add `// env-guard-exception: <reason>` on a line above this site"
            }
            ViolationKind::EmptyReason => "add a non-empty reason after `env-guard-exception:`",
            ViolationKind::MarkerAlreadyUsed => {
                "each marker authorizes exactly one site; add a second marker"
            }
        }
    }
}

/// One rejected raw site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteViolation {
    /// 1-based line of the raw call.
    pub line: usize,
    /// Rejection reason.
    pub kind: ViolationKind,
}

/// Result of scanning one file: marked sites and violations.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileScan {
    /// Lines of raw sites covered by a valid, unconsumed marker.
    pub marked_sites: Vec<usize>,
    /// Raw sites rejected by marker adjudication.
    pub violations: Vec<SiteViolation>,
}

/// Scanner failure. An unreadable or unparsable file makes the ledger
/// untrustworthy, so callers must surface the file path.
#[derive(Debug)]
pub enum ScanError {
    /// The file could not be parsed as Rust.
    Parse {
        /// File that failed to parse.
        file: PathBuf,
        /// Underlying syntax error.
        err: syn::Error,
    },
    /// The file could not be read.
    Read {
        /// File that failed to read.
        file: PathBuf,
        /// Underlying I/O error.
        err: std::io::Error,
    },
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

/// A seeded ledger entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    /// Repo-relative file path.
    pub file: String,
    /// Seeded marked-site count.
    pub count: usize,
    /// Non-empty reason summary naming the un-migratability mechanism.
    pub reason: String,
}

/// Ledger mismatch between actual and seeded counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerMismatch {
    /// More marked sites than seeded.
    AboveSeed {
        /// Repo-relative file.
        file: String,
        /// Actual marked-site count.
        actual: usize,
        /// Seeded count.
        seeded: usize,
    },
    /// Fewer marked sites than seeded; the entry must be lowered.
    BelowSeed {
        /// Repo-relative file.
        file: String,
        /// Actual marked-site count.
        actual: usize,
        /// Seeded count.
        seeded: usize,
    },
    /// Marked sites exist but the file has no ledger entry.
    MissingEntry {
        /// Repo-relative file.
        file: String,
        /// Actual marked-site count.
        actual: usize,
    },
}

impl LedgerMismatch {
    /// Human-readable failure message.
    pub fn message(&self) -> String {
        match self {
            LedgerMismatch::AboveSeed {
                file,
                actual,
                seeded,
            } => format!(
                "{file}: {actual} marked sites > seeded {seeded} — migrate the new site or justify a ledger increase in review"
            ),
            LedgerMismatch::BelowSeed {
                file,
                actual,
                seeded,
            } => format!(
                "{file}: {actual} marked sites < seeded {seeded} — lower the ledger entry to {actual} in this change"
            ),
            LedgerMismatch::MissingEntry { file, actual } => format!(
                "{file}: {actual} marked sites but no ledger entry — add an entry or migrate the site"
            ),
        }
    }
}

/// Aggregated ledger result for a workspace scan.
#[derive(Debug)]
pub struct Report {
    /// All rejected raw sites.
    pub violations: Vec<(PathBuf, SiteViolation)>,
    /// Actual marked-site counts by repo-relative file.
    pub actual: BTreeMap<String, usize>,
    /// Seeded ledger entries by repo-relative file.
    pub ledger: BTreeMap<String, LedgerEntry>,
    /// Actual/seed mismatches.
    pub mismatches: Vec<LedgerMismatch>,
}

impl Report {
    /// True when there are no marker violations and no ledger mismatches.
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty() && self.mismatches.is_empty()
    }

    /// Total marked sites across all files.
    pub fn actual_total(&self) -> usize {
        self.actual.values().sum()
    }

    /// Total seeded sites across all ledger entries.
    pub fn seeded_total(&self) -> usize {
        self.ledger.values().map(|e| e.count).sum()
    }

    /// Per-file summary table for the successful path.
    pub fn summary(&self) -> String {
        if self.actual.is_empty() && self.ledger.is_empty() {
            return "  no residual sites — every test-scope env mutation is migrated\n".to_string();
        }
        let mut out = String::from("  file | actual | seeded | reason\n");
        let mut files: BTreeSet<&str> = BTreeSet::new();
        files.extend(self.actual.keys().map(String::as_str));
        files.extend(self.ledger.keys().map(String::as_str));
        for file in files {
            let actual = self.actual.get(file).copied().unwrap_or(0);
            let (seeded, reason) = match self.ledger.get(file) {
                Some(entry) => (entry.count, entry.reason.as_str()),
                None => (0, "—"),
            };
            out.push_str(&format!("  {file} | {actual} | {seeded} | {reason}\n"));
        }
        out
    }
}

/// True when `path` is test scope by the lint's path/content rules.
pub fn is_test_scope(path: &Path, source: &str) -> bool {
    if path
        .to_string_lossy()
        .replace('\\', "/")
        .contains("/tests/")
    {
        return true;
    }
    if let Some(name) = path.file_name().and_then(OsStr::to_str)
        && (name.ends_with("_tests.rs") || name.starts_with("test_"))
    {
        return true;
    }
    source.contains("#[cfg(test)]")
}

/// True when `path` is one of the canonical guard implementation files.
pub fn is_exempt(root: &Path, path: &Path) -> bool {
    let rel = repo_relative(root, path);
    EXEMPT_FILES.contains(&rel.as_str())
}

fn repo_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Parse `source` and adjudicate each raw env mutation site.
pub fn scan_source(source: &str, file: &Path) -> Result<FileScan, ScanError> {
    let ast = syn::parse_file(source).map_err(|err| ScanError::Parse {
        file: file.to_path_buf(),
        err,
    })?;
    let mut finder = RawSiteFinder { lines: Vec::new() };
    finder.visit_file(&ast);
    finder.lines.sort_unstable();

    let lines: Vec<&str> = source.lines().collect();
    Ok(adjudicate(&finder.lines, &lines))
}

fn adjudicate(sites: &[usize], lines: &[&str]) -> FileScan {
    let markers: Vec<(usize, MarkerState)> = lines
        .iter()
        .enumerate()
        .map(|(idx, text)| (idx + 1, marker_state(text)))
        .filter(|(_, state)| !matches!(state, MarkerState::Absent))
        .collect();

    let mut scan = FileScan::default();
    let mut used_markers: BTreeSet<usize> = BTreeSet::new();

    for &line in sites {
        // The nearest preceding marker line owns this site. A marker below
        // the site never authorizes it.
        let marker = markers
            .iter()
            .rev()
            .find(|(marker_line, _)| *marker_line < line);
        match marker {
            None => scan.violations.push(SiteViolation {
                line,
                kind: ViolationKind::MissingMarker,
            }),
            Some((_, MarkerState::Empty)) => scan.violations.push(SiteViolation {
                line,
                kind: ViolationKind::EmptyReason,
            }),
            Some((marker_line, MarkerState::Valid)) => {
                if used_markers.insert(*marker_line) {
                    scan.marked_sites.push(line);
                } else {
                    scan.violations.push(SiteViolation {
                        line,
                        kind: ViolationKind::MarkerAlreadyUsed,
                    });
                }
            }
            Some((_, MarkerState::Absent)) => unreachable!("absent markers were filtered"),
        }
    }
    scan.violations.sort_by_key(|v| v.line);
    scan
}

enum MarkerState {
    Absent,
    Empty,
    Valid,
}

fn marker_state(text: &str) -> MarkerState {
    let trimmed = text.trim_start();
    // A marker is a line comment. Doc comments (`///`, `//!`) and block
    // comments are not markers even when they contain the token.
    if !trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!") {
        return MarkerState::Absent;
    }

    let mut search_from = 0;
    while let Some(offset) = trimmed[search_from..].find(MARKER) {
        let idx = search_from + offset;
        let boundary = idx == 0
            || trimmed[..idx]
                .chars()
                .last()
                .is_none_or(|prev| !is_token_char(prev));
        if boundary {
            let reason = trimmed[idx + MARKER.len()..].trim();
            return if reason.is_empty() {
                MarkerState::Empty
            } else {
                MarkerState::Valid
            };
        }
        search_from = idx + MARKER.len();
    }
    MarkerState::Absent
}

fn is_token_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

struct RawSiteFinder {
    lines: Vec<usize>,
}

fn is_env_mutation_call(func: &Expr) -> bool {
    let Expr::Path(path) = func else {
        return false;
    };
    let segments: Vec<String> = path
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    let Some(last) = segments.len().checked_sub(1) else {
        return false;
    };
    if last == 0 {
        return false;
    }
    matches!(segments[last].as_str(), "set_var" | "remove_var") && segments[last - 1] == "env"
}

impl<'ast> Visit<'ast> for RawSiteFinder {
    fn visit_expr_call(&mut self, call: &'ast ExprCall) {
        if is_env_mutation_call(&call.func) {
            self.lines.push(call.func.span().start().line);
        }
        visit::visit_expr_call(self, call);
    }
}

/// Parse the ledger text into entries keyed by repo-relative file.
pub fn parse_ledger(content: &str) -> Result<BTreeMap<String, LedgerEntry>, String> {
    let mut entries = BTreeMap::new();
    for (idx, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let file = parts
            .next()
            .ok_or_else(|| format!("line {}: missing file path", idx + 1))?;
        let count_raw = parts
            .next()
            .ok_or_else(|| format!("line {}: missing count for {file}", idx + 1))?;
        let count = count_raw
            .parse::<usize>()
            .map_err(|_| format!("line {}: count '{count_raw}' is not a usize", idx + 1))?;
        let reason: String = parts.collect::<Vec<_>>().join(" ");
        if reason.trim().is_empty() {
            return Err(format!("line {}: empty reason for {file}", idx + 1));
        }
        let entry = LedgerEntry {
            file: file.to_string(),
            count,
            reason,
        };
        if entries.insert(file.to_string(), entry).is_some() {
            return Err(format!("line {}: duplicate entry for {file}", idx + 1));
        }
    }
    Ok(entries)
}

fn read_ledger(root: &Path) -> Result<BTreeMap<String, LedgerEntry>, String> {
    let path = root.join(LEDGER_FILE);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_ledger(&content)
}

/// Scan the workspace member roots for test-scope raw env mutation and
/// compare marked-site counts against the seeded ledger.
pub fn run(root: &Path) -> Result<Report, Box<dyn std::error::Error>> {
    let ledger = read_ledger(root)?;
    let mut violations = Vec::new();
    let mut actual: BTreeMap<String, usize> = BTreeMap::new();

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
            if path.extension() != Some(OsStr::new("rs")) || is_exempt(root, path) {
                continue;
            }
            let source = std::fs::read_to_string(path).map_err(|err| ScanError::Read {
                file: path.to_path_buf(),
                err,
            })?;
            if !is_test_scope(path, &source) {
                continue;
            }
            let scan = scan_source(&source, path)?;
            for violation in scan.violations {
                violations.push((path.to_path_buf(), violation));
            }
            if !scan.marked_sites.is_empty() {
                *actual.entry(repo_relative(root, path)).or_default() += scan.marked_sites.len();
            }
        }
    }

    let mut mismatches = Vec::new();
    for (file, &count) in &actual {
        match ledger.get(file) {
            None => mismatches.push(LedgerMismatch::MissingEntry {
                file: file.clone(),
                actual: count,
            }),
            Some(entry) if count > entry.count => mismatches.push(LedgerMismatch::AboveSeed {
                file: file.clone(),
                actual: count,
                seeded: entry.count,
            }),
            Some(entry) if count < entry.count => mismatches.push(LedgerMismatch::BelowSeed {
                file: file.clone(),
                actual: count,
                seeded: entry.count,
            }),
            Some(_) => {}
        }
    }
    for (file, entry) in &ledger {
        if !actual.contains_key(file) {
            mismatches.push(LedgerMismatch::BelowSeed {
                file: file.clone(),
                actual: 0,
                seeded: entry.count,
            });
        }
    }
    mismatches.sort_by(|a, b| mismatch_file(a).cmp(mismatch_file(b)));

    Ok(Report {
        violations,
        actual,
        ledger,
        mismatches,
    })
}

fn mismatch_file(m: &LedgerMismatch) -> &str {
    match m {
        LedgerMismatch::AboveSeed { file, .. }
        | LedgerMismatch::BelowSeed { file, .. }
        | LedgerMismatch::MissingEntry { file, .. } => file,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LEDGER_FILE, LedgerMismatch, ViolationKind, is_test_scope, parse_ledger, run, scan_source,
    };
    use std::path::{Path, PathBuf};

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn scan_fixture(source: &str) -> super::FileScan {
        scan_source(source, Path::new("fixture.rs")).expect("fixture parses")
    }

    fn write_file(root: &Path, rel: &str, source: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, source).expect("write fixture");
    }

    fn seed_ledger(root: &Path, content: &str) {
        write_file(root, LEDGER_FILE, content);
    }

    const TEST_SCOPED: &str = "#[cfg(test)]\nmod tests {\n";
    const END: &str = "}\n";
    const EMPTY_TEST_SCOPED: &str = "#[cfg(test)]\nmod tests {}\n";

    #[test]
    fn lint_passes_on_seeded_tree() {
        let report = run(&workspace_root()).expect("scan workspace");
        assert!(
            report.violations.is_empty(),
            "unexpected marker violations: {:?}",
            report.violations
        );
        assert!(
            report.mismatches.is_empty(),
            "seeded ledger mismatch: {:?}",
            report.mismatches
        );
        assert_eq!(report.actual_total(), report.seeded_total());
        for entry in report.ledger.values() {
            assert!(
                !entry.reason.trim().is_empty(),
                "{} has an empty reason",
                entry.file
            );
        }
        assert!(report.is_ok());
    }

    #[test]
    fn lint_fails_on_unmarked_site() {
        let unmarked = format!(
            "{TEST_SCOPED}fn t() {{\n    unsafe {{ env::set_var(\"A\", \"1\") }};\n}}{END}"
        );
        let scan = scan_fixture(&unmarked);
        assert_eq!(scan.marked_sites.len(), 0);
        assert_eq!(scan.violations.len(), 1);
        assert_eq!(scan.violations[0].line, 4);
        assert_eq!(scan.violations[0].kind, ViolationKind::MissingMarker);

        let marked = format!(
            "{TEST_SCOPED}fn t() {{\n    // env-guard-exception: fixture reason\n    unsafe {{ env::set_var(\"A\", \"1\") }};\n}}{END}"
        );
        let scan = scan_fixture(&marked);
        assert_eq!(scan.marked_sites, vec![5]);
        assert!(scan.violations.is_empty());

        let empty = format!(
            "{TEST_SCOPED}fn t() {{\n    // env-guard-exception:\n    unsafe {{ std::env::remove_var(\"A\") }};\n}}{END}"
        );
        let scan = scan_fixture(&empty);
        assert_eq!(scan.marked_sites.len(), 0);
        assert_eq!(scan.violations.len(), 1);
        assert_eq!(scan.violations[0].line, 5);
        assert_eq!(scan.violations[0].kind, ViolationKind::EmptyReason);
    }

    #[test]
    fn lint_marker_in_doc_comment_is_not_a_marker() {
        let doc_comment = format!(
            "{TEST_SCOPED}fn t() {{\n    /// env-guard-exception: doc text must not authorize\n    unsafe {{ env::set_var(\"A\", \"1\") }};\n}}{END}"
        );
        let scan = scan_fixture(&doc_comment);
        assert!(scan.marked_sites.is_empty());
        assert_eq!(scan.violations.len(), 1);
        assert_eq!(scan.violations[0].line, 5);
        assert_eq!(scan.violations[0].kind, ViolationKind::MissingMarker);

        let string_literal = format!(
            "{TEST_SCOPED}fn t() {{\n    let _ = \"env-guard-exception: string text must not authorize\";\n    unsafe {{ env::set_var(\"A\", \"1\") }};\n}}{END}"
        );
        let scan = scan_fixture(&string_literal);
        assert!(scan.marked_sites.is_empty());
        assert_eq!(scan.violations.len(), 1);
        assert_eq!(scan.violations[0].line, 5);
    }

    #[test]
    fn lint_fails_on_one_marker_two_sites() {
        let source = format!(
            "{TEST_SCOPED}fn t() {{\n    // env-guard-exception: only one site\n    unsafe {{ env::set_var(\"A\", \"1\") }};\n    unsafe {{ env::set_var(\"B\", \"2\") }};\n}}{END}"
        );
        let scan = scan_fixture(&source);
        assert_eq!(scan.marked_sites, vec![5]);
        assert_eq!(scan.violations.len(), 1);
        assert_eq!(scan.violations[0].line, 6);
        assert_eq!(scan.violations[0].kind, ViolationKind::MarkerAlreadyUsed);
    }

    #[test]
    fn lint_fails_on_count_above_seed() {
        let root = tempfile::tempdir().expect("tempdir");
        write_file(
            root.path(),
            "crates/x/src/a.rs",
            &format!(
                "{TEST_SCOPED}fn t() {{\n    // env-guard-exception: one\n    unsafe {{ env::set_var(\"A\", \"1\") }};\n    // env-guard-exception: two\n    unsafe {{ env::remove_var(\"A\") }};\n}}{END}"
            ),
        );
        seed_ledger(root.path(), "crates/x/src/a.rs 1 one residual site\n");

        let report = run(root.path()).expect("scan fixture");
        assert!(report.violations.is_empty(), "{:?}", report.violations);
        assert_eq!(report.actual.get("crates/x/src/a.rs"), Some(&2));
        assert_eq!(
            report.mismatches,
            vec![LedgerMismatch::AboveSeed {
                file: "crates/x/src/a.rs".to_string(),
                actual: 2,
                seeded: 1,
            }]
        );
    }

    #[test]
    fn lint_fails_on_count_below_seed() {
        let root = tempfile::tempdir().expect("tempdir");
        write_file(root.path(), "crates/x/src/a.rs", EMPTY_TEST_SCOPED);
        seed_ledger(root.path(), "crates/x/src/a.rs 1 stale residual site\n");

        let report = run(root.path()).expect("scan fixture");
        assert_eq!(
            report.mismatches,
            vec![LedgerMismatch::BelowSeed {
                file: "crates/x/src/a.rs".to_string(),
                actual: 0,
                seeded: 1,
            }]
        );
        assert!(
            report.mismatches[0]
                .message()
                .contains("lower the ledger entry"),
            "{}",
            report.mismatches[0].message()
        );
    }

    #[test]
    fn lint_ledger_entry_with_empty_reason_rejected() {
        let err = parse_ledger("crates/x/src/a.rs 1   \n").expect_err("empty reason");
        assert!(err.contains("empty reason"), "{err}");
    }

    #[test]
    fn test_scope_detection() {
        assert!(is_test_scope(Path::new("crates/x/tests/a.rs"), ""));
        assert!(is_test_scope(Path::new("crates/x/src/env_tests.rs"), ""));
        assert!(is_test_scope(Path::new("crates/x/src/test_env.rs"), ""));
        assert!(is_test_scope(
            Path::new("crates/x/src/a.rs"),
            "#[cfg(test)]"
        ));
        assert!(!is_test_scope(Path::new("crates/x/src/a.rs"), "fn t() {}"));
    }
}
