# camel-config

Configuration management for the Rust Camel framework. Provides profile-based configuration with environment variable overrides, component defaults, and supervision settings.

## Overview

`camel-config` manages application configuration through:

- **Camel.toml files** - Central configuration file
- **Profile support** - Environment-specific settings ([default], [production], etc.)
- **Environment variables** - Override any setting with `CAMEL_*` variables
- **Route discovery** - Automatic route file discovery via glob patterns
- **Component defaults** - Global defaults for HTTP, Kafka, Redis, SQL, File, Container components
- **Supervision configuration** - Retry and backoff settings for route supervision

## Features

- 🎯 **Profile-based configuration** - Deep merge of profile settings with defaults
- 🌍 **Environment variable overrides** - Override any config value with `CAMEL_*` prefix
- 📁 **Route discovery** - Automatic route file discovery from glob patterns
- ⚙️ **Component defaults** - Set global defaults for all component endpoints
- 🔄 **Hot reload support** - Optional file watching for configuration changes
- 🔧 **Supervision settings** - Configure retry strategies and backoff policies

## Camel.toml Format

Create a `Camel.toml` file in your project root:

```toml
[default]
routes = ["routes/*.yaml"]
watch = false
log_level = "info"

[default.supervision]
initial_delay_ms = 1000
backoff_multiplier = 2.0
max_delay_ms = 60000
max_attempts = 5

# Component defaults - apply to all endpoints unless overridden by URI
[default.components.http]
connect_timeout_ms = 5000
response_timeout_ms = 30000
max_connections = 100
allow_private_ips = false

[default.components.kafka]
brokers = "localhost:9092"
group_id = "camel"
session_timeout_ms = 45000

[default.components.redis]
host = "localhost"
port = 6379

[default.components.sql]
max_connections = 5
min_connections = 1
idle_timeout_secs = 300

[default.components.file]
delay_ms = 500
initial_delay_ms = 1000
read_timeout_ms = 30000
write_timeout_ms = 30000

[default.components.container]
docker_host = "unix:///var/run/docker.sock"

# Observability
[default.observability.prometheus]
enabled = true
port = 9090

[production]
log_level = "warn"
watch = false

[production.components.kafka]
brokers = "prod-kafka:9092"
group_id = "camel-prod"

[production.components.redis]
host = "prod-redis"
port = 6379

[development]
log_level = "debug"
watch = true

[development.components.http]
allow_private_ips = true  # Allow internal services in dev
```

## Configuration Sections

- **`[default]`** - Base configuration (required)
- **`[default.supervision]`** - Default supervision settings
- **`[default.components.<name>]`** - Component-specific global defaults
- **`[default.observability.<name>]`** - Observability settings (prometheus, otel, tracer)
- **`[<profile>]`** - Profile-specific overrides (merged with default)
- **`[<profile>.supervision]`** - Profile-specific supervision overrides
- **`[<profile>.components.<name>]`** - Profile-specific component overrides

## Core Fields

| Field | Type | Description |
|-------|------|-------------|
| `routes` | `[String]` | Glob patterns for route files |
| `watch` | `bool` | Enable hot reload on file changes |
| `runtime_journal_path` | `String?` | Optional durability flag: when set, enables local runtime journal replay |
| `log_level` | `String` | Logging level (trace/debug/info/warn/error) |
| `timeout_ms` | `u64` | Default operation timeout |
| `supervision.*` | - | Retry and backoff settings |

## Component Defaults

Configure global defaults for each component. URI parameters always take precedence.

### HTTP Component

```toml
[default.components.http]
connect_timeout_ms = 5000      # Connection timeout (default: 30000)
response_timeout_ms = 30000    # Response timeout (default: none)
max_connections = 100          # Max concurrent connections (default: 100)
max_body_size = 10485760       # Max response body size, 10MB (default: 10MB)
max_request_body = 2097152     # Max request body for server, 2MB (default: 2MB)
allow_private_ips = false      # Allow requests to private IPs (default: false)
```

### Kafka Component

```toml
[default.components.kafka]
brokers = "localhost:9092"     # Bootstrap servers (default: localhost:9092)
group_id = "camel"             # Consumer group ID (default: camel)
session_timeout_ms = 45000     # Consumer session timeout (default: 45000)
request_timeout_ms = 30000     # Producer request timeout (default: 30000)
auto_offset_reset = "latest"   # Offset reset: earliest/latest/none (default: latest)
security_protocol = "plaintext" # Security protocol (default: plaintext)
```

### Redis Component

```toml
[default.components.redis]
host = "localhost"             # Redis host (default: localhost)
port = 6379                    # Redis port (default: 6379)
```

### SQL Component

```toml
[default.components.sql]
max_connections = 5            # Max pool connections (default: 5)
min_connections = 1            # Min pool connections (default: 1)
idle_timeout_secs = 300        # Idle connection timeout (default: 300)
max_lifetime_secs = 1800       # Max connection lifetime (default: 1800)
```

### File Component

```toml
[default.components.file]
delay_ms = 500                 # Poll interval (default: 500)
initial_delay_ms = 1000        # Initial delay before first poll (default: 1000)
read_timeout_ms = 30000        # Read timeout (default: 30000)
write_timeout_ms = 30000       # Write timeout (default: 30000)
```

### Container Component

```toml
[default.components.container]
docker_host = "unix:///var/run/docker.sock"  # Docker daemon socket (default)
```

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

# Optional: enable runtime durability/replay by configuring a journal path
export CAMEL_RUNTIME_JOURNAL_PATH=.camel/runtime-events.jsonl

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

### Configure CamelContext with Component Defaults

Use `configure_context()` to automatically apply component defaults to the CamelContext:

```rust
use camel_config::CamelConfig;
use camel_core::CamelContext;
use camel_component_http::HttpComponent;
use camel_component_kafka::KafkaComponent;

// Load configuration
let config = CamelConfig::from_file_with_env("Camel.toml")?;

// Configure context with component defaults from Camel.toml
let mut ctx = CamelContext::new();
CamelConfig::configure_context(&config, &mut ctx)?;

// Register components - they will receive global defaults via context
let http_cfg = ctx.get_component_config::<camel_component_http::HttpConfig>().cloned();
ctx.register_component(HttpComponent::with_optional_config(http_cfg));

let kafka_cfg = ctx.get_component_config::<camel_component_kafka::KafkaConfig>().cloned();
ctx.register_component(KafkaComponent::with_optional_config(kafka_cfg));
```

### Accessing Supervision Configuration

```rust
use camel_config::CamelConfig;

let config = CamelConfig::from_file("Camel.toml")?;

if let Some(supervision) = &config.supervision {
    println!("Initial delay: {}ms", supervision.initial_delay_ms);
    println!("Backoff multiplier: {}", supervision.backoff_multiplier);
    println!("Max delay: {}ms", supervision.max_delay_ms);
    println!("Max attempts: {:?}", supervision.max_attempts);
}
```

### Accessing Component Defaults Programmatically

```rust
use camel_config::CamelConfig;

let config = CamelConfig::from_file("Camel.toml")?;

// Check if HTTP defaults are configured
if let Some(http) = &config.components.http {
    println!("HTTP connect timeout: {}ms", http.connect_timeout_ms);
    println!("HTTP allow private IPs: {}", http.allow_private_ips);
}

// Check if Kafka defaults are configured
if let Some(kafka) = &config.components.kafka {
    println!("Kafka brokers: {}", kafka.brokers);
    println!("Kafka group ID: {}", kafka.group_id);
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
