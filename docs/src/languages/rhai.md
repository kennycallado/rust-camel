# Rhai

A Rhai implementation of the Language SPI. It provides `Expression`, `Predicate`, and `MutatingExpression` with an unconditional in-process sandbox and Rust-native type safety.

```rust,ignore
{{#include ../../../examples/language-rhai/src/main.rs:setup}}
```

```rust,ignore
{{#include ../../../examples/language-rhai/src/main.rs:route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: language-rhai-demo
  from: timer:tick?period=900&repeatCount=6
  steps:
    - set_header:
        key: priority
        value: high
    - set_header:
        key: amount
        value: 200
    - set_body:
        value: "order #1"
    - script:
        language: rhai
        source: |
          headers["processed"] = true;
          let status = if headers["priority"] == "high" { "PRIORITY" } else { "STANDARD" };
          body = body + " [" + status + "]";
    - to: log:all-orders?showBody=true&showHeaders=true
    - filter:
        rhai: 'header("amount") > 100'
        steps:
          - to: log:high-value-alert?showBody=true
```

</details>

You register `rhai` into `CamelContext` by name, then build expressions and predicates up front. The included example constructs two expressions and one predicate before the route. Read-only expressions and predicates expose `body` and `headers` variables plus `header()`, `set_header()`, `property()`, and `set_property()` host functions. Their writes affect only the current evaluation. A `MutatingExpression` exposes `body`, `headers`, and `properties` as mutable scope variables. The implementation writes all three back to the Exchange only after successful evaluation.

Rhai integrates with Rust types without a foreign-function boundary. Exchange values bind directly as Rhai values, and the engine has no external runtime dependency. The sandbox closes filesystem, module, and network access through independent layers. The workspace enables Rhai's `no_module` feature, each evaluation uses `Engine::new_raw()`, and `disable_symbol` blocks `eval` and `import`. The sandbox has no configuration opt-out. Rhai source is trusted operator configuration. Exchange data is untrusted under ADR-0032 and never evaluated as source code.

Resource limits bound CPU and memory use. Defaults are 100,000 max operations, 1 MiB max string size, 10,000 max array elements, 10,000 max map entries, 64 max expression depth, and a 5,000 ms execution timeout. The timeout wraps synchronous evaluation in `spawn_blocking`. It returns control to the route after five seconds but cannot cancel the blocking task. The operation limit eventually stops a CPU-bound script.

Use Rhai for complex logic in pipeline steps: branching, computation, and multi-step mutation. For flat header and body access, [Simple](simple.md) is lighter and needs no engine. For JavaScript-syntax scripting, use [JavaScript](js.md).

## String methods mutate in place

Rhai string methods such as `replace`, `trim`, and `pad` mutate the subject in place and return unit `()`. They do not return a new string. This differs from JavaScript, Python, and Rust.

Call the method as a bare statement. The statement form mutates the body in place:

```rhai
body.replace(",", "%2C");
```

Never write `body = body.replace(...)`. The right-hand side is unit, so the assignment silently drops the value. The body stays unchanged. The same applies to headers: `headers["k"] = headers["k"].replace(...)` writes unit into the map entry, and the value becomes `Null`. The failure is silent. No error is raised.

The `rhai_replace_*` characterization tests in the Rhai crate pin this behavior.

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [Rhai crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-rhai/CONTEXT.md)
