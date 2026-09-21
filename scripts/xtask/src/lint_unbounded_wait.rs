//! Structural ratchet lint for unbounded waits in test function bodies
//! (`lint-unbounded-wait`, bd rc-3lx2, ADR-0069 §13.2 R1, epic rc-99d5).
//!
//! # Why structural, not lexical
//!
//! The R1 sketch was a lexical `loop {` scan without an enclosing
//! `tokio::time::timeout`. Adjudication rejected it: it misses JoinHandle
//! awaits and bare channel receives (no loop at all), and brace counting
//! over raw text mis-fires on braces in strings and comments. This lint
//! walks the real `syn` AST of every `#[test]` / `#[tokio::test]`
//! function body (same scope rule as `lint-test-sleep`, bd rc-c9r6w):
//! closures, nested `fn` items, and associated fns are out of scope.
//!
//! # Detector classes (V1)
//!
//! A finding is an unbounded wait — an expression that can park the test
//! forever because it waits for externally driven progress with no
//! deadline at the call site (ADR-0069 taxonomy tag `unbounded-wait`):
//!
//! - awaited known wait methods: `recv`, `lock`, `acquire`, `wait`,
//!   `join_next`, `connect` (channel receive, async lock acquisition,
//!   semaphore, process exit, JoinSet, network connect);
//! - `blocking_recv()` calls (unbounded channel receive from sync tests);
//! - awaited `spawn` calls (`tokio::spawn(..)`, `spawn_blocking(..)`,
//!   `JoinSet::spawn(..)`) and `.await` on a binding whose initializer is
//!   such a call (JoinHandle waits);
//! - `tokio::net::TcpStream::connect(..)` awaited as a free-fn call;
//! - a `loop` whose body contains an `.await` in test-body scope and
//!   that has no deadline anywhere (readiness polling / retry loops).
//!
//! Stream I/O reads/writes (`.read().await`, `.write().await`,
//! `.flush().await`) are deliberately deferred to a later detector
//! revision: they are the highest-volume class and would drown the seed
//! ceiling. Adding them later is a review-visible ratchet bump. Sync
//! blocking waits (`child.wait()` on a std process, `blocking_lock`,
//! thread `join()`) are deferred with them, as are method-form deadlines
//! (`fut.timeout(d)` via `FutureExt`) and custom bounded helpers: none
//! of them create a deadline region, so sites they bound need an
//! `allow-test-wait` marker until a later revision learns them. Waits
//! embedded in macro bodies (`tokio::select!` / `join!` / `try_join!`
//! arms) are invisible — `syn` exposes a macro call as an opaque token
//! stream — so a `select!` recv arm is neither flagged nor bounded.
//! Glob imports (`use tokio::time::*`) are not resolved — same
//! limitation as `lint-test-sleep`, conservative in the over-reporting
//! direction.
//!
//! The scaffolding (imports chain, shadow set, scan_items, ratchet
//! read, workspace walk) is a deliberate third copy of the
//! `lint_test_sleep` pattern (after `lint_cancel_tokens`); extract a
//! shared module before a fourth lint duplicates it again.
//!
//! # Boundedness
//!
//! A wait is BOUNDED when its span lies inside the future argument (2nd
//! argument) of a `tokio::time::timeout` / `tokio::time::timeout_at`
//! call (qualified paths or resolved through module `use` imports,
//! aliases included — no glob imports, same limitation as
//! `lint-test-sleep`). A `loop` additionally counts as bounded when ANY
//! `timeout` call site lies inside its subtree (the per-iteration
//! deadline pattern: `loop { match timeout(d, rx.recv()).await { .. } }`).
//! When a `loop` is reported, contained wait findings are subsumed (one
//! defect, one ratchet entry).
//!
//! The visitor has no type information: `.recv().await` is flagged
//! regardless of receiver type. ADR-0069 R1 accepts this — "a narrow
//! AST lint over known wait calls. It does not claim complete proof."
//! R6 (job-level timeouts) remains the backstop.
//!
//! # Enforcement
//!
//! Soft count ratchet, mirror of `lint-test-sleep` /
//! `lint-cancel-tokens`: `scripts/xtask/ratchet-unbounded-wait.max`
//! holds the maximum allowed count of unadjudicated findings. The number
//! may only decrease (monotone). Raising it is a review-visible
//! regression signal.
//!
//! Escape hatch (ADR-0069 R1 normative wording): append
//! `// allow-test-wait: <reason>` (non-empty reason) to any source line
//! spanned by the finding expression. Marked sites are suppressed at
//! scan time and never count toward the ceiling.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use proc_macro2::LineColumn;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Item, ItemFn};

/// A reported unbounded wait in a test function body.
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

/// Parse `source` and report unbounded waits in test function bodies.
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
pub const RATCHET_FILE: &str = "ratchet-unbounded-wait.max";

/// Read the ratchet ceiling from `scripts/xtask/ratchet-unbounded-wait.max`.
/// First non-empty, non-`#` line must be a non-negative integer. A missing
/// file is an error: an accidentally deleted ratchet must not silently pass
/// (same fail-closed rule as `lint_test_sleep` / `lint_cancel_tokens`).
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
    /// Unbounded-wait findings with their source-file paths.
    pub findings: Vec<(PathBuf, Finding)>,
    /// Ceiling read from the ratchet file.
    pub max: usize,
}

/// Read the ratchet ceiling, then scan every member root under `root`
/// (`crates/`, `scripts/`, `examples/`, `benchmarks/`, `fuzz/`) for
/// unbounded waits in test function bodies. Member roots that do not
/// exist are skipped and `target`, `.worktrees`, `node_modules`, and
/// `archive` directories are pruned. A read or parse failure aborts the
/// scan with a path-qualified [`ScanError`]. A missing or malformed
/// ratchet file is an error before any scanning (fail-closed).
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

/// Visible-name -> fully-qualified target path (e.g. `timeout` ->
/// `tokio::time::timeout`).
type Imports = HashMap<String, String>;

/// Awaited method calls that wait for externally driven progress.
const WAIT_METHODS: [&str; 6] = ["recv", "lock", "acquire", "wait", "join_next", "connect"];

/// Non-awaited method calls that block without a deadline.
const BLOCKING_METHODS: [&str; 1] = ["blocking_recv"];

/// Free-function call targets that unboundedly wait when awaited.
const WAIT_CALL_TARGETS: [&str; 4] = [
    "tokio::spawn",
    "tokio::task::spawn",
    "tokio::task::spawn_blocking",
    "tokio::net::TcpStream::connect",
];

/// Calls whose 2nd argument is a future bounded by a deadline.
const TIMEOUT_TARGETS: [&str; 2] = ["tokio::time::timeout", "tokio::time::timeout_at"];

/// Source span as a `(start, end)` pair of line/column positions.
type Span = (LineColumn, LineColumn);

fn span_of<T: Spanned>(t: &T) -> Span {
    let s = t.span();
    (s.start(), s.end())
}

/// True when `inner` lies completely inside `outer`.
fn span_contains(outer: Span, inner: Span) -> bool {
    inner.0 >= outer.0 && inner.1 <= outer.1
}

/// Strip parentheses: `((expr)).await` still exposes the base expression.
fn strip_parens(e: &syn::Expr) -> &syn::Expr {
    match e {
        syn::Expr::Paren(p) => strip_parens(&p.expr),
        _ => e,
    }
}

/// True when `f` is annotated `#[test]` or `#[tokio::test]` (with or
/// without attribute arguments such as `flavor = "multi_thread"`).
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

/// Recursively walk items, descending into inline modules with a
/// per-module import chain (root module first, innermost last).
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
    // Pass 1: every name bound anywhere in the test fn (inner fn items,
    // let bindings, closure parameters) — conservative fn-wide rule: any
    // such binding shadows a same-named import.
    let mut shadow = HashSet::new();
    ShadowCollector { names: &mut shadow }.visit_block(f.block.as_ref());

    // Pass 2: bindings whose initializer spawns a task (JoinHandle
    // sources). Full descent including closures: a spawned binding is a
    // handle source regardless of nesting.
    let mut spawned = HashSet::new();
    SpawnCollector {
        chain,
        shadow: &shadow,
        names: &mut spawned,
    }
    .visit_block(f.block.as_ref());

    // Pass 3: deadline regions — the future (2nd) argument subtree of
    // every timeout call. Loops detect their per-iteration deadlines
    // structurally via `TimeoutSeeker` at visit time.
    let mut regions = Vec::new();
    TimeoutCollector {
        chain,
        shadow: &shadow,
        future_regions: &mut regions,
    }
    .visit_block(f.block.as_ref());

    // Pass 4: report unbounded waits reachable in the fn body, skipping
    // closures and nested fn items; async blocks are direct body.
    let mut finder = WaitFinder {
        chain,
        shadow: &shadow,
        spawned: &spawned,
        future_regions: &regions,
        loop_spans: Vec::new(),
        lines,
        findings: Vec::new(),
    };
    finder.visit_block(f.block.as_ref());
    findings.append(&mut finder.findings);
}

/// Collect binding names from a pattern (ident, tuple/struct
/// destructuring, reference, type-annotated, ...).
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

/// Shared path resolution for one test fn: qualified paths match the
/// target list directly; single-segment names resolve through the
/// module import chain unless a local binding shadows them.
trait ResolvesPaths {
    fn chain(&self) -> &[Imports];
    fn shadow(&self) -> &HashSet<String>;

    /// Innermost module wins: walk the chain from the last map backwards.
    fn resolve(&self, name: &str) -> Option<&String> {
        self.chain().iter().rev().find_map(|m| m.get(name))
    }

    /// True when `path` denotes one of `targets`.
    fn is_path_target(&self, path: &syn::Path, targets: &[&str]) -> bool {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        // A leading `::` lives in `path.leading_colon`, not in the
        // segments, so joining reconstructs the qualified path either way.
        let qualified = segments.join("::");
        if segments.len() == 1 {
            let name = segments[0].as_str();
            !self.shadow().contains(name)
                && self
                    .resolve(name)
                    .is_some_and(|target| targets.contains(&target.as_str()))
        } else {
            targets.contains(&qualified.as_str())
        }
    }
}

struct SpawnCollector<'a> {
    chain: &'a [Imports],
    shadow: &'a HashSet<String>,
    names: &'a mut HashSet<String>,
}

impl ResolvesPaths for SpawnCollector<'_> {
    fn chain(&self) -> &[Imports] {
        self.chain
    }
    fn shadow(&self) -> &HashSet<String> {
        self.shadow
    }
}

impl SpawnCollector<'_> {
    /// True when `e` is a spawn call: `tokio::spawn(..)` /
    /// `spawn_blocking(..)` via (possibly aliased) path, or any
    /// `.spawn(..)` method call (JoinSet, task Builder, ...).
    fn is_spawn_expr(&self, e: &syn::Expr) -> bool {
        match strip_parens(e) {
            syn::Expr::Call(c) => {
                let syn::Expr::Path(pe) = c.func.as_ref() else {
                    return false;
                };
                self.is_path_target(&pe.path, &WAIT_CALL_TARGETS)
            }
            syn::Expr::MethodCall(mc) => mc.method == "spawn",
            _ => false,
        }
    }
}

impl Visit<'_> for SpawnCollector<'_> {
    fn visit_local(&mut self, local: &syn::Local) {
        if let Some(init) = &local.init
            && self.is_spawn_expr(&init.expr)
        {
            PatIdents { names: self.names }.visit_pat(&local.pat);
        }
        visit::visit_local(self, local);
    }
}

/// Collect deadline regions: the future (2nd) argument subtree of every
/// `tokio::time::timeout` / `timeout_at` call.
struct TimeoutCollector<'a> {
    chain: &'a [Imports],
    shadow: &'a HashSet<String>,
    future_regions: &'a mut Vec<Span>,
}

impl ResolvesPaths for TimeoutCollector<'_> {
    fn chain(&self) -> &[Imports] {
        self.chain
    }
    fn shadow(&self) -> &HashSet<String> {
        self.shadow
    }
}

impl Visit<'_> for TimeoutCollector<'_> {
    fn visit_expr_call(&mut self, call: &syn::ExprCall) {
        if let syn::Expr::Path(pe) = call.func.as_ref()
            && self.is_path_target(&pe.path, &TIMEOUT_TARGETS)
            && let Some(future_arg) = call.args.iter().nth(1)
        {
            self.future_regions.push(span_of(future_arg));
        }
        visit::visit_expr_call(self, call);
    }
}

/// True when an `.await` occurs in test-body scope (closures, nested fn
/// items, and associated fns pruned — an await inside a closure runs in
/// another task, not the test body).
struct AwaitSeeker {
    found: bool,
}

impl Visit<'_> for AwaitSeeker {
    fn visit_expr_closure(&mut self, _c: &syn::ExprClosure) {}
    fn visit_item_fn(&mut self, _f: &ItemFn) {}
    fn visit_impl_item_fn(&mut self, _f: &syn::ImplItemFn) {}
    fn visit_trait_item_fn(&mut self, _f: &syn::TraitItemFn) {}

    fn visit_expr_await(&mut self, e: &syn::ExprAwait) {
        self.found = true;
        visit::visit_expr_await(self, e);
    }
}

/// True when a timeout call site occurs anywhere in `expr`'s subtree,
/// closures included (a per-iteration deadline bounds the loop even when
/// the call sits inside a nested closure body).
struct TimeoutSeeker<'a> {
    chain: &'a [Imports],
    shadow: &'a HashSet<String>,
    found: bool,
}

impl ResolvesPaths for TimeoutSeeker<'_> {
    fn chain(&self) -> &[Imports] {
        self.chain
    }
    fn shadow(&self) -> &HashSet<String> {
        self.shadow
    }
}

impl Visit<'_> for TimeoutSeeker<'_> {
    fn visit_expr_call(&mut self, call: &syn::ExprCall) {
        if let syn::Expr::Path(pe) = call.func.as_ref()
            && self.is_path_target(&pe.path, &TIMEOUT_TARGETS)
        {
            self.found = true;
        }
        visit::visit_expr_call(self, call);
    }
}

struct WaitFinder<'a> {
    chain: &'a [Imports],
    shadow: &'a HashSet<String>,
    spawned: &'a HashSet<String>,
    future_regions: &'a [Span],
    /// Spans of already-reported unbounded loops, for subsumption of the
    /// wait findings they contain.
    loop_spans: Vec<Span>,
    lines: &'a [&'a str],
    findings: Vec<Finding>,
}

impl ResolvesPaths for WaitFinder<'_> {
    fn chain(&self) -> &[Imports] {
        self.chain
    }
    fn shadow(&self) -> &HashSet<String> {
        self.shadow
    }
}

impl WaitFinder<'_> {
    /// Bounded: the span lies inside some timeout future region.
    fn bounded(&self, sp: Span) -> bool {
        self.future_regions.iter().any(|r| span_contains(*r, sp))
    }

    /// Already covered by a reported unbounded loop.
    fn subsumed(&self, sp: Span) -> bool {
        self.loop_spans.iter().any(|l| span_contains(*l, sp))
    }

    /// Suppress when any source line spanned by the finding carries
    /// `// allow-test-wait:` with non-whitespace reason text (ADR-0069
    /// R1 normative escape hatch).
    fn suppressed(&self, start: usize, end: usize) -> bool {
        const MARKER: &str = "// allow-test-wait:";
        (start..=end).any(|line| {
            self.lines.get(line.saturating_sub(1)).is_some_and(|text| {
                text.find(MARKER)
                    .is_some_and(|idx| !text[idx + MARKER.len()..].trim().is_empty())
            })
        })
    }

    fn maybe_report(&mut self, sp: Span) {
        if self.bounded(sp) || self.subsumed(sp) {
            return;
        }
        if self.suppressed(sp.0.line, sp.1.line) {
            return;
        }
        self.findings.push(Finding { line: sp.0.line });
    }
}

impl Visit<'_> for WaitFinder<'_> {
    fn visit_expr_closure(&mut self, _c: &syn::ExprClosure) {
        // Waits inside closures run in another task (route builders,
        // spawned work); the closure body is not the test fn body.
    }

    fn visit_item_fn(&mut self, _f: &ItemFn) {
        // Nested fn items are separate functions, out of scope.
    }

    fn visit_impl_item_fn(&mut self, _f: &syn::ImplItemFn) {
        // Associated fns in impl blocks are separate functions, out of scope.
    }

    fn visit_trait_item_fn(&mut self, _f: &syn::TraitItemFn) {
        // Associated fns in trait definitions are out of scope.
    }

    fn visit_expr_loop(&mut self, e: &syn::ExprLoop) {
        let sp = span_of(e);
        if !self.bounded(sp) && !self.subsumed(sp) {
            let mut seeker = AwaitSeeker { found: false };
            seeker.visit_block(&e.body);
            if seeker.found {
                // Per-iteration deadline pattern: any timeout call site
                // inside the loop subtree bounds each wait, so the loop is
                // not an unbounded readiness spin.
                let mut tseeker = TimeoutSeeker {
                    chain: self.chain,
                    shadow: self.shadow,
                    found: false,
                };
                tseeker.visit_block(&e.body);
                if !tseeker.found {
                    // The loop is one unbounded-wait defect whether or not
                    // a marker suppresses its own finding: contained waits
                    // are subsumed either way.
                    self.loop_spans.push(sp);
                    if !self.suppressed(sp.0.line, sp.1.line) {
                        self.findings.push(Finding { line: sp.0.line });
                    }
                }
            }
        }
        visit::visit_expr_loop(self, e);
    }

    fn visit_expr_await(&mut self, e: &syn::ExprAwait) {
        let sp = span_of(e);
        match strip_parens(&e.base) {
            syn::Expr::MethodCall(mc) => {
                if WAIT_METHODS.contains(&mc.method.to_string().as_str()) || mc.method == "spawn" {
                    self.maybe_report(sp);
                }
            }
            syn::Expr::Call(c) => {
                if let syn::Expr::Path(pe) = c.func.as_ref()
                    && self.is_path_target(&pe.path, &WAIT_CALL_TARGETS)
                {
                    self.maybe_report(sp);
                }
            }
            syn::Expr::Path(pe)
                if pe.path.segments.len() == 1
                    && self
                        .spawned
                        .contains(&pe.path.segments[0].ident.to_string()) =>
            {
                self.maybe_report(sp);
            }
            _ => {}
        }
        visit::visit_expr_await(self, e);
    }

    fn visit_expr_method_call(&mut self, mc: &syn::ExprMethodCall) {
        if BLOCKING_METHODS.contains(&mc.method.to_string().as_str()) {
            self.maybe_report(span_of(mc));
        }
        visit::visit_expr_method_call(self, mc);
    }
}

#[cfg(test)]
mod tests {
    use super::{RATCHET_FILE, Report, ScanError, run, scan_source};
    use std::path::Path;

    fn findings(src: &str) -> Vec<usize> {
        scan_source(src, Path::new("fixture.rs"))
            .expect("parse ok")
            .into_iter()
            .map(|f| f.line)
            .collect()
    }

    // ---- detector classes --------------------------------------------

    #[test]
    fn recv_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let v = rx.recv().await;\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn lock_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let g = m.lock().await;\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn acquire_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let p = sem.acquire().await;\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn child_wait_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let s = child.wait().await;\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn join_next_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    while let Some(j) = set.join_next().await {\n        drop(j);\n    }\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn connect_method_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let s = peer.connect().await;\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn tcp_stream_connect_free_fn_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let s = tokio::net::TcpStream::connect(a).await;\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn blocking_recv_reported_in_sync_test() {
        let src = "#[test]\nfn t() {\n    let v = rx.blocking_recv();\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn spawn_call_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let r = tokio::spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn spawn_blocking_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let r = tokio::task::spawn_blocking(f).await;\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn joinset_spawn_method_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let h = set.spawn(task).await;\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn spawned_handle_binding_await_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let h = tokio::spawn(work());\n    let r = h.await;\n}\n";
        assert_eq!(findings(src), vec![4]);
    }

    #[test]
    fn spawned_handle_from_method_spawn_await_reported() {
        let src =
            "#[tokio::test]\nasync fn t() {\n    let h = set.spawn(t1);\n    let r = h.await;\n}\n";
        assert_eq!(findings(src), vec![4]);
    }

    #[test]
    fn loop_with_await_reported_and_subsumes_waits() {
        let src = "#[tokio::test]\nasync fn t() {\n    loop {\n        let v = rx.recv().await;\n    }\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn nested_loop_subsumed_by_outer_loop_finding() {
        let src = "#[tokio::test]\nasync fn t() {\n    loop {\n        loop {\n            rx.recv().await;\n        }\n    }\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    // ---- boundedness --------------------------------------------------

    #[test]
    fn recv_inside_timeout_not_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let r = tokio::time::timeout(d, rx.recv()).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn recv_inside_aliased_timeout_not_reported() {
        let src = "use tokio::time::timeout as with_deadline;\n\n#[tokio::test]\nasync fn t() {\n    let r = with_deadline(d, rx.recv()).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn timeout_region_does_not_leak_to_later_waits() {
        let src = "#[tokio::test]\nasync fn t() {\n    let a = tokio::time::timeout(d, first.recv()).await;\n    let v = rx.recv().await;\n}\n";
        assert_eq!(findings(src), vec![4]);
    }

    #[test]
    fn glob_imported_timeout_bounds_no_inner_awaits() {
        // Glob imports are not resolved, so `timeout` creates no deadline
        // region: an awaited wait inside its async-block argument is
        // conservatively reported (over-reporting direction; a resolved
        // import would bound it).
        let src = "use tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    #[test]
    fn spawned_handle_inside_timeout_not_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let h = tokio::spawn(work());\n    let r = tokio::time::timeout(d, h).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn loop_inside_timeout_not_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let r = tokio::time::timeout(d, async {\n        loop {\n            rx.recv().await;\n        }\n    }).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn loop_with_per_iteration_timeout_not_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    loop {\n        match tokio::time::timeout(d, rx.recv()).await {\n            Ok(v) => { drop(v); }\n            Err(_) => break,\n        }\n    }\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn loop_without_await_not_reported() {
        let src =
            "#[tokio::test]\nasync fn t() {\n    loop {\n        if done() { break; }\n    }\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn recv_timeout_and_try_recv_not_reported() {
        let bounded = "#[tokio::test]\nasync fn t() {\n    let v = rx.recv_timeout(d).await;\n}\n";
        assert!(findings(bounded).is_empty());
        let nonblocking = "#[tokio::test]\nasync fn t() {\n    let v = rx.try_recv().await;\n}\n";
        assert!(findings(nonblocking).is_empty());
    }

    // ---- V1 boundary: stream I/O deferred ------------------------------

    #[test]
    fn stream_io_reads_writes_not_reported_in_v1() {
        let src = "#[tokio::test]\nasync fn t() {\n    let n = s.read(&mut b).await;\n    let w = s.write(b).await;\n    s.flush().await;\n}\n";
        assert!(findings(src).is_empty());
    }

    // ---- scope rules ---------------------------------------------------

    #[test]
    fn waits_in_closure_not_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    let f = |r: Rx| async move { r.recv().await };\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn waits_in_nested_fn_not_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    async fn helper(rx: Rx) {\n        rx.recv().await;\n    }\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn non_test_fn_ignored() {
        let src = "fn not_a_test() {\n    let v = rx.recv().await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn cfg_test_mod_depth_reported() {
        let src = "#[cfg(test)]\nmod tests {\n    #[tokio::test]\n    async fn t() {\n        rx.recv().await;\n    }\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    #[test]
    fn shadowed_spawn_fn_not_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    fn spawn<T>(f: T) -> Result<T, ()> { Ok(f) }\n    let r = spawn(work()).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    // ---- escape hatch ---------------------------------------------------

    #[test]
    fn allow_test_wait_marker_suppresses() {
        let src = "#[tokio::test]\nasync fn t() {\n    let v = rx.recv().await; // allow-test-wait: bounded by caller contract\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn empty_allow_marker_does_not_suppress() {
        let src =
            "#[tokio::test]\nasync fn t() {\n    let v = rx.recv().await; // allow-test-wait:\n}\n";
        assert_eq!(findings(src), vec![3]);
    }

    #[test]
    fn marker_on_loop_line_suppresses_loop() {
        let src = "#[tokio::test]\nasync fn t() {\n    loop { // allow-test-wait: service loop, teardown-bounded below\n        rx.recv().await;\n    }\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn multiline_wait_marker_on_last_line_suppresses() {
        let src = "#[tokio::test]\nasync fn t() {\n    let v = rx\n        .recv()\n        .await; // allow-test-wait: handshake completes or test hangs by design\n}\n";
        assert!(findings(src).is_empty());
    }

    // ---- error propagation ----------------------------------------------

    #[test]
    fn parse_error_propagates() {
        let err = match scan_source("fn broken( {", Path::new("fixture.rs")) {
            Err(ScanError::Parse { .. }) => "parse",
            _ => panic!("expected parse error"),
        };
        assert_eq!(err, "parse");
    }

    // ---- run() integration ----------------------------------------------

    /// Write a ratchet ceiling file into a throwaway workspace root
    /// (mirrors `seed_ratchet` in lint_test_sleep.rs).
    fn seed_ratchet(root: &Path, max: usize) {
        let xtask = root.join("scripts").join("xtask");
        std::fs::create_dir_all(&xtask).unwrap(); // allow-unwrap
        std::fs::write(xtask.join(RATCHET_FILE), format!("# ratchet\n{max}\n")).unwrap(); // allow-unwrap
    }

    #[test]
    fn run_walks_member_trees_and_counts_findings() {
        let dir = std::env::temp_dir().join(format!(
            "xtask-lint-unbounded-wait-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap() // allow-unwrap
                .subsec_nanos()
        ));
        let src_dir = dir.join("crates/foo/tests");
        std::fs::create_dir_all(&src_dir).unwrap(); // allow-unwrap
        std::fs::write(
            src_dir.join("t.rs"),
            "#[tokio::test]\nasync fn t() {\n    rx.recv().await;\n}\n",
        )
        .unwrap(); // allow-unwrap
        seed_ratchet(&dir, 5);
        let Report { findings, max } = run(&dir).expect("scan ok");
        assert_eq!(max, 5);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].1.line, 3);
        std::fs::remove_dir_all(&dir).unwrap(); // allow-unwrap
    }

    #[test]
    fn run_missing_ratchet_fails_closed() {
        let dir = std::env::temp_dir().join("xtask-lint-unbounded-wait-none");
        std::fs::create_dir_all(&dir).unwrap(); // allow-unwrap
        let err = run(&dir).expect_err("missing ratchet must fail");
        assert!(err.to_string().contains(RATCHET_FILE));
        std::fs::remove_dir_all(&dir).unwrap(); // allow-unwrap
    }

    #[test]
    fn run_malformed_ratchet_fails_closed() {
        let dir = std::env::temp_dir().join("xtask-lint-unbounded-wait-bad");
        std::fs::create_dir_all(&dir).unwrap(); // allow-unwrap
        seed_ratchet_raw(&dir, "# only comments\n");
        let err = run(&dir).expect_err("malformed ratchet must fail");
        assert!(err.to_string().contains("one integer line"));
        std::fs::remove_dir_all(&dir).unwrap(); // allow-unwrap
    }

    fn seed_ratchet_raw(root: &Path, content: &str) {
        let xtask = root.join("scripts").join("xtask");
        std::fs::create_dir_all(&xtask).unwrap(); // allow-unwrap
        std::fs::write(xtask.join(RATCHET_FILE), content).unwrap(); // allow-unwrap
    }

    #[test]
    fn run_skips_absent_member_roots() {
        let dir = std::env::temp_dir().join(format!(
            "xtask-lint-unbounded-wait-absent-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap() // allow-unwrap
                .subsec_nanos()
        ));
        seed_ratchet(&dir, 3);
        let Report { findings, max } = run(&dir).expect("scan ok");
        assert_eq!(max, 3);
        assert!(findings.is_empty());
        std::fs::remove_dir_all(&dir).unwrap(); // allow-unwrap
    }
}
