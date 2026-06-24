//! YAML route definition compatibility shim.

pub use camel_dsl::{
    RouteDslRoute, RouteDslRoutes, RouteDslStep, load_from_file, parse_yaml,
    parse_yaml_to_canonical, parse_yaml_to_declarative, parse_yaml_with_threshold,
};
