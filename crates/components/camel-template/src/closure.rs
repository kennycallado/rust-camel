//! Dependency-closure acquisition for the external template component
//! (ADR-0047 Stage 2, Task 2.3).
//!
//! [`acquire_closure`] performs a single-pass iterative depth-first walk over
//! the statically discoverable `include`/`extends`/`import`/`from` targets of
//! an entry template. Each source is read bounded by `max_template_size`, and
//! the full closure is accumulated into a [`ClosureSnapshot`]. The walk
//! detects:
//!
//! - cycles (a back edge to a frame still on the DFS stack, marked `Gray`),
//! - duplicate file identities (two names resolving to the same inode, e.g.
//!   hardlinks),
//! - and enforces the include-count, include-depth, and total-source-bytes
//!   bounds from [`ResolvedExternalTemplateLimits`].
//!
//! Every read goes through a [`StableTemplateReader`] so the production
//! filesystem reader (Task 2.4) and the test-local reader share identical
//! confinement and bounded-read semantics.
//!
//! This module is Stage-2 scaffolding: its public surface is consumed by the
//! Phase-4 component/endpoint implementation, so dead-code analysis of the
//! non-test lib build is relaxed here.

use std::collections::{BTreeMap, HashMap, HashSet};

use regex::Regex;

use crate::config::ResolvedExternalTemplateLimits;
use crate::error::TemplateReloadError;
use crate::path_util::{FileIdentity, OwnedHandle};

/// Reads a single template source bounded by `max_bytes`, openat-relative to
/// `root`. Implementations open via [`OwnedHandle::open_relative`] and then
/// read in a bounded loop, returning
/// [`TemplateReloadError::BoundExceeded`]`("max_template_size")` as soon as the
/// limit is exceeded, without first allocating the whole file.
///
/// The opened [`OwnedHandle`] is returned alongside the bytes and identity so
/// the caller may retain it for caching; the closure walker discards it after
/// reading.
#[allow(dead_code)] // pub(crate) trait; consumed by Phase 4 (Task 4.x reader impl).
pub(crate) trait StableTemplateReader: Send + Sync {
    fn read_relative(
        &self,
        root: &OwnedHandle,
        name: &str,
        max_bytes: usize,
    ) -> Result<(OwnedHandle, FileIdentity, Vec<u8>), TemplateReloadError>;
}

/// One acquired template file in the dependency closure.
#[derive(Debug)]
#[allow(dead_code)] // pub(crate) struct; closure entry consumed by Phase 4 (Task 4.x).
pub(crate) struct TemplateFile {
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) identity: FileIdentity,
}

/// Immutable result of acquiring a template dependency closure: every
/// statically discoverable template, keyed by its root-relative name.
#[derive(Debug)]
#[allow(dead_code)] // pub(crate) struct; closure result consumed by Phase 4 (Task 4.x).
pub(crate) struct ClosureSnapshot {
    entries: BTreeMap<String, TemplateFile>,
}

impl ClosureSnapshot {
    /// Deterministic content hash over the closure, used to decide whether a
    /// reload produced a byte-identical set. Length-delimited `(name, bytes)`
    /// tuples are fed to BLAKE3 in `BTreeMap` (sorted-by-name) order so the
    /// hash is independent of DFS visit order.
    #[allow(dead_code)] // pub method; consumed by Phase 5 (Task 5.1 closure hash).
    pub fn deterministic_hash(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        for (name, file) in &self.entries {
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            hasher.update(&(file.bytes.len() as u64).to_le_bytes());
            hasher.update(&file.bytes);
        }
        *hasher.finalize().as_bytes()
    }

    /// Read-only access to the closure entries (Phase 4 compilation).
    #[allow(dead_code)] // pub(crate) method; consumed by Phase 4 (Task 4.x compilation).
    pub(crate) fn entries(&self) -> &BTreeMap<String, TemplateFile> {
        &self.entries
    }
}

#[cfg(test)]
impl ClosureSnapshot {
    /// Test-only constructor: a single-entry snapshot with a fabricated
    /// [`FileIdentity`]. Used by the Phase-4 compile tests (`template_set`)
    /// to build a [`ClosureSnapshot`] without filesystem setup —
    /// [`crate::template_set::TemplateSet::compile`] reads only the entry's
    /// `name`/`bytes`, never its `identity`, so the fabricated identity is
    /// never consulted.
    pub(crate) fn from_single_entry(name: &str, bytes: Vec<u8>) -> Self {
        let identity = synthetic_identity(bytes.len());
        let mut entries: BTreeMap<String, TemplateFile> = BTreeMap::new();
        entries.insert(
            name.to_string(),
            TemplateFile {
                name: name.to_string(),
                bytes,
                identity,
            },
        );
        Self { entries }
    }

    /// Test-only constructor: a multi-entry snapshot with fabricated
    /// [`FileIdentity`]s. Companion to [`from_single_entry`](Self::from_single_entry)
    /// for tests that exercise `{% include %}` resolution across more than
    /// one named entry (e.g. the G13 multi-entry include / recursion-bomb
    /// tests in `template_set`). Bypasses the production
    /// `acquire_closure` DFS so the test owns the closure shape directly.
    pub(crate) fn from_entries(entries: Vec<(&str, &[u8])>) -> Self {
        let mut map: BTreeMap<String, TemplateFile> = BTreeMap::new();
        for (name, bytes) in entries {
            map.insert(
                name.to_string(),
                TemplateFile {
                    name: name.to_string(),
                    bytes: bytes.to_vec(),
                    identity: synthetic_identity(bytes.len()),
                },
            );
        }
        Self { entries: map }
    }
}

/// Fabricate a [`FileIdentity`] for test snapshots. Only `length` reflects the
/// entry size; the remaining identifying fields are zeroed. `TemplateSet`
/// compilation never consults `identity`, so the values are irrelevant — the
/// type merely needs to construct on both Unix and Windows.
#[cfg(test)]
fn synthetic_identity(len: usize) -> FileIdentity {
    let length = len as u64;
    FileIdentity {
        #[cfg(unix)]
        inode: 0,
        #[cfg(unix)]
        length,
        #[cfg(unix)]
        mtime_nsec: 0,
        #[cfg(windows)]
        volume_serial: 0,
        #[cfg(windows)]
        file_index_high: 0,
        #[cfg(windows)]
        file_index_low: 0,
        #[cfg(windows)]
        length,
        #[cfg(windows)]
        last_write_100ns: 0,
    }
}

// ===========================================================================
// DFS machinery
// ===========================================================================

/// Per-node DFS color. `Gray` nodes are currently on the stack (their subtree
/// is still being walked); a back edge to a `Gray` node is a cycle. `Black`
/// nodes are fully processed and may be skipped on re-encounter.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // DFS internal; consumed via acquire_closure (Phase 4).
enum VisitState {
    Gray,
    Black,
}

/// An in-progress DFS frame: the node name, the depth at which it was
/// discovered, and the statically discovered child include targets yet to
/// visit.
#[allow(dead_code)] // DFS internal; consumed via acquire_closure (Phase 4).
struct Frame {
    name: String,
    depth: u32,
    children: Vec<String>,
    child_idx: usize,
}

/// Mutable accumulator shared across the traversal. Grouping the mutable
/// collections keeps `read_and_record` below the `too_many_arguments` threshold
/// and makes the borrow story straightforward.
#[allow(dead_code)] // DFS internal; consumed via acquire_closure (Phase 4).
struct WalkState {
    entries: BTreeMap<String, TemplateFile>,
    state: HashMap<String, VisitState>,
    seen_identities: HashSet<FileIdentity>,
    total_bytes: usize,
}

/// Matches a MiniJinja statement tag that statically names another template
/// (`include`/`extends`/`import`/`from`). Whitespace-control markers (`{%-`,
/// `-%}`) are accepted. Only the trailing `rest` group is captured; its first
/// token must be a quoted string literal (the template name) — see
/// [`first_string_arg`].
#[allow(dead_code)] // DFS internal; consumed via acquire_closure (Phase 4).
const INCLUDE_RE_PATTERN: &str = r"\{%-?\s*(?:include|extends|import|from)\s+(?P<rest>.*?)\s*-?%\}";

/// Acquire the full dependency closure of `entry` openat-relative to `root`.
///
/// `root` is borrowed so the same handle is reused across reloads (Task 5.1).
/// The walk is single-pass DFS: each new source is READ before its outgoing
/// edges are known (read-then-walk), so include discovery and bound enforcement
/// happen in the same traversal.
///
/// Bounds enforced: per-file `max_template_size` (inside the reader),
/// closure-wide `max_total_source_bytes`, `max_include_count` (non-entry
/// templates), and `max_include_depth` (the entry is depth 0; the first include
/// is depth 1; a child is rejected when its depth exceeds the bound).
#[allow(dead_code)] // pub(crate) function; closure DFS entry point consumed by Phase 4 (Task 4.x).
pub(crate) fn acquire_closure(
    reader: &dyn StableTemplateReader,
    entry: String,
    root: &OwnedHandle,
    limits: ResolvedExternalTemplateLimits,
) -> Result<ClosureSnapshot, TemplateReloadError> {
    let include_re = Regex::new(INCLUDE_RE_PATTERN)
        .map_err(|e| TemplateReloadError::Acquire(format!("include regex compile: {e}")))?;

    let mut walk = WalkState {
        entries: BTreeMap::new(),
        state: HashMap::new(),
        seen_identities: HashSet::new(),
        total_bytes: 0,
    };
    let mut include_count: u32 = 0;

    // Acquire the entry template at depth 0 (it is not itself an include, so it
    // does not count toward `max_include_count`).
    let entry_children = read_and_record(reader, root, &entry, limits, &include_re, &mut walk)?;
    let mut stack: Vec<Frame> = Vec::new();
    stack.push(Frame {
        name: entry,
        depth: 0,
        children: entry_children,
        child_idx: 0,
    });

    while !stack.is_empty() {
        // Decide the next step inside a block so the &mut borrow of `stack`
        // ends before the push/pop below. `None` means the top frame is done;
        // `Some((child, parent, depth))` is the next child to visit.
        let next: Option<(String, String, u32)> = {
            let Some(top) = stack.last_mut() else {
                break;
            };
            match top.children.get(top.child_idx).cloned() {
                Some(child) => {
                    top.child_idx += 1;
                    Some((child, top.name.clone(), top.depth + 1))
                }
                None => None,
            }
        };

        match next {
            None => {
                // All children of the top frame are visited: mark it Black.
                if let Some(frame) = stack.pop() {
                    walk.state.insert(frame.name, VisitState::Black);
                }
            }
            Some((child, parent, depth)) => {
                match walk.state.get(&child) {
                    // Back edge to a node still on the stack: a real cycle.
                    Some(VisitState::Gray) => {
                        return Err(TemplateReloadError::Cycle(format!("{parent} -> {child}")));
                    }
                    // Already fully acquired via another path: skip.
                    Some(VisitState::Black) => continue,
                    None => {}
                }
                if depth > limits.max_include_depth {
                    return Err(TemplateReloadError::BoundExceeded("max_include_depth"));
                }
                include_count = include_count.checked_add(1).ok_or_else(|| {
                    TemplateReloadError::Acquire("include count counter overflow".into())
                })?;
                if include_count > limits.max_include_count {
                    return Err(TemplateReloadError::BoundExceeded("max_include_count"));
                }
                let children =
                    read_and_record(reader, root, &child, limits, &include_re, &mut walk)?;
                stack.push(Frame {
                    name: child,
                    depth,
                    children,
                    child_idx: 0,
                });
            }
        }
    }

    Ok(ClosureSnapshot {
        entries: walk.entries,
    })
}

/// Read one template source through `reader`, record it (running byte total,
/// identity uniqueness, closure entry, Gray state), and return its statically
/// discovered child include targets.
#[allow(dead_code)] // DFS internal; consumed via acquire_closure (Phase 4).
fn read_and_record(
    reader: &dyn StableTemplateReader,
    root: &OwnedHandle,
    name: &str,
    limits: ResolvedExternalTemplateLimits,
    include_re: &Regex,
    walk: &mut WalkState,
) -> Result<Vec<String>, TemplateReloadError> {
    let (_handle, identity, bytes) = reader.read_relative(root, name, limits.max_template_size)?;

    walk.total_bytes = walk
        .total_bytes
        .checked_add(bytes.len())
        .ok_or_else(|| TemplateReloadError::Acquire("total source byte counter overflow".into()))?;
    if walk.total_bytes > limits.max_total_source_bytes {
        return Err(TemplateReloadError::BoundExceeded("max_total_source_bytes"));
    }

    if !walk.seen_identities.insert(identity.clone()) {
        return Err(TemplateReloadError::DuplicateIdentity(format!(
            "template {name:?} shares a file identity with another closure member"
        )));
    }

    let children = parse_includes(include_re, &bytes)?;

    let name_owned = name.to_string();
    walk.entries.insert(
        name_owned.clone(),
        TemplateFile {
            name: name_owned.clone(),
            bytes,
            identity,
        },
    );
    walk.state.insert(name_owned, VisitState::Gray);
    Ok(children)
}

/// Extract the statically discoverable include/extends/import/from targets from
/// MiniJinja source. A directive whose target is not a quoted string literal is
/// rejected as not statically discoverable.
#[allow(dead_code)] // DFS internal; consumed via acquire_closure (Phase 4).
fn parse_includes(re: &Regex, source: &[u8]) -> Result<Vec<String>, TemplateReloadError> {
    let text = std::str::from_utf8(source)
        .map_err(|e| TemplateReloadError::Acquire(format!("template source is not utf-8: {e}")))?;
    let mut out = Vec::new();
    for caps in re.captures_iter(text) {
        let rest = match caps.name("rest") {
            Some(m) => m.as_str(),
            None => "",
        };
        out.push(first_string_arg(rest)?);
    }
    Ok(out)
}

/// Parse the first argument of an include/extends/import/from directive. It
/// MUST be a single- or double-quoted string literal; any other token (a bare
/// variable, an expression, `{{ ... }}`) is a render-time-computed target and
/// is rejected.
#[allow(dead_code)] // DFS internal; consumed via acquire_closure (Phase 4).
fn first_string_arg(rest: &str) -> Result<String, TemplateReloadError> {
    let trimmed = rest.trim_start();
    let mut chars = trimmed.chars();
    let quote = match chars.next() {
        Some('"') => '"',
        Some('\'') => '\'',
        Some(_) | None => {
            return Err(TemplateReloadError::Acquire(format!(
                "include/extends/import/from target is not a string literal \
                 (dynamic targets are not statically discoverable): {trimmed:?}"
            )));
        }
    };
    // Collect the inner span up to the matching quote, then verify the closing
    // quote was actually present (handles an unterminated literal).
    let inner: String = chars.clone().take_while(|&c| c != quote).collect();
    if !chars.any(|c| c == quote) {
        return Err(TemplateReloadError::Acquire(
            "unterminated string literal in include target".into(),
        ));
    }
    if inner.is_empty() {
        return Err(TemplateReloadError::Acquire("empty include target".into()));
    }
    Ok(inner)
}

// ===========================================================================
// Production filesystem reader + snapshot assembly
// ===========================================================================

/// Production [`StableTemplateReader`] backed by [`OwnedHandle::open_relative`]
/// plus a bounded read via [`OwnedHandle::read_bounded`]. Used by
/// [`build_snapshot`] to assemble a [`ClosureSnapshot`] from real filesystem
/// files. The test-local `FsReader` (in the `tests` module below) exercises the
/// same contract; this is the canonical implementation consumed by Phase 4.
#[allow(dead_code)] // pub(crate) struct; consumed by Phase 4 (Task 4.x reader impl).
pub(crate) struct FilesystemTemplateReader;

impl StableTemplateReader for FilesystemTemplateReader {
    fn read_relative(
        &self,
        root: &OwnedHandle,
        name: &str,
        max_bytes: usize,
    ) -> Result<(OwnedHandle, FileIdentity, Vec<u8>), TemplateReloadError> {
        let (handle, identity) = OwnedHandle::open_relative(root, name, max_bytes)?;
        let bytes = handle.read_bounded(max_bytes)?;
        Ok((handle, identity, bytes))
    }
}

/// Acquire a [`ClosureSnapshot`] for `entry` openat-relative to a root handle.
///
/// The caller opens the root once via [`crate::path_util::open_root`] and
/// passes the resulting [`OwnedHandle`] BY REFERENCE; `build_snapshot` does
/// NOT open the root itself, so a long-lived handle (e.g. the
/// `ReloadHandler` in Task 5.1) can re-acquire snapshots across reloads
/// against the same handle without paying for an extra root open.
///
/// `entry` is the absolute entry path (caller passes `&PathBuf` which
/// coerces to `&Path`); the root-relative name passed to [`acquire_closure`]
/// is its `file_name()`. A path with no file name is rejected as
/// [`TemplateReloadError::PathEscape`] because it cannot be a root-relative
/// template reference.
#[allow(dead_code)] // pub(crate) function; consumed by Phase 4 (Task 4.x reader impl).
pub(crate) fn build_snapshot(
    entry: &std::path::Path,
    root: &OwnedHandle,
    limits: ResolvedExternalTemplateLimits,
) -> Result<ClosureSnapshot, TemplateReloadError> {
    let name = entry
        .file_name()
        .ok_or_else(|| {
            TemplateReloadError::PathEscape(format!("entry has no file name: {}", entry.display()))
        })?
        .to_string_lossy()
        .into_owned();
    let reader = FilesystemTemplateReader;
    acquire_closure(&reader, name, root, limits)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::path_util::open_root;
    use std::fs;
    use std::path::Path;

    /// Minimal test-local [`StableTemplateReader`] that opens files from a
    /// tempdir via [`OwnedHandle::open_relative`] and reads them with the
    /// canonical [`OwnedHandle::read_bounded`] — the SAME helper the
    /// production `FilesystemTemplateReader` uses, so test and production
    /// share read geometry. Kept local so this task is self-contained.
    struct FsReader;

    impl StableTemplateReader for FsReader {
        fn read_relative(
            &self,
            root: &OwnedHandle,
            name: &str,
            max_bytes: usize,
        ) -> Result<(OwnedHandle, FileIdentity, Vec<u8>), TemplateReloadError> {
            let (handle, identity) = OwnedHandle::open_relative(root, name, max_bytes)?;
            let bytes = handle.read_bounded(max_bytes)?;
            Ok((handle, identity, bytes))
        }
    }

    fn default_limits() -> ResolvedExternalTemplateLimits {
        ResolvedExternalTemplateLimits {
            max_total_source_bytes: 1024 * 1024,
            max_include_count: 64,
            max_include_depth: 16,
            max_template_size: 1024 * 1024,
            reload_timeout_ms: 5000,
        }
    }

    fn open_root_handle(dir: &Path) -> OwnedHandle {
        let (handle, _id) = open_root(dir).expect("root opens");
        handle
    }

    #[test]
    fn acquire_closure_flat() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("page.html"), b"<h1>hello</h1>").expect("write page");
        let root = open_root_handle(dir.path());
        let snap = acquire_closure(&FsReader, "page.html".to_string(), &root, default_limits())
            .expect("closure acquired");
        assert_eq!(snap.entries().len(), 1);
        assert!(snap.entries().contains_key("page.html"));
    }

    #[test]
    fn acquire_closure_transitive() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("a.html"), b"{% include \"b.html\" %}A").expect("write a");
        fs::write(dir.path().join("b.html"), b"{% include \"c.html\" %}B").expect("write b");
        fs::write(dir.path().join("c.html"), b"<p>leaf</p>").expect("write c");
        let root = open_root_handle(dir.path());
        let snap = acquire_closure(&FsReader, "a.html".to_string(), &root, default_limits())
            .expect("closure acquired");
        assert_eq!(snap.entries().len(), 3);
        for name in ["a.html", "b.html", "c.html"] {
            assert!(snap.entries().contains_key(name), "missing {name}");
        }
    }

    #[test]
    fn acquire_closure_rejects_cycle() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("a.html"), b"{% include \"b.html\" %}A").expect("write a");
        fs::write(dir.path().join("b.html"), b"{% include \"a.html\" %}B").expect("write b");
        let root = open_root_handle(dir.path());
        let err = acquire_closure(&FsReader, "a.html".to_string(), &root, default_limits())
            .expect_err("cycle must be rejected");
        assert!(
            matches!(err, TemplateReloadError::Cycle(_)),
            "expected Cycle, got {err:?}"
        );
    }

    #[test]
    fn acquire_closure_rejects_escape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        fs::write(outside.path().join("secret.html"), b"escaped").expect("write secret");
        fs::write(
            dir.path().join("a.html"),
            b"{% include \"../secret.html\" %}A",
        )
        .expect("write a");
        let root = open_root_handle(dir.path());
        let err = acquire_closure(&FsReader, "a.html".to_string(), &root, default_limits())
            .expect_err("escape must be rejected");
        assert!(
            matches!(err, TemplateReloadError::PathEscape(_)),
            "expected PathEscape, got {err:?}"
        );
    }

    #[test]
    fn acquire_closure_rejects_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        fs::write(outside.path().join("real.html"), b"escaped").expect("write real");
        symlink(
            outside.path().join("real.html"),
            dir.path().join("link.html"),
        )
        .expect("symlink");
        fs::write(dir.path().join("a.html"), b"{% include \"link.html\" %}A").expect("write a");
        let root = open_root_handle(dir.path());
        let err = acquire_closure(&FsReader, "a.html".to_string(), &root, default_limits())
            .expect_err("symlink must be rejected");
        assert!(
            matches!(err, TemplateReloadError::PathEscape(_)),
            "expected PathEscape, got {err:?}"
        );
    }

    #[test]
    fn acquire_closure_rejects_dynamic() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("a.html"), b"{% include {{x}} %}A").expect("write a");
        let root = open_root_handle(dir.path());
        let err = acquire_closure(&FsReader, "a.html".to_string(), &root, default_limits())
            .expect_err("dynamic target must be rejected");
        assert!(
            matches!(err, TemplateReloadError::Acquire(_)),
            "expected Acquire, got {err:?}"
        );
    }

    #[test]
    fn acquire_closure_rejects_absolute_include() {
        // `{% include "/etc/passwd" %}` would otherwise be able to read any
        // file the process can read. `validate_components` rejects a leading
        // `/` as an empty first component, surfacing as PathEscape.
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("a.html"), b"{% include \"/etc/passwd\" %}A").expect("write a");
        let root = open_root_handle(dir.path());
        let err = acquire_closure(&FsReader, "a.html".to_string(), &root, default_limits())
            .expect_err("absolute include must be rejected");
        assert!(
            matches!(err, TemplateReloadError::PathEscape(_)),
            "expected PathEscape, got {err:?}"
        );
    }

    #[test]
    fn acquire_closure_rejects_duplicate_identity() {
        // Two distinct names that resolve to the same inode (a hardlink) must
        // be rejected: a single physical file referenced under two names would
        // double-count source bytes and break deterministic hashing. Uses
        // `std::fs::hard_link` so `b.html` and `c.html` share inode/dev/length.
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("a.html"),
            b"{% include \"b.html\" %}{% include \"c.html\" %}A",
        )
        .expect("write a");
        fs::write(dir.path().join("b.html"), b"B").expect("write b");
        fs::hard_link(dir.path().join("b.html"), dir.path().join("c.html"))
            .expect("hardlink b -> c");
        let root = open_root_handle(dir.path());
        let err = acquire_closure(&FsReader, "a.html".to_string(), &root, default_limits())
            .expect_err("duplicate identity must be rejected");
        assert!(
            matches!(err, TemplateReloadError::DuplicateIdentity(_)),
            "expected DuplicateIdentity, got {err:?}"
        );
    }

    #[test]
    fn build_snapshot_real_files() {
        // Arrange: a tempdir root with a page that includes a header.
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("header.html"), b"<h1>Hello</h1>").expect("write header");
        fs::write(
            dir.path().join("page.html"),
            b"{% include \"header.html\" %}Body",
        )
        .expect("write page");
        let entry = dir.path().join("page.html");
        let (root, _id) = open_root(dir.path()).expect("open root");

        // Act: build two snapshots reusing the same root handle.
        let snap1 = build_snapshot(&entry, &root, default_limits()).expect("snapshot 1 acquired");
        let snap2 = build_snapshot(&entry, &root, default_limits()).expect("snapshot 2 acquired");

        // Assert: 2 entries (page + included header) and a stable hash.
        assert_eq!(snap1.entries().len(), 2, "page + header");
        assert!(snap1.entries().contains_key("page.html"));
        assert!(snap1.entries().contains_key("header.html"));
        assert_eq!(
            snap1.deterministic_hash(),
            snap2.deterministic_hash(),
            "deterministic_hash must be stable across calls with the same root"
        );
    }

    #[test]
    fn build_snapshot_rejects_oversize() {
        // Arrange: a file larger than the configured `max_template_size`.
        let dir = tempfile::tempdir().expect("tempdir");
        let big = vec![b'x'; 1024];
        fs::write(dir.path().join("page.html"), &big).expect("write page");
        let entry = dir.path().join("page.html");
        let (root, _id) = open_root(dir.path()).expect("open root");

        // Tight limit: 100 bytes per template.
        let tight = ResolvedExternalTemplateLimits {
            max_total_source_bytes: 100,
            max_include_count: 1,
            max_include_depth: 1,
            max_template_size: 100,
            reload_timeout_ms: 5000,
        };

        // Act
        let err = build_snapshot(&entry, &root, tight).expect_err("oversize must fail");

        // Assert
        assert!(
            matches!(err, TemplateReloadError::BoundExceeded(_)),
            "expected BoundExceeded, got {err:?}"
        );
    }
}
