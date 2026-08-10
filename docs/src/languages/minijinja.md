# MiniJinja

A MiniJinja (Jinja2-compatible) template rendering implementation of the Language SPI. It renders structured output such as HTML, JSON, or prompts from Exchange data.

```rust,ignore
use camel_language_api::Language;
use camel_language_minijinja::MinijinjaLanguage;

let lang = MinijinjaLanguage::default();
let expr = lang.create_expression(
    r#"{% autoescape "html" %}
<h1>Hello, {{ headers.name }}!</h1>
{% endautoescape %}"#,
)?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: language-minijinja-demo
  from: timer:tick?period=1000&repeatCount=3
  steps:
    - set_header:
        key: name
        value: World
    - script:
        language: minijinja
        source: |
          {% autoescape "html" %}
          <h1>Hello, {{ headers.name }}!</h1>
          {% endautoescape %}
    - to: log:rendered?showBody=true&showHeaders=true
```

</details>

Each `MinijinjaExpression` owns an `Arc<minijinja::Environment<'static>>`. Templates are added once during construction and compiled immediately. Subsequent evaluations look up templates by name with no recompilation. At evaluation time, the expression renders the template against the exchange context and returns the output as the expression value. Exchange headers are available as `headers.name` inside the template.

Every template source must wrap in exactly one top-level `{% autoescape "html"|"json"|"none" %}...{% endautoescape %}` block. A lexical validator enforces this at compile time and rejects malformed templates immediately. This gives render output a declared escape strategy before any data interpolation occurs. Synchronous MiniJinja rendering runs on a Tokio blocking thread via `spawn_blocking`. The route future wraps the join handle in `tokio::time::timeout` for the configured render deadline. MiniJinja fuel provides an instruction budget that stops runaway templates, infinite loops, and algorithmic-complexity attacks.

Use MiniJinja for template-driven body generation: HTML pages, JSON payloads, and prompt strings assembled from multiple fields. For a body derived from a single expression, the [transform](../eip/transform.md) step is simpler. Phase 1 covers inline templates only. External file loading, `{% include %}`, template inheritance, and hot-reload belong to Phase 2.

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [MiniJinja crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-minijinja/CONTEXT.md)
