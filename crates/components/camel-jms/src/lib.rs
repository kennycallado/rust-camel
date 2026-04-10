pub mod bundle;
pub mod component;
pub mod config;
pub mod consumer;
pub mod headers;
pub mod producer;

pub use bundle::JmsBundle;
pub use camel_bridge::process::BrokerType;
pub use component::{
    BridgeSlot, BridgeState, JmsBridgePool, JmsComponent, is_bridge_transport_error,
};
pub use config::default_bridge_cache_dir;
pub use config::{BrokerConfig, DestinationType, JmsEndpointConfig, JmsPoolConfig};

/// Version of the Java bridge binary this crate is compatible with.
pub const BRIDGE_VERSION: &str = "0.2.0";

pub mod proto {
    tonic::include_proto!("jms_bridge");
}
