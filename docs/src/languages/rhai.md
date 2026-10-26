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

You register `rhai` into `CamelContext` by name, then build expressions and predicates up front. The included example constructs two expressions and one predicate before the route.

Rhai integrates with Rust types without a foreign-function boundary. Exchange values bind directly as Rhai values, and the engine has no external runtime dependency. The sandbox closes filesystem, module, and network access through independent layers. The workspace enables Rhai's `no_module` feature, each evaluation uses `Engine::new_raw()`, and `disable_symbol` blocks `eval` and `import`. The sandbox has no configuration opt-out. Rhai source is trusted operator configuration. Exchange data is untrusted under ADR-0032 and never evaluated as source code.

Resource limits bound CPU and memory use. Defaults are 100,000 max operations, 1 MiB max string size, 10,000 max array elements, 10,000 max map entries, 64 max expression depth, 32 max function-expression depth, 64 max call levels, and a 5,000 ms execution timeout. The timeout wraps synchronous evaluation in `spawn_blocking`. It returns control to the route after five seconds but cannot cancel the blocking task. The operation limit eventually stops a CPU-bound script.

## Read surface

Read-only expressions and predicates expose `body`, `headers`, and `properties` variables plus the `header()` and `property()` readers. A `MutatingExpression` (a `script:` step) exposes `body`, `headers`, and `properties` as mutable scope variables and also provides `header()` and `property()`. Its write-back is a validate-all-then-commit transaction: only changed entries and an assigned body are written, and the Exchange is untouched unless the whole evaluation succeeds.

Both modes expose the same five names. The `script:` readers differ in freshness:

- Read-only mode rejects mutation at create time (see [Read-only mutation rejection](#read-only-mutation-rejection)).
- Script-step `header(name)` and `property(name)` read the **pre-run snapshot** of the maps. After `headers["k"] = "new"`, a later `header("k")` still returns the pre-run value, while the mutated map propagates back to the Exchange. JavaScript is different: `camel.headers.get("k")` observes the live mutated map.
- `has_header(name)` and `has_property(name)` are not registered (rc-w1mds). Distinguish an absent key from a stored `null` through the map surface, for example `"k" in headers`.

### Read-only mutation rejection

A read-only expression or predicate rejects mutation at create time, before the route builds:

- `set_header()` and `set_property()` calls.
- Any assignment whose root variable is `body`, `headers`, or `properties`, including index and dot targets and compound operators: `headers["x"] = "1"`, `body = "changed"`, `properties["a"]["b"] = "c"`, `headers["x"] += 1`, `headers.foo = "1"`.

The rejection is conservative. No scope analysis runs, so a local that shadows one of those names is rejected too: `let headers = #{}; headers["x"] = 1` fails at create time. A read-only local cannot affect the Exchange, so this is a documented false positive. Reading is unrestricted: `headers["x"]`, `body.len`, and `let x = headers; x` all compile. The read-only engine never registers `set_header()`/`set_property()`, so a dynamic call cannot slip through; the rhai hint names the working map-assignment surface for a `script:` step. Method-call mutations on those roots (`headers.clear()`, `headers.remove(x)`) are a known remaining gap (rc-qpjps), not caught by the create-time walk.

## Values across the boundary

Values cross the Rhai boundary through two conversions. The inbound conversion turns exchange JSON values into Rhai values before evaluation. The outbound conversion turns the script result and the mutated scope maps back into JSON values after evaluation. Both conversions are fallible and typed. A value the engine cannot represent fails the step with a `conversion` error that names the source type and the target. There is no `to_string()` fallback. No value is ever converted to its display text.

### Exchange body variants

The `body` variable binds per body variant:

| Body variant | Rhai binding |
|---|---|
| `Text` / `Xml` | string |
| `Json` | native object, array, or scalar (`map`, `array`, `i64`, `f64`, `bool`, string, or unit), subject to the conversion refusals below |
| `Empty` | unit `()` |
| `Bytes` | blob |
| `Stream` | access-aware reference described under [Stream bodies](#stream-bodies) |

An `Empty` body stays `Empty` on write-back.

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

Read-only expressions and predicates compile twice. The first compilation uses `OptimizationLevel::None`. The AST walk rejects `set_header()` and `set_property()` calls, and assignments whose root variable is `body`, `headers`, or `properties`, in read-only expressions at create time; that AST is discarded after the check. The second compilation uses `OptimizationLevel::Simple`. Its AST is cached and evaluated. Constant folding at this level can optimize away a discarded statement, which the stream-body rules account for.

Use Rhai for complex logic in pipeline steps: branching, computation, and multi-step mutation. For flat header and body access, [Simple](simple.md) is lighter and needs no engine. For JavaScript-syntax scripting, use [JavaScript](js.md).

## JSON helpers

`parse_json` and `to_json` are host functions. A shared host module registers
them on every sandbox engine after the standard package, so they shadow the
stock Rhai JSON built-ins on the read-only, mutating, and expression engines. The
two wrapper types use the stable labels `json value` and `json number`.

### Parsing

`parse_json` accepts the full RFC 8259 grammar for any JSON value (object,
array, or scalar). It accepts the `\/` escape and refuses an unpaired surrogate
escape. A malformed input fails with a redacted `parse` error. That error holds
only the class and the script call position. It holds no input text and no parser
line or column.

`parse_json` stores every number as its exact source token. It does not convert a
number to `f64`. An integer-form token (no fraction and no exponent) that fits in
`i64` projects to a native `INT`, and `-0` normalizes to `0`. A decimal, an
exponent, and an integer outside the `i64` range each become a `json number`
wrapper. That wrapper keeps the exact token, so a large magnitude round-trips
without `f64` rounding.

`JsonNumber.to_float()` is the only number conversion. It returns a finite
`f64`, or an `arithmetic` error when the result is not finite (for example
`1e400`).

Object keys keep their authored insertion order. A duplicate key takes the last
value and keeps the position of its first occurrence. Replacing the value at an
existing key retains that key's position. `remove` shifts, so the remaining keys
keep their relative order.

### Parsing JSON bodies

`parse_json` accepts only a string argument: JSON text. A non-string argument
still resolves, then fails with a typed `type-mismatch` error that names the
actual type. The script `let data = parse_json(body);` fails on a `Bytes` body
with `type mismatch: expected string, actual blob at 1:12`. An object-valued
`Json` body (for example `{"a":1}`) binds as a map and fails the same script
with `actual map` at the same position. The error never reports
function-not-found; on 0.60.0 the same script failed as
`function-not-found ... at 1:12`.

The sandbox has no strict UTF-8 decode for blobs. `blob.to_string()` renders
hex, and `Blob.as_string()` is lossy (`from_utf8_lossy`): it silently replaces
malformed UTF-8, so it must not be used for JSON input. Do not decode bytes
inside the sandbox.

To parse byte input, materialize it at the route level: convert the body to
text before the script step, for example with a `set_body` conversion or a
marshal step. A native `Json` body needs no parsing; bind it directly.

### Serializing

`to_json` accepts a wrapper, a native `Map`, a native `Array`, a string, a
`bool`, an `i64`, a finite `f64`, and unit. The method form `value.to_json()`
matches the function form `to_json(value)`. `to_string` and `to_debug` on a
wrapper return the same compact JSON, and for a `json number` they return the
number token.

Output is compact JSON with raw UTF-8 (non-ASCII bytes are emitted directly). It
does not escape `/`. It escapes control characters. It emits keys in stored
order. Native `Map` keys are sorted, and a wrapper nested inside a native `Map`
or `Array` is inlined.

An unsupported value (function pointer, closure, timestamp, custom type) and a
non-finite float each fail with a `type-mismatch` error. That error does not use a
`Debug`/`Display` fallback and does not include the value.

### Wrapper surface

A wrapper supports string-key and integer-index get and set, including chained
assignment. Dot-property access falls back to string-key indexing. A negative
array index counts from the end, and an out-of-range index raises
`ErrorArrayBounds`. The helper surface is `len`, `contains`, `keys`, `remove`,
and `push`. `contains` accepts a string key on an object; a call on an array
fails with `type-mismatch`. `push` only appends and rechecks the array cap.
`remove` returns the removed value, or unit when the key or index is absent.
Assigning unit stores JSON `null`. A missing key and a JSON `null` both read as
unit. `contains` and the `in` operator distinguish a present key from an absent
one.

The wrapper has no iteration. A `for` loop over a wrapper fails with a `runtime`
error. Arithmetic (`+`, `+=`) and the container helpers `values` and `merge`
fail with `function-not-found`.

Every registered comparison between a wrapper and a native `i64`, `f64`,
`String`, or `bool`, in both operand orders, and between two wrappers, fails with
`type-mismatch`. There is no exact numeric comparison engine. Call `to_float()`
for an explicit lossy comparison. JSON `null` projects to unit, so
`parse_json("null") == ()` is true. Comparisons outside the registered set keep
Rhai builtin behavior.

### Bounds

Parsing enforces `max-string-size` on the input before descent. It enforces
`max-array-size` and `max-map-size` on each container. A cap of `0` keeps the
Rhai unlimited meaning for that cap.

The depth cap is 128. A top-level scalar has depth 0, and a top-level container
has depth 1. Depth 128 is accepted, and depth 129 fails with a `limit` error.
The cap is invariant across parsing, mutation, and serialization. Every set,
push, conversion, and `to_json` of a native container checks the depth of the
target path plus the depth of the inserted value. It fails with `limit` before
it applies when the total would exceed 128. A self-assignment such as
`j["a"] = j` that would exceed the cap is refused. A mutation that would exceed
`max-string-size` fails with `limit` before it applies. A bound violation
commits no Exchange change.

### Compatibility break

Decimal and exponent JSON numbers are now `json number`, not `f64`. A script that
did arithmetic on them, or that expected `f64`, breaks. Use `to_float()` for an
explicit lossy comparison.

Direct outbound conversion of a wrapper is deferred. Assigning a wrapper to a
body fails as an explicit compatibility break: `body = parse_json(...)` (or
`set_body(parse_json(...))`) does not work. Persist parsed JSON with `to_json` or
a native leaf. `body = to_json(j)` yields a text body that holds valid JSON.

`to_json(parse_json(s))` is semantically equivalent, not byte-for-byte identical.
It preserves the value, the numeric magnitude, and the key order. It does not
preserve whitespace, indentation, or escape spelling. Wrapped number tokens stay
exact, and a native `i64` spelling may normalize, including `-0` to `0`.

The inbound exchange conversion is unchanged. A JSON integer greater than
`i64::MAX` on that path is still refused.

## Emitting a literal `${...}`

To emit a literal `${...}` that must not be parsed as a [Simple](simple.md) template slot, use an explicit `language:` and `source:` value with a quoted rhai or JavaScript string. Escape environment and job-argument tokens as `$$`:

```yaml
- set_body:
    language: rhai
    source: '"$${env:X}"'    # literal ${env:X}
- set_body:
    language: rhai
    source: '"${header.h}"'  # literal ${header.h}; no escape needed
```

`$$` is not a Simple-template escape. At the Simple source level, `$${header.h}` is a literal `$` followed by a live slot. In a route file, the environment pass runs first and turns each `$$` into `$`, so `$${header.h}` reaches Simple as the whole slot `${header.h}` and keeps its native value. Write `$$$${header.h}` in a route file for a literal `$` plus a slot. See [Environment variable interpolation](../configuration/env-interpolation.md).

## String methods: mutation and pure alternatives

`trim()` is **pure**: it returns the trimmed string and leaves the subject
unchanged. Write `let trimmed = text.trim();`.

```rhai
let text = "  hello  ";
let trimmed = text.trim();   // "hello"
// text is still "  hello  "
```

This is a **breaking change** for scripts written against stock Rhai, where
`trim()` was an in-place method returning unit `()` (so `let x = "   ";
x.trim() != ""` was always true). rust-camel registers a pure `trim()` after
the standard package, which shadows the mutating one in both read-only
expressions and script steps.

The remaining string methods still mutate the subject in place and return unit
`()`. They do not return a new string. This differs from JavaScript, Python,
and Rust.

| In-place method | Effect | Pure alternative |
| --- | --- | --- |
| `make_upper()` | Upper-cases the subject | `to_upper()` returns a new string |
| `make_lower()` | Lower-cases the subject | `to_lower()` returns a new string |
| `crop(...)` | Cuts the subject to a range | `sub_string(...)` returns a new string |
| `truncate(len)` | Cuts the subject to `len` chars | `sub_string(0, len)` |
| `replace(from, to)` | Replaces occurrences | none — copy then mutate |
| `pad(len, ch)` | Pads the subject | none — copy then mutate |
| `remove(sub)` | Removes occurrences of a substring | none — copy then mutate |
| `clear()` | Empties the subject | none — assign `""` |

For the methods with no pure alternative, copy the subject first and mutate the
copy:

```rhai
let source = headers["url"];
let y = source;
y.replace(",", "%2C");
headers["url"] = y;
```

Call an in-place method as a bare statement so the mutation lands on the
subject:

```rhai
body.replace(",", "%2C");
```

Never write `body = body.replace(...)`. The right-hand side is unit, so the
assignment stores unit: the body is cleared to `Empty` (unit maps back to the
empty body). The same applies to headers: `headers["k"] = headers["k"].replace(...)` writes unit into the map entry, and the value becomes `Null`. The failure is silent. No error is raised.

The `trim_start`/`trim_end` methods and the `*_ascii` and `trim_to_string`
variants are **not present** in the Rhai 1.26.1 `StandardPackage`, so there is
nothing to shadow for those names.

The `rhai_replace_*` characterization tests in the Rhai crate pin the in-place
behavior.

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [Rhai crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-rhai/CONTEXT.md)
