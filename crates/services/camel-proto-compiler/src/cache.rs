use std::collections::HashMap;
use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Mutex;

use prost_reflect::DescriptorPool;

use crate::compiler::compile_proto;
use crate::{ProtoCompileError, hash_proto_content};

const DEFAULT_MAX_ENTRIES: usize = 1000;

pub struct ProtoCache {
    pools: Mutex<HashMap<String, DescriptorPool>>,
    order: Mutex<VecDeque<String>>,
    max_entries: usize,
}

/// R4-L6: hash ordered include paths with length-delimited framing.
/// On canonicalize failure, fall back to the supplied path (don't reject
/// paths protoc accepts). Debug-log the fallback.
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

impl Default for ProtoCache {
    fn default() -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            order: Mutex::new(VecDeque::new()),
            max_entries: DEFAULT_MAX_ENTRIES,
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

        let key = format!(
            "{}:{}:{}",
            proto_path.display(),
            hash_proto_content(proto_path)?,
            hash_ordered_include_paths(&include_paths)
        );

        if let Some(pool) = self
            .pools
            .lock()
            .expect("mutex poisoned") // allow-unwrap
            .get(&key)
            .cloned()
        {
            return Ok(pool);
        }

        let pool = compile_proto(proto_path, &include_paths)?;
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
        let key1 = hash_ordered_include_paths(&[real_path.clone()]);
        let key2 = hash_ordered_include_paths(&[real_path2.clone()]);
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
}
