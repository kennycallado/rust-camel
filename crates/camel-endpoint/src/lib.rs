pub mod config;
pub mod uri;

pub use config::UriConfig;
pub use uri::{parse_uri, UriComponents};

// Re-export CamelError for macro-generated code
pub use camel_api::CamelError;

// Re-export proc-macro derive - same name as trait is allowed (different namespaces)
pub use camel_endpoint_macros::UriConfig;
