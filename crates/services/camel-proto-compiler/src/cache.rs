use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use prost_reflect::DescriptorPool;

use crate::compiler::compile_proto;
use crate::{ProtoCompileError, hash_proto_content};

#[derive(Default)]
pub struct ProtoCache {
    pools: Mutex<HashMap<String, DescriptorPool>>,
}

impl ProtoCache {
    pub fn new() -> Self {
        Self::default()
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
            .expect("mutex poisoned")
            .get(&key)
            .cloned()
        {
            return Ok(pool);
        }

        let pool = compile_proto(proto_path, &include_paths)?;

        let mut guard = self.pools.lock().expect("mutex poisoned");
        guard.insert(key, pool.clone());
        Ok(pool)
    }

    pub fn len(&self) -> usize {
        self.pools.lock().expect("mutex poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.pools.lock().expect("mutex poisoned").is_empty()
    }
}
