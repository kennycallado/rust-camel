pub mod component;
pub mod config;
pub mod consumer;
pub mod headers;
pub mod producer;

pub use camel_bridge::process::BrokerType;
pub use component::JmsComponent;
pub use config::JmsConfig;
pub use config::default_bridge_cache_dir;

/// Version of the Java bridge binary this crate is compatible with.
pub const BRIDGE_VERSION: &str = "0.1.1";

pub mod proto {
    tonic::include_proto!("jms_bridge");
}
