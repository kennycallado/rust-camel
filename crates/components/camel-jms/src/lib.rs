//! JMS component for rust-camel — Apache ActiveMQ / Artemis bridge via Tower services.
//!
//! # Behavior changes (Phase B hardening)
//!
//! - **No automatic resend on transport errors** (breaking): Previously a send
//!   transport error triggered a channel refresh + automatic retry. The retry
//!   could duplicate non-idempotent writes. Now the channel is refreshed but
//!   the original error is returned to the caller. Callers that want retry
//!   semantics must implement it at the route level.
//! - **max_bridges enforcement is now race-free**: Concurrent `get_or_create_slot`
//!   calls are serialized during admission to prevent exceeding the configured
//!   limit.
//! - **Broker URLs are redacted in logs**: Userinfo (`user:pass@`) is replaced
//!   with `***@` before logging.

pub mod bundle;
pub mod component;
pub mod config;
pub mod consumer;
pub mod headers;
pub mod health;
pub(crate) mod metadata;
pub mod producer;

pub use bundle::JmsBundle;
pub use camel_bridge::process::BrokerType;
pub use component::{
    BRIDGE_TRANSPORT_ERROR_PREFIX, BridgeSlot, BridgeState, JmsBridgePool, JmsComponent,
    is_bridge_transport_error,
};
pub use config::default_bridge_cache_dir;
pub use config::{
    AcknowledgementMode, BrokerConfig, DestinationType, ExchangePattern, JmsEndpointConfig,
    JmsPoolConfig, JmsTransactionMode,
};
pub use health::JmsHealthCheck;

/// Version of the Java bridge binary this crate is compatible with.
pub const BRIDGE_VERSION: &str = "0.6.0";

/// Serializes tests that mutate `CAMEL_JMS_BRIDGE_BINARY_PATH` via
/// `std::env::set_var`. Process-global env vars race under parallel test
/// threads: one test's `/bin/false` override leaks into another test's
/// bridge resolution window. This lock serializes only the affected tests
/// (rc-alwn). Uses `std::sync::Mutex` (const-constructible for statics);
/// each consumer test annotates `#[allow(clippy::await_holding_lock)]`
/// because the guard is safe to hold across `.await` in test scope.
#[cfg(test)]
pub(crate) static BRIDGE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod bridge_client_test;

pub mod proto {
    tonic::include_proto!("jms_bridge");
}
