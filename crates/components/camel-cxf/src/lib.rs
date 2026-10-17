//! CXF/SOAP bridge component for rust-camel — connects to a Java CXF bridge
//! process via gRPC/Tonic, enabling SOAP WebService calls from Rust routes.
//!
//! Main types: `CxfBundle`, `CxfComponent`, `CxfBridgePool`, `CxfError`.
//! Main modules: `component`, `config`, `consumer`, `pool`, `producer`.
//!
//! # One-way SOAP operations
//!
//! A WSDL operation is one-way when the operation in the selected service/port
//! binding has an input and no output. The producer selects that shape from the
//! binding: an explicit `operation` name, or the bare payload root QName when the
//! name is absent. The bridge calls `Dispatch.invokeOneWay` exactly once with the
//! bare payload. Empty HTTP 200 and 202 acknowledgments complete successfully with
//! empty output bytes; the bridge does not deserialize an absent response body.
//!
//! One-way producer dispatch is fixed to non-robust mode:
//! `org.apache.cxf.oneway.robust` is `false`, and there is no public
//! `oneway.robust` option. Successful send completion does not assert remote
//! business success and does not guarantee observation of a remote SOAP fault. A
//! failure that the local bridge or transport detects remains an error; it is
//! never converted into an empty success. A local transport failure after a
//! one-way send does not replay or resend it. Recovery is a subsequent explicit
//! send, which is a separate action.
//!
//! An operation with WSDL output remains request-reply. An empty response to a
//! request-reply operation is an error: the producer never falls back to one-way,
//! fabricates a payload, or resends.
//!
//! # Limitations
//!
//! - Requires the `camel-cxf-bridge` Java process to be running and reachable over gRPC.
//!   Without the bridge, all producers/consumers will remain in `Starting` state indefinitely.
//! - SOAP/WSDL parsing is delegated to the Java CXF bridge; complex WS-Security policies
//!   must be configured on the Java side.
//! - One-way producer sends are non-robust: send completion does not assert remote
//!   business success or guarantee observation of a remote SOAP fault.

pub mod bundle;
pub mod component;
pub mod config;
pub mod consumer;
pub mod error;
pub mod health;
pub(crate) mod metadata;
pub mod pool;
pub mod producer;
pub mod signer;

pub use bundle::CxfBundle;
pub use component::CxfComponent;
pub use config::{CxfPoolConfig, CxfProfileConfig};
pub use error::CxfError;
pub use health::CxfHealthCheck;
pub use pool::{BridgeListenerInfo, BridgeSlot, BridgeState, CxfBridgePool};

pub const BRIDGE_VERSION: &str = "0.8.0";

#[cfg(test)]
mod bridge_client_test;
#[cfg(test)]
mod pool_env_test;

pub mod proto {
    tonic::include_proto!("cxf_bridge");
}
