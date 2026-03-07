# camel-config

Configuration management for the Rust Camel framework. Provides profile-based configuration with environment variable overrides and supervision settings.

## Overview

`camel-config` manages application configuration through:

- **Camel.toml files** - Central configuration file
- **Profile support** - Environment-specific settings ([default], [production], etc.)
- **Environment variables** - Override any setting with `CAMEL_*` variables
- **Route discovery** - Automatic route file discovery via glob patterns
- **Supervision configuration** - Retry and backoff settings for route supervision

## Features

- 🎯 **Profile-based configuration** - Deep merge of profile settings with defaults
- 🌍 **Environment variable overrides** - Override any config value with `CAMEL_*` prefix
- 📁 **Route discovery** - Automatic route file discovery from glob patterns
- ⚙️ **Supervision settings** - Configure retry strategies and backoff policies
- 🔄 **Hot reload support** - Optional file watching for configuration changes

## Camel.toml Format

Create a `Camel.toml` file in your project root:

```toml
[default]
routes = ["routes/*.yaml"]
watch = false
log_level = "info"
shutdown_timeout_secs = 30

[default.supervision]
initial_delay_ms = 1000
backoff_multiplier = 2.0
max_delay_ms = 60000
max_attempts = 5

[production]
log_level = "warn"
watch = false

[production.supervision]
initial_delay_ms = 5000
max_attempts = 10

[development]
log_level = "debug"
watch = true
```

### Configuration Sections

- **`[default]`** - Base configuration (required)
- **`[default.supervision]`** - Default supervision settings
- **`[<profile>]`** - Profile-specific overrides (merged with default)
- **`[<profile>.supervision]`** - Profile-specific supervision overrides

### Key Fields

| Field | Type | Description |
|-------|------|-------------|
| `routes` | `[String]` | Glob patterns for route files |
| `watch` | `bool` | Enable hot reload on file changes |
| `log_level` | `String` | Logging level (trace/debug/info/warn/error) |
| `shutdown_timeout_secs` | `u64` | Graceful shutdown timeout |
| `supervision.initial_delay_ms` | `u64` | Initial retry delay |
| `supervision.backoff_multiplier` | `f64` | Exponential backoff multiplier |
| `supervision.max_delay_ms` | `u64` | Maximum retry delay cap |
| `supervision.max_attempts` | `u32` | Maximum retry attempts (0 = unlimited) |

## Profile Selection

Set the active profile via the `CAMEL_PROFILE` environment variable:

```bash
# Use production profile
export CAMEL_PROFILE=production
cargo run

# Use development profile
export CAMEL_PROFILE=development
cargo run
```

If not set, defaults to the `[default]` profile only.

## Environment Variables

Override any configuration value with environment variables using the `CAMEL_` prefix:

```bash
# Override log level
export CAMEL_LOG_LEVEL=debug

# Override watch mode
export CAMEL_WATCH=true

# Override supervision settings
export CAMEL_SUPERVISION_INITIAL_DELAY_MS=2000
export CAMEL_SUPERVISION_MAX_ATTEMPTS=10

# Override routes
export CAMEL_ROUTES='["routes/*.yaml", "routes/extra/*.yaml"]'
```

Environment variables take precedence over file configuration and are applied after profile merging.

## Usage

### Basic Usage

```rust
use camel_config::CamelConfig;

// Load from Camel.toml in current directory
let config = CamelConfig::from_file("Camel.toml")?;

// Access configuration values
println!("Log level: {}", config.log_level);
println!("Routes: {:?}", config.routes);
```

### With Profile

```rust
use camel_config::CamelConfig;

// Load with profile (reads CAMEL_PROFILE env var or uses default)
let config = CamelConfig::from_file_with_profile("Camel.toml")?;

// The profile is automatically merged with [default]
```

### With Environment Variables

```rust
use camel_config::CamelConfig;

// Load with full override support (profile + env vars)
let config = CamelConfig::from_file_with_env("Camel.toml")?;

// Environment variables override all other sources
```

### Accessing Supervision Configuration

```rust
use camel_config::CamelConfig;

let config = CamelConfig::from_file("Camel.toml")?;

if let Some(supervision) = &config.supervision {
    println!("Initial delay: {}ms", supervision.initial_delay_ms);
    println!("Backoff multiplier: {}", supervision.backoff_multiplier);
    println!("Max delay: {}ms", supervision.max_delay_ms);
    println!("Max attempts: {}", supervision.max_attempts);
}
```

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-config = "0.1"
```

## Related Crates

- **camel-core** - Core framework and context
- **camel-dsl** - YAML route definitions and parsing
- **camel-runtime** - Route execution engine

## Documentation

For more details, see the [API documentation](https://docs.rs/camel-config) and the main [Rust Camel project](https://github.com/your-org/rust-camel).
