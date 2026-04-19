//! XSLT 3.0 transformation component powered by the xml-bridge.
//!
//! Endpoint URI format: `xslt:<stylesheet>`
//!
//! Example:
//! ```rust,ignore
//! let route = RouteBuilder::from("direct:in")
//!     .to("xslt:/path/to/transform.xslt")
//!     .build()?;
//! ```

mod client;
mod component;
mod config;
mod endpoint;
mod error;
mod producer;

pub const BRIDGE_VERSION: &str = "0.2.0";

pub use client::{BridgeState, StylesheetId, XsltBridgeClient, XsltTransformBackend};
pub use component::XsltComponent;
pub use config::{XsltComponentConfig, XsltEndpointConfig};
pub use error::XsltError;

pub mod proto {
    tonic::include_proto!("xml_bridge");
}
