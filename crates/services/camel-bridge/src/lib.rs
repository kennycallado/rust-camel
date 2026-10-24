//! Subprocess bridge lifecycle management for rust-camel — spawn, monitor, and reconnect to external bridge processes.
//!
//! Main modules: `channel`, `download`, `health`, `process`, `reconnect`, `spec`.

pub(crate) mod channel;
pub mod download;
#[cfg(unix)]
pub(crate) mod group;
#[cfg(unix)]
pub(crate) use group::GroupGuard;
pub mod health;
pub mod process;
pub mod reconnect;
pub mod spec;
pub(crate) mod tls;
