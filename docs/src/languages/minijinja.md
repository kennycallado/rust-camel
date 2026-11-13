# MiniJinja

A MiniJinja (Python Jinja2-inspired) template rendering implementation of the Language SPI. It renders structured output such as HTML, JSON, or prompts from Exchange data.

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
    - transform:
        language: minijinja
        source: |
          {% autoescape "html" %}
          <h1>Hello, {{ headers.name }}!</h1>
          {% endautoescape %}
    - to: log:rendered?showBody=true&showHeaders=true
```

</details>

Each `MinijinjaExpression` owns an `Arc<minijinja::Environment<'static>>`. Templates are added once during construction and compiled immediately. Subsequent evaluations look up templates by name with no recompilation. At evaluation time, the expression renders the template against the exchange context and returns the output as the expression value. The template context exposes `body`, `headers`, and `exchangeProperty`. Exchange headers are available as `headers.name` inside the template. The context also exposes the callable readers `header(name)`, `property(name)`, `has_header(name)`, and `has_property(name)`. The presence readers test key containment: a key whose stored value is `null` is present. The reader maps are a per-evaluation snapshot, and templates cannot mutate the Exchange. An XML body renders as a flat string.

Every template source must wrap in exactly one top-level `{% autoescape "html"|"json"|"none" %}...{% endautoescape %}` block. A lexical validator enforces this at compile time and rejects malformed templates immediately. This gives render output a declared escape strategy before any data interpolation occurs. Synchronous MiniJinja rendering runs on a Tokio blocking thread via `spawn_blocking`. The route future wraps the join handle in `tokio::time::timeout` for the configured render deadline. MiniJinja fuel provides an instruction budget that stops runaway templates, infinite loops, and algorithmic-complexity attacks.

Use MiniJinja for template-driven body generation: HTML pages, JSON payloads, and prompt strings assembled from multiple fields. For a body derived from a single expression, the [transform](../eip/transform.md) step is simpler. Templates are inline by default. A `source_file` reference loads one from a file. The engine does not evaluate `{% include %}` or template inheritance. `{% include %}`, template inheritance, and hot-reload belong to the Phase 2 template component (`camel-template`, bd rc-64if).

## Scripts from files

A `source_file` reference loads the template from a file. The `.jinja` and `.jinja2` extensions infer `minijinja`. Any other extension needs an explicit `language: minijinja`.

```yaml
- transform:
    source_file: templates/orders.jinja
```

The loader passes the file contents to the engine unchanged, byte for byte. The file needs the same single top-level `{% autoescape %}` block as an inline template. See [Scripts from files](../eip/script.md) for how to set the field and for the error cases.

## Body values and streams

The `body` context value is projected per body variant:

- `Body::Empty` renders as an empty string, not `null`.
- `Body::Text` and `Body::Xml` render as a string.
- `Body::Json` renders as the structured value.
- `Body::Bytes` renders as UTF-8 text. Bytes that are not valid UTF-8 fail with a `conversion` error; they are never replaced with U+FFFD.

A `Body::Stream` is carried as an invalid value that is validated at lookup. Any lookup of `body` refuses immediately with the stream refusal: interpolation, filter or function coercion, comparison, iteration, member access, a `{% set %}` capture, and a truth test. A template that never reads `body`, such as a header-only template, renders normally and leaves the stream unconsumed. A `body` reference in an unexecuted branch is never looked up, so it does not refuse.

## Time

MiniJinja templates have no wall-clock access: there is no `now` function, filter, or context value. The template context is exchange-derived (`body`, `headers`, `exchangeProperty`) and does not grow. The sandbox surface is unchanged.

A route that needs a timestamp computes it upstream with Simple `date:` or Rhai `now_ms()` and passes it as a header or property. Set a header with `${date:now}` and render `{{ headers.renderedAt }}`. `{{ now() }}` fails at evaluation as an unknown function.

See [Value boundaries across languages](value-boundary.md) for missing-value and null-versus-empty semantics across all languages.

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [MiniJinja crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-minijinja/CONTEXT.md)
