//! # health_registry — vertical slice (3-ring)
//!
//! Per-route health probe registry with fail-closed forced-unhealthy gating.
//! This module is the composition root for the slice: it owns the three ring
//! submodules and re-exports their public items so `camel_core::health_registry::*`
//! remains the stable public path.
//!
//! ## Ring → module map
//!
//! | Item | Ring | Rationale |
//! |---|---|---|
//! | `struct ForcedEntry` (private) | domain | Pure data: TTL + generation state for a forced-unhealthy entry |
//! | `struct RouteHealth` (private) | domain | Pure data: per-route aggregate state (active flag, live checks, forced entry, generation) |
//! | `pub struct HealthCheckRegistry` | domain | Aggregate root. Holds `RwLock<HashMap>` + `CancellationToken` + `Duration` (field types, not I/O calls) |
//! | `impl HealthCheckRegistry { new, with_forced_ttl, register_for_route, mark_route_started, mark_route_stopped, unregister_for_route, force_unhealthy_for_route, cancel_token }` | domain | Entity operations: lock + mutate state, no framework I/O calls |
//! | `enum CheckTask` (private) | application | Use-case-internal probe representation (Live vs Forced) |
//! | `impl HealthCheckRegistry { pub async fn check_all }` | application | Use-case orchestration: `tokio::time::timeout` + `futures::join_all` + `tracing::warn!` + `chrono::Utc::now()` + panic catching |
//! | `impl camel_component_api::HealthCheckRegistry for HealthCheckRegistry` | adapters | External trait delegation (port forwarding to inherent method) |
//!
//! ## Visibility contract
//!
//! `HealthCheckRegistry` and its inherent methods are `pub` (they were `pub`
//! before the slice reorg). The trait delegation in `adapters` exposes the
//! same method through the `camel_component_api` port. `CheckTask` and the
//! private state structs stay private to the slice.

mod adapters;
mod application;
mod domain;

// Only `domain` exposes a public top-level item (`HealthCheckRegistry`).
// `application` adds an inherent `check_all` method on the type (not a
// re-exportable item), and `adapters` adds a trait impl (also not a
// re-exportable item). Both methods and the trait impl are reachable via
// the type re-exported from `domain`.
pub use self::domain::*;
