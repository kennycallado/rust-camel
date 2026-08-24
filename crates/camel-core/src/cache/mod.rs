//! Pluggable cache repository implementations.
//!
//! Each backend implements [`camel_api::cache::CacheRepository`] and is
//! registered with the runtime through the cache registry.

pub mod disk_offload;
pub mod memory;
pub mod redb;

pub use disk_offload::DiskOffloadRepository;
pub use memory::MemoryCacheRepository;
pub use redb::RedbCacheRepository;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::cache::MemoryCacheRepository;
    use crate::context::CamelContext;
    use crate::registry::RegistryError;

    #[tokio::test]
    async fn memory_cache_registered_as_default() {
        let ctx = CamelContext::builder()
            .build()
            .await
            .expect("build context");
        let repo = ctx.cache_repository("memory");
        assert!(repo.is_some());
        assert_eq!(repo.unwrap().name(), "memory");
    }

    #[tokio::test]
    async fn custom_backend_registered_alongside_memory() {
        let mut ctx = CamelContext::builder()
            .build()
            .await
            .expect("build context");
        let custom = Arc::new(MemoryCacheRepository::new("custom", 100));
        ctx.register_cache_repository("custom", custom)
            .expect("custom cache repository registration should succeed");
        assert!(ctx.cache_repository("custom").is_some());
        assert!(ctx.cache_repository("memory").is_some());
    }

    #[tokio::test]
    async fn duplicate_registration_rejected() {
        let mut ctx = CamelContext::builder()
            .build()
            .await
            .expect("build context");
        let dup = Arc::new(MemoryCacheRepository::new("memory-dup", 100));
        let err = ctx.register_cache_repository("memory", dup).unwrap_err();
        assert!(
            matches!(&err, RegistryError::AlreadyRegistered(name) if name == "memory"),
            "expected AlreadyRegistered('memory'), got {err:?}"
        );
    }
}
