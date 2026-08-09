//! Endpoint URI parsing and configuration for rust-camel components.
//!
//! Main types: `UriConfig` (trait), `UriComponents`. Main modules: `config`, `uri`.

pub mod config;
pub mod uri;

pub use config::UriConfig;
pub use uri::{UriComponents, parse_uri};

// Re-export CamelError for macro-generated code
pub use camel_api::CamelError;

// Re-export component-metadata types so macro-generated `uri_options()` and
// `metadata()` resolve through the configured crate path (`#endpoint_crate`).
pub use camel_api::component_metadata::{
    ComponentCapabilities, ComponentMetadata, OptionKind, UriOption,
};

// Re-export proc-macro derive - same name as trait is allowed (different namespaces)
pub use camel_endpoint_macros::UriConfig;
