# camel-config

Configuration system for rust-camel.

## Overview

This crate provides configuration loading and route discovery for rust-camel applications.

## Features

- **Configuration Loading**: Load configuration from TOML files
- **Profile Support**: Multiple environments (development, production, etc.)
- **Environment Variables**: Override configuration with `CAMEL_*` prefix
- **Route Discovery**: Auto-discover route files using glob patterns

## Usage

### Basic Usage

```rust
use camel_config::CamelConfig;
use camel_core::CamelContext;

// Load configuration
let config = CamelConfig::from_file("Camel.toml")?;

// Create context
let mut ctx = CamelContext::new();
ctx.register_component(TimerComponent::new());

// Load routes from discovered files
let routes = camel_config::discover_routes(&config.routes)?;
for route in routes {
    ctx.add_route_definition(route)?;
}

ctx.start().await?;
```

### Configuration File

Create a `Camel.toml` file:

```toml
[default]
routes = ["routes/**/*.yaml"]
log_level = "INFO"

[production]
log_level = "ERROR"
```

### Profile Selection

```rust
// Via environment variable
std::env::set_var("CAMEL_PROFILE", "production");

// Or programmatically
let config = CamelConfig::from_file_with_profile("Camel.toml", Some("production"))?;
```

### Environment Variables

Override any configuration value:

```bash
export CAMEL_PROFILE=production
export CAMEL_LOG_LEVEL=DEBUG
export CAMEL_ROUTES_0="custom/*.yaml"
```

### Route Discovery

Routes are discovered using glob patterns:

```yaml
# routes/my-route.yaml
routes:
  - id: "hello-timer"
    from: "timer:tick?period=1000"
    steps:
      - to: "log:info"
```

## API

### CamelConfig

- `from_file(path)` - Load configuration from file
- `from_file_with_profile(path, profile)` - Load with specific profile
- `from_file_with_profile_and_env(path, profile)` - Load with env var overrides

### discover_routes

- `discover_routes(patterns)` - Discover route files matching patterns

## Configuration Schema

```toml
[default]
routes = ["routes/**/*.yaml"]
log_level = "INFO" | "DEBUG" | "WARN" | "ERROR"
timeout_ms = 5000

[default.components.timer]
period = 1000

[default.components.http]
connect_timeout_ms = 5000
max_connections = 100

[default.observability]
# Metrics are reserved for future use - parsed but not implemented yet
metrics_enabled = true
metrics_port = 9090

[default.observability.tracer]
enabled = true
detail_level = "minimal"  # "minimal" | "medium" | "full"

[default.observability.tracer.outputs.stdout]
enabled = true
format = "json"  # "json" | "plain"

# Optional supervision configuration
[default.supervision]
max_attempts = 5
initial_delay_ms = 1000
backoff_multiplier = 2.0
max_delay_ms = 60000

[production]
log_level = "ERROR"
```

### Notes

- **Metrics**: `metrics_enabled` and `metrics_port` under `[observability]` are reserved for future use. They are parsed but have no backing implementation yet.
- **Components**: Configuration in `[components]` (timer/http settings) is parsed from config but NOT automatically applied to the context. Users must register components manually and can read component config via the `CamelConfig` struct if needed.

## Examples

See the `examples/` directory:
- `config-basic` - Basic configuration usage
- `config-profiles` - Multi-environment setup

## License

Apache-2.0
