# camel-language-minijinja

> MiniJinja template rendering language for rust-camel

## Overview

`camel-language-minijinja` provides a [MiniJinja](https://docs.rs/minijinja/2.21.0/minijinja/)-backed
inline template Language for rust-camel. It implements the `Language` and `Expression` SPI contracts
from `camel-language-api`.

Phase 1 supports inline templates only — source is authored in route configuration and compiled at
route startup. External file loading, includes, inheritance, and hot reload belong to the Phase 2
Component under **bd rc-64if**.

Route usage reuses the existing `set_body: { language: minijinja, source: ... }` DSL (no new Step
or schema fields). Route resolution calls `create_expression` once and reuses the compiled MiniJinja
environment for every Exchange.

See [ADR-0047](../../../docs/adr/0047-template-rendering-engine.md) for the full decision record including
backend selection, security model, and Phase 2 scoping.

## Escape-mode contract

Every template **must** be wrapped in exactly one top-level `{% autoescape "html" %}…{% endautoescape %}`,
`{% autoescape "json" %}…{% endautoescape %}`, or `{% autoescape "none" %}…{% endautoescape %}`
block. There is no global default — the explicit wrapper is mandatory.

| Condition | Behaviour |
|-----------|-----------|
| Missing or duplicated top-level `autoescape` | `LanguageError::ParseError` at compile time |
| Nested `autoescape` inside the wrapper | `LanguageError::ParseError` at compile time |
| Malformed mode argument | `LanguageError::ParseError` at compile time |
| Text outside the wrapper (whitespace allowed) | `LanguageError::ParseError` at compile time |
| `autoescape` inside `{% raw %}…{% endraw %}`, `{# … #}`, or string literals | Correctly ignored by the lexical validator |

## Security model

- **Source-from-configuration-only (S1).** Template source is always authored in route config.
  Message headers cannot replace the source or select a different template.
- **Strict undefined (S2).** Referencing a missing path such as `{{ headers.user.missing.field }}`
  errors at render time. There is no silent null or empty-string coercion.
- **Context exposure limited (S3).** Templates see only `body`, `headers`, and `exchangeProperty`.
  CamelContext, filesystem, environment variables, and the registry are not accessible.

  > **Note:** `Body::Xml` is exposed as a flat string in templates — not as
  > traversable structured data. XML-aware traversal belongs to `camel-xslt`
  > or Phase 2 template includes.
- **`Body::Stream` rejected (S4).** A streaming body cannot be synchronously rendered. Add
  `stream_cache` upstream (`crates/camel-processor/src/stream_cache.rs`) to materialise it first.
- **No custom filters or globals (S8).** Only MiniJinja built-ins (`urlencode`, `upper`, `default`,
  etc.) are available. No `shell_escape` / `sql_escape` or host-defined filters.

## Timeout caveat

`execution-timeout-ms` wraps the render call in `tokio::time::timeout` + `tokio::task::spawn_blocking`.
When the timeout fires, the route future resolves to `LanguageError::EvalError("minijinja execution timeout")`,
but the blocking thread may continue executing until it trips `fuel` or `max-recursion-depth`, or
the bounded output writer stops accepting data. This is the same partial-mitigation caveat shared
with the Rhai and Boa engines.

Keep all four limits (`fuel`, `max-recursion-depth`, `max-output-size`, `execution-timeout-ms`)
finite to bound residual thread execution.

## Configuration

Resource limits are configurable in `Camel.toml`. Every field is `Option<…>`; un-set fields fall
back to the defaults shown.

```toml
[languages.minijinja.limits]
max-template-source-size = 1048576      # 1 MiB (default)
max-context-size = 4194304              # 4 MiB (default)
max-output-size = 4194304               # 4 MiB (default)
fuel = 100000                           # default
max-recursion-depth = 64                # default
execution-timeout-ms = 5000             # default
```

Setting any individual field overrides only that field; all others remain at defaults.

## Examples

Each example wraps content in the required top-level `autoescape` block.

### HTML fragment

```jinja
{% autoescape "html" %}
<html><body><h1>{{ headers.title }}</h1><ul>
{% for item in headers.items %}<li>{{ item }}</li>{% endfor %}
</ul></body></html>
{% endautoescape %}
```

### JSON snippet

```jinja
{% autoescape "json" %}
{"v":[{% for item in headers.items %}{{ item }}{% if not loop.last %},{% endif %}{% endfor %}]}
{% endautoescape %}
```

### LLM prompt

```jinja
{% autoescape "none" %}
System: {{ headers.system_prompt }}

User: {{ body }}
{% endautoescape %}
```

## License

Apache-2.0
