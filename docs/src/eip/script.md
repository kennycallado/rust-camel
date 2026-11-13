# Script

The Script step is a Message Translator from Hohpe and Woolf. It runs an inline Rhai script against the exchange so the route can mutate headers, properties, and body in one step. rust-camel registers Rhai as the scripting language.

```rust,ignore
{{#include ../../../examples/language-rhai/src/main.rs:script-step}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: script-demo
  from: timer:tick?period=1000
  steps:
    - script:
        language: rhai
        source: |
          headers["tenant"] = "acme";
          body = body + "_processed"
    - to: log:scripted?showBody=true&showHeaders=true
```

</details>

The `.script("rhai", source)` call hands the script text to the Rhai engine. The engine exposes three variables on the exchange: `headers` and `properties` as mutable maps, and `body` as the current body string. The script reads and writes all three. Assignments to `headers["key"]` and `body` propagate to the next step in the pipeline.

Script handles logic that one expression cannot express. A Rhai block runs conditionals, loops, and several mutations in a single step. [Transform](transform.md) evaluates one expression and writes one result. A route that needs a single body replacement uses Transform. A route that needs branching logic or several field changes uses Script.

Script interprets Rhai at runtime, so it runs slower than a native `.process(|ex| ...)` closure. The closure compiles to Rust and reads the full `Exchange` type. A route that needs maximum throughput uses `process`. A route that needs logic it can change without a rebuild uses Script.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the step compiles into a `Service<Exchange>` in the Tower middleware pipeline. The Rhai integration is documented in [camel-language-api/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md).

The example source is at [`examples/language-rhai`](https://github.com/kennycallado/rust-camel/tree/main/examples/language-rhai).

## Scripts from files

A `source_file` field loads the source from a file. The field replaces the inline `language`+`source` pair. The loader reads the file once, when the route loads.

```yaml
- id: script-file-demo
  from: timer:tick?period=1000
  steps:
    - script:
        source_file: scripts/enrich.rhai
    - to: log:scripted?showBody=true
```

The extension selects the language when `language:` is absent:

| Extension | Language |
|---|---|
| `.rhai` | `rhai` |
| `.js` | `js` |
| `.jsonpath` | `jsonpath` |
| `.xpath` | `xpath` |
| `.jinja`, `.jinja2` | `minijinja` |

The extension match is case-sensitive. Set `language:` for any other extension.

A relative path resolves against the directory of the route file that holds the reference. An absolute path is used unchanged. The Rust DSL constructor `LanguageExpressionDef::file` resolves a relative path against the process current working directory, because no route file exists at that call.

The contents reach the engine verbatim. Environment interpolation runs on the route document only. A script file receives no interpolation and no template substitution, so a `${env:NAME}` token reaches the engine as literal text.

`source_file` is one source form. It cannot pair with `source` or with a shorthand field such as `rhai:`. Route load fails with a named error in these cases:

| Failure | Error text |
|---|---|
| Missing or unreadable file | `cannot read script file`, the absolute path, and the OS error |
| Empty or whitespace-only file | `script file ... is empty` and the absolute path |
| `source` and `source_file` together | the context, both field names, and the absolute path |
| `source_file` with a shorthand field | the exactly-one source form error |
| Unknown extension without `language:` | the absolute path and the extension |

Every read, extension-inference, and `source`/`source_file` conflict error names the resolved absolute path.

A real file extension also improves editor support: syntax highlighting, lint, format, and go-to-definition.
