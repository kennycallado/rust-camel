# camel-health

> Health check support for rust-camel

## Overview

`camel-health` provides health check capabilities for the rust-camel framework. It integrates with health monitoring systems and exposes the health status of routes, components, and the overall integration context.

## Features

- Health check integration with `CamelContext`
- Route health monitoring
- Component health status
- Integration with external health check systems

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-health = "0.2"
```

## Usage

```rust
use camel_health::HealthCheck;
use camel_core::CamelContext;

// Create context with health checks
let ctx = CamelContext::new();

// Check health status
// (API depends on implementation details)
```

## Documentation

- [API Documentation](https://docs.rs/camel-health)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
