use crate::process::BridgeError;

/// Implemented by components that hold stateful resources in a bridge
/// (e.g. compiled XSDs, compiled XSLT stylesheets) and need to re-seed
/// those resources when the bridge process restarts.
///
/// The orchestration loop in each component calls `on_reconnect` after the
/// new bridge process announces its gRPC port and before the channel is
/// handed back to callers.
///
/// # Contract
/// - MUST be non-blocking in the synchronous sense: implementors should spawn
///   a Tokio task for async work rather than blocking the reconnect loop.
/// - Returning `Err` is advisory: the reconnect loop logs the error but does
///   NOT abort — the bridge is considered live; individual resource re-seeds
///   may be retried lazily.
pub trait BridgeReconnectHandler: Send + Sync + std::fmt::Debug {
    /// Called after a new bridge process starts and its port is known.
    ///
    /// `port` is the gRPC port the new bridge is listening on.
    fn on_reconnect(&self, port: u16) -> Result<(), BridgeError>;
}
