use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use prost::Message;
use prost_reflect::DescriptorPool;
use prost_reflect::prost_types::{
    DescriptorProto, EnumDescriptorProto, FileDescriptorSet, UninterpretedOption,
};
use protox::file::{
    ChainFileResolver, File, FileResolver, GoogleFileResolver, IncludeFileResolver,
};
use tracing::debug;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

use crate::ProtoCompileError;
use crate::nesting::{MAX_NESTING_DEPTH, ScanMode, scan_nesting};

/// Maximum size of one schema input (source file or descriptor set).
pub(crate) const MAX_SCHEMA_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of distinct include-resolved schema files per compile.
pub(crate) const MAX_IMPORT_FILES: usize = 256;
/// Maximum cumulative schema bytes opened per compile.
pub(crate) const MAX_IMPORT_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum longest-chain length in a descriptor-set dependency graph.
/// prost-reflect resolves public imports with unbounded recursion whose
/// stack depth equals this chain length.
pub(crate) const MAX_DESCRIPTOR_CHAIN: usize = 256;
/// Upper bound on descriptor-graph traversal work. Every direct dependency
/// occurrence, public or private, starts a public-import walk whose cost is
/// `E(d) = 1 + sum E(public deps of d)`; `W = file_count + sum E(d)` over
/// every file and every direct dependency occurrence bounds that traversal.
/// This is an upper bound, not an exact call count. Diamond-rich graphs
/// expand exponentially because already-seen dependencies are re-walked;
/// this bound rejects them before pool building.
pub(crate) const MAX_DESCRIPTOR_EXPANSION: u64 = 100_000;
/// Upper bound on source-compilation lifetime: `(file_count + 1) * W`, where
/// `W` is the descriptor-graph work bound. protox builds its internal pool
/// incrementally as each file is added, so a graph whose final `W` fits the
/// resolution budget can still cost up to `N + 1` traversals of the full
/// graph. This is a conservative upper bound, not an exact call count.
pub(crate) const MAX_SOURCE_LIFETIME: u64 = 100_000;

/// Per-compile import budget shared by every include resolver.
#[derive(Default)]
struct ImportBudget {
    files: std::sync::atomic::AtomicUsize,
    bytes: std::sync::atomic::AtomicU64,
}

impl ImportBudget {
    /// Records one opened file of `len` bytes; errors when a limit is hit.
    fn record(&self, name: &str, len: usize) -> Result<(), protox::Error> {
        let files = self.files.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let bytes = self
            .bytes
            .fetch_add(len as u64, std::sync::atomic::Ordering::SeqCst)
            + len as u64;
        if files > MAX_IMPORT_FILES || bytes > MAX_IMPORT_TOTAL_BYTES {
            return Err(protox::Error::new(ImportBudgetExceeded {
                name: name.to_owned(),
            }));
        }
        Ok(())
    }
}

/// Error raised when the import graph exceeds the per-compile budget.
struct ImportBudgetExceeded {
    name: String,
}

impl std::fmt::Display for ImportBudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: import graph exceeds the limit of {} files or {} total bytes",
            self.name, MAX_IMPORT_FILES, MAX_IMPORT_TOTAL_BYTES
        )
    }
}

impl std::fmt::Debug for ImportBudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for ImportBudgetExceeded {}

/// Error raised when an include path resolves to a non-regular file.
struct UnsupportedFileKind {
    name: String,
}

impl std::fmt::Display for UnsupportedFileKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: not a regular file", self.name)
    }
}

impl std::fmt::Debug for UnsupportedFileKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for UnsupportedFileKind {}

/// Error raised when one schema input exceeds `MAX_SCHEMA_BYTES`.
struct SchemaTooLarge {
    name: String,
    len: u64,
}

impl std::fmt::Display for SchemaTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: schema input of {} bytes exceeds the limit of {} bytes",
            self.name, self.len, MAX_SCHEMA_BYTES
        )
    }
}

impl std::fmt::Debug for SchemaTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for SchemaTooLarge {}

/// Error raised when a path is shadowed by another include directory.
struct ShadowedInput {
    name: String,
    expected: PathBuf,
    found: PathBuf,
}

impl std::fmt::Display for ShadowedInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "path '{}' is shadowed by '{}' in the include paths",
            self.expected.display(),
            self.found.display()
        )
    }
}

impl std::fmt::Debug for ShadowedInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self)
    }
}

impl std::error::Error for ShadowedInput {}

/// Error raised when an include file cannot be opened or read.
struct OpenInclude {
    name: String,
    err: std::io::Error,
}

impl std::fmt::Display for OpenInclude {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.err)
    }
}

impl std::fmt::Debug for OpenInclude {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for OpenInclude {}

/// Failure modes of the capped schema reader.
pub(crate) enum SchemaReadError {
    Io(std::io::Error),
    NotRegular,
    TooLarge(u64),
}

impl SchemaReadError {
    pub(crate) fn detail(&self, path: &Path) -> String {
        match self {
            SchemaReadError::NotRegular => format!("{}: not a regular file", path.display()),
            SchemaReadError::TooLarge(len) => format!(
                "{}: schema input of {len} bytes exceeds the limit of {MAX_SCHEMA_BYTES} bytes",
                path.display()
            ),
            SchemaReadError::Io(err) => format!("{}: {err}", path.display()),
        }
    }
}

#[cfg(unix)]
/// `O_NONBLOCK` from libc: value differs per OS AND architecture (Linux MIPS
/// 0x80, SPARC 0x4000), so no table is hand-written.
const SCHEMA_OPEN_FLAGS: i32 = libc::O_NONBLOCK;
#[cfg(windows)]
/// `FILE_FLAG_BACKUP_SEMANTICS`: lets the handle open a directory so the
/// descriptor metadata (not the OS error) decides the rejection.
const SCHEMA_OPEN_FLAGS: u32 = 0x0200_0000;
#[cfg(not(any(unix, windows)))]
const SCHEMA_OPEN_FLAGS: u32 = 0;

/// Opens a schema input for validation. On Unix the open is nonblocking so a
/// writerless FIFO cannot hang before the regular-file check; for regular
/// files the flag is a no-op.
#[cfg(any(unix, windows))]
fn open_schema_input(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(SCHEMA_OPEN_FLAGS)
        .open(path)
}

/// Portable fallback: custom open flags are unavailable on this platform.
#[cfg(not(any(unix, windows)))]
fn open_schema_input(path: &Path) -> std::io::Result<std::fs::File> {
    let _ = SCHEMA_OPEN_FLAGS;
    std::fs::OpenOptions::new().read(true).open(path)
}

/// Opens `path` once, checks that it is a regular file, and reads at most
/// `MAX_SCHEMA_BYTES` bytes from the same open descriptor.
pub(crate) fn read_schema_bytes(path: &Path) -> Result<Vec<u8>, SchemaReadError> {
    use std::io::Read;
    let mut handle = open_schema_input(path).map_err(SchemaReadError::Io)?;
    let meta = handle.metadata().map_err(SchemaReadError::Io)?;
    if !meta.is_file() {
        return Err(SchemaReadError::NotRegular);
    }
    if meta.len() > MAX_SCHEMA_BYTES as u64 {
        return Err(SchemaReadError::TooLarge(meta.len()));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    handle
        .by_ref()
        .take(MAX_SCHEMA_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(SchemaReadError::Io)?;
    if bytes.len() > MAX_SCHEMA_BYTES {
        return Err(SchemaReadError::TooLarge(bytes.len() as u64));
    }
    Ok(bytes)
}

/// Remedy appended when a source uses protobuf editions.
const EDITIONS_REMEDY: &str = "protobuf editions are not supported; rewrite the schema with syntax = \"proto3\" or \"proto2\"";

/// Error raised by the nesting guard. `Debug` equals `Display` so the
/// protox `Debug` form of the error stays readable.
struct NestingExceeded {
    name: String,
    depth: usize,
}

impl std::fmt::Display for NestingExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: nesting depth {} exceeds the limit of {}",
            self.name, self.depth, MAX_NESTING_DEPTH
        )
    }
}

impl std::fmt::Debug for NestingExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for NestingExceeded {}

/// Naive path equality, equivalent to protox `check_shadow`'s comparison:
/// ignores `.` components and is case-insensitive on Windows.
fn paths_equal(l: &Path, r: &Path) -> bool {
    fn component_eq(l: &std::path::Component<'_>, r: &std::path::Component<'_>) -> bool {
        #[cfg(windows)]
        {
            l.as_os_str().eq_ignore_ascii_case(r.as_os_str())
        }
        #[cfg(not(windows))]
        {
            l == r
        }
    }
    let mut lhs = l
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir));
    let mut rhs = r
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir));
    loop {
        match (lhs.next(), rhs.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) if component_eq(&a, &b) => {}
            _ => return false,
        }
    }
}

/// Include-directory resolver that reads, scans and parses one validated buffer.
struct NestingGuardedInclude {
    dir: PathBuf,
    budget: Arc<ImportBudget>,
    top: Arc<Mutex<Option<(String, PathBuf)>>>,
}

impl NestingGuardedInclude {
    fn new(
        dir: PathBuf,
        budget: Arc<ImportBudget>,
        top: Arc<Mutex<Option<(String, PathBuf)>>>,
    ) -> Self {
        Self { dir, budget, top }
    }
}

impl FileResolver for NestingGuardedInclude {
    fn resolve_path(&self, path: &Path) -> Option<String> {
        let name = protox::file::IncludeFileResolver::new(self.dir.clone()).resolve_path(path)?;
        // Record the first resolving include for the shadow check in open_file.
        let mut guard = self.top.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some((name.clone(), path.to_path_buf()));
        }
        Some(name)
    }

    fn open_file(&self, name: &str) -> Result<File, protox::Error> {
        use std::io::Read;
        let candidate = self.dir.join(name);
        let mut handle = match open_schema_input(&candidate) {
            Ok(h) => h,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(protox::Error::file_not_found(name));
            }
            Err(err) => {
                return Err(protox::Error::new(OpenInclude {
                    name: name.to_owned(),
                    err,
                }));
            }
        };
        // Metadata of the open descriptor: no swap race between stat and read.
        let meta = handle.metadata().map_err(|err| {
            protox::Error::new(OpenInclude {
                name: name.to_owned(),
                err,
            })
        })?;
        if !meta.is_file() {
            return Err(protox::Error::new(UnsupportedFileKind {
                name: name.to_owned(),
            }));
        }
        if meta.len() > MAX_SCHEMA_BYTES as u64 {
            return Err(protox::Error::new(SchemaTooLarge {
                name: name.to_owned(),
                len: meta.len(),
            }));
        }
        let mut text = String::new();
        handle
            .by_ref()
            .take(MAX_SCHEMA_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|err| {
                protox::Error::new(OpenInclude {
                    name: name.to_owned(),
                    err,
                })
            })?;
        if text.len() > MAX_SCHEMA_BYTES {
            return Err(protox::Error::new(SchemaTooLarge {
                name: name.to_owned(),
                len: text.len() as u64,
            }));
        }
        self.budget.record(name, text.len())?;
        if let Err(depth) = scan_nesting(text.as_bytes(), MAX_NESTING_DEPTH, ScanMode::ProtoSource)
        {
            return Err(protox::Error::new(NestingExceeded {
                name: name.to_owned(),
                depth,
            }));
        }
        // Shadow replication (protox checks file.path(); File::from_source has none):
        // `resolve_path` records the first resolving include; a different include
        // that opens the same name is the shadow protox would reject.
        if let Some((rec_name, rec_path)) =
            self.top.lock().unwrap_or_else(|p| p.into_inner()).clone()
            && rec_name == name
            && !paths_equal(Path::new(&rec_path), candidate.as_path())
        {
            return Err(protox::Error::new(ShadowedInput {
                name: name.to_owned(),
                expected: rec_path,
                found: candidate,
            }));
        }
        // Parse the SAME validated buffer.
        File::from_source(name, &text)
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(m) = payload.downcast_ref::<String>() {
        m.clone()
    } else if let Some(m) = payload.downcast_ref::<&str>() {
        (*m).to_string()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Runs `f`; a panic becomes `ProtoCompileError::Compile` with detail
/// `internal compiler panic: <message>`. The global panic hook is untouched.
pub(crate) fn contained_compile(
    path: &Path,
    f: impl FnOnce() -> Result<DescriptorPool, ProtoCompileError>,
) -> Result<DescriptorPool, ProtoCompileError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        Err(ProtoCompileError::Compile {
            path: path.to_path_buf(),
            detail: format!("internal compiler panic: {}", panic_message(payload)),
        })
    })
}

/// Runs `f`; a panic becomes `ProtoCompileError::DescriptorDecode` with text
/// `internal decoder panic: <message> (<path>)`. The global panic hook is
/// untouched.
pub(crate) fn contained_decode(
    path: &Path,
    f: impl FnOnce() -> Result<DescriptorPool, ProtoCompileError>,
) -> Result<DescriptorPool, ProtoCompileError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        Err(ProtoCompileError::DescriptorDecode(format!(
            "internal decoder panic: {} ({})",
            panic_message(payload),
            path.display()
        )))
    })
}

/// True when `path` names a precompiled descriptor set by extension
/// (`binpb`, `pb`, `desc`, `protoset`; ASCII case-insensitive).
fn is_descriptor_set(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            ["binpb", "pb", "desc", "protoset"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

fn check_uninterpreted(opts: &[UninterpretedOption]) -> Result<(), usize> {
    for o in opts {
        if let Some(text) = &o.aggregate_value {
            scan_nesting(text.as_bytes(), MAX_NESTING_DEPTH, ScanMode::OptionText)?;
        }
    }
    Ok(())
}

fn check_enum(e: &EnumDescriptorProto) -> Result<(), usize> {
    if let Some(o) = &e.options {
        check_uninterpreted(&o.uninterpreted_option)?;
    }
    for v in &e.value {
        if let Some(o) = &v.options {
            check_uninterpreted(&o.uninterpreted_option)?;
        }
    }
    Ok(())
}

fn check_message(m: &DescriptorProto) -> Result<(), usize> {
    if let Some(o) = &m.options {
        check_uninterpreted(&o.uninterpreted_option)?;
    }
    for f in m.field.iter().chain(m.extension.iter()) {
        if let Some(o) = &f.options {
            check_uninterpreted(&o.uninterpreted_option)?;
        }
    }
    for d in &m.oneof_decl {
        if let Some(o) = &d.options {
            check_uninterpreted(&o.uninterpreted_option)?;
        }
    }
    for r in &m.extension_range {
        if let Some(o) = &r.options {
            check_uninterpreted(&o.uninterpreted_option)?;
        }
    }
    for e in &m.enum_type {
        check_enum(e)?;
    }
    for n in &m.nested_type {
        check_message(n)?;
    }
    Ok(())
}

fn check_set_options(set: &FileDescriptorSet) -> Result<(), usize> {
    for f in &set.file {
        if let Some(o) = &f.options {
            check_uninterpreted(&o.uninterpreted_option)?;
        }
        for m in &f.message_type {
            check_message(m)?;
        }
        for e in &f.enum_type {
            check_enum(e)?;
        }
        for x in &f.extension {
            if let Some(o) = &x.options {
                check_uninterpreted(&o.uninterpreted_option)?;
            }
        }
        for s in &f.service {
            if let Some(o) = &s.options {
                check_uninterpreted(&o.uninterpreted_option)?;
            }
            for m in &s.method {
                if let Some(o) = &m.options {
                    check_uninterpreted(&o.uninterpreted_option)?;
                }
            }
        }
    }
    Ok(())
}

/// True when any file in `set` declares protobuf editions.
fn has_editions(set: &FileDescriptorSet) -> bool {
    set.file
        .iter()
        .any(|f| f.syntax.as_deref() == Some("editions"))
}

/// Rejects cyclic dependency graphs before pool decoding. prost-reflect's
/// public-dependency traversal recurses without a bound on cycles.
fn check_import_cycles(set: &FileDescriptorSet) -> Result<(), String> {
    use std::collections::HashMap;
    let deps: HashMap<&str, Vec<&str>> = set
        .file
        .iter()
        .map(|f| {
            (
                f.name.as_deref().unwrap_or(""),
                f.dependency.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    // 0 = unvisited, 1 = on stack, 2 = done
    let mut state: HashMap<&str, u8> = deps.keys().map(|k| (*k, 0u8)).collect();
    for start in deps.keys() {
        if state[start] != 0 {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(*start, 0)];
        let mut path: Vec<&str> = Vec::new();
        while let Some((node, idx)) = stack.pop() {
            if idx == 0 {
                state.insert(node, 1);
                path.push(node);
            }
            let node_deps = match deps.get(node) {
                Some(d) => d,
                None => {
                    path.pop();
                    state.insert(node, 2);
                    continue;
                }
            };
            if let Some(dep) = node_deps.get(idx).copied() {
                stack.push((node, idx + 1));
                match state.get(dep).copied().unwrap_or(0) {
                    0 => stack.push((dep, 0)),
                    1 => {
                        let mut cycle = path.clone();
                        cycle.push(dep);
                        return Err(cycle.join(" -> "));
                    }
                    _ => {}
                }
            } else {
                state.insert(node, 2);
                path.pop();
            }
        }
    }
    Ok(())
}

/// Bounds the descriptor dependency graph before pool building: the longest
/// dependency chain (prost-reflect public-import recursion depth) and the
/// public-import traversal work `W` (every direct dependency occurrence,
/// private included, is charged an entry-point walk). Cycles are rejected
/// first by `check_import_cycles`, so the topological walk terminates.
/// Returns `W` so source lifetime accounting can reuse it.
fn check_descriptor_graph(set: &FileDescriptorSet) -> Result<u64, String> {
    use std::collections::HashMap;
    let names: HashMap<&str, usize> = set
        .file
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.name.as_deref().map(|n| (n, i)))
        .collect();
    let n = set.file.len();
    // dependency[i][k] = Some(index) for the k-th dependency of file i
    // (public_dependency positions index into this ORIGINAL list).
    let deps_resolved: Vec<Vec<Option<usize>>> = set
        .file
        .iter()
        .map(|f| {
            f.dependency
                .iter()
                .map(|d| names.get(d.as_str()).copied())
                .collect()
        })
        .collect();
    let all_deps: Vec<Vec<usize>> = deps_resolved
        .iter()
        .map(|v| v.iter().copied().flatten().collect())
        .collect();
    let public_deps: Vec<Vec<usize>> = deps_resolved
        .iter()
        .enumerate()
        .map(|(i, v)| {
            set.file[i]
                .public_dependency
                .iter()
                .filter_map(|&pi| v.get(pi as usize).and_then(|o| *o))
                .collect()
        })
        .collect();
    // Kahn topological order (in-degree over all_deps).
    let mut indegree = vec![0usize; n];
    for out in &all_deps {
        for &d in out {
            indegree[d] += 1;
        }
    }
    let mut queue: std::collections::VecDeque<usize> =
        (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i);
        for &d in &all_deps[i] {
            indegree[d] -= 1;
            if indegree[d] == 0 {
                queue.push_back(d);
            }
        }
    }
    if order.len() != n {
        return Err("cyclic dependency graph".to_string());
    }
    // The Kahn pass above counts dependents, so it orders roots before
    // leaves. Reverse it so every dependency precedes its dependents, which
    // the chain and E DP below requires.
    order.reverse();
    // Longest chain and per-file public-import expansion E(v) =
    // 1 + sum E(d) over every public dependency occurrence, in topological
    // order, saturating so exponential values cannot overflow.
    let cap = MAX_DESCRIPTOR_EXPANSION;
    let mut chain = vec![1usize; n];
    let mut expand = vec![1u64; n];
    for &i in &order {
        for &d in &all_deps[i] {
            chain[i] = chain[i].max(chain[d] + 1);
        }
        for &d in &public_deps[i] {
            expand[i] = expand[i].saturating_add(expand[d]);
        }
        if chain[i] > MAX_DESCRIPTOR_CHAIN {
            return Err(format!(
                "descriptor import chain exceeds the limit of {MAX_DESCRIPTOR_CHAIN} files"
            ));
        }
    }
    // Work W = file_count + sum E(d) over EVERY file and EVERY direct
    // dependency occurrence, private entries included. Entry points are not
    // deduplicated: a repeated dependency occurrence is charged again.
    let mut work: u64 = n as u64;
    for &i in &order {
        for &d in &all_deps[i] {
            work = work.saturating_add(expand[d]);
        }
    }
    if work > cap {
        return Err(format!(
            "descriptor public-import graph expands beyond the resolution budget of {cap} steps"
        ));
    }
    Ok(work)
}

fn source_error(path: &Path, detail: String) -> ProtoCompileError {
    ProtoCompileError::Compile {
        path: path.to_path_buf(),
        detail: format!("{}: {detail}", path.display()),
    }
}

/// Validates a preloaded source graph before any protox pool is built: no
/// import cycles, the descriptor-graph chain and work `W` bounds from
/// [`check_descriptor_graph`], and the source lifetime `(file_count + 1) * W`
/// bound. Rejections are `Compile` naming the root path.
fn check_source_graph(path: &Path, set: &FileDescriptorSet) -> Result<(), ProtoCompileError> {
    check_import_cycles(set)
        .map_err(|cycle| source_error(path, format!("cyclic import graph: {cycle}")))?;
    let work = check_descriptor_graph(set).map_err(|msg| source_error(path, msg))?;
    let lifetime = (set.file.len() as u64 + 1).saturating_mul(work);
    if lifetime > MAX_SOURCE_LIFETIME {
        return Err(source_error(
            path,
            format!(
                "source compilation lifetime {lifetime} exceeds the limit of {MAX_SOURCE_LIFETIME} steps"
            ),
        ));
    }
    Ok(())
}

/// Immutable preloaded source closure. Loaded files keep their parsed
/// descriptor and source text in declaration (preorder) order; names that were
/// not found are recorded so the snapshot resolver reports `file_not_found`
/// without touching the filesystem again.
pub(crate) struct SourceSnapshot {
    root_name: String,
    files: Vec<File>,
    /// `Some(index)` into `files`, or `None` for a recorded missing import.
    entries: HashMap<String, Option<usize>>,
}

impl SourceSnapshot {
    fn file_descriptor_set(&self) -> FileDescriptorSet {
        FileDescriptorSet {
            file: self
                .files
                .iter()
                .map(|f| f.file_descriptor_proto().clone())
                .collect(),
        }
    }

    fn push(&mut self, name: &str, file: File) {
        let index = self.files.len();
        self.files.push(file);
        self.entries.insert(name.to_owned(), Some(index));
    }

    fn record_missing(&mut self, name: &str) {
        self.entries.insert(name.to_owned(), None);
    }
}

/// A resolver that only serves files captured by [`preload_snapshot`].
/// Unknown or recorded-missing names yield `file_not_found`; there is no
/// filesystem or Google fallback after preflight.
struct SnapshotResolver {
    snapshot: SourceSnapshot,
}

impl FileResolver for SnapshotResolver {
    fn resolve_path(&self, _path: &Path) -> Option<String> {
        Some(self.snapshot.root_name.clone())
    }

    fn open_file(&self, name: &str) -> Result<File, protox::Error> {
        match self.snapshot.entries.get(name).copied().flatten() {
            Some(index) => Ok(self.snapshot.files[index].clone()),
            None => Err(protox::Error::file_not_found(name)),
        }
    }
}

/// One pending depth-first frame: the file name and its not-yet-visited
/// dependencies.
struct PreloadFrame {
    name: String,
    deps: Vec<String>,
    next: usize,
}

/// Iteratively loads the root and its reachable imports through `chain`
/// (explicit includes, parent, then Google). Each file is parsed once with
/// [`File::from_source`] by the guarded include resolver; no pool is built.
/// Missing imports are recorded, not errors, so the later `Compiler` can emit
/// its canonical import diagnostic from the importing file's source location.
fn preload_with(
    proto_path: &Path,
    chain: &ChainFileResolver,
) -> Result<SourceSnapshot, ProtoCompileError> {
    let root_name = match chain.resolve_path(proto_path) {
        Some(name) => name,
        // A path the include chain cannot name is still a valid protox file
        // name when it is relative; validate it the way protox would.
        None => IncludeFileResolver::new(PathBuf::new())
            .resolve_path(proto_path)
            .ok_or_else(|| {
                source_error(proto_path, "file is not in any include path".to_owned())
            })?,
    };
    // 0 = unseen, 1 = visiting, 2 = done.
    let mut state: HashMap<String, u8> = HashMap::new();
    let mut snapshot = SourceSnapshot {
        root_name: root_name.clone(),
        files: Vec::new(),
        entries: HashMap::new(),
    };

    let root_file = chain
        .open_file(&root_name)
        .map_err(|e| compile_error(proto_path, &e))?;
    state.insert(root_name.clone(), 1);
    let deps = root_file.file_descriptor_proto().dependency.clone();
    snapshot.push(&root_name, root_file);
    let mut frames = vec![PreloadFrame {
        name: root_name,
        deps,
        next: 0,
    }];

    while let Some(top) = frames.len().checked_sub(1) {
        let dep = {
            let frame = &mut frames[top];
            let dep = frame.deps.get(frame.next).cloned();
            frame.next += 1;
            dep
        };
        let Some(dep) = dep else {
            state.insert(frames[top].name.clone(), 2);
            frames.pop();
            continue;
        };
        if state.get(&dep).copied().unwrap_or(0) != 0 {
            // Already loaded or on the current path; a cycle is reported by
            // `check_import_cycles` after the closure is complete.
            continue;
        }
        match chain.open_file(&dep) {
            Ok(file) => {
                let deps = file.file_descriptor_proto().dependency.clone();
                state.insert(dep.clone(), 1);
                snapshot.push(&dep, file);
                frames.push(PreloadFrame {
                    name: dep,
                    deps,
                    next: 0,
                });
            }
            Err(err) if err.is_file_not_found() => {
                state.insert(dep.clone(), 2);
                snapshot.record_missing(&dep);
            }
            Err(err) => return Err(compile_error(proto_path, &err)),
        }
    }
    Ok(snapshot)
}

fn build_chain(proto_path: &Path, includes: &[PathBuf]) -> ChainFileResolver {
    let parent = match proto_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut resolver = ChainFileResolver::new();
    let budget = Arc::new(ImportBudget::default());
    let top = Arc::new(Mutex::new(None));
    for dir in includes.iter().cloned().chain(std::iter::once(parent)) {
        resolver.add(NestingGuardedInclude::new(
            dir,
            Arc::clone(&budget),
            Arc::clone(&top),
        ));
    }
    resolver.add(GoogleFileResolver::new());
    resolver
}

/// Include resolver over the embedded-source registry (mission 350).
/// `resolve_path` accepts a `camel-embedded:` ref naming a registered
/// asset; `open_file` serves the registered bytes through the SAME
/// hardening as [`NestingGuardedInclude`]: per-file size cap, per-compile
/// import budget, UTF-8 requirement, and the bracket-nesting scan. No
/// shadow check is needed: the registry is the single source of truth,
/// keyed by canonical asset name. Unregistered names yield
/// `file_not_found` so the chain falls through (only Google well-known
/// types follow).
///
/// Scope note: the resolver serves ONLY registered assets — import-tree
/// assets beyond the declared `protoFile` are not embedded by the seal
/// path, and this resolver does not add them.
struct EmbeddedGuardedInclude {
    budget: Arc<ImportBudget>,
}

impl FileResolver for EmbeddedGuardedInclude {
    fn resolve_path(&self, path: &Path) -> Option<String> {
        let name = crate::embedded::strip_ref(path)?;
        crate::embedded::contains(name).then(|| name.to_owned())
    }

    fn open_file(&self, name: &str) -> Result<File, protox::Error> {
        let Some(bytes) = crate::embedded::get(name) else {
            return Err(protox::Error::file_not_found(name));
        };
        if bytes.len() > MAX_SCHEMA_BYTES {
            return Err(protox::Error::new(SchemaTooLarge {
                name: name.to_owned(),
                len: bytes.len() as u64,
            }));
        }
        let text = String::from_utf8(bytes.to_vec()).map_err(|err| {
            protox::Error::new(OpenInclude {
                name: name.to_owned(),
                err: std::io::Error::new(std::io::ErrorKind::InvalidData, err),
            })
        })?;
        self.budget.record(name, text.len())?;
        if let Err(depth) = scan_nesting(text.as_bytes(), MAX_NESTING_DEPTH, ScanMode::ProtoSource)
        {
            return Err(protox::Error::new(NestingExceeded {
                name: name.to_owned(),
                depth,
            }));
        }
        File::from_source(name, &text)
    }
}

/// Builds the hardened resolver chain for an embedded compile: the
/// registry resolver first, then the Google well-known types. No
/// filesystem include directories participate.
fn build_embedded_chain() -> ChainFileResolver {
    let mut resolver = ChainFileResolver::new();
    resolver.add(EmbeddedGuardedInclude {
        budget: Arc::new(ImportBudget::default()),
    });
    resolver.add(GoogleFileResolver::new());
    resolver
}

/// Compiles an embedded registry asset (a `camel-embedded:` ref) into a
/// [`DescriptorPool`]. Descriptor-set assets decode from the registered
/// bytes through the same containment as the disk path; source assets
/// preload and compile through [`EmbeddedGuardedInclude`] and the shared
/// graph validation. Nothing touches the filesystem.
pub(crate) fn compile_proto_embedded(
    proto_path: &Path,
    bytes: Arc<[u8]>,
) -> Result<DescriptorPool, ProtoCompileError> {
    if let Some(name) = crate::embedded::strip_ref(proto_path)
        && is_descriptor_set(Path::new(name))
    {
        debug!(ref = %proto_path.display(), "loading embedded descriptor set");
        return load_descriptor_set_bytes(proto_path, &bytes);
    }
    contained_compile(proto_path, || {
        let snapshot = preload_with(proto_path, &build_embedded_chain())?;
        compile_from_snapshot(proto_path, snapshot)
    })
}

/// Loads the root and its import closure through the guarded include chain,
/// without building a protox pool. The returned snapshot is immutable: later
/// compilation serves only these files.
pub(crate) fn preload_snapshot(
    proto_path: &Path,
    includes: &[PathBuf],
) -> Result<SourceSnapshot, ProtoCompileError> {
    let chain = build_chain(proto_path, includes);
    preload_with(proto_path, &chain)
}

/// Compiles an already-preloaded snapshot. Validates cycles, chain and work
/// `W` first, then builds the pool from the snapshot only; source files are
/// never reopened.
pub(crate) fn compile_from_snapshot(
    proto_path: &Path,
    snapshot: SourceSnapshot,
) -> Result<DescriptorPool, ProtoCompileError> {
    check_source_graph(proto_path, &snapshot.file_descriptor_set())?;
    contained_compile(proto_path, || {
        let mut compiler = protox::Compiler::with_file_resolver(SnapshotResolver { snapshot });
        compiler.include_imports(true).include_source_info(false);
        compiler
            .open_file(proto_path)
            .map_err(|e| compile_error(proto_path, &e))?;
        let bytes = compiler.encode_file_descriptor_set();
        DescriptorPool::decode(bytes.as_slice()).map_err(|e| {
            ProtoCompileError::DescriptorDecode(format!("{}: {e}", proto_path.display()))
        })
    })
}

fn load_descriptor_set(path: &Path) -> Result<DescriptorPool, ProtoCompileError> {
    let bytes = match read_schema_bytes(path) {
        Ok(bytes) => bytes,
        Err(SchemaReadError::Io(err)) => return Err(ProtoCompileError::Io(err)),
        Err(err) => {
            return Err(ProtoCompileError::DescriptorDecode(err.detail(path)));
        }
    };
    load_descriptor_set_bytes(path, &bytes)
}

/// Decodes a `FileDescriptorSet` from already-loaded bytes through the
/// SAME hardening as the disk path ([`load_descriptor_set`]): size cap,
/// editions rejection, import-cycle rejection, descriptor-graph bounds,
/// option-text nesting scan, and panic containment.
fn load_descriptor_set_bytes(
    path: &Path,
    bytes: &[u8],
) -> Result<DescriptorPool, ProtoCompileError> {
    if bytes.len() > MAX_SCHEMA_BYTES {
        return Err(ProtoCompileError::DescriptorDecode(
            SchemaReadError::TooLarge(bytes.len() as u64).detail(path),
        ));
    }
    contained_decode(path, move || {
        let set = FileDescriptorSet::decode(bytes)
            .map_err(|e| ProtoCompileError::DescriptorDecode(format!("{}: {e}", path.display())))?;
        if has_editions(&set) {
            return Err(ProtoCompileError::DescriptorDecode(format!(
                "{}: protobuf editions descriptor sets are not supported; rewrite the schema with syntax = \"proto3\" or \"proto2\"",
                path.display()
            )));
        }
        check_import_cycles(&set).map_err(|cycle| {
            ProtoCompileError::DescriptorDecode(format!(
                "{}: cyclic import graph: {cycle}",
                path.display()
            ))
        })?;
        check_descriptor_graph(&set).map_err(|msg| {
            ProtoCompileError::DescriptorDecode(format!("{}: {msg}", path.display()))
        })?;
        check_set_options(&set).map_err(|depth| {
            ProtoCompileError::DescriptorDecode(format!(
                "{}: option text nesting depth {depth} exceeds the limit of {MAX_NESTING_DEPTH}",
                path.display()
            ))
        })?;
        DescriptorPool::decode(bytes)
            .map_err(|e| ProtoCompileError::DescriptorDecode(format!("{}: {e}", path.display())))
    })
}

/// Maps a protox error to `Compile`. `detail` is the protox `Debug` form
/// (`file:line:col: message`); the editions remedy is appended when the
/// detail contains `found 'edition'`.
fn compile_error(path: &Path, e: &protox::Error) -> ProtoCompileError {
    let mut detail = format!("{e:?}");
    if detail.contains("found 'edition'") {
        detail.push_str(". ");
        detail.push_str(EDITIONS_REMEDY);
    }
    ProtoCompileError::Compile {
        path: path.to_path_buf(),
        detail,
    }
}

fn compile_source(
    proto_path: &Path,
    includes: &[PathBuf],
) -> Result<DescriptorPool, ProtoCompileError> {
    // Preflight (preload + graph validation) and pool building share one
    // containment boundary, so a parse panic in the closure is a typed error
    // too.
    contained_compile(proto_path, || {
        let snapshot = preload_snapshot(proto_path, includes)?;
        compile_from_snapshot(proto_path, snapshot)
    })
}

/// Compiles a `.proto` source file, or loads a precompiled descriptor set,
/// into a [`DescriptorPool`].
///
/// For a `.proto` source, the parent directory of `proto_path` and every path
/// in `includes` are used as include directories. A path whose extension is
/// `binpb`, `pb`, `desc` or `protoset` (ASCII case-insensitive) is treated as
/// a precompiled `FileDescriptorSet`: it must contain all its imports, and
/// `includes` is ignored. A path with the reserved `camel-embedded:` prefix
/// resolves from the in-process embedded registry with the same
/// source/descriptor-set split and the same hardening — the filesystem is
/// never consulted. Compilation is in process; no external compiler is
/// invoked.
pub fn compile_proto<P, I>(proto_path: P, includes: I) -> Result<DescriptorPool, ProtoCompileError>
where
    P: AsRef<Path>,
    I: IntoIterator,
    I::Item: AsRef<Path>,
{
    let proto_path = proto_path.as_ref();
    // Embedded references resolve from the in-process registry BEFORE any
    // `.exists()` check; an unregistered ref fails closed naming the ref.
    if let Some(name) = crate::embedded::strip_ref(proto_path) {
        let bytes = crate::embedded::get(name)
            .ok_or_else(|| ProtoCompileError::ProtoNotFound(proto_path.to_path_buf()))?;
        debug!(ref = %proto_path.display(), "compiling embedded proto");
        return compile_proto_embedded(proto_path, bytes);
    }
    if !proto_path.exists() {
        return Err(ProtoCompileError::ProtoNotFound(proto_path.to_path_buf()));
    }
    if is_descriptor_set(proto_path) {
        debug!(descriptor_set = %proto_path.display(), "loading precompiled descriptor set");
        return load_descriptor_set(proto_path);
    }
    let include_paths = includes
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .collect::<Vec<PathBuf>>();
    debug!(proto = %proto_path.display(), "compiling proto");
    compile_source(proto_path, &include_paths)
}

#[cfg(test)]
#[path = "compiler_tests.rs"]
mod tests;
