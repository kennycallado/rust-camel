pub mod bundle;
pub mod component;
pub mod config;
pub mod consumer;
pub mod error;
pub mod pool;
pub mod producer;

pub use bundle::CxfBundle;
pub use component::CxfComponent;
pub use config::{CxfPoolConfig, CxfProfileConfig};
pub use error::CxfError;
pub use pool::{BridgeSlot, BridgeState, CxfBridgePool};

pub const BRIDGE_VERSION: &str = "0.1.0";

pub mod proto {
    tonic::include_proto!("cxf_bridge");
}
