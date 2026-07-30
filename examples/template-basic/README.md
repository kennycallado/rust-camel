# template-basic

Minimal example showing external template rendering with the `camel-template` component using a file-based MiniJinja template.

## What this example shows

- Registering the `template` component in a `CamelContext`
- Using `template:file:///<abs-path>` as a producer endpoint
- Template rendering with `{{ body }}` from the exchange body and header variables

## Structure

```
template-basic/
├── Cargo.toml
├── templates/
│   └── page.html.tmpl   # MiniJinja template with {{ title }} and {{ body }}
└── src/
    └── main.rs           # Route: timer → set body/headers → template → log
```

## Route

```
timer:tick → set_body("World") → set_header("title") → template:file:///... → log:info
```

The exchange body becomes `{{ body }}` in the template. All exchange headers are also available as template variables.

## Running

```bash
cd examples/template-basic
cargo run
```

Output (every 2 seconds):

```
INFO template-demo: Rendered template <rendered HTML>
```
