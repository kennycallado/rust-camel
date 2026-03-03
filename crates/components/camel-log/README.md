# camel-component-log

> Log component for rust-camel

## Overview

The Log component logs exchange information using the `tracing` crate. It's useful for debugging, monitoring, and audit logging in integration flows.

This is a **producer-only** component - it can only be used as a destination (`to`) in routes, not as a source (`from`).

## Features

- Multiple log levels (trace, debug, info, warn, error)
- Configurable body and header display
- `tracing` integration for structured logging
- Pass-through processing (exchange continues unchanged)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-log = "0.2"
```

## URI Format

```
log:category[?options]
```

## URI Options

| Option | Default | Description |
|--------|---------|-------------|
| `level` | `info` | Log level: trace, debug, info, warn, error |
| `showHeaders` | `false` | Include headers in log output |
| `showBody` | `true` | Include body in log output |

## Usage

### Basic Logging

```rust
use camel_builder::RouteBuilder;
use camel_component_log::LogComponent;

let route = RouteBuilder::from("timer:tick?period=1000")
    .to("log:myapp.timer") // Log at INFO level
    .build()?;
```

### With Debug Level

```rust
let route = RouteBuilder::from("direct:input")
    .to("log:debug?level=debug")
    .build()?;
```

### With Headers

```rust
let route = RouteBuilder::from("direct:input")
    .to("log:audit?level=info&showHeaders=true")
    .build()?;
```

### Body Only

```rust
let route = RouteBuilder::from("direct:input")
    .to("log:messages?showBody=true&showHeaders=false")
    .build()?;
```

## Log Output Format

```
[category] Body: <body> | Headers: {key1=value1, key2=value2}
```

When body is empty:
```
[category] Exchange received
```

## Example: Request Logging

```rust
use camel_builder::RouteBuilder;
use camel_component_log::LogComponent;
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::new();
    ctx.register_component("log", Box::new(LogComponent::new()));

    let route = RouteBuilder::from("http://0.0.0.0:8080/api")
        .to("log:api.requests?level=info&showHeaders=true")
        .process(|ex| async move {
            // Handle request
            Ok(ex)
        })
        .build()?;

    ctx.add_route(route).await?;
    ctx.start().await?;

    Ok(())
}
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-log)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
