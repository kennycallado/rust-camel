//! Component metadata schema types.
//!
//! Defines the core metadata structures used by component registries and
//! schema generation: option definitions, capability declarations, and the
//! top-level component descriptor.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// OptionKind — closed enum of supported option value types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[cfg_attr(feature = "schema", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    String,
    Int,
    Bool,
    Float,
    Duration,
    Enum(Vec<String>),
    List(Box<OptionKind>),
}

// ---------------------------------------------------------------------------
// UriOption — a single URI-parameter definition with builder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
#[cfg_attr(feature = "schema", ts(rename_all = "snake_case"))]
pub struct UriOption {
    pub name: String,
    pub description: String,
    pub kind: OptionKind,
    #[cfg_attr(feature = "schema", schemars(default))]
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    #[cfg_attr(feature = "schema", schemars(default))]
    #[serde(default)]
    pub secret: bool,
}

impl UriOption {
    pub fn new(name: &str, description: &str, kind: OptionKind) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            kind,
            required: false,
            default_value: None,
            aliases: Vec::new(),
            deprecated: None,
            secret: false,
        }
    }

    #[must_use]
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    #[must_use]
    pub fn with_default(mut self, value: &str) -> Self {
        self.default_value = Some(value.to_string());
        self
    }

    #[must_use]
    pub fn with_alias(mut self, alias: &str) -> Self {
        self.aliases.push(alias.to_string());
        self
    }

    #[must_use]
    pub fn deprecated(mut self, reason: &str) -> Self {
        self.deprecated = Some(reason.to_string());
        self
    }

    #[must_use]
    pub fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
}

// ---------------------------------------------------------------------------
// ComponentCapabilities — named boolean flags + query matching
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
pub struct ComponentCapabilities {
    pub supports_consumer: bool,
    pub supports_producer: bool,
    pub supports_polling_consumer: bool,
    pub supports_streaming: bool,
}

impl ComponentCapabilities {
    /// Returns `true` when every field present in `query` matches this
    /// capability set.  Fields set to `None` in the query are ignored
    /// (no constraint).
    pub fn matches_query(&self, query: &CapabilityQuery) -> bool {
        query
            .supports_consumer
            .is_none_or(|v| self.supports_consumer == v)
            && query
                .supports_producer
                .is_none_or(|v| self.supports_producer == v)
            && query
                .supports_polling_consumer
                .is_none_or(|v| self.supports_polling_consumer == v)
            && query
                .supports_streaming
                .is_none_or(|v| self.supports_streaming == v)
    }
}

// ---------------------------------------------------------------------------
// CapabilityQuery — tri-state query for filtering components
// ---------------------------------------------------------------------------

/// Tri-state query struct.  Each field, when `Some(v)`, constrains the
/// corresponding [`ComponentCapabilities`] field to equal `v`.  `None`
/// means "don't care".
#[derive(Debug, Clone, Default)]
pub struct CapabilityQuery {
    pub supports_consumer: Option<bool>,
    pub supports_producer: Option<bool>,
    pub supports_polling_consumer: Option<bool>,
    pub supports_streaming: Option<bool>,
}

// ---------------------------------------------------------------------------
// ComponentMetadata — top-level component descriptor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
pub struct ComponentMetadata {
    pub scheme: String,
    pub schema_version: String,
    pub version: String,
    pub description: String,
    pub uri_syntax: String,
    pub capabilities: ComponentCapabilities,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uri_options: Vec<UriOption>,
}

impl ComponentMetadata {
    /// Current schema version string.
    pub const SCHEMA_VERSION: &'static str = "1";

    /// Build a minimal metadata entry for `scheme`.
    ///
    /// Sets `schema_version` to [`SCHEMA_VERSION`].  The `version` field is
    /// left empty — components that override [`Component::metadata`] should
    /// supply their own `env!("CARGO_PKG_VERSION")`.
    pub fn minimal(scheme: &str) -> Self {
        Self {
            scheme: scheme.to_string(),
            schema_version: Self::SCHEMA_VERSION.to_string(),
            version: String::new(),
            description: String::new(),
            uri_syntax: String::new(),
            capabilities: ComponentCapabilities::default(),
            uri_options: Vec::new(),
        }
    }

    /// Validate that this metadata's `scheme` matches the given
    /// `component_scheme`.  Returns `Err` with a descriptive message on
    /// mismatch.
    pub fn validate_scheme(&self, component_scheme: &str) -> Result<(), String> {
        if self.scheme == component_scheme {
            Ok(())
        } else {
            Err(format!(
                "Scheme mismatch: component metadata scheme is '{}' but component scheme is '{}'",
                self.scheme, component_scheme
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// ComponentMetadataCatalog — runtime query interface
// ---------------------------------------------------------------------------

/// Query interface for component metadata at runtime.
///
/// Implementations store [`ComponentMetadata`] entries keyed by URI scheme
/// and provide lookup and capability-filtering operations.
pub trait ComponentMetadataCatalog: Send + Sync {
    /// Look up metadata for a component by its URI scheme.
    fn get_metadata(&self, scheme: &str) -> Option<ComponentMetadata>;

    /// Return all schemes that have registered metadata.
    fn schemes(&self) -> Vec<String>;

    /// Return metadata for all registered components.
    fn all_metadata(&self) -> Vec<ComponentMetadata>;

    /// Filter components by capability query. Returns all metadata
    /// entries whose capabilities match every field in the query.
    fn query_capabilities(&self, query: &CapabilityQuery) -> Vec<ComponentMetadata> {
        self.all_metadata()
            .into_iter()
            .filter(|m| m.capabilities.matches_query(query))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_option_builder_chain() {
        let opt = UriOption::new("my_param", "A test parameter", OptionKind::String)
            .required()
            .with_default("default_val")
            .with_alias("mp")
            .with_alias("my-param")
            .deprecated("Use new_param instead")
            .secret();

        assert_eq!(opt.name, "my_param");
        assert_eq!(opt.description, "A test parameter");
        assert_eq!(opt.kind, OptionKind::String);
        assert!(opt.required);
        assert_eq!(opt.default_value, Some("default_val".to_string()));
        assert_eq!(opt.aliases, vec!["mp".to_string(), "my-param".to_string()]);
        assert_eq!(opt.deprecated, Some("Use new_param instead".to_string()));
        assert!(opt.secret);
    }

    #[test]
    fn enum_kind_holds_variants() {
        let kind = OptionKind::Enum(vec!["a".to_string(), "b".to_string()]);
        match &kind {
            OptionKind::Enum(variants) => {
                assert_eq!(variants.len(), 2);
                assert_eq!(variants[0], "a");
                assert_eq!(variants[1], "b");
            }
            other => panic!("Expected Enum variant, got {other:?}"),
        }
    }

    #[test]
    fn component_capabilities_default_all_false() {
        let caps = ComponentCapabilities::default();
        assert!(!caps.supports_consumer);
        assert!(!caps.supports_producer);
        assert!(!caps.supports_polling_consumer);
        assert!(!caps.supports_streaming);
    }

    #[test]
    fn minimal_metadata_has_scheme_and_empty_fields() {
        let meta = ComponentMetadata::minimal("timer");
        assert_eq!(meta.scheme, "timer");
        assert_eq!(meta.schema_version, "1");
        assert!(meta.description.is_empty());
        assert!(meta.uri_syntax.is_empty());
        assert!(meta.uri_options.is_empty());
        assert!(meta.version.is_empty());
    }

    #[test]
    fn validate_scheme_mismatch_returns_err() {
        let meta = ComponentMetadata::minimal("timer");
        let result = meta.validate_scheme("log");
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("timer"));
        assert!(err_msg.contains("log"));
    }

    #[test]
    fn capability_query_tri_state_matching() {
        let caps = ComponentCapabilities {
            supports_consumer: true,
            ..Default::default()
        };

        // supports_consumer = true matches query asking for Some(true)
        assert!(caps.matches_query(&CapabilityQuery {
            supports_consumer: Some(true),
            ..Default::default()
        }));

        // supports_producer = false does NOT match query asking for
        // Some(true)
        assert!(!caps.matches_query(&CapabilityQuery {
            supports_producer: Some(true),
            ..Default::default()
        }));

        // supports_producer = false DOES match query asking for
        // Some(false)
        assert!(caps.matches_query(&CapabilityQuery {
            supports_producer: Some(false),
            ..Default::default()
        }));
    }

    // -----------------------------------------------------------------------
    // Mock catalog for testing
    // -----------------------------------------------------------------------

    struct MockCatalog {
        entries: std::collections::HashMap<String, ComponentMetadata>,
    }

    impl ComponentMetadataCatalog for MockCatalog {
        fn get_metadata(&self, scheme: &str) -> Option<ComponentMetadata> {
            self.entries.get(scheme).cloned()
        }

        fn schemes(&self) -> Vec<String> {
            self.entries.keys().cloned().collect()
        }

        fn all_metadata(&self) -> Vec<ComponentMetadata> {
            self.entries.values().cloned().collect()
        }
    }

    #[test]
    fn catalog_trait_object_safety() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("timer".to_string(), ComponentMetadata::minimal("timer"));
        let catalog = MockCatalog { entries };
        let dyn_catalog: &dyn ComponentMetadataCatalog = &catalog;
        assert_eq!(dyn_catalog.schemes(), vec!["timer".to_string()]);
    }

    #[test]
    fn catalog_query_capabilities_default_impl() {
        let consumer = ComponentMetadata {
            scheme: "timer".to_string(),
            capabilities: ComponentCapabilities {
                supports_consumer: true,
                ..Default::default()
            },
            ..ComponentMetadata::minimal("timer")
        };
        let producer = ComponentMetadata {
            scheme: "log".to_string(),
            capabilities: ComponentCapabilities {
                supports_producer: true,
                ..Default::default()
            },
            ..ComponentMetadata::minimal("log")
        };
        let mut entries = std::collections::HashMap::new();
        entries.insert("timer".to_string(), consumer);
        entries.insert("log".to_string(), producer);
        let catalog = MockCatalog { entries };

        let query_consumer = CapabilityQuery {
            supports_consumer: Some(true),
            ..Default::default()
        };
        let results = catalog.query_capabilities(&query_consumer);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].scheme, "timer");

        let query_producer = CapabilityQuery {
            supports_producer: Some(true),
            ..Default::default()
        };
        let results = catalog.query_capabilities(&query_producer);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].scheme, "log");

        // Query with no constraints returns all
        let query_none = CapabilityQuery::default();
        let results = catalog.query_capabilities(&query_none);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn catalog_schemes_and_all_metadata() {
        let mut entries = std::collections::HashMap::new();
        entries.insert("timer".to_string(), ComponentMetadata::minimal("timer"));
        entries.insert("log".to_string(), ComponentMetadata::minimal("log"));
        let catalog = MockCatalog { entries };

        let mut schemes = catalog.schemes();
        schemes.sort();
        assert_eq!(schemes, vec!["log".to_string(), "timer".to_string()]);

        let all = catalog.all_metadata();
        assert_eq!(all.len(), 2);
        let schemes_from_meta: std::collections::BTreeSet<&str> =
            all.iter().map(|m| m.scheme.as_str()).collect();
        assert!(schemes_from_meta.contains("timer"));
        assert!(schemes_from_meta.contains("log"));
    }
}
