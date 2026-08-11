pub const ROUTE_SCHEMA: &str = include_str!("../schema/route-schema.json");

pub mod completion;
pub mod diagnostic;
pub mod document;
pub mod engine;
pub mod error;
pub mod hover;
pub mod route_view;
pub mod rule;
pub mod rules;

pub use completion::*;
pub use diagnostic::*;
pub use document::*;
pub use engine::*;
pub use error::*;
pub use hover::*;
pub use route_view::*;
pub use rule::*;

// Re-export metadata types so downstream crates (e.g. camel-lsp) can access
// ComponentMetadataCatalog and its associated types via camel-lint without
// depending on camel-api directly.
pub use camel_api::component_metadata::{
    CapabilityQuery, ComponentMetadata, ComponentMetadataCatalog, OptionKind, UriOption,
};

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests {
    #[test]
    fn route_schema_is_embedded() {
        let schema = super::ROUTE_SCHEMA;
        assert!(!schema.is_empty(), "ROUTE_SCHEMA must be non-empty");
        assert!(schema.starts_with('{'), "ROUTE_SCHEMA must start with '{{'");
        assert!(schema.ends_with('}'), "ROUTE_SCHEMA must end with '}}'");
    }
}
