//! Configuration layer for rust-camel — YAML/JSON loading, route discovery, and property resolution.
//!
//! Main types: `CamelConfig`, `JournalConfig`, `ComponentsConfig`, `PropertiesResolver`.
//! Main modules: `config`, `context_ext`, `discovery`, `properties`, `yaml`, `json`.

pub mod config;
pub(crate) mod context_ext;
pub mod discovery;
pub(crate) mod filter;
pub(crate) mod include;
pub mod json;
pub mod properties;
pub mod wasm_limits;
pub mod yaml;

pub use config::{
    CamelConfig, ComponentsConfig, HealthCamelConfig, JournalConfig, JournalDurability,
    KeycloakIntrospectionConfig, KeycloakJwksConfig, KeycloakSecurityConfig,
    KeycloakValidationConfig, ObservabilityConfig, OtelCamelConfig, SecurityConfig,
    StreamCachingConfig, SupervisionCamelConfig,
};
pub use discovery::{
    DiscoveryError, discover_routes, discover_routes_with_threshold,
    discover_routes_with_threshold_and_security,
};
pub use json::{
    load_json_from_file, parse_json, parse_json_to_canonical, parse_json_to_declarative,
    parse_json_with_threshold,
};
pub use properties::{PropertiesResolver, ResolveError};
pub use wasm_limits::WasmLimitsConfig;
pub use yaml::{
    RouteDslRoute, RouteDslRoutes, RouteDslStep, load_from_file, parse_yaml,
    parse_yaml_to_canonical, parse_yaml_to_declarative, parse_yaml_with_threshold,
};
