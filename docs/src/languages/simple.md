# Simple

A lightweight expression and predicate language for header, body, property, and exception access. It uses `${...}` interpolation syntax and supports compound predicates with `&&` and `||`.

```rust
{{#include ../../../examples/language-simple/src/main.rs:setup}}
```

```rust
{{#include ../../../examples/language-simple/src/main.rs:route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: language-simple-demo
  from: timer:tick?period=800&repeatCount=8
  steps:
    - set_header:
        key: type
        value: order
    - set_header:
        key: priority
        value: high
    - set_header:
        key: approved
        value: "true"
    - set_body:
        simple: "${header.type}"
    - filter:
        simple: "${body} != null"
        steps:
          - to: log:body-present?showBody=true&showHeaders=true
    - filter:
        simple: "${header.type} == 'order'"
        steps:
          - to: log:orders?showBody=true&showHeaders=true
    - filter:
        simple: "${header.type} == 'order' && ${header.priority} == 'high'"
        steps:
          - to: log:high-priority-orders?showBody=true&showHeaders=true
    - filter:
        simple: "${header.approved} == true"
        steps:
          - to: log:approved-orders?showBody=true&showHeaders=true
```

</details>

Simple parses its source once into an AST and reuses it for each Exchange. You build predicates and expressions up front with `create_predicate` and `create_expression`, then move them into route closures. Predicates return a boolean for filter and choice steps. Expressions return a `Value` for enrichment and transformation. The included example constructs four predicates and one expression before the route.

The language evaluates `${header.x}`, `${body}`, and `${exchangeProperty.y}` against the Exchange. Missing headers, properties, and exception messages evaluate to `Value::Null`. At the predicate boundary, null is false. All other values, including an empty string, are true. Within `&&` and `||`, null and empty strings are false. Simple also supports language delegation through `${lang:expr}`, which resolves the target language at evaluation time.

Simple is the most common language for predicates in EIP patterns. Use it for flat header and body access in filter, choice, and enrichment steps. It requires no external engine. For structured JSON queries, use [JSONPath](jsonpath.md).

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [Simple crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-simple/CONTEXT.md)
