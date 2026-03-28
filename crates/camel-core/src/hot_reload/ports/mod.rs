//! Ports for the `hot_reload` bounded context.
//!
//! This module is intentionally empty. The `hot_reload` BC does not define its own
//! ports — it consumes `lifecycle` ports directly through the composition root
//! (`context.rs`). If `hot_reload` ever needs to expose its own port abstractions
//! (e.g., for a `FileWatcherPort`), they should be added here.
