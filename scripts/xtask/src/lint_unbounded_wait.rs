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
//! (`fut.timeout(d)` via `FutureExt`) and custom bounded helpers: none of
//! them create a deadline region, so sites they bound need an
//! `allow-test-wait` marker until a later revision learns them. Waits
//! embedded in macro bodies (`tokio::select!` / `join!` / `try_join!`
//! arms) are invisible — `syn` exposes a macro call as an opaque token
//! stream — so a `select!` recv arm is neither flagged nor bounded.
//! Glob imports (`use tokio::time::*`) are resolved through candidate-set
//! expansion (see Resolution).
//!
//! The scaffolding (imports chain, scope model of terminal body items
//! and non-terminal locals, scan_items, ratchet read, workspace walk)
//! is a deliberate third copy of the `lint_test_sleep` pattern (after
//! `lint_cancel_tokens`); extract a shared module before a fourth lint
//! duplicates it again.
//!
//! # Resolution
//!
//! Path resolution uses a candidate-set walk: imports, module aliases
//! (`extern crate` renames), transitive alias targets, and glob roots are
//! all expanded. A leading `::` (absolute marker) bypasses all alias
//! rewriting. Only namespace-known items (`fn`/`const`/`static`/
//! tuple-or-unit struct; `mod`/type items) terminate the walk; imports
//! are never terminal (their namespace is unknowable at `syn` level, so
//! they always contribute candidates and continue). Ambiguity never
//! bounds (a non-singleton candidate set cannot prove a timeout target)
//! and never suppresses a wait finding (any matching candidate fires
//! detection). Scope-aware body bindings replace the old fn-wide shadow
//! note: terminal body items (fn/const/static/struct items directly in
//! the fn block) terminate for the whole body; non-terminal locals
//! (let/closure/for/match-arm bindings, nested-block items) add a
//! `<local>` candidate but the walk continues so a late shadow cannot
//! suppress an earlier awaited import call.
//!
//! # Boundedness
//!
//! A wait is BOUNDED when its span lies inside the future argument (2nd
//! argument) of a `tokio::time::timeout` / `tokio::time::timeout_at`
//! call (qualified paths or resolved through module `use` imports,
//! aliases and globs included). Bounding uses three precedence rules
//! over the candidate set (excluding bare-name artifacts):
//!
//! 1. **Named rule:** a single `Named` candidate from `body_top` or the
//!    module chain (not `body_nested`) that is a timeout target bounds
//!    when no other `Named` candidate exists, no `Local` reading exists,
//!    and no `Glob` candidate sits in a strictly inner scope.
//! 2. **Literal rule:** a `Literal` candidate (the as-written qualified
//!    path) that is a timeout target bounds when no `Named` candidate
//!    and no `Local` reading exist. `Glob` candidates do not block.
//! 3. **Glob rule:** when every non-artifact candidate is `Glob`-derived,
//!    the set bounds iff it is a singleton timeout target.
//!
//! Otherwise the wait is unbounded (reported). A `Local` reading always
//! blocks bounding. The rule-2 relaxation (glob does not block literal)
//! is a documented decision: a glob can displace the extern-crate reading
//! only by exporting a same-named module — an identity re-export resolves
//! to the same crate; a non-identity masquerade is corpus-zero and
//! adversarial-only. Detection is immune (any-hit).
//!
//! A `loop` additionally counts as bounded when ANY `timeout` call site
//! lies inside its subtree (the per-iteration deadline pattern:
//! `loop { match timeout(d, rx.recv()).await { .. } }`). When a `loop`
//! is reported, contained wait findings are subsumed (one defect, one
//! ratchet entry).
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

/// Collected import and item information for one scope.
///
/// `named` is a multimap: same-scope same-name imports push; never overwrite.
/// `items_value` = fn/const/static + tuple/unit struct constructors.
/// `items_type` = struct/enum/union/trait/type/mod.
/// `globs` = glob root prefixes (e.g. `"tokio::time"` for `use tokio::time::*`).
#[derive(Default, Clone)]
struct Imports {
    named: HashMap<String, Vec<String>>,
    items_value: HashSet<String>,
    items_type: HashSet<String>,
    globs: Vec<String>,
}

impl Imports {
    fn new() -> Self {
        Self::default()
    }
}

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

/// Provenance of a candidate reading, used for precedence-rule bounding.
#[derive(Debug, Clone, Copy)]
enum Provenance {
    /// Explicit import reading (from body_top, module chain, or body_nested).
    Named {
        depth: usize,
        from_body_top_or_chain: bool,
    },
    /// Glob-derived reading.
    Glob { depth: usize },
    /// The original/extern joined path of a qualified call.
    Literal,
    /// A non-terminal local binding (let/closure/for/etc.).
    Local,
    /// Bare as-written token (detection aid, excluded from bounding).
    BareArtifact,
}

/// A candidate reading with its provenance.
#[derive(Debug, Clone)]
struct Candidate {
    path: String,
    provenance: Provenance,
}

/// What one scope contributed to the candidate walk (see
/// [`ResolvesPaths::push_scope_readings`]).
#[derive(Default)]
struct ScopeContribution {
    /// The scope had a named import binding for the looked-up name.
    had_named_binding: bool,
    /// A relative-qualified glob (`crate`/`super`/`self`) contributed.
    relative_glob_contributed: bool,
    /// An extern-anchored glob contributed.
    absolute_glob_contributed: bool,
    /// A namespace-known item terminated the walk at this scope.
    terminal: bool,
}

/// Flatten body_nested scopes into a single [`Imports`] for expansion.
fn flatten_body_nested(body_nested: &[(Imports, usize)]) -> Imports {
    let mut result = Imports::new();
    for (imports, _) in body_nested {
        for (name, targets) in &imports.named {
            result
                .named
                .entry(name.clone())
                .or_default()
                .extend(targets.clone());
        }
        result.globs.extend(imports.globs.clone());
    }
    result
}

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

/// Walk a use tree, populating an [`Imports`] multimap.
///
/// `absolute` is true when the containing `use` statement has a leading `::`,
/// causing targets to be prefixed with `"::"`.
fn collect_use_tree(tree: &syn::UseTree, prefix: String, imports: &mut Imports, absolute: bool) {
    match tree {
        syn::UseTree::Path(p) => {
            let next = if prefix.is_empty() {
                if absolute {
                    format!("::{}", p.ident)
                } else {
                    p.ident.to_string()
                }
            } else {
                format!("{prefix}::{}", p.ident)
            };
            collect_use_tree(&p.tree, next, imports, false);
        }
        syn::UseTree::Name(n) => {
            let target = if prefix.is_empty() {
                if absolute {
                    format!("::{}", n.ident)
                } else {
                    n.ident.to_string()
                }
            } else {
                format!("{prefix}::{}", n.ident)
            };
            imports
                .named
                .entry(n.ident.to_string())
                .or_default()
                .push(target);
        }
        syn::UseTree::Rename(r) => {
            let target = if prefix.is_empty() {
                if absolute {
                    format!("::{}", r.ident)
                } else {
                    r.ident.to_string()
                }
            } else {
                format!("{prefix}::{}", r.ident)
            };
            imports
                .named
                .entry(r.rename.to_string())
                .or_default()
                .push(target);
        }
        syn::UseTree::Group(g) => {
            for inner in &g.items {
                collect_use_tree(inner, prefix.clone(), imports, absolute);
            }
        }
        syn::UseTree::Glob(_) => {
            if !prefix.is_empty() || absolute {
                let glob_root = if prefix.is_empty() {
                    "::".to_string()
                } else {
                    prefix.clone()
                };
                imports.globs.push(glob_root);
            }
        }
    }
}

/// Register one module-level item into the imports map.
fn collect_item(item: &Item, imports: &mut Imports) {
    match item {
        Item::Use(use_item) => {
            let absolute = use_item.leading_colon.is_some();
            collect_use_tree(&use_item.tree, String::new(), imports, absolute);
        }
        Item::Fn(f) => {
            imports.items_value.insert(f.sig.ident.to_string());
        }
        Item::Const(c) => {
            imports.items_value.insert(c.ident.to_string());
        }
        Item::Static(s) => {
            imports.items_value.insert(s.ident.to_string());
        }
        Item::Struct(s) => {
            imports.items_type.insert(s.ident.to_string());
            if matches!(s.fields, syn::Fields::Unnamed(_) | syn::Fields::Unit) {
                imports.items_value.insert(s.ident.to_string());
            }
        }
        Item::Enum(e) => {
            imports.items_type.insert(e.ident.to_string());
        }
        Item::Union(u) => {
            imports.items_type.insert(u.ident.to_string());
        }
        Item::Trait(t) => {
            imports.items_type.insert(t.ident.to_string());
        }
        Item::Type(t) => {
            imports.items_type.insert(t.ident.to_string());
        }
        Item::Mod(m) => {
            imports.items_type.insert(m.ident.to_string());
        }
        Item::ExternCrate(e) => {
            if let Some((_, rename)) = &e.rename {
                imports
                    .named
                    .entry(rename.to_string())
                    .or_default()
                    .push(format!("::{}", e.ident));
            }
        }
        _ => {}
    }
}

/// Collect imports and items from a list of module-level items.
fn collect_imports(items: &[Item], imports: &mut Imports) {
    for item in items {
        collect_item(item, imports);
    }
}

/// Extract the defining name from an item, if it has one.
fn item_name(item: &Item) -> Option<String> {
    match item {
        Item::Fn(f) => Some(f.sig.ident.to_string()),
        Item::Const(c) => Some(c.ident.to_string()),
        Item::Static(s) => Some(s.ident.to_string()),
        Item::Struct(s) => Some(s.ident.to_string()),
        Item::Enum(e) => Some(e.ident.to_string()),
        Item::Union(u) => Some(u.ident.to_string()),
        Item::Trait(t) => Some(t.ident.to_string()),
        Item::Type(t) => Some(t.ident.to_string()),
        Item::Mod(m) => Some(m.ident.to_string()),
        Item::ExternCrate(e) => Some(
            e.rename
                .as_ref()
                .map(|(_, ident)| ident.to_string())
                .unwrap_or_else(|| e.ident.to_string()),
        ),
        _ => None,
    }
}

/// Expand a path through named imports and glob roots, returning all
/// possible resolved readings tagged with the scope depth of the glob
/// that produced them (`Some(depth)`) or `None` for explicit
/// (named-target / as-written) readings. Memoized and cycle-safe
/// within a single call.
fn expand_readings(
    path: &str,
    chain: &[Imports],
    body_top: &Imports,
    body_nested: &[(Imports, usize)],
) -> Vec<(String, Option<usize>)> {
    if path.starts_with("::") {
        return vec![(path.to_string(), None)];
    }
    let flat = flatten_body_nested(body_nested);
    // Flattened body_nested globs sit at the deepest scope; use a depth
    // beyond every real block-nesting depth so their products order
    // deeper than body_top (500) and the module chain (< 500).
    let nested_depth = 1000 + body_nested.iter().map(|(_, d)| *d).max().unwrap_or(0);
    let mut memo: HashMap<String, Vec<(String, Option<usize>)>> = HashMap::new();
    let visited: HashSet<String> = HashSet::new();
    expand_inner(
        path,
        chain,
        body_top,
        &flat,
        nested_depth,
        &mut memo,
        &visited,
    )
}

fn expand_inner(
    path: &str,
    chain: &[Imports],
    body_top: &Imports,
    body_nested: &Imports,
    nested_depth: usize,
    memo: &mut HashMap<String, Vec<(String, Option<usize>)>>,
    visited: &HashSet<String>,
) -> Vec<(String, Option<usize>)> {
    if let Some(cached) = memo.get(path) {
        return cached.clone();
    }

    // splitn(2, "..") always yields at least one part, but spell the
    // invariant out instead of unwrapping (lint-unwrap discipline).
    let mut parts = path.splitn(2, "::");
    let leading = parts.next().unwrap_or_default();
    debug_assert!(!leading.is_empty(), "splitn yields a first part");
    let rest = parts.next();

    // Cycle detection
    if visited.contains(leading) {
        // Cycle-truncated reading: memoizing it would leak this
        // branch's truncation into later, cycle-free branches.
        return vec![(path.to_string(), None)];
    }

    let mut new_visited = visited.clone();
    new_visited.insert(leading.to_string());

    let mut result: Vec<(String, Option<usize>)> = Vec::new();

    // Look up in named maps (body_nested first, then body_top, then chain reversed)
    let mut found = false;

    for imports in std::iter::once(body_nested)
        .chain(std::iter::once(body_top))
        .chain(chain.iter().rev())
    {
        if let Some(targets) = imports.named.get(leading) {
            found = true;
            for target in targets {
                let new_path = match rest {
                    Some(r) => format!("{target}::{r}"),
                    None => target.clone(),
                };
                let branch_visited = new_visited.clone();
                result.extend(expand_inner(
                    &new_path,
                    chain,
                    body_top,
                    body_nested,
                    nested_depth,
                    memo,
                    &branch_visited,
                ));
            }
        }
    }

    // If not found in named maps, branch through globs — but skip when
    // the leading segment is already a known crate name (first segment of
    // any resolved glob root).
    if !found {
        // Globs carry their scope depth so glob-derived expansion
        // products can block the named precedence rule from inner scopes.
        let scoped_globs: Vec<(&String, usize)> = body_nested
            .globs
            .iter()
            .map(|g| (g, nested_depth))
            .chain(body_top.globs.iter().map(|g| (g, 500)))
            .chain(
                chain
                    .iter()
                    .enumerate()
                    .flat_map(|(i, s)| s.globs.iter().map(move |g| (g, i + 1))),
            )
            .collect();

        if scoped_globs.is_empty() {
            result.push((path.to_string(), None));
        } else {
            // Collect known crate names from resolved glob roots
            let known_firsts: HashSet<String> = scoped_globs
                .iter()
                .flat_map(|(g, _)| resolve_glob_root(g, chain, body_top, body_nested))
                .filter_map(|r| r.split("::").next().map(|s| s.to_string()))
                .collect();

            if known_firsts.contains(leading) {
                // Leading segment is a known crate name; emit as-is
                result.push((path.to_string(), None));
            } else {
                // Leading segment is not a known crate name (first
                // segment of any resolved glob root); emit the original
                // path as a fallback (extern crate / crate root reference) AND
                // branch through globs for additional candidate readings.
                // Branch products are GLOB-derived: they must never count as
                // the explicit named reading of the bounding rule.
                result.push((path.to_string(), None));
                for (glob, gdepth) in &scoped_globs {
                    for resolved in resolve_glob_root(glob, chain, body_top, body_nested) {
                        if leading == *glob {
                            // Leading segment is the glob root itself; replace it
                            if let Some(r) = rest {
                                result.push((format!("{resolved}::{r}"), Some(*gdepth)));
                            } else {
                                result.push((resolved.clone(), Some(*gdepth)));
                            }
                        } else {
                            // Prepend resolved root to the leading segment
                            if let Some(r) = rest {
                                result.push((format!("{resolved}::{leading}::{r}"), Some(*gdepth)));
                            } else {
                                result.push((format!("{resolved}::{leading}"), Some(*gdepth)));
                            }
                        }
                    }
                }
            }
        }
    }

    // Deduplicate
    let mut seen = HashSet::new();
    result.retain(|s| seen.insert(s.0.clone()));

    // Only memoize root-context expansions: a result computed under a
    // branch-local `visited` set can be cycle-truncated, and reusing it
    // in another branch would drop readings (false negatives).
    if visited.is_empty() {
        memo.insert(path.to_string(), result.clone());
    }
    result
}

/// Resolve a glob root through named maps, unioning across all scopes
/// (body_nested → body_top → chain). Returns all resolved roots; if no
/// named binding exists for the first segment, returns the glob root as-is.
fn resolve_glob_root(
    glob: &str,
    chain: &[Imports],
    body_top: &Imports,
    body_nested: &Imports,
) -> Vec<String> {
    let segments: Vec<&str> = glob.split("::").collect();
    if segments.is_empty() {
        return vec![glob.to_string()];
    }

    let first = segments[0];
    let rest = &segments[1..];

    // Union readings across all scopes (not first-hit)
    let mut result = HashSet::new();
    for imports in std::iter::once(body_nested)
        .chain(std::iter::once(body_top))
        .chain(chain.iter().rev())
    {
        if let Some(targets) = imports.named.get(first) {
            for target in targets {
                if rest.is_empty() {
                    result.insert(target.clone());
                } else {
                    result.insert(format!("{target}::{}", rest.join("::")));
                }
            }
        }
    }

    if result.is_empty() {
        vec![glob.to_string()]
    } else {
        result.into_iter().collect()
    }
}

/// Shared path resolution for one test fn via candidate-set walk.
trait ResolvesPaths {
    fn chain(&self) -> &[Imports];
    fn body_top(&self) -> &Imports;
    fn body_nested(&self) -> &[(Imports, usize)];
    fn non_terminal_locals(&self) -> &HashSet<String>;

    /// Push the candidate readings for `name` contributed by one
    /// scope's import map, and report what the scope contributed.
    /// `prov_depth` and `from_body_top_or_chain` tag `Named` provenance
    /// for the bounding precedence rules; `terminal_items` runs the
    /// namespace-known-item termination check when given (`Some` for
    /// body_top / chain scopes, `None` for the non-terminal
    /// body_nested scopes). The cross-scope bare-artifact fallback is
    /// deliberately not part of this helper.
    fn push_scope_readings(
        &self,
        imports: &Imports,
        prov_depth: usize,
        from_body_top_or_chain: bool,
        terminal_items: Option<&HashSet<String>>,
        name: &str,
        out: &mut Vec<Candidate>,
    ) -> ScopeContribution {
        /// True when a glob path starts with a relative qualifier
        /// (`crate`, `super`, `self`) — such globs cannot reach the
        /// extern prelude, so the original name reading must be retained.
        fn is_relative_glob(glob: &str) -> bool {
            matches!(
                glob.split("::").next().unwrap_or(""),
                "crate" | "super" | "self"
            )
        }

        let mut contribution = ScopeContribution::default();

        if let Some(targets) = imports.named.get(name) {
            contribution.had_named_binding = true;
            for target in targets {
                for (expanded, glob_depth) in
                    expand_readings(target, self.chain(), self.body_top(), self.body_nested())
                {
                    // Glob-branched expansion products carry glob
                    // provenance; only direct readings are Named.
                    let provenance = match glob_depth {
                        Some(d) => Provenance::Glob { depth: d },
                        None => Provenance::Named {
                            depth: prov_depth,
                            from_body_top_or_chain,
                        },
                    };
                    out.push(Candidate {
                        path: expanded,
                        provenance,
                    });
                }
            }
        }

        // Namespace-known items are terminal: the walk stops after
        // this scope (no glob candidates from it, no later scopes).
        if let Some(items) = terminal_items
            && items.contains(name)
        {
            out.push(Candidate {
                path: "<local>".to_string(),
                provenance: Provenance::Local,
            });
            contribution.terminal = true;
            return contribution;
        }

        // Flatten body_nested once for resolve_glob_root
        if !imports.globs.is_empty() {
            let flat_bn = flatten_body_nested(self.body_nested());
            for glob in &imports.globs {
                for resolved in resolve_glob_root(glob, self.chain(), self.body_top(), &flat_bn) {
                    if name == glob.as_str() {
                        out.push(Candidate {
                            path: resolved.clone(),
                            provenance: Provenance::Glob { depth: prov_depth },
                        });
                        if is_relative_glob(glob) {
                            contribution.relative_glob_contributed = true;
                        } else {
                            contribution.absolute_glob_contributed = true;
                        }
                    } else if resolved.split("::").next().unwrap_or("") != name {
                        out.push(Candidate {
                            path: format!("{resolved}::{name}"),
                            provenance: Provenance::Glob { depth: prov_depth },
                        });
                        if is_relative_glob(glob) {
                            contribution.relative_glob_contributed = true;
                        } else {
                            contribution.absolute_glob_contributed = true;
                        }
                    }
                }
            }
        }

        contribution
    }

    /// Build the candidate set for `name`.
    /// `value_ns` = true for single-segment (value) lookups, false for qualified first-segment.
    fn candidates(&self, name: &str, value_ns: bool) -> Vec<Candidate> {
        let mut result = Vec::new();
        let mut had_named_binding = false;
        let mut relative_glob_contributed = false;
        let mut absolute_glob_contributed = false;

        // 1. non_terminal_locals (value lookups only)
        if value_ns && self.non_terminal_locals().contains(name) {
            result.push(Candidate {
                path: "<local>".to_string(),
                provenance: Provenance::Local,
            });
        }

        // 2. body_nested scopes (non-terminal: named + globs, no items check)
        for (imports, depth) in self.body_nested() {
            let contribution =
                self.push_scope_readings(imports, *depth + 1000, false, None, name, &mut result);
            had_named_binding |= contribution.had_named_binding;
            relative_glob_contributed |= contribution.relative_glob_contributed;
            absolute_glob_contributed |= contribution.absolute_glob_contributed;
        }

        // 3. body_top + chain (unified: named + items terminal + globs)
        // body_top
        {
            let scope = self.body_top();
            let items = if value_ns {
                &scope.items_value
            } else {
                &scope.items_type
            };
            let contribution =
                self.push_scope_readings(scope, 500, true, Some(items), name, &mut result);
            had_named_binding |= contribution.had_named_binding;
            relative_glob_contributed |= contribution.relative_glob_contributed;
            absolute_glob_contributed |= contribution.absolute_glob_contributed;
            if contribution.terminal {
                return result; // TERMINAL
            }
        }

        // chain
        for (rev_index, scope) in self.chain().iter().rev().enumerate() {
            let prov_depth = self.chain().len() - rev_index;
            let items = if value_ns {
                &scope.items_value
            } else {
                &scope.items_type
            };
            let contribution =
                self.push_scope_readings(scope, prov_depth, true, Some(items), name, &mut result);
            had_named_binding |= contribution.had_named_binding;
            relative_glob_contributed |= contribution.relative_glob_contributed;
            absolute_glob_contributed |= contribution.absolute_glob_contributed;
            if contribution.terminal {
                return result; // TERMINAL
            }
        }

        // When any glob (relative or extern-anchored) contributed candidates
        // and no named binding was found, also retain the original name reading
        // (extern-prelude / as-written). Without this, an extern-anchored glob
        // like `use tracing_subscriber::prelude::*;` swallows the literal
        // reading of "tokio" — candidates become
        // {tracing_subscriber::prelude::tokio} and the real tokio::spawn is
        // lost. The self-prefix guard (skip when name equals the glob root's
        // first segment) already protects the `use tokio::*;` + name "tokio"
        // self-reference case.
        if (relative_glob_contributed || absolute_glob_contributed) && !had_named_binding {
            result.push(Candidate {
                path: name.to_string(),
                provenance: Provenance::BareArtifact,
            });
        }

        result
    }

    /// True when `path` denotes a timeout target (bounding).
    fn is_bounding_path(&self, path: &syn::Path) -> bool {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let qualified = segments.join("::");

        // Leading colon bypasses aliases
        if path.leading_colon.is_some() {
            return TIMEOUT_TARGETS.contains(&qualified.as_str());
        }

        let candidates: Vec<Candidate> = if segments.len() == 1 {
            self.candidates(&segments[0], true)
        } else {
            let first_candidates = self.candidates(&segments[0], false);
            if first_candidates.is_empty() {
                // Literal fallback: the original/extern joined path
                vec![Candidate {
                    path: qualified.clone(),
                    provenance: Provenance::Literal,
                }]
            } else {
                let rest = segments[1..].join("::");
                first_candidates
                    .into_iter()
                    .map(|c| Candidate {
                        path: format!("{}::{}", c.path, rest),
                        // The as-written first segment joined with the rest
                        // IS the literal/extern joined path (rule 2 reading).
                        // Only bare single-segment calls keep the
                        // BareArtifact token — it never resolves.
                        provenance: if matches!(c.provenance, Provenance::BareArtifact) {
                            Provenance::Literal
                        } else {
                            c.provenance
                        },
                    })
                    .collect()
            }
        };

        // Filter out BareArtifact candidates for bounding
        let non_bare: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| !matches!(c.provenance, Provenance::BareArtifact))
            .collect();

        // Local reading blocks all rules
        if non_bare
            .iter()
            .any(|c| matches!(c.provenance, Provenance::Local))
        {
            return false;
        }

        // Collect by provenance
        let named: Vec<&Candidate> = non_bare
            .iter()
            .filter(|c| matches!(c.provenance, Provenance::Named { .. }))
            .copied()
            .collect();
        let globs: Vec<&Candidate> = non_bare
            .iter()
            .filter(|c| matches!(c.provenance, Provenance::Glob { .. }))
            .copied()
            .collect();
        let literals: Vec<&Candidate> = non_bare
            .iter()
            .filter(|c| matches!(c.provenance, Provenance::Literal))
            .copied()
            .collect();

        // Rule 1: single Named from body_top/chain, no inner glob, no other Named
        if named.len() == 1
            && let Provenance::Named {
                depth,
                from_body_top_or_chain,
            } = named[0].provenance
            && from_body_top_or_chain
        {
            let c = named[0];
            let glob_blocks = globs.iter().any(|g| {
                if let Provenance::Glob { depth: g_depth } = g.provenance {
                    g_depth > depth
                } else {
                    false
                }
            });
            if !glob_blocks {
                let normalized = c.path.strip_prefix("::").unwrap_or(&c.path);
                if TIMEOUT_TARGETS.contains(&normalized) {
                    return true;
                }
            }
        }

        // Rule 2: Literal candidate bounds when no Named and no Local
        if named.is_empty() && !literals.is_empty() {
            for c in &literals {
                let normalized = c.path.strip_prefix("::").unwrap_or(&c.path);
                if TIMEOUT_TARGETS.contains(&normalized) {
                    return true;
                }
            }
        }

        // Rule 3: all non-artifact candidates are Glob-derived, singleton
        if named.is_empty() && literals.is_empty() && !globs.is_empty() {
            let all_glob = non_bare
                .iter()
                .all(|c| matches!(c.provenance, Provenance::Glob { .. }));
            if all_glob && globs.len() == 1 {
                let c = globs[0];
                let normalized = c.path.strip_prefix("::").unwrap_or(&c.path);
                if TIMEOUT_TARGETS.contains(&normalized) {
                    return true;
                }
            }
        }

        false
    }

    /// True when `path` denotes a wait-call target (detection).
    fn is_wait_path(&self, path: &syn::Path) -> bool {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let qualified = segments.join("::");

        if path.leading_colon.is_some() {
            return WAIT_CALL_TARGETS.contains(&qualified.as_str());
        }

        let candidates: Vec<Candidate> = if segments.len() == 1 {
            self.candidates(&segments[0], true)
        } else {
            let first_candidates = self.candidates(&segments[0], false);
            if first_candidates.is_empty() {
                return WAIT_CALL_TARGETS.contains(&qualified.as_str());
            }
            let rest = segments[1..].join("::");
            first_candidates
                .into_iter()
                .map(|c| Candidate {
                    path: format!("{}::{}", c.path, rest),
                    provenance: c.provenance,
                })
                .collect()
        };

        candidates.iter().any(|c| {
            let normalized = c.path.strip_prefix("::").unwrap_or(&c.path);
            WAIT_CALL_TARGETS.contains(&normalized)
        })
    }
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

/// Collect body_nested imports and non_terminal_locals from a test fn body.
///
/// depth=0 means we're in the fn block itself (direct statements).
/// depth>0 means we're inside a nested block.
struct BodyNestedCollector<'a> {
    body_nested: &'a mut Vec<(Imports, usize)>,
    non_terminal_locals: &'a mut HashSet<String>,
    depth: usize,
}

impl Visit<'_> for BodyNestedCollector<'_> {
    fn visit_block(&mut self, block: &syn::Block) {
        // Collect use statements from nested blocks only (depth > 0),
        // one Imports entry per block scope with its nesting depth.
        if self.depth > 0 {
            let mut scope_imports = Imports::new();
            for stmt in &block.stmts {
                if let syn::Stmt::Item(Item::Use(use_item)) = stmt {
                    let absolute = use_item.leading_colon.is_some();
                    collect_use_tree(&use_item.tree, String::new(), &mut scope_imports, absolute);
                }
            }
            if !scope_imports.named.is_empty() || !scope_imports.globs.is_empty() {
                self.body_nested.push((scope_imports, self.depth));
            }
        }
        visit::visit_block(self, block);
    }

    fn visit_expr_block(&mut self, e: &syn::ExprBlock) {
        self.depth += 1;
        visit::visit_expr_block(self, e);
        self.depth -= 1;
    }

    fn visit_expr_if(&mut self, e: &syn::ExprIf) {
        self.visit_expr(&e.cond);
        self.depth += 1;
        self.visit_block(&e.then_branch);
        self.depth -= 1;
        if let Some((_, else_branch)) = &e.else_branch {
            self.visit_expr(else_branch);
        }
    }

    fn visit_expr_loop(&mut self, e: &syn::ExprLoop) {
        self.depth += 1;
        self.visit_block(&e.body);
        self.depth -= 1;
    }

    fn visit_expr_for_loop(&mut self, e: &syn::ExprForLoop) {
        PatIdents {
            names: self.non_terminal_locals,
        }
        .visit_pat(&e.pat);
        self.visit_expr(&e.expr);
        self.depth += 1;
        self.visit_block(&e.body);
        self.depth -= 1;
    }

    fn visit_expr_while(&mut self, e: &syn::ExprWhile) {
        self.visit_expr(&e.cond);
        self.depth += 1;
        self.visit_block(&e.body);
        self.depth -= 1;
    }

    fn visit_expr_async(&mut self, e: &syn::ExprAsync) {
        self.depth += 1;
        self.visit_block(&e.block);
        self.depth -= 1;
    }

    fn visit_expr_try_block(&mut self, e: &syn::ExprTryBlock) {
        self.depth += 1;
        self.visit_block(&e.block);
        self.depth -= 1;
    }

    fn visit_expr_unsafe(&mut self, e: &syn::ExprUnsafe) {
        self.depth += 1;
        self.visit_block(&e.block);
        self.depth -= 1;
    }

    fn visit_expr_const(&mut self, e: &syn::ExprConst) {
        self.depth += 1;
        self.visit_block(&e.block);
        self.depth -= 1;
    }

    fn visit_local(&mut self, local: &syn::Local) {
        PatIdents {
            names: self.non_terminal_locals,
        }
        .visit_pat(&local.pat);
        visit::visit_local(self, local);
    }

    fn visit_expr_closure(&mut self, c: &syn::ExprClosure) {
        for arg in &c.inputs {
            PatIdents {
                names: self.non_terminal_locals,
            }
            .visit_pat(arg);
        }
        // Bump depth so closures count as nesting (use statements inside
        // closures reach body_nested, matching the over-report direction).
        self.depth += 1;
        visit::visit_expr_closure(self, c);
        self.depth -= 1;
    }

    fn visit_expr_let(&mut self, e: &syn::ExprLet) {
        PatIdents {
            names: self.non_terminal_locals,
        }
        .visit_pat(&e.pat);
        visit::visit_expr_let(self, e);
    }

    fn visit_arm(&mut self, arm: &syn::Arm) {
        PatIdents {
            names: self.non_terminal_locals,
        }
        .visit_pat(&arm.pat);
        visit::visit_arm(self, arm);
    }

    fn visit_item(&mut self, item: &Item) {
        if self.depth > 0
            && let Some(name) = item_name(item)
        {
            self.non_terminal_locals.insert(name);
        }
        match item {
            Item::Fn(f) => {
                self.depth += 1;
                visit::visit_item_fn(self, f);
                self.depth -= 1;
            }
            Item::Mod(m) => {
                if let Some(content) = &m.content {
                    self.depth += 1;
                    for item in &content.1 {
                        self.visit_item(item);
                    }
                    self.depth -= 1;
                }
            }
            _ => {}
        }
    }
}

fn scan_test_fn(f: &ItemFn, chain: &[Imports], lines: &[&str], findings: &mut Vec<Finding>) {
    // Collect body_top from direct statements via collect_item
    let mut body_top = Imports::new();
    for stmt in &f.block.stmts {
        if let syn::Stmt::Item(item) = stmt {
            collect_item(item, &mut body_top);
        }
    }

    // Collect body_nested (per-block-scope) and non_terminal_locals
    let mut body_nested: Vec<(Imports, usize)> = Vec::new();
    let mut non_terminal_locals = HashSet::new();
    BodyNestedCollector {
        body_nested: &mut body_nested,
        non_terminal_locals: &mut non_terminal_locals,
        depth: 0,
    }
    .visit_block(f.block.as_ref());

    // Pass 1: SpawnCollector — find bindings whose initializer spawns
    let mut spawned = HashSet::new();
    SpawnCollector {
        chain,
        body_top: &body_top,
        body_nested: &body_nested,
        non_terminal_locals: &non_terminal_locals,
        names: &mut spawned,
    }
    .visit_block(f.block.as_ref());

    // Pass 2: TimeoutCollector — collect deadline regions
    let mut regions = Vec::new();
    TimeoutCollector {
        chain,
        body_top: &body_top,
        body_nested: &body_nested,
        non_terminal_locals: &non_terminal_locals,
        future_regions: &mut regions,
    }
    .visit_block(f.block.as_ref());

    // Pass 3: WaitFinder — report unbounded waits
    let mut finder = WaitFinder {
        chain,
        body_top: &body_top,
        body_nested: &body_nested,
        non_terminal_locals: &non_terminal_locals,
        spawned: &spawned,
        future_regions: &regions,
        loop_spans: Vec::new(),
        lines,
        findings: Vec::new(),
    };
    finder.visit_block(f.block.as_ref());
    findings.append(&mut finder.findings);
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

struct SpawnCollector<'a> {
    chain: &'a [Imports],
    body_top: &'a Imports,
    body_nested: &'a [(Imports, usize)],
    non_terminal_locals: &'a HashSet<String>,
    names: &'a mut HashSet<String>,
}

impl ResolvesPaths for SpawnCollector<'_> {
    fn chain(&self) -> &[Imports] {
        self.chain
    }
    fn body_top(&self) -> &Imports {
        self.body_top
    }
    fn body_nested(&self) -> &[(Imports, usize)] {
        self.body_nested
    }
    fn non_terminal_locals(&self) -> &HashSet<String> {
        self.non_terminal_locals
    }
}

impl SpawnCollector<'_> {
    fn is_spawn_expr(&self, e: &syn::Expr) -> bool {
        match strip_parens(e) {
            syn::Expr::Call(c) => {
                let syn::Expr::Path(pe) = c.func.as_ref() else {
                    return false;
                };
                self.is_wait_path(&pe.path)
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
    body_top: &'a Imports,
    body_nested: &'a [(Imports, usize)],
    non_terminal_locals: &'a HashSet<String>,
    future_regions: &'a mut Vec<Span>,
}

impl ResolvesPaths for TimeoutCollector<'_> {
    fn chain(&self) -> &[Imports] {
        self.chain
    }
    fn body_top(&self) -> &Imports {
        self.body_top
    }
    fn body_nested(&self) -> &[(Imports, usize)] {
        self.body_nested
    }
    fn non_terminal_locals(&self) -> &HashSet<String> {
        self.non_terminal_locals
    }
}

impl Visit<'_> for TimeoutCollector<'_> {
    fn visit_expr_call(&mut self, call: &syn::ExprCall) {
        if let syn::Expr::Path(pe) = call.func.as_ref()
            && self.is_bounding_path(&pe.path)
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

    fn visit_expr_await(&mut self, _e: &syn::ExprAwait) {
        self.found = true;
    }
}

/// True when a timeout call site occurs anywhere in `expr`'s subtree,
/// closures included (a per-iteration deadline bounds the loop even when
/// the call sits inside a nested closure body).
struct TimeoutSeeker<'a> {
    chain: &'a [Imports],
    body_top: &'a Imports,
    body_nested: &'a [(Imports, usize)],
    non_terminal_locals: &'a HashSet<String>,
    found: bool,
}

impl ResolvesPaths for TimeoutSeeker<'_> {
    fn chain(&self) -> &[Imports] {
        self.chain
    }
    fn body_top(&self) -> &Imports {
        self.body_top
    }
    fn body_nested(&self) -> &[(Imports, usize)] {
        self.body_nested
    }
    fn non_terminal_locals(&self) -> &HashSet<String> {
        self.non_terminal_locals
    }
}

impl Visit<'_> for TimeoutSeeker<'_> {
    fn visit_expr_call(&mut self, call: &syn::ExprCall) {
        if let syn::Expr::Path(pe) = call.func.as_ref()
            && self.is_bounding_path(&pe.path)
        {
            self.found = true;
        }
        visit::visit_expr_call(self, call);
    }
}

struct WaitFinder<'a> {
    chain: &'a [Imports],
    body_top: &'a Imports,
    body_nested: &'a [(Imports, usize)],
    non_terminal_locals: &'a HashSet<String>,
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
    fn body_top(&self) -> &Imports {
        self.body_top
    }
    fn body_nested(&self) -> &[(Imports, usize)] {
        self.body_nested
    }
    fn non_terminal_locals(&self) -> &HashSet<String> {
        self.non_terminal_locals
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
                    body_top: self.body_top,
                    body_nested: self.body_nested,
                    non_terminal_locals: self.non_terminal_locals,
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
                    && self.is_wait_path(&pe.path)
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
        // A resolved glob import now bounds the inner await (flip is
        // deliberate per the blessed spec).
        let src = "use tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert!(findings(src).is_empty());
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

    // ---- alias and glob resolution (new tests) -------------------------

    #[test]
    fn module_alias_timeout_bounds_inner_await() {
        let src = "use tokio::time as clock;\n\n#[tokio::test]\nasync fn t() {\n    let r = clock::timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn module_alias_timeout_at_bounds_inner_await() {
        let src = "use tokio::time as clock;\n\n#[tokio::test]\nasync fn t() {\n    let r = clock::timeout_at(deadline, async { rx.recv().await; }).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn module_alias_spawn_await_reported() {
        let src = "use tokio::task as task;\n\n#[tokio::test]\nasync fn t() {\n    let r = task::spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    #[test]
    fn module_alias_spawn_blocking_await_reported() {
        let src = "use tokio::task as task;\n\n#[tokio::test]\nasync fn t() {\n    let r = task::spawn_blocking(work()).await;\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    #[test]
    fn module_alias_tcp_connect_await_reported() {
        let src = "use tokio::net as net;\n\n#[tokio::test]\nasync fn t() {\n    let s = net::TcpStream::connect(addr).await;\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    #[test]
    fn transitive_alias_timeout_bounds_inner_await() {
        let src = "use tokio::time as clock;\nuse clock::timeout as t;\n\n#[tokio::test]\nasync fn f() {\n    let r = t(d, async { rx.recv().await; }).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn alias_cycle_terminates_and_reports() {
        let src = "use beta::x as alpha;\nuse alpha::y as beta;\n\n#[tokio::test]\nasync fn t() {\n    let r = alpha(d, async { rx.recv().await; }).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn glob_prefix_qualified_spawn_await_reported() {
        let src = "use tokio::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = task::spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    #[test]
    fn glob_imported_timeout_bounds_await_loop() {
        let src = "use tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    loop {\n        let r = timeout(d, q.recv()).await;\n    }\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn recv_inside_aliased_timeout_block_not_reported() {
        let src = "use tokio::time::timeout as with_deadline;\n\n#[tokio::test]\nasync fn t() {\n    let r = with_deadline(d, async { rx.recv().await; }).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn leading_colon_path_bypasses_alias() {
        let src = "use crate::fake as tokio;\n\n#[tokio::test]\nasync fn t() {\n    let r = ::tokio::time::timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn aliased_extern_name_cannot_bypass_resolution() {
        let src = "use crate::fake as tokio;\n\n#[tokio::test]\nasync fn t() {\n    let r = tokio::time::timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    #[test]
    fn absolute_import_target_immune_to_alias_rewrite() {
        let src = "use crate::fake as tokio;\nuse ::tokio::time::timeout as t;\n\n#[tokio::test]\nasync fn f() {\n    let r = t(d, async { rx.recv().await; }).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn extern_crate_alias_spawn_await_reported() {
        let src = "extern crate tokio as runtime;\n\n#[tokio::test]\nasync fn t() {\n    let r = runtime::task::spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    #[test]
    fn glob_prefix_inside_alias_target_spawn_reported() {
        let src = "use tokio::*;\nuse task::spawn as s;\n\n#[tokio::test]\nasync fn t() {\n    let r = s(work()).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn nested_block_glob_spawn_reported() {
        let src = "#[tokio::test]\nasync fn t() {\n    {\n        use tokio::task::*;\n        let r = spawn(work()).await;\n    }\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    // ---- glob self-referential regression tests ------------------------

    #[test]
    fn glob_in_scope_qualified_spawn_still_reported() {
        let src = "use tokio::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = tokio::spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    #[test]
    fn glob_in_scope_qualified_timeout_still_bounds() {
        let src = "use tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = tokio::time::timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    // ---- namespace / scope / shadow adversarial battery -----------------

    #[test]
    fn local_shadow_fn_beats_glob_import() {
        let src = "use tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    fn timeout<T>(d: T, f: T) -> T { f }\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn same_scope_explicit_and_glob_stay_ambiguous() {
        let src = "use my::timeout;\nuse tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn type_only_import_does_not_mask_glob_spawn() {
        let src = "use types::spawn;\nuse tokio::task::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn scope_local_fn_item_beats_glob_import() {
        let src = "mod m {\n    use tokio::time::*;\n    fn timeout<T>(d: T, f: T) -> T { f }\n    #[tokio::test]\n    async fn t() {\n        let r = timeout(d, async { rx.recv().await; }).await;\n    }\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn inner_glob_does_not_suppress_outer_spawn_import() {
        let src = "use tokio::task::spawn;\nmod inner {\n    use super::*;\n    use crate::helpers::*;\n    #[tokio::test]\n    async fn t() {\n        let r = spawn(work()).await;\n    }\n}\n";
        assert_eq!(findings(src), vec![7]);
    }

    #[test]
    fn nested_block_import_cannot_wrongly_bound() {
        let src = "use my::timeout;\n\n#[tokio::test]\nasync fn t() {\n    { use tokio::time::timeout; }\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn conflicting_sibling_block_imports_stay_ambiguous() {
        let src = "#[tokio::test]\nasync fn t() {\n    { use tokio::time::timeout; }\n    { use crate::helpers::timeout; }\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    #[test]
    fn qualified_alias_immune_to_value_shadow() {
        let src = "use tokio::time as clock;\n\n#[tokio::test]\nasync fn t() {\n    let clock = 5;\n    let r = clock::timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn tuple_struct_ctor_beats_glob_import() {
        let src = "struct timeout<D, F>(D, F);\nuse tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn top_level_body_import_and_outer_import_ambiguous() {
        let src = "use tokio::task::spawn;\n\n#[tokio::test]\nasync fn t() {\n    use crate::helpers::spawn;\n    let r = spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn dual_namespace_imports_keep_both_readings() {
        let src = "use tokio::task::spawn;\nuse types::spawn;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn enum_name_does_not_suppress_spawn_call() {
        let src = "enum spawn { A }\nuse tokio::task::spawn;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn named_field_struct_does_not_suppress_spawn_call() {
        let src = "struct spawn { x: u32 }\nuse tokio::task::spawn;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn type_alias_does_not_suppress_spawn_call() {
        let src = "type spawn = ();\nuse tokio::task::spawn;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn late_local_binding_does_not_suppress_earlier_detection() {
        let src = "use tokio::task::spawn;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n    let spawn = helper;\n}\n";
        assert_eq!(findings(src), vec![5]);
    }

    // ---- precedence-rule bounding (Task 1.4) ---------------------------

    #[test]
    fn named_import_bounds_despite_same_scope_glob() {
        // Rule 1: explicit named import from body_top beats same-scope glob
        let src = "use tokio::time::timeout;\nuse super::*;\n\n#[tokio::test]\nasync fn t() {\n    loop {\n        let r = timeout(d, q.recv()).await;\n    }\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn qualified_timeout_bounds_despite_glob() {
        // Rule 2: literal qualified path bounds; globs do not block
        let src = "use streaming::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = tokio::time::timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn inner_glob_blocks_named_rule() {
        // Rule 1 blocked: inner-scope glob is strictly deeper than named import
        let src = "use tokio::time::timeout;\nmod inner {\n    use super::*;\n    use crate::helpers::*;\n    #[tokio::test]\n    async fn t() {\n        let r = timeout(d, async { rx.recv().await; }).await;\n    }\n}\n";
        assert_eq!(findings(src), vec![7]);
    }

    #[test]
    fn local_binding_blocks_bounding_beside_named_import() {
        // Local reading blocks all rules
        let src = "use tokio::time::timeout;\n\n#[tokio::test]\nasync fn t() {\n    let timeout = helper;\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert_eq!(findings(src), vec![6]);
    }

    #[test]
    fn deeper_nested_glob_blocks_named_rule() {
        // Rule 1 blocked: body_nested Named (origin restriction) + deeper glob
        let src = "#[tokio::test]\nasync fn t() {\n    {\n        use tokio::time::timeout;\n        {\n            use crate::helpers::*;\n            let r = timeout(d, async { rx.recv().await; }).await;\n        }\n    }\n}\n";
        assert_eq!(findings(src), vec![7]);
    }

    #[test]
    fn nested_block_named_import_alone_does_not_bound() {
        // Rule 1 blocked: Named from body_nested (origin restriction)
        let src = "#[tokio::test]\nasync fn t() {\n    { use tokio::time::timeout; }\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n";
        assert_eq!(findings(src), vec![4]);
    }
}
