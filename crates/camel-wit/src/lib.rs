/// WIT source for the `plugin` world (standalone file).
pub const PLUGIN_WIT: &str = include_str!("../wit/camel-plugin.wit");

/// WIT source for the `bean` world (standalone file, same package as PLUGIN_WIT).
pub const BEAN_WIT: &str = include_str!("../wit/camel-bean.wit");

/// Combined WIT package with both `plugin` and `bean` worlds in a single document.
pub const FULL_WIT: &str = include_str!("../wit/camel-all.wit");

/// Absolute path to the `wit/` directory bundled with this crate.
///
/// This path is resolved at **compile time** via `CARGO_MANIFEST_DIR` and
/// points to the `camel-wit` source directory (local path dep or registry
/// unpack location). It is stable during builds and in development tooling,
/// but is **not** a reliable runtime path in redistributed binaries.
///
/// Prefer the `*_WIT` string constants for embedding WIT content robustly.
/// Use this only for CLI tooling that needs filesystem access at build/dev time
/// (e.g. `wasm-tools`, `wit-bindgen` CLI invoked from a build script).
pub fn wit_dir() -> &'static std::path::Path {
    std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/wit"))
}
