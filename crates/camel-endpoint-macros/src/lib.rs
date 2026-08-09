//! Proc-macro derive for `UriConfig` — generates URI parsing implementations from struct field attributes.
//!
//! Main macro: `#[derive(UriConfig)]`. Supports `#[uri_scheme]`, `#[uri_param]`, and related attributes.

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
/// - `#[uri_config(crate = "path")]` - Optional, overrides the generated code's crate
///   path. Defaults to `camel_endpoint`. Component crates using the
///   `camel-component-api` re-exports set this to `camel_component_api`.
/// - `#[uri_config(metadata(scheme = "..", description = "..", producer, consumer,
///   polling_consumer, streaming))]` - Optional, opts in to generating an inherent
///   `fn metadata() -> ComponentMetadata` on the config struct. The group mixes bare
///   capability flags (`producer`, `consumer`, `polling_consumer`, `streaming`) with
///   `key = "value"` pairs (`scheme`, `description`). When `scheme` is omitted it falls
///   back to the `#[uri_scheme]` value.
///
/// ## Field-level attributes
///
/// - `#[uri_param]` - Marks a field as a URI query parameter (uses field name as param name)
/// - `#[uri_param(default = "value")]` - Provides a default value if param not present
/// - `#[uri_param(name = "paramName")]` - Maps to a different query parameter name
/// - `#[uri_param(desc = "text")]` - Human-readable description for the generated
///   `UriOption`.
/// - `#[uri_param(required)]` - Marks the option required (bare flag; also accepts
///   `required = true`). Without it, `Option<T>` fields are not required, and
///   non-`Option` fields without a `default` are required.
/// - `#[uri_param(secret)]` - Marks the option as secret (bare flag; also accepts
///   `secret = true`). Combining `secret` with `default` is a compile error.
/// - `#[uri_param(deprecated = "reason")]` - Deprecation notice.
/// - `#[uri_param(aliases = ["a", "b"])]` - Alias parameter names.
/// - `#[uri_param(kind = "duration|bool|int|float|string|enum:A,B")]` - Overrides the
///   inferred `OptionKind`. Inference never produces `Enum`; the only way to get an
///   `Enum` option is an explicit `kind = "enum:..."`. An unrecognized kind string is a
///   spanned compile error.
///
/// # Generated helper functions
///
/// In addition to the URI parsing impl, the derive always generates:
///
/// - `pub fn uri_options() -> Vec<UriOption>` - one entry per `#[uri_param]` field
///   (the path field is excluded). `OptionKind` is inferred from the Rust type after
///   unwrapping `Option<T>`.
///
/// And, when `#[uri_config(metadata(..))]` is present:
///
/// - `pub fn metadata() -> ComponentMetadata` - built from the metadata attribute and
///   the derived `uri_options()`. Component structs delegate their `Component::metadata`
///   override to this (e.g. `fn metadata(&self) -> ComponentMetadata { Config::metadata() }`).
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
///
/// # OptionKind type inference
///
/// The `OptionKind` for each `#[uri_param]` field is inferred from its Rust
/// type after unwrapping `Option<T>`:
///
/// | Rust type                     | Inferred `OptionKind`                               |
/// |-------------------------------|-----------------------------------------------------|
/// | `std::time::Duration`         | `Duration`                                          |
/// | `bool`                        | `Bool`                                              |
/// | `u8`, `u16`, `u32`, `u64`, `usize`, `i8`, `i16`, `i32`, `i64`, `isize` | `Int` |
/// | `f32`, `f64`                  | `Float`                                             |
/// | `String`, `&str`              | `String`                                            |
/// | `Vec<T>`                      | `List(Box::new(inner_kind_of_T))`                   |
/// | anything else (enums, custom types, …) | `String`              |
///
/// **Inference never produces `OptionKind::Enum`.** The only way to get an
/// `Enum` option is an explicit `kind = "enum:A,B,C"` override.
///
/// # Guardrail: `secret` + `default` is a compile error
///
/// `#[uri_param(secret, default = "x")]` produces a compile-time error:
/// *\"`#[uri_param]` cannot have both `secret` and `default`; a secret must
/// never carry a default value.\"* This prevents sensitive values from being embedded
/// in generated code or discovery output.
///
/// # Delegation convention
///
/// The macro generates `uri_options()` and (when opted in) `metadata()` as
/// inherent methods on the **config** struct. The **component** struct
/// implements `Component`, whose `metadata()` default returns
/// `ComponentMetadata::minimal(scheme)` with empty `uri_options`. Every
/// component MUST override `metadata()` to delegate to its config struct:
///
/// ```ignore
/// impl Component for MyComponent {
///     fn scheme(&self) -> &str { "my-scheme" }
///
///     fn metadata(&self) -> ComponentMetadata {
///         MyConfig::metadata()
///         // Or, without the metadata(..) opt-in:
///         // ComponentMetadata::minimal(self.scheme())
///         //     .with_uri_options(MyConfig::uri_options())
///     }
/// }
/// ```
///
/// Without this delegation step, the catalog returns empty `uri_options`.
///
/// # Worked example: component with metadata
///
/// ```ignore
/// use camel_endpoint::UriConfig;
///
/// #[derive(Debug, Clone, UriConfig)]
/// #[uri_scheme = "sql"]
/// #[uri_config(
///     metadata(
///         scheme = "sql",
///         description = "Execute SQL against a configured datasource",
///         producer,
///         consumer,
///     )
/// )]
/// struct SqlConfig {
///     query: String,
///
///     #[uri_param(secret, desc = "Database connection URL")]
///     db_url: String,
///
///     #[uri_param(
///         name = "outputType",
///         desc = "Output type for query results",
///         kind = "enum:SelectList,SelectOne,StreamList",
///         default = "SelectList"
///     )]
///     output_type: SqlOutputType,
///
///     #[uri_param(
///         name = "maxConnections",
///         desc = "Maximum connections in the pool",
///         default = "5"
///     )]
///     max_connections: u32,
/// }
///
/// // Generated:
/// // - SqlConfig::uri_options() returns 3 UriOption entries
/// // - SqlConfig::metadata() returns ComponentMetadata with scheme "sql",
/// //   producer + consumer capabilities, and the derived uri_options
///
/// // Component delegation (hand-written):
/// impl Component for SqlComponent {
///     fn scheme(&self) -> &str { "sql" }
///     fn metadata(&self) -> ComponentMetadata { SqlConfig::metadata() }
/// }
/// ```
#[proc_macro_derive(UriConfig, attributes(uri_scheme, uri_param, uri_config))]
pub fn derive_uri_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match uri_config::impl_uri_config(&input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}
