//! Library surface for the `xtask` binary.
//!
//! The archive wrapper (`crate::archive`) is exposed so the
//! `#[ignore]` integration tests in `tests/archive_e2e.rs` can drive
//! `run` against a scratch openspec root in a tempdir; the binary's
//! `main.rs` consumes the same module through this lib target.

pub mod archive;
