use serde::{Deserialize, Serialize};

/// Per-route Unit of Work configuration.
///
/// When present on a `RouteDefinition`, wraps the route's Tower pipeline with
/// an `ExchangeUoWLayer` that tracks in-flight exchanges and fires optional
/// completion hooks.
///
/// The `Default` implementation yields `{ on_complete: None, on_failure: None }`,
/// which is semantically equivalent to "no UoW behaviour". A `RouteDefinition`
/// only gains UoW overhead when `unit_of_work` is `Some(config)` — a default
/// config wrapped in `Some` will still install the layer (with no hooks).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UnitOfWorkConfig {
    /// URI of the producer to call when an exchange completes successfully.
    /// Example: `"log:on-complete"`
    pub on_complete: Option<String>,
    /// URI of the producer to call when an exchange fails
    /// (inner pipeline returned `Err(_)` or `exchange.has_error()` is true).
    /// Example: `"log:on-failure"`
    pub on_failure: Option<String>,
}
