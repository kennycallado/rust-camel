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
- **Configurable root**: An optional `[components.template] root` anchors relative entries and, when set, confines every template read to one canonical directory tree
- **openat-based confinement**: All file reads go through `openat`-relative handles; `..`, symlinks, and cycles are rejected

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-template = "*"
```

## URI Format

```
template:file:///<absolute-path-to-template>
template:file:<relative-path-to-template>
```

The URI has two parts. The outer scheme is `template`. The inner scheme is `file`, followed by an absolute path or a canonical relative path.

| Inner form | Description |
|------------|-------------|
| `file:///<abs-path>` | Absolute filesystem path to the entry template |
| `file:<rel-path>` | Canonical relative entry path, resolved against the configured root |

Bare paths (`template:/srv/t/page.html`) and non-`file:` inner schemes are rejected at endpoint construction with `CamelError::Config`.

The absolute form must be free of `..` segments. The relative form is the canonical `template:file:<rel-path>` spelling: it has no `./` prefix, no `.` or `..` components, and no empty components. Any other relative spelling is a configuration error. A relative entry requires `[components.template] root`; without it, endpoint construction fails with `CamelError::Config`.

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
[components.template]
default-autoescape = "html"

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

`default-autoescape` is an optional operator default for the entry template. Precedence is: a valid explicit top-level `{% autoescape %}` block in the entry, then the configured default, then rejection at compile time. The default applies only when the entry contains no `{% autoescape %}` or `{% endautoescape %}` statement at all, so an invalid, partial, duplicate, or malformed explicit statement still rejects. The injected block wraps the entry only; include targets keep their own escaping semantics. The key is absent by default, and absence always requires the explicit block — the framework never applies an implicit HTML default. Only the exact lowercase values `"html"`, `"json"`, and `"none"` are accepted; any other value fails config load and names the key with the allowed values. Inline `language: minijinja` templates are unchanged and keep their mandatory explicit block.

## Configured root (`root`)

```toml
[components.template]
root = "templates"
```

`root` may be absolute or relative to the process working directory. It is resolved and canonicalized at route start; a missing root directory fails the route with an error that names the configured value. A relative root depends on where the process starts, so run from the project directory or configure an absolute root.

When `root` is configured, it anchors every `template:` endpoint. The relative entry form is the canonical `template:file:<rel-path>` spelling described in [URI Format](#uri-format).

| `root` configured | Entry form | Behavior |
|-------------------|------------|----------|
| no | absolute | The entry's parent directory anchors the closure. Existing behavior is unchanged. |
| no | relative | `CamelError::Config` at endpoint construction. |
| yes | relative | Resolved under the canonical root; the entry must exist. |
| yes | absolute | Validated inside the canonical root. An entry outside it is a configuration error. |

### Confinement guarantees

- The configured root and the entry candidate are both canonicalized at route start.
- `.` and `..` components are rejected in relative entry URIs, and `..` is rejected in the dependency closure.
- Leaf and intermediate symlinks are rejected with `O_NOFOLLOW`, even when the link target stays inside the root.
- The dependency closure is confined to the canonical root. Every `include`, `extends`, `import`, and `from` target resolves relative to the canonical root, not the entry's directory, and a target that escapes the root is rejected. Without a configured root, the closure resolves relative to the entry's parent directory.
- A missing root fails at route start with an error naming the configured root value and the filesystem error. A missing entry and an absolute entry outside the canonical root fail with an error naming the entry and the canonical root.

**Absolute spelling rule.** An absolute entry must be spelled under the configured root's canonical path. An absolute path that reaches the root through a symlink alias is rejected; use a relative entry or the canonical absolute spelling. Confinement opens the original path components to reject symlinks, so a canonical-name open cannot be substituted for an alias spelling.

`root` changes file location and confinement only. ADR-0047 remains in force: render semantics and the autoescape contract are unchanged, and no implicit autoescape is added.

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
| Relative entry with no configured root | `CamelError::Config` | Endpoint construction fails |
| `.` or `..` component in a relative entry URI | `CamelError::Config` | Endpoint construction fails |
| Missing configured root or entry | `CamelError::Config` | Route fails to start |
| Absolute entry outside the configured root | `CamelError::Config` | Route fails to start |
| Symlink, `..`, or include escape in the closure | `CamelError::TemplateReload` | Route fails to start (fail-closed) |

On every render failure, the exchange body is **not mutated** — the inbound body reaches the error handler byte-identical to what was submitted.

### Error phases: compile vs render

The component separates control-plane compile failures from data-plane render failures, because the two happen at different times, carry different error types, and have different recovery semantics.

**1. Compile phase (control plane).** Runs at startup build and at hot-reload build. Failures include MiniJinja syntax errors, autoescape wrapper violations, non-UTF-8 template source, a missing entry template, and dependency-closure acquisition problems (cycles, path escapes, duplicate identities). Syntax, autoescape, non-UTF-8 source, and missing-entry failures are `TemplateReloadError::Compile` (display prefix `template compilation failed:`); the closure problems use their dedicated `TemplateReloadError` variants. All of them surface as `CamelError::TemplateReload`, and at startup the route fails closed. A failed hot-reload keeps the prior compiled set.

**2. Render phase (data plane).** Runs per Exchange. Failures include strict-undefined variable access, unknown filters or functions, template lookup failure at render time, the output cap (`LimitedWriter`), execution timeout, `spawn_blocking` join errors, and non-UTF-8 output. These are `TemplateReloadError::Render` (display prefix `template render failed:`) and surface as `CamelError::ProcessorError`. The Exchange body is left unchanged.

**3. Logging.** On a render failure the producer emits one WARN with target `camel::template` carrying `phase=render`, `template` (the entry name), and `class` (best-effort error class: `undefined`, `filter`, `function`, `syntax`, `lookup`, `timeout`, `join`, `utf8`, or `other`). The log carries no error string and no exchange payload or context: minijinja interpolates operand values into some error displays, so the full error text flows exclusively on the `CamelError::ProcessorError` surface, which the empty-500 HTTP body policy keeps out of response bodies. Compile-phase diagnostics keep the distinct `template compilation failed` label.

**4. HTTP response body policy (explicit, unchanged).** When a render or compile error reaches an HTTP consumer, the HTTP error policy returns an empty 500 response body. That is the HTTP consumer's policy, not the template component's, and this component deliberately keeps it: diagnostics live in logs only, so neither an error detail nor a payload can leak through the response body.

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
