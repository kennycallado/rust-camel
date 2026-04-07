use camel_api::CamelError;

use crate::endpoint::Endpoint;

/// A Component is a factory for Endpoints.
///
/// Each component handles a specific URI scheme (e.g. "timer", "log", "direct").
pub trait Component: Send + Sync {
    /// The URI scheme this component handles (e.g., "timer", "log").
    fn scheme(&self) -> &str;

    /// Create an endpoint from a URI string.
    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError>;
}
