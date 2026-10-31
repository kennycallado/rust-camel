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
          key: "title"
          value: "Template Demo"
      - to: "template:file:///srv/templates/page.html.tmpl"
      - log: "Rendered template"
```

The Rust example reads the template path from `env!("CARGO_MANIFEST_DIR")` so it builds a runnable path against the example's own `templates/` directory. Substitute your real absolute path in production URIs.

</details>

## URI

```text
template:file:///<absolute-path-to-template>
template:file:<relative-path-to-template>
```

The URI has two parts. The outer scheme is `template`. The inner scheme is `file`, followed by an absolute path or a canonical relative path. Bare paths (`template:/srv/t/page.html`) and non-`file` inner schemes are rejected at Endpoint construction with `CamelError::Config`.

The absolute form has an empty authority (`template:file:///`). The path must be absolute and free of `..` segments.

The canonical relative form is `template:file:<rel-path>`. It has no `./` prefix, no `.` or `..` components, and no empty components. Any other relative spelling is a configuration error. A relative entry requires `[components.template] root`; without it, Endpoint construction fails with `CamelError::Config`.

The component is zero-override. The URI is operator-configured at route construction. No Exchange header or property can replace the entry, the root, or the template source. The `template:file:///` form is the only absolute shape; there is no `template:http://` or header-driven loader.

### Configured root

```toml
[components.template]
root = "templates"
```

`root` may be absolute or relative to the process working directory. The component resolves and canonicalizes it at route start; a missing directory fails the route with an error naming the configured value. A relative root depends on where the process starts: run from the project directory, or configure an absolute root.

When `root` is configured, it anchors every `template:` Endpoint:

| `root` configured | Entry form | Behavior |
|-------------------|------------|----------|
| no | absolute | The entry's parent directory anchors the closure. Existing behavior is unchanged. |
| no | relative | `CamelError::Config` at Endpoint construction. |
| yes | relative | Resolved under the canonical root; the entry must exist. |
| yes | absolute | Validated inside the canonical root. An entry outside it is a configuration error. |

Confinement is enforced when the route starts:

- The configured root and the entry candidate are both canonicalized.
- `.` and `..` components are rejected in relative entry URIs, and `..` is rejected in the dependency closure.
- Leaf and intermediate symlinks are rejected with `O_NOFOLLOW`, even when the link target stays inside the root.
- The dependency closure is confined to the canonical root. Every `include`, `extends`, `import`, and `from` target resolves relative to the canonical root, not the entry's directory, and a target that escapes the root is rejected. Without a configured root, the closure resolves relative to the entry's parent directory.
- A missing root fails the route with an error naming the configured root value and the filesystem error. A missing entry and an absolute entry outside the canonical root fail with an error naming the entry and the canonical root.

Absolute entries must be spelled under the configured root's canonical path. An absolute path that reaches the root through a symlink alias is rejected; use a relative entry or the canonical absolute spelling. Confinement opens the original path components to reject symlinks, so an alias spelling cannot be accepted.

The root setting changes file location and confinement only. ADR-0047 remains in force: render semantics and the autoescape contract are unchanged, and no implicit autoescape is added.

## Render contract

The Producer replaces `exchange.input.body` with the rendered output as `Body::Text`. Headers and properties are preserved unchanged. On any render failure — strict-undefined variable, output-size overflow, fuel exhaustion, recursion limit, or execution timeout — the body is left byte-identical to the inbound value. The route `ErrorHandler` then owns the operational signal.

The rendering context exposes three top-level keys: `body`, `headers`, and `exchangeProperty`. `Body::Text` and `Body::Bytes` (lossy UTF-8) are accepted. `Body::Json` exposes its fields as `{{ body.field }}`. `Body::Stream` is rejected with a guidance error pointing to `stream_cache` upstream. `Body::Empty` renders to an empty string without tripping strict-undefined.

Every template must select `html`, `json`, or `none` through a top-level `{% autoescape %}` block or through the component-wide `default-autoescape` configuration described below. The component enforces the selection at compile time. There is no global default; per-render output context is an operator decision, not a framework assumption.

### Default autoescape

The external component accepts an optional operator default under `[components.template]`:

```toml
[components.template]
default-autoescape = "html"
```

| Value | Meaning |
| --- | --- |
| `html` | Escape output for an HTML text or attribute context. |
| `json` | Escape output for a JSON string context. |
| `none` | Do not escape. This is an explicit operator opt-out, not an implicit fallback. |

Only the exact lowercase strings are accepted. Any other string, or a non-string value, fails config load and names `components.template.default-autoescape` together with the allowed values.

Precedence is: a valid explicit top-level `{% autoescape %}` block in the entry, then the configured default, then rejection at compile time.

The default applies only when the entry contains no `{% autoescape %}` or `{% endautoescape %}` statement at all. An entry with an invalid mode, a partial block, a duplicate block, or malformed syntax still rejects at compile time. The configuration never repairs an explicit statement.

The injected block wraps the entry only. Include, import, and extends targets keep their own escaping semantics; the environment-wide auto-escape callback is never changed.

The key is absent by default, and absence always requires the explicit block. The framework never applies an implicit HTML default. Inline `language: minijinja` templates are not affected: they keep the mandatory explicit block required by ADR-0047.

### Filters

The component registers exactly two filters of its own — `truncate(length, end="…")` and `dump` — on top of the filters the locked MiniJinja engine itself provides. It registers no functions or globals of its own. Any other unregistered filter name fails at render time.

`truncate(length, end="…")` cuts a string to `length` Unicode scalar values. Only a non-negative integer length renders; a negative, non-integral, boolean, or non-numeric length is a render error. The `end` marker is appended only when truncation occurs, and it defaults to `…`. The filter has no HTML awareness: its output is ordinary text and stays subject to the active escape mode.

`dump` produces deterministic JSON serialization of the value, with object keys sorted. An unserializable value is a render error. The output is ordinary text, so it stays subject to the active escape mode; it is not marked safe.

### HTML escaping and `&#x2f;`

The component keeps MiniJinja's built-in HTML escaper unchanged. That escaper escapes `/` as `&#x2f;`, alongside the other HTML metacharacters, and the component neither adds a custom escaper nor weakens the built-in one.

`|safe` marks content as safe, and it is for trusted content only. Pre-rendered or interpolated content without explicit safe marking stays escaped. MiniJinja's formatter checks safe-string marking, not content provenance, so pre-rendering alone does not bypass autoescaping.

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

File reads go through `openat`-relative handles, anchored at the canonical root when `root` is configured and at the entry's parent directory otherwise. `..` segments, symlinks, absolute closure-target paths, and include cycles are rejected at acquisition time. The environment registers exactly two component-owned filters — `truncate` and `dump` (see [Filters](#filters)) — and no functions or globals of its own; any other unregistered filter name fails at render time.

## When to use it

Use the Template component when the rendered output needs blocks, loops, filters, macros, or context-aware escaping. Use the inline `minijinja` language under `set_body: { language: minijinja, source: ... }` for one-shot substitution where the template fits in a route definition. Use a different component when the transformation is XML, JSON bridging, or parameter-bound SQL.

The atomic-write contract and the accepted `fileExist` values live in the [camel-template CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-template/CONTEXT.md). The architectural rationale is in [ADR-0047](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0047-template-rendering-engine.md). Example source: [`examples/template-basic`](https://github.com/kennycallado/rust-camel/tree/main/examples/template-basic).
