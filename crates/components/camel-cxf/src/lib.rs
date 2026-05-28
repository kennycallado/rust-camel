//! CXF/SOAP bridge component for rust-camel — connects to a Java CXF bridge
//! process via gRPC/Tonic, enabling SOAP WebService calls from Rust routes.
//!
//! Main types: `CxfBundle`, `CxfComponent`, `CxfBridgePool`, `CxfError`.
//! Main modules: `component`, `config`, `consumer`, `pool`, `producer`.
//!
//! # Limitations
//!
//! - Requires the `camel-cxf-bridge` Java process to be running and reachable over gRPC.
//!   Without the bridge, all producers/consumers will remain in `Starting` state indefinitely.
//! - SOAP/WSDL parsing is delegated to the Java CXF bridge; complex WS-Security policies
//!   must be configured on the Java side.
//! - Only synchronous request-reply SOAP calls are supported; one-way SOAP operations
//!   are not currently modelled.

pub mod bundle;
pub mod component;
pub mod config;
pub mod consumer;
pub mod error;
pub mod health;
pub mod pool;
pub mod producer;

pub use bundle::CxfBundle;
pub use component::CxfComponent;
pub use config::{CxfPoolConfig, CxfProfileConfig};
pub use error::CxfError;
pub use health::CxfHealthCheck;
pub use pool::{BridgeSlot, BridgeState, CxfBridgePool};

pub const BRIDGE_VERSION: &str = "0.3.0";

pub mod proto {
    tonic::include_proto!("cxf_bridge");
}
