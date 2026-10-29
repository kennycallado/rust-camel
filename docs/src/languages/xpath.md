# XPath

An XPath 1.0 expression and predicate language over `sxd-document` and `sxd-xpath`. It evaluates `//node` and `/path` queries against an Exchange XML body.

```rust,ignore
{{#include ../../../examples/language-xpath/src/main.rs:setup}}
```

```rust,ignore
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
        xpath: "boolean(/catalog/book[@in-stock='true'])"
        steps:
          - to: log:in-stock?showBody=true&showHeaders=true
```

</details>

You register `xpath` into `CamelContext` by name, then build expressions and predicates up front. The included example constructs one expression (`/catalog/book[1]/title`) and one predicate (`boolean(/catalog/book[@in-stock='true'])`) before the route. Expressions extract values from the XML body. Predicates gate filter steps and must yield a boolean under the strict-bool predicate contract (`language-value-boundary`, sealed Q3), so a match test is wrapped in `boolean()`.

The XPath query is trusted operator configuration. The XML body is untrusted, adversary-controlled data under ADR-0032. Exchange data never enters the query string. `max_input_bytes` bounds the raw XML body before parsing. The default is 1 MiB. Both `sxd-document` and `sxd-xpath` are pure Rust and register no filesystem or network resolver. The parser has no `<!ENTITY>` declaration handler. Recursive entity expansion, including a billion-laughs payload, is structurally unavailable. External entity declarations cannot trigger a file or network fetch.

Known limitations apply. Namespace prefixes are unsupported because the evaluation context has no prefix-to-URI map. Evaluation has no wall-clock timeout. The query is trusted, and the untrusted XML input has a byte bound. The `sxd-xpath` library is unmaintained. A replacement must preserve the security posture above.

## Number and boolean boundary

The language enforces three value rules at the XPath-to-JSON boundary.

Non-finite numeric results (`+Inf`, `-Inf`, and `NaN`) are refused. Evaluation fails with a typed error of class `arithmetic`, and the step fails. A non-finite result never becomes a Null value.

Finite numbers split by integrality and interval. A finite integral result in `[-2^63, 2^63)` projects to JSON integer (`i64`) form, and one in `[2^63, 2^64)` to unsigned (`u64`) form. Fractional results and integral results outside those intervals stay in float form. The projection preserves the evaluated value exactly; it does not recover precision lost during XPath evaluation, where numbers are already IEEE-754 doubles.

Predicates accept XPath boolean results only. There is no truthiness coercion: a nodeset, string, or number does not pass or fail a filter by non-emptiness or non-zero value. Write the test explicitly, for example `boolean(/catalog/book[@in-stock='true'])`.

Use XPath when the body carries XML and you need to select elements or test attributes. For structured JSON bodies, use [JSONPath](jsonpath.md).

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [XPath crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-xpath/CONTEXT.md)
