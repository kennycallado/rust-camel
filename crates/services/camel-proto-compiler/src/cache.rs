use std::collections::HashMap;
use std::collections::VecDeque;
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
            "{}:{}",
            proto_path.display(),
            hash_proto_content(proto_path)?
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
