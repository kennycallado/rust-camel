# Convert Body

The Convert Body step is a Message Translator (Hohpe & Woolf). It converts the exchange body to a target `BodyType` variant so downstream steps read the same data under a different type.

```rust,ignore
{{#include ../../../examples/convert-body-to/src/main.rs:convert-body-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: text-to-json
  from: timer:text-to-json?period=2000&repeatCount=3
  error_handler:
    retry:
      max_attempts: 0
  steps:
    - set_body:
        value: '{"message": "hello from text", "count": 42}'
    - convert_body_to: json
    - to: log:info?showBody=true
```

</details>

The `.convert_body_to(BodyType::Json)` call picks a target variant from the `BodyType` enum: `Text`, `Json`, `Bytes`, `Xml`, or `Empty`. The step reads the current body, re-encodes it to that variant, and stores the result on the exchange. Steps after the conversion then read the body under its new type.

Use Convert Body when the data is correct but the type is wrong. A source that emits bytes and a sink that expects JSON sit at a type boundary. The step changes the type and keeps the content. [Transform](transform.md) does a different job. It replaces the body from an expression or a literal value. Convert Body re-types the existing data. Transform overwrites it.

Convert Body works only within the built-in `BodyType` variants. [Marshal and Unmarshal](marshal-unmarshal.md) handle named wire formats such as CSV or Protobuf. Those steps translate between a structured type and a serialized representation, not between body types.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the step compiles into a `Service<Exchange>` in the Tower middleware pipeline. The processor contract and `BodyType` definition are documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/convert-body-to`](https://github.com/kennycallado/rust-camel/tree/main/examples/convert-body-to).
