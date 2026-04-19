//! Schema validation component for XML, JSON, and YAML payloads.
//!
//! Endpoint URI format: `validator:<schema>`
//!
//! JSON Schema and YAML validation run natively in Rust.
//! XSD validation is executed through the xml-bridge backend.

pub mod compiled;
pub mod component;
pub mod config;
pub mod error;
pub mod xsd_bridge;

pub use component::ValidatorComponent;
pub use config::{SchemaType, ValidatorConfig};

/// Version of the Java XML bridge binary this crate is compatible with.
pub const BRIDGE_VERSION: &str = "0.1.0";

pub mod proto {
    tonic::include_proto!("xml_bridge");
}
