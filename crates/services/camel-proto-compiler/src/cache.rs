use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use prost_reflect::DescriptorPool;
use sha2::{Digest, Sha256};

use crate::compiler::{
    SourceSnapshot, compile_from_snapshot, compile_proto, compile_proto_embedded,
    is_descriptor_set, preload_embedded_snapshot, preload_snapshot_contained,
};
use crate::{ProtoCompileError, hash_bytes, hash_proto_content};

const DEFAULT_MAX_ENTRIES: usize = 1000;

pub struct ProtoCache {
    pools: Mutex<HashMap<String, DescriptorPool>>,
    order: Mutex<VecDeque<String>>,
    max_entries: usize,
    /// Test-only count of compile invocations (cache misses that reached the
    /// compiler). Never bumped on a cache hit.
    #[cfg(test)]
    compiles: std::sync::atomic::AtomicUsize,
}

/// R4-L6: hash ordered include paths with length-delimited framing.
/// On canonicalize failure, fall back to the supplied path (don't reject
/// paths the compiler accepts). Debug-log the fallback.
fn hash_ordered_include_paths(paths: &[std::path::PathBuf]) -> String {
    let mut hasher = DefaultHasher::new();
    for p in paths {
        let canonical = std::fs::canonicalize(p).unwrap_or_else(|_| {
            tracing::debug!(path = %p.display(), "include path canonicalize failed; using raw path in cache key");
            p.clone()
        });
        let s = canonical.to_string_lossy();
        s.len().to_string().hash(&mut hasher);
        b':'.hash(&mut hasher);
        s.hash(&mut hasher);
        b';'.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// Source cache key: `path : root-content-hash : include-paths-hash :
/// fingerprint-sha256`.
///
/// `root-content-hash` is the SHA-256 of the root bytes: the file bytes on
/// disk, or the registered bytes for an embedded asset. Hashing the root up
/// front also preserves the historical early-read
/// `ProtoCompileError::Io` surface for an unreadable root.
///
/// The fingerprint covers the root plus its transitive imports as resolved:
/// each canonical import name maps to a tagged content hash (`b'S'` = source
/// text hash, `b'B'` = built-in/immutable input with no digest). A BTreeMap
/// orders names, so the encoding is stable across preload order. Missing
/// imports never reach this function: they produce no key and cache nothing.
fn source_cache_key(
    proto_path: &Path,
    root_hash: &str,
    include_paths: &[PathBuf],
    snapshot: &SourceSnapshot,
) -> String {
    let mut files: BTreeMap<String, (u8, Option<[u8; 32]>)> = BTreeMap::new();
    for (name, file) in snapshot.resolved_files() {
        match file.source() {
            Some(text) => {
                let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
                files.insert(name.to_owned(), (b'S', Some(digest)));
            }
            None => {
                files.insert(name.to_owned(), (b'B', None));
            }
        }
    }

    let mut buf = Vec::new();
    for (name, (tag, digest)) in &files {
        buf.extend_from_slice(&(name.len() as u64).to_le_bytes());
        buf.extend_from_slice(name.as_bytes());
        buf.push(*tag);
        if let Some(digest) = digest {
            buf.extend_from_slice(digest);
        }
    }
    let fingerprint = hash_bytes(&buf);

    format!(
        "{}:{}:{}:{}",
        proto_path.display(),
        root_hash,
        hash_ordered_include_paths(include_paths),
        fingerprint
    )
}

impl Default for ProtoCache {
    fn default() -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            order: Mutex::new(VecDeque::new()),
            max_entries: DEFAULT_MAX_ENTRIES,
            #[cfg(test)]
            compiles: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl ProtoCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cache with a custom maximum number of entries.
    ///
    /// When the cache reaches capacity, the oldest entry is evicted (FIFO).
    pub fn with_max_entries(max_entries: usize) -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            order: Mutex::new(VecDeque::with_capacity(max_entries)),
            max_entries: max_entries.max(1),
            #[cfg(test)]
            compiles: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn insert_entry(&self, key: String, pool: DescriptorPool) {
        let mut pools = self.pools.lock().expect("mutex poisoned"); // allow-unwrap
        let mut order = self.order.lock().expect("mutex poisoned"); // allow-unwrap

        // If key already exists, just update the pool (no eviction needed).
        // We cannot use the entry API here because we need the key for the
        // order queue below, and entry() takes ownership.
        #[allow(clippy::map_entry)]
        if pools.contains_key(&key) {
            pools.insert(key, pool);
            return;
        }

        // Evict oldest entries until we have room.
        while pools.len() >= self.max_entries {
            if let Some(old_key) = order.pop_front() {
                pools.remove(&old_key);
            } else {
                break;
            }
        }

        // A concurrent miss on the same key can push it twice; drop any
        // stale occurrence so eviction cannot remove a live entry.
        order.retain(|queued| queued != &key);
        order.push_back(key.clone());
        pools.insert(key, pool);
    }

    pub fn get_or_compile<P, I>(
        &self,
        proto_path: P,
        includes: I,
    ) -> Result<DescriptorPool, ProtoCompileError>
    where
        P: AsRef<Path>,
        I: IntoIterator,
        I::Item: AsRef<Path>,
    {
        let proto_path = proto_path.as_ref();
        let include_paths = includes
            .into_iter()
            .map(|p| p.as_ref().to_path_buf())
            .collect::<Vec<_>>();

        // An embedded reference hashes the registered bytes for descriptor
        // sets and preloads the embedded closure for sources — never the
        // filesystem. An unregistered reference fails closed here.
        if let Some(name) = crate::embedded::strip_ref(proto_path) {
            let bytes = crate::embedded::get(name)
                .ok_or_else(|| ProtoCompileError::ProtoNotFound(proto_path.to_path_buf()))?;
            if is_descriptor_set(Path::new(name)) {
                let key = format!("{}:{}", proto_path.display(), hash_bytes(&bytes));
                if let Some(pool) = self.lookup(&key) {
                    return Ok(pool);
                }
                #[cfg(test)]
                self.compiles
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let pool = compile_proto_embedded(proto_path, bytes)?;
                self.insert_entry(key, pool.clone());
                return Ok(pool);
            }
            let root_hash = hash_bytes(&bytes);
            let snapshot = preload_embedded_snapshot(proto_path)?;
            return self.compile_source_snapshot(proto_path, &root_hash, &include_paths, snapshot);
        }

        // Includes are ignored by a descriptor-set compile, so they must not
        // participate in its cache key.
        if is_descriptor_set(proto_path) {
            let key = format!(
                "{}:{}",
                proto_path.display(),
                hash_proto_content(proto_path)?
            );
            if let Some(pool) = self.lookup(&key) {
                return Ok(pool);
            }
            #[cfg(test)]
            self.compiles
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let pool = compile_proto(proto_path, &include_paths)?;
            self.insert_entry(key, pool.clone());
            return Ok(pool);
        }

        let root_hash = hash_proto_content(proto_path)?;
        let snapshot = preload_snapshot_contained(proto_path, &include_paths)?;
        self.compile_source_snapshot(proto_path, &root_hash, &include_paths, snapshot)
    }

    fn lookup(&self, key: &str) -> Option<DescriptorPool> {
        self.pools
            .lock()
            .expect("mutex poisoned") // allow-unwrap
            .get(key)
            .cloned()
    }

    /// Serves a preloaded source snapshot from the cache, or compiles and
    /// caches it. A snapshot with a missing import never gets a key: the
    /// compiler reports the canonical error and nothing is cached.
    fn compile_source_snapshot(
        &self,
        proto_path: &Path,
        root_hash: &str,
        include_paths: &[PathBuf],
        snapshot: SourceSnapshot,
    ) -> Result<DescriptorPool, ProtoCompileError> {
        if snapshot.has_missing() {
            #[cfg(test)]
            self.compiles
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return compile_from_snapshot(proto_path, snapshot);
        }
        let key = source_cache_key(proto_path, root_hash, include_paths, &snapshot);
        if let Some(pool) = self.lookup(&key) {
            return Ok(pool);
        }
        #[cfg(test)]
        self.compiles
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pool = compile_from_snapshot(proto_path, snapshot)?;
        self.insert_entry(key, pool.clone());
        Ok(pool)
    }

    pub fn len(&self) -> usize {
        self.pools.lock().expect("mutex poisoned").len() // allow-unwrap
    }

    pub fn is_empty(&self) -> bool {
        self.pools.lock().expect("mutex poisoned").is_empty() // allow-unwrap
    }
}

#[cfg(test)]
impl ProtoCache {
    /// Number of compile invocations observed by this cache.
    fn compiles(&self) -> usize {
        self.compiles.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_distinguishes_include_paths() {
        let a = hash_ordered_include_paths(&[std::path::PathBuf::from("/proto")]);
        let b = hash_ordered_include_paths(&[std::path::PathBuf::from("/other")]);
        assert_ne!(a, b);
    }

    #[test]
    fn cache_key_order_significant() {
        let a = hash_ordered_include_paths(&[
            std::path::PathBuf::from("/a"),
            std::path::PathBuf::from("/b"),
        ]);
        let b = hash_ordered_include_paths(&[
            std::path::PathBuf::from("/b"),
            std::path::PathBuf::from("/a"),
        ]);
        assert_ne!(a, b);
    }

    #[test]
    fn cache_key_length_delimited_no_collision() {
        let a = hash_ordered_include_paths(&[
            std::path::PathBuf::from("a"),
            std::path::PathBuf::from("bc"),
        ]);
        let b = hash_ordered_include_paths(&[
            std::path::PathBuf::from("ab"),
            std::path::PathBuf::from("c"),
        ]);
        assert_ne!(a, b);
    }

    #[test]
    fn cache_key_canonicalize_success_branch() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let real_path = dir.path().to_path_buf();
        // The canonicalized path may differ from the raw path (symlinks, /private on macOS).
        // Two different real dirs must produce different keys.
        let dir2 = TempDir::new().unwrap();
        let real_path2 = dir2.path().to_path_buf();
        let key1 = hash_ordered_include_paths(std::slice::from_ref(&real_path));
        let key2 = hash_ordered_include_paths(std::slice::from_ref(&real_path2));
        assert_ne!(
            key1, key2,
            "two distinct real dirs must produce different cache keys"
        );
        // Also: a real canonicalized path should hash differently from a nonexistent
        // raw string of the same display() form (proves canonicalize is actually called).
        let _raw_string = std::path::PathBuf::from(real_path.to_string_lossy().to_string());
        // If canonicalize resolved a symlink, these differ; if not (same path), they're equal —
        // so just assert the real-dir path produces a valid hash (16 hex chars).
        let key_real = hash_ordered_include_paths(&[real_path]);
        assert_eq!(key_real.len(), 16, "hash must be 16 hex chars");
    }

    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    fn write_proto(dir: &Path, name: &str, contents: &str) {
        fs::write(dir.join(name), contents).expect("write proto");
    }

    #[test]
    fn unchanged_imports_hit() {
        let dir = TempDir::new().expect("tmp dir");
        write_proto(
            dir.path(),
            "common.proto",
            "syntax = \"proto3\";\npackage common;\nmessage Common { string v1 = 1; }\n",
        );
        write_proto(
            dir.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage root_t11;\nimport \"common.proto\";\nmessage Root { common.Common c = 1; }\n",
        );

        let cache = ProtoCache::new();
        let root = dir.path().join("root.proto");

        let first = cache
            .get_or_compile(&root, [dir.path()])
            .expect("first compile");
        assert!(first.get_message_by_name("root_t11.Root").is_some());

        let before = cache.compiles();
        let second = cache
            .get_or_compile(&root, [dir.path()])
            .expect("second compile");
        assert_eq!(
            cache.compiles(),
            before,
            "identical root and imports must be a cache hit"
        );
        assert_eq!(cache.len(), 1);
        assert!(second.get_message_by_name("root_t11.Root").is_some());
    }

    #[test]
    fn imported_file_change_miss() {
        let dir = TempDir::new().expect("tmp dir");
        write_proto(
            dir.path(),
            "common.proto",
            "syntax = \"proto3\";\npackage common;\nmessage Common { string v1 = 1; }\n",
        );
        write_proto(
            dir.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage root_t12;\nimport \"common.proto\";\nmessage Root { common.Common c = 1; }\n",
        );

        let cache = ProtoCache::new();
        let root = dir.path().join("root.proto");

        let first = cache
            .get_or_compile(&root, [dir.path()])
            .expect("first compile");
        assert!(
            first
                .get_message_by_name("common.Common")
                .expect("common message")
                .get_field_by_name("v1")
                .is_some()
        );
        assert_eq!(cache.compiles(), 1);
        assert_eq!(cache.len(), 1);

        // Only the imported file changes; the root bytes stay identical.
        write_proto(
            dir.path(),
            "common.proto",
            "syntax = \"proto3\";\npackage common;\nmessage Common { string v1 = 1; int32 extra = 2; }\n",
        );

        let second = cache
            .get_or_compile(&root, [dir.path()])
            .expect("second compile");
        assert!(
            second
                .get_message_by_name("common.Common")
                .expect("common message")
                .get_field_by_name("extra")
                .is_some(),
            "an imported-file change must invalidate the cached pool"
        );
        assert_eq!(
            cache.len(),
            2,
            "a changed import must create a new cache entry"
        );
    }

    #[test]
    fn transitive_import_change_miss() {
        let dir = TempDir::new().expect("tmp dir");
        write_proto(
            dir.path(),
            "leaf.proto",
            "syntax = \"proto3\";\npackage leaf;\nmessage Leaf { string v1 = 1; }\n",
        );
        write_proto(
            dir.path(),
            "mid.proto",
            "syntax = \"proto3\";\npackage mid;\nimport \"leaf.proto\";\nmessage Mid { leaf.Leaf l = 1; }\n",
        );
        write_proto(
            dir.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage root_t13;\nimport \"mid.proto\";\nmessage Root { mid.Mid m = 1; }\n",
        );

        let cache = ProtoCache::new();
        let root = dir.path().join("root.proto");

        let first = cache
            .get_or_compile(&root, [dir.path()])
            .expect("first compile");
        assert!(
            first
                .get_message_by_name("leaf.Leaf")
                .expect("leaf message")
                .get_field_by_name("v1")
                .is_some()
        );

        write_proto(
            dir.path(),
            "leaf.proto",
            "syntax = \"proto3\";\npackage leaf;\nmessage Leaf { string v1 = 1; int32 extra = 2; }\n",
        );

        let second = cache
            .get_or_compile(&root, [dir.path()])
            .expect("second compile");
        assert!(
            second
                .get_message_by_name("leaf.Leaf")
                .expect("leaf message")
                .get_field_by_name("extra")
                .is_some(),
            "a transitive import change must invalidate the cached pool"
        );
        assert_eq!(
            cache.len(),
            2,
            "a transitive import change must create a new cache entry"
        );
    }

    #[test]
    fn import_add_then_remove() {
        const ROOT_V1: &str =
            "syntax = \"proto3\";\npackage root_t14;\nmessage RootV1 { string a = 1; }\n";
        let dir = TempDir::new().expect("tmp dir");
        write_proto(
            dir.path(),
            "common.proto",
            "syntax = \"proto3\";\npackage common;\nmessage Common { string v1 = 1; }\n",
        );
        write_proto(dir.path(), "root.proto", ROOT_V1);

        let cache = ProtoCache::new();
        let root = dir.path().join("root.proto");

        cache
            .get_or_compile(&root, [dir.path()])
            .expect("v1 compile");
        assert_eq!(cache.len(), 1);

        // v2 adds an import.
        write_proto(
            dir.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage root_t14;\nimport \"common.proto\";\nmessage RootV2 { common.Common c = 1; }\n",
        );
        let v2 = cache
            .get_or_compile(&root, [dir.path()])
            .expect("v2 compile");
        assert!(v2.get_message_by_name("root_t14.RootV2").is_some());
        assert_eq!(cache.compiles(), 2, "adding an import must miss");
        assert_eq!(cache.len(), 2);

        // v3 restores the exact v1 bytes. This is a HIT, not a miss as the
        // mission's literal "removed import -> miss" wording suggests: the
        // resolved input set is again just the root file, so the original key
        // is restored and serving entry 1 is correct (its pool is identical
        // to what a fresh compile would produce). Keying on file mtimes is
        // rejected: mtime is not content, is not portable across filesystems,
        // and does not catch a restore that preserves mtime.
        write_proto(dir.path(), "root.proto", ROOT_V1);
        let before = cache.compiles();
        let v3 = cache
            .get_or_compile(&root, [dir.path()])
            .expect("v3 compile");
        assert_eq!(
            cache.compiles(),
            before,
            "restoring the original resolved input set must hit the original entry"
        );
        assert_eq!(cache.len(), 2);
        assert!(v3.get_message_by_name("root_t14.RootV1").is_some());
    }

    #[test]
    fn missing_import_no_partial_key() {
        let dir = TempDir::new().expect("tmp dir");
        write_proto(
            dir.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage root_t15;\nimport \"missing.proto\";\nmessage Root { string a = 1; }\n",
        );

        let cache = ProtoCache::new();
        let root = dir.path().join("root.proto");

        let err = cache
            .get_or_compile(&root, [dir.path()])
            .expect_err("a missing import must fail");
        match &err {
            ProtoCompileError::Compile { detail, .. } => {
                assert!(
                    detail.contains("missing.proto"),
                    "error detail should name the missing import, got: {detail}"
                );
            }
            other => panic!("expected Compile variant, got {other:?}"),
        }
        assert_eq!(cache.len(), 0, "a failed compile must not be cached");

        // Once the import exists, the same inputs compile and cache normally:
        // there is no negative caching of the earlier failure.
        write_proto(
            dir.path(),
            "missing.proto",
            "syntax = \"proto3\";\npackage missing;\nmessage Missing { string x = 1; }\n",
        );
        let pool = cache
            .get_or_compile(&root, [dir.path()])
            .expect("compile after the import is written");
        assert!(pool.get_message_by_name("root_t15.Root").is_some());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn different_roots_same_import_separate() {
        let dir = TempDir::new().expect("tmp dir");
        write_proto(
            dir.path(),
            "common.proto",
            "syntax = \"proto3\";\npackage common;\nmessage Common { string v = 1; }\n",
        );
        write_proto(
            dir.path(),
            "a.proto",
            "syntax = \"proto3\";\npackage a;\nimport \"common.proto\";\nmessage A { common.Common c = 1; }\n",
        );
        write_proto(
            dir.path(),
            "b.proto",
            "syntax = \"proto3\";\npackage b;\nimport \"common.proto\";\nmessage B { common.Common c = 1; }\n",
        );

        let cache = ProtoCache::new();
        let a = cache
            .get_or_compile(dir.path().join("a.proto"), [dir.path()])
            .expect("compile a");
        let b = cache
            .get_or_compile(dir.path().join("b.proto"), [dir.path()])
            .expect("compile b");

        assert_eq!(cache.len(), 2);
        assert!(a.get_message_by_name("a.A").is_some());
        assert!(b.get_message_by_name("b.B").is_some());
    }

    #[test]
    fn wkt_import_hits() {
        let dir = TempDir::new().expect("tmp dir");
        write_proto(
            dir.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage root_t17;\nimport \"google/protobuf/timestamp.proto\";\nmessage Root { google.protobuf.Timestamp ts = 1; }\n",
        );

        let cache = ProtoCache::new();
        let root = dir.path().join("root.proto");

        let first = cache
            .get_or_compile(&root, [dir.path()])
            .expect("first compile");
        assert!(
            first
                .get_message_by_name("root_t17.Root")
                .expect("root message")
                .get_field_by_name("ts")
                .is_some()
        );
        assert!(
            first
                .get_message_by_name("google.protobuf.Timestamp")
                .is_some()
        );

        let before = cache.compiles();
        let second = cache
            .get_or_compile(&root, [dir.path()])
            .expect("second compile");
        assert_eq!(cache.compiles(), before, "unchanged WKT import must hit");
        assert_eq!(cache.len(), 1);
        assert!(
            second
                .get_message_by_name("google.protobuf.Timestamp")
                .is_some()
        );
    }

    #[test]
    fn include_shadow_change_misses() {
        let dir = TempDir::new().expect("tmp dir");
        let dir1 = dir.path().join("dir1");
        let dir2 = dir.path().join("dir2");
        fs::create_dir_all(&dir1).expect("create dir1");
        fs::create_dir_all(&dir2).expect("create dir2");

        write_proto(
            &dir1,
            "common.proto",
            "syntax = \"proto3\";\npackage common_t18;\nmessage Common { string v1 = 1; }\n",
        );
        write_proto(
            &dir2,
            "common.proto",
            "syntax = \"proto3\";\npackage common_t18;\nmessage Common { string v2 = 1; }\n",
        );
        write_proto(
            dir.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage root_t18;\nimport \"common.proto\";\nmessage Root { common_t18.Common c = 1; }\n",
        );

        let cache = ProtoCache::new();
        let root = dir.path().join("root.proto");

        let first = cache
            .get_or_compile(&root, [dir1.as_path(), dir2.as_path()])
            .expect("compile with dir1 shadowing");
        assert!(
            first
                .get_message_by_name("common_t18.Common")
                .expect("common message")
                .get_field_by_name("v1")
                .is_some()
        );
        assert_eq!(cache.compiles(), 1);
        assert_eq!(cache.len(), 1);

        let second = cache
            .get_or_compile(&root, [dir2.as_path(), dir1.as_path()])
            .expect("compile with dir2 shadowing");
        assert!(
            second
                .get_message_by_name("common_t18.Common")
                .expect("common message")
                .get_field_by_name("v2")
                .is_some(),
            "a shadowing include change must invalidate the cached pool"
        );
        assert_eq!(cache.compiles(), 2, "a changed shadow must miss");
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn descriptor_set_key_regression() {
        let dir = TempDir::new().expect("tmp dir");
        write_proto(
            dir.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage pkg_t19_root;\nmessage Root { string name = 1; }\n",
        );

        let pool = crate::compile_proto(dir.path().join("root.proto"), [dir.path()])
            .expect("compile source for descriptor set");
        let bytes = pool.encode_to_vec();
        let set = dir.path().join("set.binpb");
        fs::write(&set, &bytes).expect("write descriptor set");

        let cache = ProtoCache::new();
        let first = cache
            .get_or_compile(&set, [dir.path()])
            .expect("first descriptor-set load");
        assert!(first.get_message_by_name("pkg_t19_root.Root").is_some());

        let before = cache.compiles();
        let second = cache
            .get_or_compile(&set, [dir.path()])
            .expect("second descriptor-set load");
        assert_eq!(
            cache.compiles(),
            before,
            "identical descriptor-set bytes must hit"
        );
        assert_eq!(cache.len(), 1);
        assert!(second.get_message_by_name("pkg_t19_root.Root").is_some());

        // Change one byte inside the encoded package name: same length, still
        // a valid descriptor set, but different content -> a miss.
        let needle = b"pkg_t19_root";
        let mut mutated = bytes.clone();
        let pos = mutated
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("encoded package name is present");
        mutated[pos + needle.len() - 1] = b's';
        fs::write(&set, &mutated).expect("write mutated descriptor set");

        let before = cache.compiles();
        let changed = cache
            .get_or_compile(&set, [dir.path()])
            .expect("changed descriptor-set bytes load");
        assert_eq!(
            cache.compiles(),
            before + 1,
            "changed descriptor-set bytes must miss"
        );
        assert_eq!(cache.len(), 2);
        assert!(
            changed.get_message_by_name("pkg_t19_roos.Root").is_some(),
            "the changed package must be visible"
        );

        // Include directories are ignored for descriptor sets, so a different
        // include list must still hit the same entry.
        let other = TempDir::new().expect("other tmp dir");
        let before = cache.compiles();
        cache
            .get_or_compile(&set, [other.path()])
            .expect("descriptor-set load with different includes");
        assert_eq!(
            cache.compiles(),
            before,
            "includes must not affect the descriptor-set cache key"
        );
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn embedded_source_hit() {
        crate::embedded::register(
            "test-400/emb400-common.proto",
            b"syntax = \"proto3\";\npackage emb400;\nmessage Common { string v = 1; }\n".to_vec(),
        );
        crate::embedded::register(
            "test-400/unique-emb.proto",
            b"syntax = \"proto3\";\npackage emb400;\nimport \"test-400/emb400-common.proto\";\nmessage Root { emb400.Common c = 1; }\n"
                .to_vec(),
        );

        let cache = ProtoCache::new();
        let reference = "camel-embedded:test-400/unique-emb.proto";

        let first = cache
            .get_or_compile(reference, std::iter::empty::<&Path>())
            .expect("first embedded compile");
        assert!(first.get_message_by_name("emb400.Root").is_some());

        let before = cache.compiles();
        let second = cache
            .get_or_compile(reference, std::iter::empty::<&Path>())
            .expect("second embedded compile");
        assert_eq!(
            cache.compiles(),
            before,
            "unchanged embedded bytes must hit"
        );
        assert_eq!(cache.len(), 1);
        assert!(second.get_message_by_name("emb400.Root").is_some());
    }

    #[test]
    fn unreadable_root_surfaces_io() {
        let dir = TempDir::new().expect("tmp dir");
        let cache = ProtoCache::new();
        let missing = dir.path().join("definitely-missing.proto");

        let err = cache
            .get_or_compile(&missing, std::iter::once(dir.path()))
            .expect_err("a missing root must fail");

        assert!(
            matches!(err, ProtoCompileError::Io(_)),
            "an unreadable root must surface Io, got {err:?}"
        );
        assert_eq!(cache.len(), 0, "a failed root read must cache nothing");
    }

    #[test]
    fn unreadable_import_dir_fails_clean() {
        let dir = TempDir::new().expect("tmp dir");
        write_proto(
            dir.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage root_dir_import;\nimport \"sub/dep.proto\";\nmessage Root { string a = 1; }\n",
        );
        // The include-resolved import name points at a directory, so opening
        // it as a file fails with a non-not-found I/O error.
        fs::create_dir(dir.path().join("sub")).expect("create sub");
        fs::create_dir(dir.path().join("sub/dep.proto")).expect("create dep.proto as dir");

        let cache = ProtoCache::new();
        let root = dir.path().join("root.proto");
        let before = cache.compiles();

        let err = cache
            .get_or_compile(&root, [dir.path()])
            .expect_err("a directory import must fail cleanly");

        // The preload stage surfaces the directory read as a Compile error;
        // this is the pre-existing failure path, and clean failure (no panic,
        // nothing cached) is what this test pins.
        assert!(
            matches!(err, ProtoCompileError::Compile { .. }),
            "a directory import must surface Compile, got {err:?}"
        );
        assert_eq!(cache.len(), 0, "a failed import must cache nothing");
        assert_eq!(
            cache.compiles(),
            before,
            "no compile may run for an unreadable import"
        );
    }

    #[test]
    fn embedded_descriptor_set_hit_miss() {
        let dir = TempDir::new().expect("tmp dir");
        write_proto(
            dir.path(),
            "root.proto",
            "syntax = \"proto3\";\npackage emb400_desc;\nmessage Root { string name = 1; }\n",
        );

        let pool = crate::compile_proto(dir.path().join("root.proto"), [dir.path()])
            .expect("compile source for embedded descriptor set");
        let bytes = pool.encode_to_vec();
        let name = "emb400-desc/set.binpb";
        crate::embedded::register(name, bytes.clone());

        let cache = ProtoCache::new();
        let reference = "camel-embedded:emb400-desc/set.binpb";

        let first = cache
            .get_or_compile(reference, std::iter::empty::<&Path>())
            .expect("first embedded descriptor-set load");
        assert!(first.get_message_by_name("emb400_desc.Root").is_some());

        let before = cache.compiles();
        let second = cache
            .get_or_compile(reference, std::iter::empty::<&Path>())
            .expect("second embedded descriptor-set load");
        assert_eq!(
            cache.compiles(),
            before,
            "unchanged embedded descriptor-set bytes must hit"
        );
        assert_eq!(cache.len(), 1);
        assert!(second.get_message_by_name("emb400_desc.Root").is_some());

        // Flip a byte in the middle of the payload: the key must change, so
        // the third call is a miss whether or not the bytes still decode.
        let mut mutated = bytes;
        let mid = mutated.len() / 2;
        mutated[mid] ^= 0xFF;
        crate::embedded::register(name, mutated);

        let before_miss = cache.compiles();
        match cache.get_or_compile(reference, std::iter::empty::<&Path>()) {
            Ok(_) => {
                assert_eq!(
                    cache.compiles(),
                    before_miss + 1,
                    "changed embedded descriptor-set bytes must miss"
                );
                assert_eq!(cache.len(), 2, "a changed descriptor set adds an entry");
            }
            Err(_) => {
                assert_eq!(
                    cache.compiles(),
                    before_miss + 1,
                    "an undecodable change is still a miss"
                );
                assert_eq!(
                    cache.len(),
                    1,
                    "a rejected descriptor set must not be cached"
                );
            }
        }
    }
}
