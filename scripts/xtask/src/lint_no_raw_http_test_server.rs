//! Ratchet lint for hand-rolled HTTP/1.x test-server response literals
//! (`lint-no-raw-http-test-server`, bd rc-nhoh8, mission 402).
//!
//! Tests that serve raw response bytes directly over a `TcpStream` must
//! inject `Connection: close`, otherwise the client cannot tell where the
//! body ends and the test flakes or hangs. The shared harness
//! (`test_support::http`) injects the header and owns framing, so tests
//! should build responses through it instead of spelling the wire bytes.
//!
//! A literal is a finding when it starts with an exact `HTTP/1.0 ` or
//! `HTTP/1.1 ` prefix, the status token that follows begins with a digit
//! or an interpolation placeholder (`{`, covering `format!` construction
//! shapes), and it carries no structurally valid `Connection: close`
//! header. A `close` token in the body or in another header's value does
//! not exempt the literal.
//!
//! Literals inside macro token streams (`format!`, `assert!`, ...) are
//! scanned as well. Two structural exclusions apply:
//! literals that are direct arguments of `starts_with` / `ends_with` /
//! `contains` calls (assertion sites), and literals passed to the shared
//! harness constructors (`Response::*`, `HttpTestServer::start*`).
//!
//! Findings are restricted to test scope: `tests/**` directories, files
//! named `*_test.rs` / `*_tests.rs`, `#[cfg(test)]` modules, and
//! `#[test]` / `#[tokio::test]` functions.
//!
//! Enforcement is a soft count ratchet, NOT a build break for existing
//! code: `scripts/xtask/ratchet-raw-http-test-server.max` holds the
//! maximum allowed count of unadjudicated findings. The number may only
//! decrease (monotone). A missing or malformed ratchet file is an error
//! (fail-closed).
//!
//! Escape hatch: put `// allow-raw-http: <non-empty reason>` on the line
//! IMMEDIATELY ABOVE a literal. The marker suppresses that site only and
//! is listed in the command's marker report.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::scan_state::TestScopeTracker;
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::visit::{self, Visit};
use syn::{Expr, Lit, Macro, Meta};

/// Marker prefix accepted on the line above a literal.
const ALLOW_MARKER: &str = "// allow-raw-http:";

/// Exact HTTP status-line prefixes recognized at the start of a literal.
const STATUS_PREFIXES: [&str; 2] = ["HTTP/1.0 ", "HTTP/1.1 "];

/// Receiver methods whose direct literal arguments are assertion probes,
/// not response payloads.
const EXCLUDED_RECEIVER_METHODS: [&str; 3] = ["starts_with", "ends_with", "contains"];

/// Shared-harness constructor names that own response framing.
const HARNESS_CONSTRUCTORS: [&str; 8] = [
    "start",
    "start_with",
    "start_permissive",
    "raw",
    "chunked",
    "staged",
    "ok",
    "with_status",
];

/// Type path segments that identify the shared HTTP test harness.
const HARNESS_TYPE_SEGMENTS: [&str; 2] = ["Response", "HttpTestServer"];

/// A reported raw HTTP response literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Source file containing the literal.
    pub file: PathBuf,
    /// 1-based line of the literal's opening token.
    pub line: usize,
}

/// An `// allow-raw-http: <reason>` marker that suppressed a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    /// Source file containing the marker.
    pub file: PathBuf,
    /// 1-based line of the marker comment itself.
    pub line: usize,
}

/// Scanner failure. An unreadable or unparsable file makes the ratchet
/// report untrustworthy, so callers must surface the file path.
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

/// Findings and markers produced by scanning one source file.
#[derive(Debug, Default)]
pub struct ScanOutcome {
    /// Unadjudicated raw response literals.
    pub findings: Vec<Finding>,
    /// Markers that suppressed a site.
    pub markers: Vec<Marker>,
}

/// Member roots scanned by [`run`], relative to the workspace root.
const MEMBER_ROOTS: [&str; 5] = ["crates", "scripts", "examples", "benchmarks", "fuzz"];

/// Name of the ratchet file inside `scripts/xtask/`.
pub const RATCHET_FILE: &str = "ratchet-raw-http-test-server.max";

/// Read the ratchet ceiling from
/// `scripts/xtask/ratchet-raw-http-test-server.max`. First non-empty,
/// non-`#` line must be a non-negative integer. A missing file is an
/// error: an accidentally deleted ratchet must not silently pass.
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
    /// Unadjudicated findings with their source-file paths.
    pub findings: Vec<Finding>,
    /// Adjudicated sites suppressed by a marker.
    pub markers: Vec<Marker>,
    /// Ceiling read from the ratchet file.
    pub max: usize,
}

/// Read the ratchet ceiling, then scan every member root under `root`
/// (`crates/`, `scripts/`, `examples/`, `benchmarks/`, `fuzz/`) for raw
/// HTTP/1.x response literals in test scope. Every member root must exist:
/// a missing root is an error because a partial checkout must not narrow
/// the scan silently. `target`, `.worktrees`, `node_modules`, and
/// `archive` directories are pruned. A read or parse failure aborts the
/// scan with a path-qualified [`ScanError`]. A missing or malformed
/// ratchet file is an error before any scanning (fail-closed).
pub fn run(root: &Path) -> Result<Report, Box<dyn std::error::Error>> {
    let max = read_ratchet_max(root)?;
    for member in MEMBER_ROOTS {
        let member_root = root.join(member);
        if !member_root.is_dir() {
            return Err(format!(
                "required member root '{member}' is missing at {}: the raw-http scan requires a full checkout",
                member_root.display()
            )
            .into());
        }
    }
    let mut findings = Vec::new();
    let mut markers = Vec::new();
    for member in MEMBER_ROOTS {
        let member_root = root.join(member);
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
            let outcome = scan_source(&source, path)?;
            findings.extend(outcome.findings);
            markers.extend(outcome.markers);
        }
    }
    findings.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    markers.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    Ok(Report {
        findings,
        markers,
        max,
    })
}

/// Parse `source` and report raw HTTP/1.x response literals that live in
/// test scope. `file` supplies the finding path and the test-file name
/// rules (`tests/**`, `*_test.rs`, `*_tests.rs`).
pub fn scan_source(source: &str, file: &Path) -> Result<ScanOutcome, ScanError> {
    let ast = syn::parse_file(source).map_err(|err| ScanError::Parse {
        file: file.to_path_buf(),
        err,
    })?;
    let lines: Vec<&str> = source.lines().collect();
    let in_test_scope = test_scope_flags(&lines, file);
    let mut finder = RawHttpFinder {
        file: file.to_path_buf(),
        lines: &lines,
        in_test_scope: &in_test_scope,
        outcome: ScanOutcome::default(),
    };
    finder.visit_file(&ast);
    finder.outcome.findings.sort_by_key(|f| f.line);
    finder.outcome.markers.sort_by_key(|m| m.line);
    Ok(finder.outcome)
}

/// Wholesale test scope for `tests/**` directories and `*_test.rs` /
/// `*_tests.rs` file names; otherwise derive per-line scope from the
/// canonical [`TestScopeTracker`].
fn test_scope_flags(lines: &[&str], file: &Path) -> Vec<bool> {
    if is_test_file(file) {
        return vec![true; lines.len()];
    }
    let mut tracker = TestScopeTracker::new();
    lines
        .iter()
        .map(|line| tracker.line_in_test_scope(line.trim()))
        .collect()
}

/// True when `path` is a test file by location (`tests/**`) or name
/// (`*_test.rs` / `*_tests.rs`).
fn is_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    if name.ends_with("_test.rs") || name.ends_with("_tests.rs") {
        return true;
    }
    path.components()
        .any(|c| c.as_os_str() == OsStr::new("tests"))
}

/// True for a marker comment carrying a non-empty reason after the prefix.
fn marker_is_valid(text: &str) -> bool {
    text.find(ALLOW_MARKER)
        .is_some_and(|idx| !text[idx + ALLOW_MARKER.len()..].trim().is_empty())
}

/// True when any line of `text` is an HTTP/1.x status line whose status token
/// starts with a digit or `{`, and the literal is not exempted by a
/// `Connection: close` header. One leading BOM is stripped first; line
/// anchoring covers leading CRLF, noise lines, and other prefixes.
fn is_raw_response_literal(text: &str) -> bool {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let has_status_line = text.lines().any(|line| {
        STATUS_PREFIXES
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .is_some_and(
                |rest| matches!(rest.chars().next(), Some(ch) if ch.is_ascii_digit() || ch == '{'),
            )
    });
    has_status_line && !has_connection_close_header(text)
}

/// True when the header block carries a `Connection: close` header line.
/// The header block ends at the first `\r\n\r\n` measured from the literal
/// start; a literal without one is treated as header-block scope in full.
/// Every line is inspected because a leading break or noise line can precede
/// the status line.
fn has_connection_close_header(text: &str) -> bool {
    let header_block = match text.find("\r\n\r\n") {
        Some(idx) => &text[..idx],
        None => text,
    };
    header_block.split("\r\n").any(header_line_has_close)
}

/// True when one header line names `connection` (case-insensitive) and its
/// value carries the `close` token.
fn header_line_has_close(line: &str) -> bool {
    let Some((name, value)) = line.split_once(':') else {
        return false;
    };
    name.trim().eq_ignore_ascii_case("connection")
        && value
            .split(|c: char| c == ',' || c == ';' || c.is_ascii_whitespace())
            .any(|token| token.eq_ignore_ascii_case("close"))
}

/// True when `path` contains a harness type segment (`Response` /
/// `HttpTestServer`).
fn path_names_harness_type(path: &syn::Path) -> bool {
    path.segments
        .iter()
        .any(|segment| HARNESS_TYPE_SEGMENTS.contains(&segment.ident.to_string().as_str()))
}

/// True when the receiver expression chain names a shared harness type,
/// including through calls, fields, and parentheses.
fn receiver_names_harness_type(expr: &Expr) -> bool {
    match expr {
        Expr::Path(path_expr) => path_names_harness_type(&path_expr.path),
        Expr::Call(call) => {
            matches!(&*call.func, Expr::Path(path_expr) if path_names_harness_type(&path_expr.path))
                || call.args.iter().any(receiver_names_harness_type)
        }
        Expr::MethodCall(call) => receiver_names_harness_type(&call.receiver),
        Expr::Field(field) => receiver_names_harness_type(&field.base),
        Expr::Paren(paren) => receiver_names_harness_type(&paren.expr),
        Expr::Reference(reference) => receiver_names_harness_type(&reference.expr),
        Expr::Try(try_expr) => receiver_names_harness_type(&try_expr.expr),
        _ => false,
    }
}

/// True when `func` is a shared-harness constructor call: its final path
/// segment is a harness constructor and its path contains a harness type
/// segment (`Response` / `HttpTestServer`).
fn is_harness_call(func: &Expr) -> bool {
    let Expr::Path(path_expr) = func else {
        return false;
    };
    let Some(last) = path_expr.path.segments.last() else {
        return false;
    };
    let method = last.ident.to_string();
    if !HARNESS_CONSTRUCTORS.contains(&method.as_str()) {
        return false;
    }
    path_names_harness_type(&path_expr.path)
}

/// Recover the string value of a literal token inside a macro token stream.
fn token_literal_text(lit: &proc_macro2::Literal) -> Option<String> {
    let stream: TokenStream = TokenTree::Literal(lit.clone()).into();
    if let Ok(text) = syn::parse2::<syn::LitStr>(stream.clone()) {
        return Some(text.value());
    }
    if let Ok(bytes) = syn::parse2::<syn::LitByteStr>(stream) {
        return Some(String::from_utf8_lossy(&bytes.value()).into_owned());
    }
    None
}

/// True when the group at `idx` is the argument list of an excluded
/// receiver call (`starts_with` / `ends_with` / `contains`).
fn is_excluded_method_arg_group(trees: &[TokenTree], idx: usize) -> bool {
    let Some(TokenTree::Group(group)) = trees.get(idx) else {
        return false;
    };
    if group.delimiter() != Delimiter::Parenthesis || idx < 2 {
        return false;
    }
    matches!(
        &trees[idx - 1],
        TokenTree::Ident(ident)
            if EXCLUDED_RECEIVER_METHODS.contains(&ident.to_string().as_str())
    ) && matches!(&trees[idx - 2], TokenTree::Punct(punct) if punct.as_char() == '.')
}

/// True when the group at `idx` is the argument list of a shared-harness
/// constructor call.
fn is_harness_arg_group(trees: &[TokenTree], idx: usize) -> bool {
    let Some(TokenTree::Group(group)) = trees.get(idx) else {
        return false;
    };
    if group.delimiter() != Delimiter::Parenthesis || idx == 0 {
        return false;
    }
    let TokenTree::Ident(method) = &trees[idx - 1] else {
        return false;
    };
    if !HARNESS_CONSTRUCTORS.contains(&method.to_string().as_str()) {
        return false;
    }
    path_segments_before(trees, idx - 1)
        .iter()
        .any(|segment| HARNESS_TYPE_SEGMENTS.contains(&segment.as_str()))
}

/// Collect the identifier segments of the `::`-separated path ending at
/// `end`, including the identifier at `end` itself.
fn path_segments_before(trees: &[TokenTree], end: usize) -> Vec<String> {
    let mut segments = Vec::new();
    let mut idx = end;
    while let Some(TokenTree::Ident(ident)) = trees.get(idx) {
        segments.push(ident.to_string());
        // Two colon tokens precede the ident; the previous path segment
        // sits three tokens back (`Response` in `Response::raw`).
        if idx >= 3 && is_colon(&trees[idx - 1]) && is_colon(&trees[idx - 2]) {
            idx -= 3;
        } else {
            break;
        }
    }
    segments.reverse();
    segments
}

/// True for a `:` punctuation token (one half of `::`).
fn is_colon(tree: &TokenTree) -> bool {
    matches!(tree, TokenTree::Punct(punct) if punct.as_char() == ':')
}

struct RawHttpFinder<'a> {
    file: PathBuf,
    lines: &'a [&'a str],
    in_test_scope: &'a [bool],
    outcome: ScanOutcome,
}

impl RawHttpFinder<'_> {
    /// Record `text` at `line` when it is an unadjudicated raw response
    /// literal in test scope.
    fn check_literal_text(&mut self, text: &str, line: usize) {
        if line == 0 || !self.line_in_test_scope(line) {
            return;
        }
        if !is_raw_response_literal(text) {
            return;
        }
        if self.suppressed(line) {
            return;
        }
        self.outcome.findings.push(Finding {
            file: self.file.clone(),
            line,
        });
    }

    fn line_in_test_scope(&self, line: usize) -> bool {
        self.in_test_scope.get(line - 1).copied().unwrap_or(false)
    }

    /// Suppress when the line immediately above `line` carries a valid
    /// `// allow-raw-http:` marker; record the marker location.
    fn suppressed(&mut self, line: usize) -> bool {
        if line < 2 {
            return false;
        }
        let Some(text) = self.lines.get(line - 2) else {
            return false;
        };
        if !marker_is_valid(text) {
            return false;
        }
        self.outcome.markers.push(Marker {
            file: self.file.clone(),
            line: line - 1,
        });
        true
    }

    /// Scan a macro token stream for literals. `exempt` carries the
    /// harness-constructor argument exemption through nested groups.
    fn scan_tokens(&mut self, tokens: &TokenStream, exempt: bool) {
        let trees: Vec<TokenTree> = tokens.clone().into_iter().collect();
        for (idx, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Literal(lit) => {
                    if !exempt && let Some(text) = token_literal_text(lit) {
                        self.check_literal_text(&text, lit.span().start().line);
                    }
                }
                TokenTree::Group(group) => {
                    if exempt || is_harness_arg_group(&trees, idx) {
                        self.scan_tokens(&group.stream(), true);
                    } else if is_excluded_method_arg_group(&trees, idx) {
                        self.scan_method_arg_tokens(&group.stream());
                    } else {
                        self.scan_tokens(&group.stream(), false);
                    }
                }
                _ => {}
            }
        }
    }

    /// Scan the direct argument list of an assertion probe: only literals
    /// at the top level of the argument list are excluded.
    fn scan_method_arg_tokens(&mut self, tokens: &TokenStream) {
        let trees: Vec<TokenTree> = tokens.clone().into_iter().collect();
        for (idx, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Literal(_) => {}
                TokenTree::Group(group) => {
                    if is_harness_arg_group(&trees, idx) {
                        self.scan_tokens(&group.stream(), true);
                    } else {
                        self.scan_tokens(&group.stream(), false);
                    }
                }
                _ => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for RawHttpFinder<'_> {
    fn visit_lit(&mut self, lit: &'ast Lit) {
        match lit {
            Lit::Str(text) => {
                self.check_literal_text(&text.value(), text.span().start().line);
            }
            Lit::ByteStr(bytes) => {
                let value = bytes.value();
                let text = String::from_utf8_lossy(&value);
                self.check_literal_text(&text, bytes.span().start().line);
            }
            _ => {}
        }
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        self.scan_tokens(&mac.tokens, false);
    }

    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        if let Meta::List(list) = &attr.meta {
            self.scan_tokens(&list.tokens, false);
        }
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if is_harness_call(&call.func) {
            return;
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let method = call.method.to_string();
        if EXCLUDED_RECEIVER_METHODS.contains(&method.as_str()) {
            visit::visit_expr(self, &call.receiver);
            for arg in &call.args {
                if !matches!(arg, Expr::Lit(_)) {
                    visit::visit_expr(self, arg);
                }
            }
            return;
        }
        if HARNESS_CONSTRUCTORS.contains(&method.as_str())
            && receiver_names_harness_type(&call.receiver)
        {
            visit::visit_expr(self, &call.receiver);
            return;
        }
        visit::visit_expr_method_call(self, call);
    }
}

#[cfg(test)]
mod tests {
    use super::{Finding, RATCHET_FILE, ScanOutcome, is_raw_response_literal, run, scan_source};
    use std::path::Path;

    /// Wrap `body` in a `#[cfg(test)]` module so the fixture lives in test
    /// scope without relying on the fixture file's name.
    fn scanned(body: &str) -> ScanOutcome {
        let body = body.trim_start_matches('\n');
        let src = format!("#[cfg(test)]\nmod tests {{\n{body}\n}}\n");
        scan_source(&src, Path::new("fixture.rs")).expect("fixture parses")
    }

    fn findings(body: &str) -> Vec<Finding> {
        scanned(body).findings
    }

    /// Create every member root `run()` requires.
    fn seed_member_roots(root: &Path) {
        for member in super::MEMBER_ROOTS {
            std::fs::create_dir_all(root.join(member)).unwrap(); // allow-unwrap
        }
    }

    fn seed_ratchet(root: &Path, max: usize) {
        seed_member_roots(root);
        let xtask = root.join("scripts").join("xtask");
        std::fs::create_dir_all(&xtask).unwrap(); // allow-unwrap
        std::fs::write(xtask.join(RATCHET_FILE), format!("# ratchet\n{max}\n")).unwrap(); // allow-unwrap
    }

    fn write_fixture(root: &Path, rel: &str, source: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap(); // allow-unwrap
        std::fs::write(path, source).unwrap(); // allow-unwrap
    }

    const REPORTED_SITE: &str = r#"
fn t() {
    let _ = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
}
"#;

    #[test]
    fn status_line_at_literal_start_fires() {
        assert!(is_raw_response_literal(
            // allow-raw-http: detector self-test fixture, never served
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"
        ));
    }

    #[test]
    fn status_line_after_leading_crlf_fires() {
        // allow-raw-http: detector self-test fixture, never served
        assert!(is_raw_response_literal("\r\nHTTP/1.1 200 OK\r\n\r\n"));
    }

    #[test]
    fn status_line_after_leading_noise_line_fires() {
        assert!(is_raw_response_literal(
            // allow-raw-http: detector self-test fixture, never served
            "garbage\r\nHTTP/1.1 404 Not Found\r\n\r\n"
        ));
    }

    #[test]
    fn connection_close_after_leading_crlf_is_exempt() {
        assert!(!is_raw_response_literal(
            "\r\nHTTP/1.1 200 OK\r\nConnection: close\r\n\r\n"
        ));
    }

    #[test]
    fn prose_mentioning_http_version_is_silent() {
        assert!(!is_raw_response_literal(
            "see the HTTP/1.1 specification for details"
        ));
    }

    #[test]
    fn status_line_after_leading_bom_fires() {
        // allow-raw-http: detector self-test fixture, never served
        assert!(is_raw_response_literal("\u{feff}HTTP/1.1 200 OK\r\n\r\n"));
    }

    #[test]
    fn plain_response_literal_without_close_is_reported() {
        let findings = findings(REPORTED_SITE);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file.as_path(), Path::new("fixture.rs"));
        assert_eq!(findings[0].line, 4);
    }

    #[test]
    fn literal_with_valid_close_header_is_clean() {
        let findings = findings(
            r#"
fn t() {
    let _ = b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok";
}
"#,
        );
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn close_token_in_body_does_not_exempt() {
        let findings = findings(
            r#"
fn t() {
    let _ = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nConnection: close";
}
"#,
        );
        assert_eq!(findings.len(), 1, "got: {findings:?}");
    }

    #[test]
    fn format_macro_literal_is_reported() {
        let findings = findings(
            r#"
fn t() {
    let _ = format!("HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\n\r\n");
}
"#,
        );
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].line, 4);
    }

    #[test]
    fn dynamic_whole_status_placeholder_is_reported() {
        let findings = findings(
            r#"
fn t() {
    let _ = format!("HTTP/1.1 {status_line}\r\nContent-Length: 0\r\n\r\n");
}
"#,
        );
        assert_eq!(findings.len(), 1, "got: {findings:?}");
    }

    #[test]
    fn dynamic_status_and_reason_placeholders_are_reported() {
        let findings = findings(
            r#"
fn t() {
    let _ = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: 0\r\n\r\n",
        status, reason
    );
}
"#,
        );
        assert_eq!(findings.len(), 1, "got: {findings:?}");
    }

    #[test]
    fn starts_with_argument_is_excluded() {
        let findings = findings(
            r#"
fn t() {
    let _ = head.starts_with(b"HTTP/1.1 200 OK\r\n");
}
"#,
        );
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn ends_with_argument_is_excluded() {
        let findings = findings(
            r#"
fn t() {
    let _ = head.ends_with(b"HTTP/1.1 200 OK\r\n");
}
"#,
        );
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn contains_argument_is_excluded() {
        let findings = findings(
            r#"
fn t() {
    let _ = head.contains("HTTP/1.1 200 OK");
}
"#,
        );
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn assertion_macro_nesting_is_excluded() {
        let findings = findings(
            r#"
fn t() {
    assert!(head.starts_with(b"HTTP/1.1 200 OK\r\n"));
}
"#,
        );
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn request_literal_is_not_reported() {
        let findings = findings(
            r#"
fn t() {
    let _ = "GET /x HTTP/1.1\r\nHost: h\r\n\r\n";
}
"#,
        );
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn harness_constructor_argument_is_exempt() {
        let findings = findings(
            r#"
fn t() {
    let _ = Response::raw(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let _ = HttpTestServer::start(Responder::Fixed(Response::raw(
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n",
    )));
    assert_eq!(
        Response::raw(b"HTTP/1.1 500 Internal Server Error\r\n\r\n".to_vec()),
        something
    );
}
"#,
        );
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn non_harness_macro_literal_is_reported() {
        let findings = findings(
            r#"
fn t() {
    assert_eq!(
        b"HTTP/1.1 500 Internal Server Error\r\n\r\n".to_vec(),
        something
    );
}
"#,
        );
        assert_eq!(findings.len(), 1, "got: {findings:?}");
    }

    #[test]
    fn harness_method_form_is_exempt() {
        let findings = findings(
            r#"
fn t() {
    let resp = Response::ok("x");
    let _ = resp;
    Response::ok("x").ok(b"HTTP/1.1 200 OK\r\n\r\n");
    Response::raw(b"HTTP/1.1 204 No Content\r\n\r\n").with_status(204);
}
"#,
        );
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn marker_suppresses_site() {
        let findings = findings(
            r#"
fn t() {
    // allow-raw-http: benchmark loadgen CLI test; standalone std server
    let _ = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
}
"#,
        );
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn empty_reason_marker_does_not_suppress() {
        let findings = findings(
            r#"
fn t() {
    // allow-raw-http:
    let _ = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
}
"#,
        );
        assert_eq!(findings.len(), 1, "got: {findings:?}");
    }

    #[test]
    fn marker_report_lists_locations() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 0);
        write_fixture(
            root.path(),
            "crates/x/src/a.rs",
            r#"#[cfg(test)]
mod tests {
    fn t() {
        // allow-raw-http: first adjudicated site
        let _ = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        // allow-raw-http: second adjudicated site
        let _ = b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n";
    }
}
"#,
        );

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert!(report.findings.is_empty(), "got: {:?}", report.findings);
        assert_eq!(report.markers.len(), 2, "got: {:?}", report.markers);
        assert_eq!(report.markers[0].line, 4);
        assert_eq!(report.markers[1].line, 6);
        assert!(
            report
                .markers
                .iter()
                .all(|m| m.file.ends_with("crates/x/src/a.rs"))
        );
    }

    #[test]
    fn ratchet_exceeded_fails() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 0);
        write_fixture(
            root.path(),
            "crates/x/src/a.rs",
            "#[cfg(test)]\nmod tests {\n    fn t() {\n        let _ = b\"HTTP/1.1 200 OK\\r\\nContent-Length: 0\\r\\n\\r\\n\";\n    }\n}\n",
        );

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert_eq!(report.findings.len(), 1);
        assert!(report.findings.len() > report.max);
    }

    #[test]
    fn ratchet_at_baseline_passes() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 1);
        write_fixture(
            root.path(),
            "crates/x/src/a.rs",
            "#[cfg(test)]\nmod tests {\n    fn t() {\n        let _ = b\"HTTP/1.1 200 OK\\r\\nContent-Length: 0\\r\\n\\r\\n\";\n    }\n}\n",
        );

        let report = run(root.path()).unwrap(); // allow-unwrap

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings.len(), report.max);
    }

    #[test]
    fn missing_ratchet_fails_closed() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_member_roots(root.path());
        write_fixture(
            root.path(),
            "crates/x/src/a.rs",
            "#[cfg(test)]\nmod tests {\n    fn t() {\n        let _ = b\"HTTP/1.1 200 OK\\r\\nContent-Length: 0\\r\\n\\r\\n\";\n    }\n}\n",
        );

        let err = run(root.path()).unwrap_err(); // allow-unwrap

        assert!(err.to_string().contains(RATCHET_FILE), "got: {err}");
    }

    #[test]
    fn missing_member_root_fails_closed() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 0);
        std::fs::remove_dir(root.path().join("benchmarks")).unwrap(); // allow-unwrap

        let err = run(root.path()).unwrap_err(); // allow-unwrap

        let msg = err.to_string();
        assert!(msg.contains("benchmarks"), "got: {msg}");
        assert!(
            msg.contains(&root.path().display().to_string()),
            "got: {msg}"
        );
        assert!(msg.contains("full checkout"), "got: {msg}");
    }

    #[test]
    fn missing_member_root_wins_over_earlier_scan_error() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        seed_ratchet(root.path(), 0);
        write_fixture(root.path(), "crates/x/src/a.rs", "let = ;\n");
        std::fs::remove_dir(root.path().join("fuzz")).unwrap(); // allow-unwrap

        let err = run(root.path()).unwrap_err(); // allow-unwrap

        let msg = err.to_string();
        assert!(msg.contains("fuzz"), "got: {msg}");
        assert!(
            msg.contains(&root.path().display().to_string()),
            "got: {msg}"
        );
        assert!(msg.contains("full checkout"), "got: {msg}");
        assert!(!msg.contains("parse error"), "got: {msg}");
    }
}
