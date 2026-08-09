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
          headers["tenant"] = "acme"
          body = body + "_processed"
    - to: log:scripted?showBody=true&showHeaders=true
```

</details>

The `.script("rhai", source)` call hands the script text to the Rhai engine. The engine exposes three variables on the exchange: `headers` and `properties` as mutable maps, and `body` as the current body string. The script reads and writes all three. Assignments to `headers["key"]` and `body` propagate to the next step in the pipeline.

Script handles logic that one expression cannot express. A Rhai block runs conditionals, loops, and several mutations in a single step. [Transform](transform.md) evaluates one expression and writes one result. A route that needs a single body replacement uses Transform. A route that needs branching logic or several field changes uses Script.

Script interprets Rhai at runtime, so it runs slower than a native `.process(|ex| ...)` closure. The closure compiles to Rust and reads the full `Exchange` type. A route that needs maximum throughput uses `process`. A route that needs logic it can change without a rebuild uses Script.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the step compiles into a `Service<Exchange>` in the Tower middleware pipeline. The Rhai integration is documented in [camel-language-api/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md).

The example source is at [`examples/language-rhai`](https://github.com/kennycallado/rust-camel/tree/main/examples/language-rhai).
