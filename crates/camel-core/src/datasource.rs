//! # datasource — vertical slice (3-ring)
//!
//! Runtime catalog of datasource pools keyed by name, with a `PoolFactory`
//! registry and lazy `OnceCell` pool initialization. This module is the
//! composition root for the slice: it owns the three ring submodules and
//! re-exports their public items so `camel_core::datasource::*` remains the
//! stable public path.
//!
//! ## Ring → module map
//!
//! | Item | Ring | Rationale |
//! |---|---|---|
//! | `type CacheKey = (String, String)` | domain | Internal type alias for the pool cache key (datasource name + factory name) |
//! | `pub struct RuntimeDatasourceCatalog` | domain | Aggregate root. Holds `HashMap<DatasourceConfig>`, `RwLock<HashMap<PoolFactory>>`, `DashMap<CacheKey, OnceCell<Handle>>`, `Option<Arc<HealthCheckRegistry>>` (field types, not I/O calls) |
//! | `impl RuntimeDatasourceCatalog { new, with_health_registry }` | domain | Simple constructors; field initialization only |
//! | `impl RuntimeDatasourceCatalog { get_config, get_pool, register_factory, resolve_factory }` | application | Use-case orchestration: pool resolution, factory matching, lazy pool creation, health-check registration |
//! | `fn scheme_hint(db_url: &str) -> String` (private) | application | Pure helper: extracts URL scheme for safe error messages without leaking credentials |
//! | `struct DatasourceHealthCheck` + `impl AsyncHealthCheck for DatasourceHealthCheck` (private) | application | Helper used by `get_pool` to register a health check with the optional registry; bridges to the `AsyncHealthCheck` port |
//! | `impl DatasourceCatalog for RuntimeDatasourceCatalog` | adapters | External port delegation (trait methods forward to inherent use-case methods) |
//!
//! ## Visibility contract
//!
//! `RuntimeDatasourceCatalog` and its inherent constructors are `pub` (same
//! as before the slice reorg). The use-case methods on the struct are
//! `pub(crate)` (callable within the crate, including the trait delegation in
//! the sibling `adapters` module); external callers reach them through the
//! `DatasourceCatalog` port impl. `CacheKey`, `scheme_hint`, and
//! `DatasourceHealthCheck` stay private to the slice.

mod adapters;
mod application;
mod domain;

// Only `domain` exposes a public top-level item (`RuntimeDatasourceCatalog`).
// `application` adds inherent use-case methods on the type (not
// re-exportable items), and `adapters` adds a trait impl (also not a
// re-exportable item). Both the methods and the trait impl are reachable via
// the type re-exported from `domain`.
pub use self::domain::*;
