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
- **Route-level configuration**: Auto-startup, startup ordering, concurrency, error handling, circuit breaker

- **All step types**: to, log, set_header, set_body, filter, choice, split, aggregate, wire_tap, multicast, stop, script

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
    ctx.add_route_definition(route)?;
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
| `filter` | Filter messages | `- filter: { simple: "${header.type} == 'allowed'", steps: [...] }` |
| `choice` | Content-based router | `- choice: { when: [...], otherwise: [...] }` |
| `split` | Split message | `- split: { expression: "body_lines", steps: [...] }` |
| `aggregate` | Aggregate messages | `- aggregate: { header: "id", completion_size: 5 }` |
| `wire_tap` | Fire-and-forget tap | `- wire_tap: "direct:audit"` |
| `multicast` | Fan-out to multiple | `- multicast: { steps: [...] }` |
| `stop` | Stop pipeline | `- stop: true` |
| `script` | Execute script | `- script: { language: "simple", source: "${body}" }` |

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
      retry:
        max_attempts: 3
        initial_delay_ms: 100
    circuit_breaker:           # Optional circuit breaker
      failure_threshold: 5
      open_duration_ms: 30000
```

## Documentation
- [API Documentation](https://docs.rs/camel-dsl)
- [Examples](../://github.com/kennycallado/rust-camel/tree/examples)
- [Repository](https://github.com/kennycallado/rust-camel)

