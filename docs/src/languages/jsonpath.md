# JSONPath

An RFC 9535 JSONPath expression and predicate language over `jsonpath-rust`. It evaluates `$.` queries against an Exchange JSON body.

```rust,ignore
{{#include ../../../examples/language-jsonpath/src/main.rs:setup}}
```

```rust,ignore
{{#include ../../../examples/language-jsonpath/src/main.rs:route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: language-jsonpath-demo
  from: timer:tick?period=800&repeatCount=6
  steps:
    - set_body:
        value: '{"customer": "Alice", "active": true}'
    - set_header:
        key: customer
        jsonpath: "$.customer"
    - to: log:all-orders?showBody=true&showHeaders=true
    - filter:
        jsonpath: "$.active"
        steps:
          - to: log:active-orders?showBody=true&showHeaders=true
```

</details>

`JsonPathLanguage` validates the query prefix and syntax when it creates an Expression or Predicate. You register the language into `CamelContext` by name, then build expressions and predicates up front. The included example registers `jsonpath` and constructs one expression (`$.customer`) and one predicate (`$.active`) before the route. Expressions extract values from the JSON body. Predicates gate filter steps.

The query is trusted operator configuration. The JSON body is untrusted, adversary-controlled data under ADR-0032. Exchange data never enters the query string. The implementation stores the operator query and passes body content separately to `jsonpath-rust`. Resource bounds protect against large or deeply nested input. `max_input_bytes` bounds a text body before JSON parsing. The default is 16 MiB. `max_depth` bounds JSON nesting. The default is 64 levels.

Use JSONPath when the body contains structured JSON and you need to extract nested fields or test conditions. For flat header and body access, [Simple](simple.md) is lighter and needs no JSON parsing.

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [JSONPath crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-jsonpath/CONTEXT.md)
