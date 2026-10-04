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

You register `rhai` into `CamelContext` by name, then build expressions and predicates up front. The included example constructs two expressions and one predicate before the route. Read-only expressions and predicates expose `body` and `headers` variables plus the `header()` and `property()` readers. `set_header()` and `set_property()` are rejected at create time with a hint to use a `script:` step; the read-only engine never registers them, so a dynamic call cannot slip through. A `MutatingExpression` exposes `body`, `headers`, and `properties` as mutable scope variables. Its write-back is a validate-all-then-commit transaction: only changed entries and an assigned body are written, and the Exchange is untouched unless the whole evaluation succeeds.

Rhai integrates with Rust types without a foreign-function boundary. Exchange values bind directly as Rhai values, and the engine has no external runtime dependency. The sandbox closes filesystem, module, and network access through independent layers. The workspace enables Rhai's `no_module` feature, each evaluation uses `Engine::new_raw()`, and `disable_symbol` blocks `eval` and `import`. The sandbox has no configuration opt-out. Rhai source is trusted operator configuration. Exchange data is untrusted under ADR-0032 and never evaluated as source code.

Resource limits bound CPU and memory use. Defaults are 100,000 max operations, 1 MiB max string size, 10,000 max array elements, 10,000 max map entries, 64 max expression depth, 32 max function-expression depth, 64 max call levels, and a 5,000 ms execution timeout. The timeout wraps synchronous evaluation in `spawn_blocking`. It returns control to the route after five seconds but cannot cancel the blocking task. The operation limit eventually stops a CPU-bound script.

## Values across the boundary

Values cross the Rhai boundary through two conversions. The inbound conversion turns exchange JSON values into Rhai values before evaluation. The outbound conversion turns the script result and the mutated scope maps back into JSON values after evaluation. Both conversions are fallible and typed. A value the engine cannot represent fails the step with a `conversion` error that names the source type and the target. There is no `to_string()` fallback. No value is ever converted to its display text.

### Exchange body variants

The `body` variable binds per body variant. `Text` and `Xml` bodies bind as strings. A `Json` body binds natively as the corresponding Rhai value: object, array, or scalar. An `Empty` body binds as unit `()` and stays `Empty` on write-back. A `Bytes` body binds as a Blob. A `Stream` body binds as the access-aware reference described under [Stream bodies](#stream-bodies).

### Round-trip matrix

These types round-trip natively. Storing a value through the boundary and reading it back in a later evaluation returns an equal value with its type preserved:

| Rhai type | JSON type | Notes |
|---|---|---|
| `String` | string | |
| `bool` | boolean | |
| `INT` (`i64`) | number | |
| finite `FLOAT` (`f64`) | number | |
| unit `()` | null | |
| Map (`#{}`) | object | Recursed element by element |
| Array | array | Recursed element by element |

### Refused values

These values have no native JSON representation. The step fails with a typed `conversion` error:

- Not-a-number and ±infinity (`float (non-finite)`)
- Function pointers and closures (`FnPtr`)
- Timestamps (`timestamp`)
- Any other custom type (reported by its Rhai type name)

On the inbound side, a JSON integer greater than `i64::MAX` is refused (`u64 > i64::MAX`). The engine does not store a lossy float. The error target names the exchange slot: the property key, the header key, or `body`. Keys inside a map value are runtime data and never appear in the error target.

### One-way conversions

Two Rhai types change type on the way out. The conversion is one-way and lossless:

| Rhai type | JSON result |
|---|---|
| `char` | one-character string |
| Blob | array of integers 0–255 |

### Stream bodies

A `Stream` body is not copied into the script. Both read-only expressions and predicates and mutating expressions bind `body` as an access-aware reference. Any value read through `body` on a Stream body fails the step with a `conversion` error that names `Body::Stream`; the stream itself is never touched, so an untouched stream body stays identical and unconsumed. The registered guard surface is `to_string`, `to_debug`, and the operators `==`, `!=`, `<`, `>`, `+`, `-`, `*`, `/`. What the guard surface determines is suppressibility: a failure on the registered guard surface is an ordinary suppressible script error, while a materializing operation outside the guard set is a boundary refusal that still fails the step even when caught in-script. Two exceptions are documented: `type_of(body)` does not count as a read and returns exactly the string `"StreamBodyRef"`, and a bare discarded `body;` statement is optimized away. In a mutating expression, `body = "replacement"` is allowed without reading the marker: assignment replaces the reference without materializing it, so a read later in the same script sees the replacement value. The residue split after `try`/`catch`: a caught materializing operation outside the guard set still fails the step; a caught pre-clone failure, such as indexing, is an ordinary suppressible script error.

### Two-AST compile discipline

Read-only expressions and predicates compile twice. The first compilation uses `OptimizationLevel::None`. The AST walk rejects `set_header()` and `set_property()` calls in read-only expressions at create time; that AST is discarded after the check. The second compilation uses `OptimizationLevel::Simple`. Its AST is cached and evaluated. Constant folding at this level can optimize away a discarded statement, which the stream-body rules account for.

Use Rhai for complex logic in pipeline steps: branching, computation, and multi-step mutation. For flat header and body access, [Simple](simple.md) is lighter and needs no engine. For JavaScript-syntax scripting, use [JavaScript](js.md).

## String methods mutate in place

Rhai string methods such as `replace`, `trim`, and `pad` mutate the subject in place and return unit `()`. They do not return a new string. This differs from JavaScript, Python, and Rust.

Call the method as a bare statement. The statement form mutates the body in place:

```rhai
body.replace(",", "%2C");
```

Never write `body = body.replace(...)`. The right-hand side is unit, so the assignment stores unit: the body is cleared to `Empty` (unit maps back to the empty body). The same applies to headers: `headers["k"] = headers["k"].replace(...)` writes unit into the map entry, and the value becomes `Null`. The failure is silent. No error is raised.

The `rhai_replace_*` characterization tests in the Rhai crate pin this behavior.

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [Rhai crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-rhai/CONTEXT.md)
