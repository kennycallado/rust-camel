//! Compatibility shim: ports now live under `lifecycle/application/ports` (Use Cases ring,
//! ADR-0045 §5). This module re-exports them at the old path so the ~15 internal
//! `crate::lifecycle::ports::` references compile unchanged during the transition.
//! `pub use` (not `pub(crate) use`) so `lib.rs`'s public port re-exports (`pub use
//! crate::lifecycle::ports::{...}`) don't hit E0364.
pub use crate::lifecycle::application::ports::*;
