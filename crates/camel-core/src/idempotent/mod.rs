//! Idempotent repository implementations.

pub mod memory_repository;
pub use memory_repository::MemoryIdempotentRepository;
