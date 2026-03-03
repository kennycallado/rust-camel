# camel-dsl

> DSL support for rust-camel (YAML, etc)

## Overview

`camel-dsl` provides Domain Specific Language support for defining routes in rust-camel. It enables route definitions in formats like YAML, making it easier to configure integration flows declaratively.

This crate is useful when you want to externalize route configuration or define routes without writing Rust code.

## Features

- YAML route definitions
- Declarative integration flows
- External configuration support

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-dsl = "0.2"
```

## Usage

```yaml
# routes.yaml
routes:
  - from: "timer:tick?period=1000"
    steps:
      - set_header:
          key: "source"
          value: "timer"
      - log: "Timer fired!"
      - to: "mock:result"
```

```rust
use camel_dsl::YamlRouteLoader;
use camel_core::CamelContext;

// Load routes from YAML
let routes = YamlRouteLoader::from_file("routes.yaml")?;

// Add to context
for route in routes {
    ctx.add_route(route).await?;
}
```

## Documentation

- [API Documentation](https://docs.rs/camel-dsl)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
