# camel-endpoint

> Endpoint parsing and URI handling for rust-camel

## Overview

`camel-endpoint` provides URI parsing and endpoint abstraction for the rust-camel framework. It handles the parsing of endpoint URIs in the format `scheme:path?param1=value1&param2=value2` that Camel is known for.

This crate is used internally by components to parse their configuration from URI strings.

## Features

- URI parsing with scheme, path, and query parameters
- `UriComponents` struct for easy access to URI parts
- Error handling for malformed URIs

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-endpoint = "0.2"
```

## Usage

```rust
use camel_endpoint::{parse_uri, UriComponents};

// Parse a Camel-style URI
let uri = "timer:tick?period=1000&delay=500";
let components = parse_uri(uri).unwrap();

assert_eq!(components.scheme, "timer");
assert_eq!(components.path, "tick");
assert_eq!(components.params.get("period"), Some(&"1000".to_string()));
assert_eq!(components.params.get("delay"), Some(&"500".to_string()));
```

## URI Format

Endpoints follow the standard Camel URI format:

```
scheme:path[?param1=value1&param2=value2]
```

- `scheme`: The component name (e.g., `timer`, `file`, `http`)
- `path`: Component-specific path (e.g., timer name, file directory)
- `params`: Optional query parameters for configuration

## Documentation

- [API Documentation](https://docs.rs/camel-endpoint)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
