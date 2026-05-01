# camel-endpoint-macros

> Proc-macros for camel-endpoint

## Overview

Proc-macros for camel-endpoint.

Proc-macros for the `camel-endpoint` crate.

## UriConfig Derive Macro

Generates `from_uri()` implementation for configuration structs based on field attributes.

## Struct Attributes

- `#[uri_scheme = "xxx"]` - Required. Defines the URI scheme.
- `#[uri_config(skip_impl)]` - Skip automatic `from_uri()` implementation.
- `#[uri_config(crate = "path")]` - Custom crate path for generated code (default: `camel_endpoint`). Use `camel_component_api` when the derive is used inside component crates.

## Field Attributes

- `#[uri_param]` - Marks field as query parameter.
- `#[uri_param(default = "value")]` - Default value if not specified.
- `#[uri_param(name = "paramName")]` - Map to different query param name.

## Example

```rust,ignore
use camel_component_api::UriConfig;
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "timer"]
#[uri_config(crate = "camel_component_api")]
struct TimerConfig {
    name: String,

    #[uri_param(default = "*")]
    period: u64,
}
```

## License

Apache-2.0

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-endpoint-macros = "*"
```
