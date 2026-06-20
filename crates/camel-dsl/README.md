# camel-dsl

>
> DSL support for rust-camel (YAML, JSON)

## Overview

`camel-dsl` provides Domain Specific Language support for defining routes in rust-camel declaratively. Routes can be defined in YAML or JSON files and loaded at runtime, enabling external configuration without recompiling the application.

 This crate is useful when you want to:
- Externalize route configuration
- Define routes without writing Rust code
- Enable non-developers to modify routes via configuration files

## Features

- **YAML and JSON route definitions**: Define routes using YAML or JSON syntax
- **Declarative integration flows**: Use all available EIPs and DSL
- **External configuration**: Load routes from files at runtime
- **Language expressions**: Use `simple:` and `rhai:` syntax for dynamic values
- **Route-level configuration**: Auto-startup, startup ordering, concurrency, error handling, circuit breaker, unit-of-work hooks

- **Environment variable interpolation**: Inject env vars in route files using `${env:VAR_NAME}` syntax with optional defaults `${env:VAR_NAME:-default}`
- **All step types**: to, log, set_header, set_body, transform, filter, choice, split, aggregate, delay, wire_tap, multicast, recipient_list, stop, script, bean, throttle, load_balance, dynamic_router, routing_slip, do_try

## Supported YAML Steps

### Core Steps
to, log, set_header, set_body, transform, filter, choice, split, aggregate, delay, wire_tap, multicast, recipient_list, stop, script, bean, do_try

### Delay
Delay exchange processing:

```yaml
steps:
  - delay: 500                      # shorthand: 500ms fixed
  - delay:                          # full form:
      delay_ms: 1000
      dynamic_header: CamelDelayMs
```

### Throttle
Rate-limit message processing:

```yaml
steps:
  - throttle:
      max_requests: 10
      period_secs: 1
      strategy: "delay"
      steps:
        - to: "mock:result"
```

### Load Balance
Distribute across endpoints:

```yaml
steps:
  - load_balance:
      strategy: "round_robin"
      parallel: false
      steps:
        - to: "mock:a"
        - to: "mock:b"
```

Weighted distribution (follows Apache Camel's `distributionRatio` pattern):

```yaml
steps:
  - load_balance:
      strategy: "weighted"
      distribution_ratio: "4,2,1"
      steps:
        - to: "seda:x"
        - to: "seda:y"
        - to: "seda:z"
```

The `distribution_ratio` string maps weights to steps by position. `"4,2,1"` means for every 7 messages: 4 to `seda:x`, 2 to `seda:y`, 1 to `seda:z`.

### Dynamic Router
Route to endpoints determined at runtime:

```yaml
steps:
  - dynamic_router:
      simple: "${header.dest}"
```

> **Note:** The `dynamic_router` uses a 60s timeout by default. This is a rust-camel extension — Apache Camel's `dynamicRouter` does not expose a timeout option.

### Routing Slip
Dynamic routing slip pattern:

```yaml
steps:
  - routing_slip:
      simple: "${header.slip}"
```

### Recipient List
Dynamically resolve endpoints from an expression at runtime:

```yaml
steps:
  - recipient_list:
      simple: "${header.destinations}"
```

The expression should evaluate to a comma-separated list of endpoint URIs. Supports parallel execution and aggregation strategies.

### Bean
Invoke a registered bean:

```yaml
steps:
  - bean:
      name: "myProcessor"
      method: "handle"
```

## Installation
Add to your `Cargo.toml`:

```toml
[dependencies]
camel-dsl = "0.8"
```

## Quick Start

### Basic Route

Create a YAML file `routes.yaml`:

```yaml
routes:
  - id: "hello-timer"
    from: "timer:tick?period=2000"
    steps:
      - log: "Timer fired!"
      - to: "log:info"
```

Load and add to context:

```rust
use camel_dsl::load_from_file;
use camel_core::context::CamelContext;
use camel_component_timer::TimerComponent;
use camel_component_log::LogComponent;

let mut ctx = CamelContext::builder().build().await?;
ctx.register_component(TimerComponent::new());
ctx.register_component(LogComponent::new());

let routes = load_from_file("routes.yaml")?;
for route in routes {
    ctx.add_route_definition(route).await?;
}

ctx.start().await?;
```

### With Language Expressions

```yaml
routes:
  - id: "filter-demo"
    from: "timer:tick?period=1000"
    steps:
      - set_header:
          key: "type"
          value: "allowed"
      - filter:
          simple: "${header.type} == 'allowed'"
          steps:
            - log: "Passed filter!"
            - to: "log:filtered"
```

## JSON Route Definitions

JSON route definitions use the same shape as YAML — the only difference is the serialization format. This means all step types, route-level configuration, and language expressions work identically in both formats.

### Example

Create a JSON file `routes.json`:

```json
{
  "routes": [
    {
      "id": "hello-timer",
      "from": "timer:tick?period=2000",
      "steps": [
        { "log": "Timer fired!" },
        { "to": "log:info" }
      ]
    }
  ]
}
```

### Programmatic Loading

```rust
// Parse a JSON string directly
use camel_dsl::parse_json;

let routes = parse_json(&json_string)?;

// Load from a file
use camel_dsl::json::load_json_from_file;
use std::path::Path;

let routes = load_json_from_file(Path::new("routes.json"))?;
```

The type aliases `JsonRoutes`, `JsonRoute`, and `JsonStep` are convenience wrappers around the YAML AST types and are **not a stable SDK contract**. SDKs and external consumers that need forward compatibility should target `CanonicalRouteSpec` instead:

```rust
use camel_dsl::parse_json_to_canonical;

let specs = parse_json_to_canonical(&json_string)?;
```

### Discovery Behavior

When using [`discover_routes`] to load route files via glob patterns, **JSON files require an explicit `.json` glob pattern**. Broad patterns like `routes/*` will intentionally not load `.json` files — this prevents accidental loading from mixed-format directories.

```rust
use camel_dsl::discover_routes;

// ✅ Explicit .json pattern — loads JSON files
let routes = discover_routes(&["routes/*.json".to_string()])?;

// ❌ Broad pattern — will NOT load .json files (returns an error if any are matched)
let routes = discover_routes(&["routes/*".to_string()])?;

// Mixing both formats in one call is fine:
let routes = discover_routes(&[
    "routes/*.yaml".to_string(),
    "routes/*.json".to_string(),
])?;
```

## Canonical Route Spec

The `canonical` module provides direct parsing of the cross-language route IR, bypassing the YAML/JSON DSL input formats:

```rust
use camel_dsl::canonical::{parse_canonical_json, parse_canonical_route};

// Parse a batch of canonical routes from JSON
let routes = parse_canonical_json(r#"{
    "routes": [{
        "route_id": "my-route",
        "from": "timer:tick?period=1000",
        "steps": [
            { "step": "log", "config": { "message": "Hello" } },
            { "step": "to", "config": { "uri": "log:info" } }
        ],
        "version": 1
    }]
}"#)?;

// Or compile a single CanonicalRouteSpec
let spec = CanonicalRouteSpec::new("my-route", "timer:tick");
let route = parse_canonical_route(spec)?;
```

### Generate Type Artifacts

```bash
cargo run -p xtask schema
```

Produces JSON Schema and TypeScript types in `schemas/`.

## Available Step Types

| Step | Description | Example |
|------|-------------|---------|
| `to` | Send to endpoint | `- to: "log:info"` |
| `log` | Log message | `- log: "Processing"` |
| `set_header` | Set header | `- set_header: { key: "x", value: "y" }` |
| `set_body` | Set body | `- set_body: { value: "content" }` |
| `transform` | Transform body | `- transform: { simple: "${body}" }` |
| `marshal` | Serialize body using a data format (e.g., Json → Text) | `- marshal: json` |
| `unmarshal` | Deserialize body using a data format (e.g., Text → Json) | `- unmarshal: xml` |
| `filter` | Filter messages | `- filter: { simple: "${header.type} == 'allowed'", steps: [...] }` |
| `choice` | Content-based router | `- choice: { when: [...], otherwise: [...] }` |
| `split` | Split message | `- split: { expression: "body_lines", steps: [...] }` |
| `aggregate` | Aggregate messages with size/timeout completion | `- aggregate: { header: "id", completion_size: 5, completion_timeout_ms: 5000 }` |
| `delay` | Delay exchange processing | `- delay: 500` or `- delay: { delay_ms: 1000, dynamic_header: "CamelDelayMs" }` |
| `wire_tap` | Fire-and-forget tap | `- wire_tap: "direct:audit"` |
| `multicast` | Fan-out to multiple | `- multicast: { steps: [...] }` |
| `recipient_list` | Dynamic recipient list | `- recipient_list: { simple: "${header.destinations}" }` |
| `stop` | Stop pipeline | `- stop: true` |
| `script` | Execute script | `- script: { language: "simple", source: "${body}" }` |
| `bean` | Invoke bean method | `- bean: { name: "orderService", method: "process" }` |

### Marshal/Unmarshal Example

```yaml
routes:
  - id: "convert-format"
    from: "direct:input"
    steps:
      - unmarshal: json
      - marshal: xml
      - to: "direct:output"
```

This converts the message body from JSON to XML using the built-in data formats.

### Bean Step

The `bean` step allows you to invoke business logic registered in the BeanRegistry:

```yaml
routes:
  - id: "process-order"
    from: "direct:orders"
    steps:
      - bean:
          name: "orderService"
          method: "validate"
      - bean:
          name: "orderService"
          method: "process"
```

**Prerequisites:**
- Register beans in your Rust code using `BeanRegistry`
- Pass the registry to `DefaultRouteController::with_beans()`

```rust
use camel_bean::BeanRegistry;
use camel_core::DefaultRouteController;

let mut bean_registry = BeanRegistry::new();
bean_registry.register("orderService", OrderService);

let controller = DefaultRouteController::with_beans(bean_registry);
```

See `examples/bean-demo` for a complete example.

### Aggregate Step

The `aggregate` step supports size-based, timeout-based, or combined completion:

```yaml
routes:
  - id: "aggregate-demo"
    from: "timer:orders?period=200"
    steps:
      - aggregate:
          header: "orderId"
          completion_size: 3
          completion_timeout_ms: 5000
          force_completion_on_stop: true
          discard_on_timeout: false
      - log: "Batch completed"
      - to: "log:info"
```

| Field | Type | Description |
|-------|------|-------------|
| `header` | string | Correlation header name |
| `completion_size` | integer | Complete when N exchanges aggregated |
| `completion_timeout_ms` | integer | Inactivity timeout in ms (resets per exchange) |
| `force_completion_on_stop` | boolean | Force-complete all buckets on route stop |
| `discard_on_timeout` | boolean | Discard (instead of emit) incomplete exchanges on timeout |

## Environment Variable Interpolation

Use `${env:VAR_NAME}` anywhere in a route file to inject an environment variable at load time. This works for both YAML and JSON formats when loaded via [`discover_routes`]:

```yaml
routes:
  - id: "env-demo"
    from: "timer:tick?period=1000"
    steps:
      - log: "Broker: ${env:BROKER_URL}"
      - to: "${env:OUTPUT_ENDPOINT}"
```

The substitution happens before parsing, so it works in any position — URIs, log messages, header values, etc. If an environment variable is not set and no default is provided via the `:-` syntax, discovery returns a `DiscoveryError::Env` error — the literal `${env:VAR_NAME}` string is **not** left in place.

> **Caution:** Because interpolation is textual (performed before parsing), env values injected into JSON string positions must already be valid for their JSON context. For example, an env var containing an unescaped double quote will produce invalid JSON, causing a `DiscoveryError::Json` parse error. YAML is more forgiving of unquoted values but the same principle applies to structured contexts.

You can specify a default value when the variable is not set:

```yaml
routes:
  - id: "env-defaults"
    from: "timer:tick?period=${env:POLL_MS:-1000}"
    steps:
      - log: "Target: ${env:OUTPUT_URI:-log:info}"
      - to: "${env:OUTPUT_URI:-log:info}"
```

If `POLL_MS` is not set, the default `1000` is used. The `:-` syntax follows the same convention as shell parameter expansion.

> **Note:** Direct file loaders (`load_from_file` for YAML, `json::load_json_from_file` for JSON) read and parse files without interpolating environment variables. Use [`discover_routes`] if you need env interpolation.

## Language Expressions
Many steps support language expressions for dynamic values:
 predicates:

### Syntax
```yaml
# Simple language shortcut
- filter:
    simple: "${header.type} == 'allowed'"
    steps:
      - log: "Match!"

# Explicit language + source
- set_header:
    key: "computed"
    language: "simple"
    source: "${header.base} + '-suffix'"

# Rhai script
- script:
    language: "rhai"
    source: |
      let body = exchange.body();
      body.to_upper()
```

### Available Languages
- `simple` - Built-in Simple language (supports header/body interpolation)
- `rhai` - Rhai scripting language (requires `camel-language-rhai` feature)

## Route-Level Configuration
```yaml
routes:
  - id: "my-route"
    from: "timer:tick?period=1000"
    auto_startup: true        # Default: true
    startup_order: 100       # Default: 1000, lower = earlier
    concurrency: concurrent  # or "sequential"
    error_handler:             # Optional error handling
      dead_letter_channel: "log:errors"
      # Legacy single catch-all retry (still supported)
      retry:
        max_attempts: 3
        initial_delay_ms: 100

      # New ordered exception clauses (first-match-wins)
      on_exceptions:
        - kind: "Io"
          retry:
            max_attempts: 3
            initial_delay_ms: 100
            handled_by: "log:io-errors"
        - kind: "ProcessorError"
          message_contains: "validation"
          retry:
            max_attempts: 1
        - kind: "ProcessorError"
          message_contains: "recoverable"
          continued: true   # ← NEW: clear error, pipeline continues to next step
    circuit_breaker:           # Optional circuit breaker
      failure_threshold: 5
      open_duration_ms: 30000
    on_complete: "direct:on-complete"  # Optional completion hook URI
    on_failure: "direct:on-failure"    # Optional failure hook URI
```

#### `continued` field on `on_exceptions`

Each `on_exceptions` entry accepts `continued: true` to clear the error and allow the pipeline to continue to the next step after the error handler runs. It is mutually exclusive with `handled: true`.

```yaml
error_handler:
  dead_letter_channel: "log:errors"
  on_exceptions:
    - kind: "ProcessorError"
      continued: true           # ← clear error, pipeline continues
```

**Mutual exclusion** — `continued: true` and `handled: true` cannot be set simultaneously; the DSL compiler rejects both with a configuration error.

### Unit of Work Hooks (YAML)

```yaml
routes:
  - id: my-route
    from: "timer:tick"
    on_complete: "log:done"
    on_failure: "log:failed"
    steps:
      - log: "processing"
```

## Security Policy

The `security_policy` directive controls per-route authorization. It is a route-level setting, not a step -- it is evaluated before any step in the pipeline runs.

### `roles:` -- Role-Based Access Control

Require one or more roles. By default the user must have **all** listed roles; set `all_required: false` to accept any one.

```yaml
routes:
  - id: "admin-route"
    from: "direct:admin"
    security_policy:
      roles: ["admin", "operator"]
      all_required: true
    steps:
      - log: "Admin access granted"
```

### `scopes:` -- OAuth Scope Check

Require one or more OAuth scopes. Same `all_required` semantics as roles.

```yaml
routes:
  - id: "scoped-route"
    from: "direct:api"
    security_policy:
      scopes: ["read:orders", "write:orders"]
      all_required: false
    steps:
      - to: "log:info"
```

### `ref:` -- Named Policy Reference

Reference a security policy registered in the `SecurityPolicyRegistry` by name.

```yaml
routes:
  - id: "ref-route"
    from: "direct:secure"
    security_policy:
      ref: "admin-only"
    steps:
      - log: "Referenced policy applied"
```

### `wasm:` -- WASM Policy Evaluator

Delegate authorization to a WASM module registered in the `SecurityPolicyRegistry`. The value is the **registry name** of a policy declared in `[security.policies.wasm.<name>]` in Camel.toml (see `camel-config` README).

```yaml
routes:
  - id: "wasm-policy-route"
    from: "direct:check"
    security_policy:
      wasm: "auth-policy"
    steps:
      - to: "log:info"
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `wasm` | string | yes | Registry name of a WASM policy declared in Camel.toml |

> **Note** — Per-route `config:` is rejected. The policy's `config` is set once at registration time in Camel.toml (ADR-0014 §4). The runtime registry is instance-based, not factory-based; the same `<name>` shares one initialized policy across all referencing routes.

### `permission:` -- Attribute-Based Access Control (ABAC)

Evaluate a permission request against a registered `PermissionEvaluator` (e.g. Keycloak UMA).

```yaml
routes:
  - id: "uma-route"
    from: "direct:resource"
    security_policy:
      permission:
        policy: "resource-access"
        resource: "${header.resource-id}"
        action: "read"
        scopes: ["view"]
        cache_ttl_secs: 60
        cache_negative_ttl_secs: 5
    steps:
      - to: "log:info"
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `policy` | string | yes | Name of the registered permission provider |
| `resource` | string or expression | yes | Resource identifier (supports `${header.*}` expressions) |
| `action` | string or expression | yes | Requested action (supports `${header.*}` expressions) |
| `scopes` | [string] | no | Additional permission scopes |
| `cache_ttl_secs` | u64 | no | Override positive cache TTL |
| `cache_negative_ttl_secs` | u64 | no | Override negative cache TTL |

### Limitations

Routes that declare `security_policy` cannot currently use the canonical/hot-reload path. They must be loaded through the standard YAML/JSON DSL loader.

## Documentation
- [API Documentation](https://docs.rs/camel-dsl)
- [Examples](https://github.com/kennycallado/rust-camel/tree/main/examples)
- [Repository](https://github.com/kennycallado/rust-camel)
