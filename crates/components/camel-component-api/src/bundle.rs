use crate::ComponentRegistrar;

/// A bundle groups one or more related component schemes and their shared config.
///
/// Bundles own their TOML key and deserialize their own config block.
/// `camel-cli` uses `register_bundle!` to wire them.
pub trait ComponentBundle: Sized {
    /// Key under [components.<key>] in Camel.toml.
    fn config_key() -> &'static str;

    /// Deserialize the raw toml::Value block for this bundle.
    fn from_toml(value: toml::Value) -> Result<Self, camel_api::CamelError>;

    /// Register all schemes this bundle owns into the context.
    fn register_all(self, ctx: &mut dyn ComponentRegistrar);
}
