# Marshal and Unmarshal

Marshal and Unmarshal are the Message Translator pair (Hohpe & Woolf). Marshal serializes the body to a named wire format. Unmarshal deserializes a wire-format body back into a structured type. Together they cross the boundary between the pipeline and external systems.

```rust,ignore
{{#include ../../../examples/marshal-csv/src/main.rs:marshal-route}}
```

```rust,ignore
{{#include ../../../examples/marshal-unmarshal/src/main.rs:unmarshal-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: json-roundtrip
  from: timer:json-roundtrip?period=2000&repeatCount=3
  error_handler:
    retry:
      max_attempts: 0
  steps:
    - set_body:
        value: '{"message": "hello", "count": 42}'
    - unmarshal: json
    - marshal: json
    - to: log:info?showBody=true

- id: csv-marshal
  from: timer:csv-marshal?period=2000&repeatCount=3
  error_handler:
    retry:
      max_attempts: 0
  steps:
    - set_body:
        value: '[{"name":"Carol","age":28},{"name":"Dave","age":35}]'
    - unmarshal: json
    - marshal: csv
    - to: log:info?showBody=true
```

</details>

The `.marshal("csv")` call names the wire format as a string. The processor looks up that name in the data format registry and applies the format to the current body. Marshal stores the serialized result on the exchange. Unmarshal reverses the flow. It parses the body through the named format and stores the parsed structure on the exchange.

The string parameter keeps the route declaration format-agnostic. Built-in formats are `json`, `csv`, `xml`, and `zip`. Protobuf ships as the separate `camel-dataformat-protobuf` crate and must be registered in the data format registry before a route uses it. Each format owns its body-type mapping and its configuration. See [Data Formats](../data-formats/index.md) for the format catalog, the `DataFormat` trait, and per-format options. This page covers only the route-level step.

A route that crosses a system boundary pairs the two steps. Marshal prepares the body for the wire on the outgoing side. Unmarshal restores a structured type on the incoming side. Each step in between then reads the body shape it expects.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), both steps compile into `Service<Exchange>` services in the Tower middleware pipeline. The data format registry and the marshal/unmarshal hooks are documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example sources are at [`examples/marshal-csv`](https://github.com/kennycallado/rust-camel/tree/main/examples/marshal-csv) and [`examples/marshal-unmarshal`](https://github.com/kennycallado/rust-camel/tree/main/examples/marshal-unmarshal).
