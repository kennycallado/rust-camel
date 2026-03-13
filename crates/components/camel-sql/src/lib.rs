pub mod config;
pub mod consumer;
pub mod endpoint;
pub mod headers;
pub mod producer;
pub mod query;

use camel_api::CamelError;
use camel_component::{Component, Endpoint};

pub use config::{SqlConfig, SqlOutputType};

pub struct SqlComponent;

impl SqlComponent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SqlComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for SqlComponent {
    fn scheme(&self) -> &str {
        "sql"
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = SqlConfig::from_uri(uri)?;
        Ok(Box::new(endpoint::SqlEndpoint::new(uri.to_string(), config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component::Component;

    #[test]
    fn test_component_scheme() {
        let c = SqlComponent::new();
        assert_eq!(c.scheme(), "sql");
    }

    #[test]
    fn test_component_creates_endpoint() {
        let c = SqlComponent::new();
        let ep = c.create_endpoint("sql:select 1?db_url=postgres://localhost/test");
        assert!(ep.is_ok());
    }

    #[test]
    fn test_component_rejects_wrong_scheme() {
        let c = SqlComponent::new();
        let ep = c.create_endpoint("redis://localhost");
        assert!(ep.is_err());
    }

    #[test]
    fn test_endpoint_uri() {
        let c = SqlComponent::new();
        let ep = c.create_endpoint("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert_eq!(ep.uri(), "sql:select 1?db_url=postgres://localhost/test");
    }
}
