# Endpoint URI Derive Macro

camel-endpoint-macros implements `#[derive(UriConfig)]`. The canonical attribute contract and its
attribute table live in [`camel-endpoint/CONTEXT.md`](../camel-endpoint/CONTEXT.md#uriconfig-derive-contract).
This file does not repeat that table. The macro implementation defines the accepted attribute
names, and camel-endpoint owns their user-facing documentation.

## Generated crate path

Generated code resolves `UriConfig`, `UriComponents`, `CamelError`, `parse_uri()`, and URI parsing
helpers through one crate path. The default path is `camel_endpoint`.

`#[uri_config(crate = "path")]` overrides that path. The value must be a valid Rust path. Component
crates that use the camel-component-api re-exports set it to `camel_component_api`.

## `skip_impl` mode

By default, the derive generates the `UriConfig` implementation and an inherent
`parse_uri_components()` method. `#[uri_config(skip_impl)]` generates only the inherent method.
The caller then implements `UriConfig`, usually to add custom validation around the generated
parser.

The options can be combined as
`#[uri_config(skip_impl, crate = "camel_component_api")]`.

## `#[non_exhaustive]` posture

ADR-0049 does not apply to this crate. Its policy covers public contract enums in three contract
crates. This proc-macro crate exports a derive macro and no public contract enum, so
`#[non_exhaustive]` is not applicable.

## Related decisions

- ADR-0049 defines the workspace policy for public contract enums.
