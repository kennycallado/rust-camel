mod uri_config;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

/// Derive macro for UriConfig trait implementation.
///
/// This macro generates the `from_uri()` implementation based on struct field attributes.
///
/// # Attributes
///
/// ## Struct-level attributes
///
/// - `#[uri_scheme = "xxx"]` - Required, defines the URI scheme
/// - `#[uri_config(skip_impl)]` - Optional, generates only the parsing helper method
///   instead of the full trait impl. Use this when you need custom `validate()` logic.
///
/// ## Field-level attributes
///
/// - `#[uri_param]` - Marks a field as a URI query parameter (uses field name as param name)
/// - `#[uri_param(default = "value")]` - Provides a default value if param not present
/// - `#[uri_param(name = "paramName")]` - Maps to a different query parameter name
///
/// # Example
///
/// ## Basic usage
///
/// ```ignore
/// use camel_endpoint::UriConfig;
///
/// #[derive(Debug, Clone, UriConfig)]
/// #[uri_scheme = "timer"]
/// struct TimerConfig {
///     // First field without #[uri_param] gets the path component
///     name: String,
///
///     // Query parameters
///     #[uri_param(default = "1000")]
///     period: u64,
///
///     #[uri_param(default = "true")]
///     repeat: bool,
///
///     #[uri_param(name = "cronExpr")]
///     cron: Option<String>,
/// }
///
/// // Generated impl allows:
/// let config = TimerConfig::from_uri("timer:tick?period=5000").unwrap();
/// assert_eq!(config.name, "tick");
/// assert_eq!(config.period, 5000);
/// assert!(config.repeat); // uses default
/// assert!(config.cron.is_none()); // Option defaults to None
/// ```
///
/// ## Custom validation with `skip_impl`
///
/// ```ignore
/// use camel_endpoint::UriConfig;
///
/// #[derive(Debug, Clone, UriConfig)]
/// #[uri_scheme = "file"]
/// #[uri_config(skip_impl)]
/// struct FileConfig {
///     directory: String,
///     #[uri_param(default = "false")]
///     delete: bool,
///     #[uri_param(name = "move")]
///     move_to: Option<String>,
/// }
///
/// // Implement the trait manually with custom validation
/// impl UriConfig for FileConfig {
///     fn scheme() -> &'static str { "file" }
///     
///     fn from_uri(uri: &str) -> Result<Self, CamelError> {
///         let parts = parse_uri(uri)?;
///         Self::from_components(parts)
///     }
///     
///     fn from_components(parts: UriComponents) -> Result<Self, CamelError> {
///         Self::parse_uri_components(parts)?.validate()
///     }
///     
///     fn validate(self) -> Result<Self, CamelError> {
///         // Custom validation: move_to is None if delete is true
///         let move_to = if self.delete { None } else { self.move_to };
///         Ok(Self { move_to, ..self })
///     }
/// }
/// ```
#[proc_macro_derive(UriConfig, attributes(uri_scheme, uri_param, uri_config))]
pub fn derive_uri_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    uri_config::impl_uri_config(&input).into()
}
