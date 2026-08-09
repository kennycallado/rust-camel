# Transform

The Transform step is a Message Translator from Hohpe and Woolf. It sets the exchange body from a literal value, a Simple expression, or a Rhai expression. In the Rust builder API, `transform` is an alias for `set_body`. Both compile to the same `SetBody` processor.

```rust,ignore
{{#include ../../../examples/transform-pipeline/src/main.rs:transform-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: transform-demo
  from: timer:tick?period=1000
  steps:
    - set_header:
        key: prefix
        value: hello
    - transform:
        simple: "${header.prefix} world"
    - to: log:transformed?showBody=true
```

</details>

The Rust `.set_body(value)` call accepts any type that implements `Into<Body>`. Strings, JSON values, and byte vectors all qualify. The step discards the old body and stores the new value on the exchange. When the new body depends on the current exchange, call `.set_body_fn(closure)` instead. The closure receives an `&Exchange` and returns a `Body`, so it can read headers, properties, and the inbound body to compute the replacement. The `.transform(value)` method forwards to `set_body` and exists for parity with the Apache Camel route DSL.

The YAML `transform:` step accepts three shapes that map to the `SetBodyConfig` fields in the DSL layer. A literal (`transform: "hello"`) stores the value as-is. A Simple expression (`transform: { simple: "${body.upper()}" }`) evaluates the expression and stores the result. A Rhai expression (`transform: { rhai: "body + '_processed'" }`) does the same through the Rhai engine. The Rust API splits the static and dynamic cases across `set_body` and `set_body_fn`. The YAML form unifies them under one step.

Transform overwrites the body. [Convert Body](convert-body.md) re-types the existing body without changing its content. A route that needs new content uses Transform. A route that needs the same content under a different type (`Text` to `Json`) uses Convert Body.

Transform evaluates one expression and writes one result. [Script](script.md) runs a full Rhai block that can mutate headers, properties, and body in the same step. A route that needs a single replacement uses Transform. A route that needs branching logic or several mutations uses Script.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the step compiles into a `Service<Exchange>` in the Tower middleware pipeline. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/transform-pipeline`](https://github.com/kennycallado/rust-camel/tree/main/examples/transform-pipeline).
