# JavaScript

A Boa-backed JavaScript implementation of the Language SPI. It provides `Expression`, `Predicate`, and `MutatingExpression` for the synchronous `script:` path defined by ADR-0006.

```rust,ignore
{{#include ../../../examples/language-js/src/main.rs:full}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: language-js-demo
  from: timer:tick?period=1000&repeatCount=3
  steps:
    - script:
        language: js
        source: |
          camel.headers.set("greeting", "Hello from JS");
          camel.headers.set("processedBy", "language-js-example");
          camel.body = "JS was here";
          "done";
    - to: log:js-output?showBody=true&showHeaders=true
```

</details>

The route above uses a `.script("js", ...)` step, which runs as a `MutatingExpression`. The script reads and writes exchange data through the `camel` global. `camel.headers` and `camel.properties` are map-like views with `get`, `set`, `has`, `remove`, and `keys`. `camel.body` holds the body value. After successful evaluation, the implementation writes body, header, and property changes back to the Exchange. An error leaves the Exchange unchanged.

Each evaluation creates a fresh Boa `Context`. State does not leak between evaluations. Boa receives no filesystem, network, environment, stdio, or WASI capability. The only host bindings are the exchange snapshot under `camel` and a tracing-backed `console`. Script source is trusted operator configuration. Exchange data is untrusted under ADR-0032. The implementation converts exchange data to JavaScript values. It never concatenates exchange data into script source or evaluates it as code. Untrusted JavaScript must run out-of-process through the `function:` path from ADR-0005.

Resource limits bound runaway scripts. The default execution timeout is 5,000 ms, enforced by `tokio::time::timeout` around `spawn_blocking`. The timeout returns control to the route but cannot cancel a running blocking task. Boa runtime limits eventually stop it. Boa caps loop iterations at 100,000, recursion depth at 512, and stack slots at 10,240. Source size is bounded at 1 MiB. Boa 0.21 has no heap-size limit. A small script can still allocate a large object graph. Do not run untrusted JavaScript in-process.

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [JS crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-js/CONTEXT.md)
