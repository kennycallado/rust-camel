# camel-config

> Configuration and route discovery for rust-camel

Configuration management for the Rust Camel framework. Provides profile-based configuration with environment variable overrides, component defaults, and supervision settings.

## Overview

`camel-config` manages application configuration through:

- **Camel.toml files** - Central configuration file
- **Config modularization** - Split config across files with `include = [...]`
- **Profile support** - Environment-specific settings ([default], [production], etc.)
- **Environment variables** - Override any setting with `CAMEL_*` variables
- **Route discovery** - Automatic route file discovery via glob patterns
- **Component defaults** - Global defaults for HTTP, Kafka, JMS, Redis, SQL, File, Container components
- **Supervision configuration** - Retry and backoff settings for route supervision

## Features

- 🎯 **Profile-based configuration** - Deep merge of profile settings with defaults
- 📦 **Config modularization** - Split `Camel.toml` across multiple files with `include`
- 🌍 **Environment variable overrides** - Override any config value with `CAMEL_*` prefix
- 📁 **Route discovery** - Automatic route file discovery from glob patterns
- ⚙️ **Component defaults** - Set global defaults for all component endpoints
- 🔄 **Hot reload support** - Optional file watching for configuration changes
- 🔧 **Supervision settings** - Configure retry strategies and backoff policies

## Camel.toml Format

Create a `Camel.toml` file in your project root:

```toml
[default]
routes = ["routes/*.yaml"]    # Default examples use YAML; explicit .json globs are also supported
watch = false
log_level = "info"
drain_timeout_ms = 10000  # Graceful drain timeout for hot-reload restart/remove (default: 10s)
watch_debounce_ms = 300  # Debounce window for hot-reload file watcher (ms)

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

[default.components.jms]
default_broker = "main"

[default.components.jms.brokers.main]
broker_url = "tcp://localhost:61616"
broker_type = "activemq"

[default.components.container]
docker_host = "unix:///var/run/docker.sock"

# Observability
[default.observability.prometheus]
enabled = true
port = 9090

[default.observability.health]
enabled = false    # standalone health server (disabled by default)
port = 8080        # health server port

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
- **`[default.observability.<name>]`** - Observability settings (prometheus, health, otel, tracer)
- **`[<profile>]`** - Profile-specific overrides (merged with default)
- **`[<profile>.supervision]`** - Profile-specific supervision overrides
- **`[<profile>.components.<name>]`** - Profile-specific component overrides
- **`[beans.<name>]`** - WASM bean plugin registrations

## Core Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `routes` | `[String]` |  | Glob patterns for route files (default examples use YAML; explicit `.json` globs also supported) |
| `watch` | `bool` |  | Enable hot reload on file changes |
| `runtime_journal_path` | `String?` |  | Optional durability flag: when set, enables local runtime journal replay |
| `log_level` | `String` |  | Logging level (trace/debug/info/warn/error) |
| `timeout_ms` | `u64` |  | Default operation timeout |
| `drain_timeout_ms` | `u64` |  | Max time to wait for in-flight exchanges to complete on Restart/Remove (default: 10000) |
| `watch_debounce_ms` | `u64` | `300` | Debounce window in ms for the hot-reload file watcher. Set to `0` to disable debouncing. |
| `supervision.*` | - |  | Retry and backoff settings |
| `observability.health.enabled` | `bool` | `false` | Enable standalone health server |
| `observability.health.port` | `u16` | `8080` | Health server port |

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

### JMS Component

Requires the `jms` feature flag:

```toml
[dependencies]
camel-config = { version = "*", features = ["jms"] }
```

```toml
[default.components.jms]
default_broker = "main"                       # which broker to use when no ?broker= param
max_bridges = 8                               # max concurrent bridge processes (default: 8)
bridge_cache_dir = "/path/to/cache"           # bridge binary cache directory (default: XDG cache)
bridge_start_timeout_ms = 10000               # bridge startup timeout in ms (default: 10000)
broker_reconnect_interval_ms = 5000           # reconnect interval in ms (default: 5000)

[default.components.jms.brokers.main]
broker_url = "tcp://localhost:61616"          # broker connection URL
broker_type = "activemq"                      # "activemq" or "artemis"
username = "admin"                            # optional broker username
password = "admin"                            # optional broker password

# Add more brokers as needed:
# [default.components.jms.brokers.secondary]
# broker_url = "tcp://other-host:61616"
# broker_type = "artemis"
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
ssl_mode = "require"           # SSL mode (optional)
ssl_root_cert = "/etc/ssl/ca.pem"  # CA certificate path (optional)
ssl_cert = "/etc/ssl/client.pem"   # Client certificate path (optional)
ssl_key = "/etc/ssl/client.key"    # Client key path (optional)
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

### Beans

```toml
[beans.auth]
plugin = "my-auth-bean"
```

WASM bean plugins registered at startup. Each entry creates an isolated WASM instance.

#### Optional WASM runtime limits

Each bean can override the WASM runtime defaults via a `[limits]` sub-table. Fields are `Option<T>`; any unset field falls back to the runtime default (per ADR-0011 — no silent surprises).

```toml
[default.beans.carto-kit]
plugin = "carto-kit"

[default.beans.carto-kit.limits]
timeout-secs = 600          # default: 30
max-memory = 4294967296     # default: 52428800 (50 MiB)
max-concurrent-calls = 4    # default: 4 (informational for beans)
```

The shared `WasmLimitsConfig` type lives in `crates/camel-config/src/wasm_limits.rs` and is re-exported as `camel_config::WasmLimitsConfig`. The same type is embedded in `[security.permissions.providers.<name>]` for WASM authorization policies. See ADR-0014 for the design rationale.

## Config Modularization (include)

Split your `Camel.toml` into smaller files using the top-level `include` field.
Included files are merged in order — last wins — before the root file is applied.
This is useful for sharing component defaults across deployments while keeping
environment-specific overrides in the root file.

```toml
# Camel.toml
include = ["config/components.toml", "config/observability.toml"]

[default]
routes = ["routes/*.yaml"]
log_level = "info"

[production]
log_level = "warn"
```

```toml
# config/components.toml  (flat — no profile sections required)
[components.http]
connect_timeout_ms = 5000
max_connections = 200

[components.kafka]
brokers = "kafka:9092"
group_id = "camel"
```

```toml
# config/observability.toml
[observability.prometheus]
enabled = true
port = 9090
```

**Rules:**

- Paths are relative to the file declaring `include` (no absolute paths, no URL schemes).
- Path traversal outside the directory is rejected (`../` and symlinks that escape).
- Duplicate paths in the same `include` list are a fatal error.
- Included files may not themselves use `include` (no recursive includes in V1 — a warning is logged and the field is ignored).
- Profile propagation: if a profile is active, included files are processed with the same profile (using lenient mode — flat files without profile sections are accepted as-is).
- All include errors are fatal; there is no partial-config fallback.

## Security Configuration

The `[security]` section configures authentication and authorization for the runtime. All sub-sections are optional -- enable only the providers you need.

### `[security.oidc]` -- OpenID Connect

Validates JWT tokens against an OIDC issuer.

```toml
[security.oidc]
issuer = "https://auth.example.com/realms/my-realm"
jwks_uri = "https://auth.example.com/realms/my-realm/protocol/openid-connect/certs"
audience = ["my-service"]
client_id = "my-service"
client_secret = "{{env:OIDC_CLIENT_SECRET}}"   # resolved from env
token_endpoint = "https://auth.example.com/realms/my-realm/protocol/openid-connect/token"
introspection_endpoint = "https://auth.example.com/realms/my-realm/protocol/openid-connect/token/introspect"
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `issuer` | string | yes | OIDC issuer URL |
| `jwks_uri` | string | no | Override JWKS endpoint (auto-discovered from issuer if omitted) |
| `audience` | [string] | no | Accepted `aud` claim values |
| `client_id` | string | no | OAuth2 client ID |
| `client_secret` | string | no | OAuth2 client secret (redacted in debug output) |
| `token_endpoint` | string | no | Override token endpoint |
| `introspection_endpoint` | string | no | Override introspection endpoint |

### `[security.native]` -- Native / Static Authentication

Static credentials and optional built-in token issuer for development or trusted-network deployments.

```toml
[security.native]
subject = "system"
issuer = "rust-camel-native"
bearer_token = "{{env:SYSTEM_BEARER_TOKEN}}"
roles = ["admin", "operator"]
scopes = ["read", "write"]

# Optional: built-in JWT issuer for machine-to-machine tokens
[security.native.token_issuer]
issuer = "rust-camel-internal"
audience = ["internal"]
token_ttl_secs = 900
signing_key_env = "SIGNING_KEY"    # env var holding the HMAC or RSA key

# Optional: pre-registered M2M clients
[[security.native.clients]]
client_id = "svc-orders"
client_secret_env = "ORDERS_SECRET"
roles = ["service"]
scopes = ["read:orders", "write:orders"]
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `subject` | string | yes | Static subject identity |
| `issuer` | string | no | Issuer claim for the native identity |
| `bearer_token` | string | no | Static bearer token (redacted in debug output) |
| `api_key` | string | no | Static API key (redacted in debug output) |
| `roles` | [string] | no | Granted roles |
| `scopes` | [string] | no | Granted OAuth scopes |
| `token_issuer` | table | no | Built-in JWT issuer config (`NativeIssuerConfig`) |
| `clients` | [table] | no | M2M client registrations (`NativeM2mClientConfig`) |

### `[security.keycloak]` -- Keycloak Integration

Full Keycloak integration with local JWT validation, introspection caching, and optional UMA authorization.

```toml
[security.keycloak]
server_url = "https://keycloak.example.com"
realm = "my-realm"
client_id = "my-service"
client_secret = "{{env:KEYCLOAK_CLIENT_SECRET}}"    # redacted in debug output

[security.keycloak.validation]
method = "local"               # "local" (default) or "introspection"
audience = ["my-service"]
clock_skew_secs = 30

[security.keycloak.jwks]
cache_ttl_secs = 3600          # JWKS cache TTL (default: 3600)
refresh_skew_secs = 60         # Refresh before expiry (default: 60)

[security.keycloak.introspection]
max_entries = 10000            # LRU cache size (default: 10000)
default_ttl_secs = 60          # Positive cache TTL (default: 60)
negative_ttl_secs = 5          # Negative cache TTL (default: 5)

# Optional: Keycloak UMA (User-Managed Access) authorization
[security.keycloak.uma]
provider = "keycloak-uma"

[security.keycloak.uma.cache]
positive_ttl_secs = 30         # default: 30
negative_ttl_secs = 5          # default: 5
max_entries = 10000            # default: 10000
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `server_url` | string | yes | Keycloak base URL |
| `realm` | string | yes | Realm name |
| `client_id` | string | yes | OAuth2 client ID |
| `client_secret` | string | yes | OAuth2 client secret (redacted in debug output) |
| `validation.method` | string | no | `"local"` (default) or `"introspection"` |
| `validation.audience` | [string] | no | Accepted audience values |
| `validation.clock_skew_secs` | u64 | no | Allowed clock skew (default: 30) |
| `jwks.cache_ttl_secs` | u64 | no | JWKS cache TTL (default: 3600) |
| `jwks.refresh_skew_secs` | u64 | no | Pre-expiry refresh window (default: 60) |
| `introspection.max_entries` | usize | no | Cache capacity (default: 10000) |
| `introspection.default_ttl_secs` | u64 | no | Positive cache TTL (default: 60) |
| `introspection.negative_ttl_secs` | u64 | no | Negative cache TTL (default: 5) |
| `uma.provider` | string | no | UMA provider name (e.g. `"keycloak-uma"`) |
| `uma.cache.*` | table | no | Permission cache tuning (same fields as `PermissionCacheConfig`) |

### `[security.permissions]` -- Permission Providers

Named permission evaluator configurations. Each entry registers a provider with its own cache settings.

```toml
[security.permissions.resource-access]
provider = "keycloak-uma"
path = "/resource-check"

[security.permissions.resource-access.cache]
positive_ttl_secs = 30
negative_ttl_secs = 5
max_entries = 10000
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `provider` | string | yes | Provider implementation name |
| `path` | string | no | Provider-specific path or endpoint |
| `config` | map | no | Extra key-value provider config |
| `cache.positive_ttl_secs` | u64 | no | Positive cache TTL (default: 30) |
| `cache.negative_ttl_secs` | u64 | no | Negative cache TTL (default: 5) |
| `cache.max_entries` | usize | no | LRU cache capacity (default: 10000) |

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
// Inside an async function
let mut ctx = CamelConfig::configure_context(&config).await?;

// Register component bundles — each bundle reads its own config key from [components.*]
use camel_component_api::ComponentBundle;

if let Some(raw) = config.components.raw.get("http").cloned() {
    camel_component_http::HttpBundle::from_toml(raw)?.register_all(&mut ctx);
}

if let Some(raw) = config.components.raw.get("kafka").cloned() {
    camel_component_kafka::KafkaBundle::from_toml(raw)?.register_all(&mut ctx);
}
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

Component config is stored as raw TOML values under `[components.*]` and owned by each bundle:

```rust
use camel_config::CamelConfig;

let config = CamelConfig::from_file("Camel.toml")?;

// Check if HTTP config is present in the raw map
if let Some(raw) = config.components.raw.get("http") {
    println!("HTTP config present: {:?}", raw);
}

// Parse the config via the bundle (same as what camel-cli does)
use camel_component_api::ComponentBundle;
if let Some(raw) = config.components.raw.get("kafka").cloned() {
    let bundle = camel_component_kafka::KafkaBundle::from_toml(raw)?;
    println!("Kafka bundle ready");
}
```

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-config = "*"
```

## Related Crates

- **camel-core** - Core framework and context
- **camel-dsl** - YAML and JSON route definitions and parsing
- **camel-processor** - EIP processors and middleware patterns

## Documentation

For more details, see the [API documentation](https://docs.rs/camel-config) and the main [Rust Camel project](https://github.com/your-org/rust-camel).
