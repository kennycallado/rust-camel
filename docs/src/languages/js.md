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

The route above uses a `.script("js", ...)` step, which runs as a `MutatingExpression`. The script reads and writes exchange data through the `camel` global. `camel.headers` and `camel.properties` are map-like views with `get`, `set`, `has`, `remove`, and `keys`. `camel.body` holds the body value. `camel.header(name)` and `camel.property(name)` read a header or exchange property. `camel.has_header(name)` and `camel.has_property(name)` test key containment: a key whose stored value is `null` is present. The backing views are live, so `camel.headers.get("k")` and `camel.has_header("k")` observe writes made earlier in the same script. `camel.set_property(name, value)` writes an exchange property. After successful evaluation, the implementation writes body, header, and property changes back to the Exchange. An error leaves the Exchange unchanged.

Evaluations reuse one persistent worker Boa `Context`. Each evaluation reinstalls and verifies the `camel` and `console` bindings. On integrity drift the worker recycles the realm. There is no realm isolation. Global properties and intrinsic state may survive across exchanges and routes until realm recycling. Boa receives no filesystem, network, environment, stdio, or WASI capability. The only host bindings are the exchange snapshot under `camel` and a tracing-backed `console`. Script source is trusted operator configuration. Exchange data is untrusted under ADR-0032. The implementation converts exchange data to JavaScript values. It never concatenates exchange data into script source or evaluates it as code. Untrusted JavaScript must run out-of-process through the `function:` path from ADR-0005.

## Body values

`camel.body` exposes the native body value:

- `Body::Json` as the structured object, array, or scalar.
- `Body::Text` and `Body::Xml` as a string.
- `Body::Bytes` as an array of integers 0-255, with no UTF-8 decoding.
- `Body::Empty` as `null`.

## Write-back

A mutating step writes back only the header and property entries the script changed. Untouched entries keep their original JSON form, so a JSON `10.0` that JavaScript represents as the integral number `10` is not rewritten. Keys the script removed are removed from the Exchange. The body is written back only when it changed. Every pending change, body included, is validated before any of it is committed, so a conversion failure leaves the Exchange unchanged. Mutating the exposed byte array, for example `camel.body[1] = 5`, writes the body back as a `Body::Json` array; the `Bytes` to array exposure is one-way.

## Stream bodies

A `Body::Stream` is exposed as a lazy accessor, not a copied value. A script that never reads `camel.body`, such as a header-only filter or step, succeeds and the original `Body::Stream` stays unconsumed. Reading `camel.body` refuses with a payload-free runtime error whose message points at `stream_cache` or `convert_body_to`; an unhandled refusal fails the step. Assigning `camel.body = value` replaces the body without reading it. Replacing the accessor with a different getter is refused. Deleting `camel.body`, when the stream was never read, leaves `Body::Empty`.

Resource limits bound runaway scripts. `[languages.js.limits]` configures four limits: execution timeout, max loop iterations, max recursion depth, and max stack size. `None` selects the rust-camel runtime default. The default execution timeout is 5,000 ms, enforced by `tokio::time::timeout` around `spawn_blocking`. The timeout returns control to the route but cannot cancel a running blocking task. Boa runtime limits eventually stop it. The rust-camel default caps loop iterations at 100,000, recursion depth at 512, and stack slots at 10,240. Source size is bounded at 1 MiB. Do not run untrusted JavaScript in-process.

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [JS crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-js/CONTEXT.md)
