//! Configuration layer for rust-camel — YAML/JSON loading, route discovery, and property resolution.
//!
//! Main types: `CamelConfig`, `JournalConfig`, `ComponentsConfig`, `PropertiesResolver`.
//! Main modules: `config`, `context_ext`, `discovery`, `properties`, `yaml`.

pub mod config;
pub mod context_ext;
pub mod discovery;
pub(crate) mod include;
pub mod properties;
pub mod yaml;

pub use config::{
    CamelConfig, ComponentsConfig, HealthCamelConfig, JournalConfig, JournalDurability,
    ObservabilityConfig, OtelCamelConfig, StreamCachingConfig, SupervisionCamelConfig,
};
pub use discovery::{DiscoveryError, discover_routes, discover_routes_with_threshold};
pub use properties::{PropertiesResolver, ResolveError};
pub use yaml::{
    YamlRoute, YamlRoutes, YamlStep, load_from_file, parse_yaml, parse_yaml_to_canonical,
    parse_yaml_to_declarative, parse_yaml_with_threshold,
};
