# camel-template

> External template component for rust-camel (MiniJinja, file-based, ADR-0047 Stage 2)

## Overview

The Template component renders MiniJinja templates loaded from the filesystem against the body and headers of each inbound exchange. It is producer-only — you place it on the `to:` side of a route to transform the exchange body into rendered output.

Templates are compiled once at route startup (fail-closed) and cached for zero-filesystem-I/O hot-path rendering. The component supports hot-reload: a control-plane `ReloadTemplates` command re-acquires the dependency closure, recompiles, and atomically swaps the compiled set without disturbing in-flight renders.

## Features

- **File-based**: Templates live in standard `.html.tmpl` files on disk
- **Fail-closed**: Compilation errors prevent the route from starting; render errors preserve the original exchange body unchanged
- **Zero-override**: The template source and root directory are operator-configured at startup — no exchange header or property can override them
- **Bounded acquisition**: Configurable limits on total source bytes, include count/depth, single-template size, and reload wall-clock timeout
- **Bounded render**: Per-render limits on context size, output size, fuel, recursion depth, and execution timeout (inherited from the MiniJinja language engine)
- **Atomic hot-reload**: Dependency-closure re-acquisition, compilation, and swap — prior set retained on failure
- **openat-based confinement**: All file reads go through `openat`-relative handles; `..`, symlinks, absolute paths, and cycles are rejected

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-template = "*"
```

## URI Format

```
template:file:///<absolute-path-to-template>
```

The URI has two parts:

| Part | Description |
|------|-------------|
| `template` | Outer scheme identifying this component |
| `file:///<abs-path>` | Inner scheme with an absolute filesystem path to the entry template |

Bare paths (`template:/srv/t/page.html`) and non-`file:` inner schemes are rejected at endpoint construction. The path must be absolute and free of `..` segments.

## Usage

### Basic Template Render

```rust
use camel_builder::RouteBuilder;
use camel_core::CamelContext;
use camel_processor::LogLevel;
use camel_template::TemplateComponent;
use camel_template::ExternalTemplateLimitsConfig;
use camel_language_api::MinijinjaLimitsConfig;
use camel_api::Value;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::builder().build().await?;

    ctx.register_component(TemplateComponent::new(
        ExternalTemplateLimitsConfig::default(),
        MinijinjaLimitsConfig::default(),
    ));

    let route = RouteBuilder::from("timer:tick?period=2000")
        .route_id("template-demo")
        .set_body("World")
        .set_header("title", Value::String("Greeting".into()))
        .to("template:file:///srv/templates/page.html.tmpl")
        .log("Rendered template", LogLevel::Info)
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;
    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;
    Ok(())
}
```

Template file (`/srv/templates/page.html.tmpl`):

```html
{% autoescape "html" -%}
<!DOCTYPE html>
<html>
<head><title>{{ title }}</title></head>
<body>
  <h1>{{ title }}</h1>
  <p>Hello, {{ body }}!</p>
</body>
</html>
{%- endautoescape %}
```

The exchange body becomes the `{{ body }}` variable in the template context. All exchange headers are also available as top-level variables.

### Template with Exchange Context

```rust
let route = RouteBuilder::from("timer:order-event")
    .route_id("order-confirmation")
    .set_body("Order #1234 received")
    .set_header("customer_name", Value::String("Alice".into()))
    .set_header("total", Value::String("$42.00".into()))
    .to("template:file:///srv/templates/email.html.tmpl")
    .to("log:info?showBody=true")
    .build()?;
```

Template (`email.html.tmpl`):

```html
{% autoescape "html" -%}
<p>Dear {{ customer_name }},</p>
<p>{{ body }}</p>
<p>Total: {{ total }}</p>
{%- endautoescape %}
```

### Configuration via Camel.toml

```toml
[components.template.limits]
max-total-source-bytes = 33554432
max-include-count = 128
max-template-size = 2097152
reload-timeout-ms = 10000

[components.template.render-limits]
max-context-size = 65536
max-output-size = 1048576
fuel = 100000
```

## Error Behavior

| Scenario | Error | Behavior |
|----------|-------|----------|
| Missing template file on startup | `CamelError::TemplateReload` | Route fails to start (fail-closed) |
| Template compilation error | `CamelError::TemplateReload` | Route fails to start; error details logged |
| `Body::Stream` at render time | `CamelError::ProcessorError` | Body unchanged, error propagated |
| Strict-undefined variable access | `CamelError::ProcessorError` | Body unchanged, error propagated |
| Output size exceeds limit | `CamelError::ProcessorError` | Render halted, body unchanged |
| Execution timeout | `CamelError::ProcessorError` | Render cancelled, body unchanged |
| Bounded-acquisition exceeded | `CamelError::TemplateReload` | Route fails to start (fail-closed) |

On every render failure, the exchange body is **not mutated** — the inbound body reaches the error handler byte-identical to what was submitted.

### Empty body caveat

An empty inbound body (`Body::Empty`) is exposed to the template as `null`. Under minijinja's default stringification, `{{ body }}` renders the **literal word `none`** — not an empty string, and not an error (strict-undefined does not trip because the `body` key *is present*). If a route may receive empty bodies, guard the reference explicitly:

```jinja
{% if body %}{{ body }}{% endif %}
{# or #}
{{ body | default("") }}
```

See bd `rc-wnqj` for a planned shared-engine fix.

## Documentation

- [API Documentation](https://docs.rs/camel-template)
- [Repository](https://github.com/kennycallado/rust-camel)
- [ADR-0047: Template Rendering Design](https://github.com/kennycallado/rust-camel/docs/adr/0047-template-rendering.md)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
