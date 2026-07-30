//! Idempotent repository implementations.

pub mod memory_repository;
pub mod redb_repository;
pub use memory_repository::MemoryIdempotentRepository;
pub use redb_repository::RedbIdempotentRepository;
