# Template

The Template component renders MiniJinja templates loaded from the filesystem against the body, headers, and properties of each inbound exchange. It is producer-only — you place it on the `to:` side of a route to transform the exchange body into rendered output.

The template-basic example wires a timer-driven source through a header set and the template Producer:

```rust,ignore
{{#include ../../../examples/template-basic/src/main.rs:template-basic-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: template-demo
    from: "timer:tick?period=2000"
    steps:
      - set_body: "World"
      - set_header:
          title: "Template Demo"
      - to: "template:file:///srv/templates/page.html.tmpl"
      - log: "Rendered template"
```

The Rust example reads the template path from `env!("CARGO_MANIFEST_DIR")` so it builds a runnable path against the example's own `templates/` directory. Substitute your real absolute path in production URIs.

</details>

## URI

```
template:file:///<absolute-path-to-template>
```

The URI has two parts. The outer scheme is `template`. The inner scheme is `file`, followed by an empty authority and an absolute path. Bare paths (`template:/srv/t/page.html`) and non-`file` inner schemes are rejected at Endpoint construction. The path must be absolute and free of `..` segments.

The component is zero-override. The URI is operator-configured at route construction. No Exchange header or property can replace the entry, the root, or the template source. The `template:file:///` form is the only accepted shape; there is no `template:http://` or header-driven loader.

## Render contract

The Producer replaces `exchange.input.body` with the rendered output as `Body::Text`. Headers and properties are preserved unchanged. On any render failure — strict-undefined variable, output-size overflow, fuel exhaustion, recursion limit, or execution timeout — the body is left byte-identical to the inbound value. The route `ErrorHandler` then owns the operational signal.

The rendering context exposes three top-level keys: `body`, `headers`, and `exchangeProperty`. `Body::Text` and `Body::Bytes` (lossy UTF-8) are accepted. `Body::Json` exposes its fields as `{{ body.field }}`. `Body::Stream` is rejected with a guidance error pointing to `stream_cache` upstream. `Body::Empty` renders to an empty string without tripping strict-undefined.

Every template must declare a top-level `{% autoescape %}` block selecting `html`, `json`, or `none`. The component enforces the explicit declaration at compile time. There is no global default; per-render output context is an operator decision, not a framework assumption.

## Limits

The bundle owns two independent limit layers under `[components.template]`. Both default when absent. A zero value is rejected at startup; a limit cannot be silently disabled.

```toml
[components.template.limits]
max-total-source-bytes = 16777216
max-include-count = 64
max-include-depth = 16
max-template-size = 1048572
reload-timeout-ms = 5000

[components.template.render-limits]
max-context-size = 65536
max-output-size = 1048576
fuel = 100000
```

| Layer | Field | Default | Bounds |
| --- | --- | --- | --- |
| acquisition | `max-total-source-bytes` | 16 MiB | total source bytes across the dependency closure |
| acquisition | `max-include-count` | 64 | included or imported templates per closure |
| acquisition | `max-include-depth` | 16 | nested include/extends depth |
| acquisition | `max-template-size` | 1 MiB | single template file in bytes |
| acquisition | `reload-timeout-ms` | 5000 | wall-clock budget for a full reload build |
| render | `max-context-size` | 64 KiB | serialized context bytes per render |
| render | `max-output-size` | 1 MiB | rendered output bytes per render |
| render | `fuel` | 100000 | per-render instruction accounting |

The two layers are checked at different times. Acquisition limits apply while building the compiled template set. Render limits apply per Exchange on the hot path. Exhausting any limit fails the operation; no limit truncates and reports success.

## Lifecycle

Templates are compiled once at route startup (fail-closed). A missing file, a syntax error, or an acquisition-limit overflow prevents the route from starting. The compiled set is cached for zero-filesystem-I/O hot-path rendering. A control-plane `ReloadTemplates` command re-acquires the dependency closure, recompiles, and atomically swaps the compiled set without disturbing in-flight renders. A failed reload preserves the prior set.

File reads go through `openat`-relative handles. `..` segments, symlinks, absolute path escapes, and include cycles are rejected at acquisition time. The MiniJinja environment registers no functions, filters, or globals — an unknown function call fails at render, not at the host boundary.

## When to use it

Use the Template component when the rendered output needs blocks, loops, filters, macros, or context-aware escaping. Use the inline `minijinja` language under `set_body: { language: minijinja, source: ... }` for one-shot substitution where the template fits in a route definition. Use a different component when the transformation is XML, JSON bridging, or parameter-bound SQL.

The atomic-write contract and the accepted `fileExist` values live in the [camel-template CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-template/CONTEXT.md). The architectural rationale is in [ADR-0047](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0047-template-rendering-engine.md). Example source: [`examples/template-basic`](https://github.com/kennycallado/rust-camel/tree/main/examples/template-basic).
