# camel-endpoint-macros

Proc-macros for the `camel-endpoint` crate.

## UriConfig Derive Macro

Generates `from_uri()` implementation for configuration structs based on field attributes.

## Attributes

- `#[uri_scheme = "xxx"]` - Required. Defines the URI scheme.
- `#[uri_param]` - Marks field as query parameter.
- `#[uri_param(default = "value")]` - Default value if not specified.
- `#[uri_param(name = "paramName")]` - Map to different query param name.

## Example

```rust,ignore
use camel_endpoint::UriConfig;
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "timer"]
struct TimerConfig {
    name: String,
    
    #[uri_param(default = "1000")]
    period: u64,
}
```

## License

Apache-2.0
