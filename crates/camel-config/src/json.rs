//! JSON route definition compatibility shim.
//!
//! Mirrors [`crate::yaml`]: re-exports the JSON parse helpers from
//! [`camel_dsl::json`] so callers of `camel-config` can use JSON and YAML
//! through a symmetric API surface.

pub use camel_dsl::json::{
    load_json_from_file, parse_json, parse_json_to_canonical, parse_json_to_declarative,
    parse_json_with_threshold,
};
