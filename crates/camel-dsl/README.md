# camel-dsl

> 
> DSL support for rust-camel (YAML, etc)

## Overview

`camel-dsl` provides Domain Specific Language support for defining routes in rust-camel declaratively. Routes can be defined in YAML files and loaded at runtime, enabling external configuration without recompiling the application.

 This crate is useful when you want to:
- Externalize route configuration
- Define routes without writing Rust code
- Enable non-developers to modify routes via configuration files

## Features

- **YAML route definitions**: Define routes using YAML syntax
- **Declarative integration flows**: Use all available EIPs and DSL
- **External configuration**: Load routes from files at runtime
- **Language expressions**: Use `simple:` and `rhai:` syntax for dynamic values
- **Route-level configuration**: Auto-startup, startup ordering, concurrency, error handling, circuit breaker, unit-of-work hooks

- **Environment variable interpolation**: Inject env vars in route files using `${env:VAR_NAME}` syntax
- **All step types**: to, log, set_header, set_body, transform, filter, choice, split, aggregate, wire_tap, multicast, stop, script, bean

## Installation
Add to your `Cargo.toml`:

```toml
[dependencies]
camel-dsl = "0.2"
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

let mut ctx = CamelContext::new();
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

## Available Step Types

| Step | Description | Example |
|------|-------------|---------|
| `to` | Send to endpoint | `- to: "log:info"` |
| `log` | Log message | `- log: "Processing"` |
| `set_header` | Set header | `- set_header: { key: "x", value: "y" }` |
| `set_body` | Set body | `- set_body: { value: "content" }` |
| `transform` | Transform body (alias for `set_body`) | `- transform: { simple: "${body}" }` |
| `marshal` | Serialize body using a data format (e.g., Json → Text) | `- marshal: json` |
| `unmarshal` | Deserialize body using a data format (e.g., Text → Json) | `- unmarshal: xml` |
| `filter` | Filter messages | `- filter: { simple: "${header.type} == 'allowed'", steps: [...] }` |
| `choice` | Content-based router | `- choice: { when: [...], otherwise: [...] }` |
| `split` | Split message | `- split: { expression: "body_lines", steps: [...] }` |
| `aggregate` | Aggregate messages with size/timeout completion | `- aggregate: { header: "id", completion_size: 5, completion_timeout_ms: 5000 }` |
| `wire_tap` | Fire-and-forget tap | `- wire_tap: "direct:audit"` |
| `multicast` | Fan-out to multiple | `- multicast: { steps: [...] }` |
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

Use `${env:VAR_NAME}` anywhere in a YAML route file to inject an environment variable at load time:

```yaml
routes:
  - id: "env-demo"
    from: "timer:tick?period=1000"
    steps:
      - log: "Broker: ${env:BROKER_URL}"
      - to: "${env:OUTPUT_ENDPOINT}"
```

The substitution happens before YAML parsing, so it works in any position — URIs, log messages, header values, etc. Unset variables are left as-is (the literal `${env:VAR_NAME}` string).

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
    circuit_breaker:           # Optional circuit breaker
      failure_threshold: 5
      open_duration_ms: 30000
    on_complete: "direct:on-complete"  # Optional completion hook URI
    on_failure: "direct:on-failure"    # Optional failure hook URI
```

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

## Documentation
- [API Documentation](https://docs.rs/camel-dsl)
- [Examples](https://github.com/kennycallado/rust-camel/tree/main/examples)
- [Repository](https://github.com/kennycallado/rust-camel)
