# XPath

An XPath 1.0 expression and predicate language over `sxd-document` and `sxd-xpath`. It evaluates `//node` and `/path` queries against an Exchange XML body.

```rust
{{#include ../../../examples/language-xpath/src/main.rs:setup}}
```

```rust
{{#include ../../../examples/language-xpath/src/main.rs:route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: language-xpath-demo
  from: timer:tick?period=800&repeatCount=6
  steps:
    - set_body:
        value: "<catalog><book in-stock='true'><title>The Rust Book</title></book></catalog>"
    - set_header:
        key: title
        xpath: "/catalog/book[1]/title"
    - to: log:all-books?showBody=true&showHeaders=true
    - filter:
        xpath: "/catalog/book[@in-stock='true']"
        steps:
          - to: log:in-stock?showBody=true&showHeaders=true
```

</details>

You register `xpath` into `CamelContext` by name, then build expressions and predicates up front. The included example constructs one expression (`/catalog/book[1]/title`) and one predicate (`/catalog/book[@in-stock='true']`) before the route. Expressions extract values from the XML body. Predicates gate filter steps.

The XPath query is trusted operator configuration. The XML body is untrusted, adversary-controlled data under ADR-0032. Exchange data never enters the query string. `max_input_bytes` bounds the raw XML body before parsing. The default is 1 MiB. Both `sxd-document` and `sxd-xpath` are pure Rust and register no filesystem or network resolver. The parser has no `<!ENTITY>` declaration handler. Recursive entity expansion, including a billion-laughs payload, is structurally unavailable. External entity declarations cannot trigger a file or network fetch.

Known limitations apply. Namespace prefixes are unsupported because the evaluation context has no prefix-to-URI map. Evaluation has no wall-clock timeout. The query is trusted, and the untrusted XML input has a byte bound. The `sxd-xpath` library is unmaintained. A replacement must preserve the security posture above.

Use XPath when the body carries XML and you need to select elements or test attributes. For structured JSON bodies, use [JSONPath](jsonpath.md).

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [XPath crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-xpath/CONTEXT.md)
