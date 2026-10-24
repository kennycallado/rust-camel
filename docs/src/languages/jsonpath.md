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

A predicate result must be a JSON boolean. Any other value kind — string, number, object, array, or `null` — is a type error; there is no truthiness coercion. To express match existence ("at least one item matches"), register the language with `predicate_mode: Matches`:

```rust,ignore
use camel_language_jsonpath::{JsonPathConfig, JsonPathLanguage, JsonPathPredicateMode};

let lang = JsonPathLanguage::with_config(JsonPathConfig {
    predicate_mode: JsonPathPredicateMode::Matches,
    ..JsonPathConfig::default()
});
ctx.register_language("jsonpath", Box::new(lang))?;
```

In `Matches` mode a JSONPath predicate coerces its raw match sequence: zero matches (including a missing path) are `false`, a sole boolean keeps its value, and any other non-empty match sequence is `true`. `$.items[?@.price>10]` then means "at least one item costs more than 10", the same intent XPath expresses with `boolean(/order/items/item[price>10])`. The coercion is cardinality-only: an empty string and `0` are `true`, unlike XPath `boolean()` value rules, and a literal `false` boolean stays `false`. Strict remains the default, and evaluation failures stay typed errors in both modes.

The query is trusted operator configuration. The JSON body is untrusted, adversary-controlled data under ADR-0032. Exchange data never enters the query string. The implementation stores the operator query and passes body content separately to `jsonpath-rust`. Resource bounds protect against large or deeply nested input. `max_input_bytes` bounds a text body before JSON parsing. The default is 16 MiB. `max_depth` bounds JSON nesting. The default is 64 levels. Evaluation has no wall-clock timeout. The query is trusted, and the untrusted JSON input has byte and depth bounds.

Use JSONPath when the body contains structured JSON and you need to extract nested fields or test conditions. For flat header and body access, [Simple](simple.md) is lighter and needs no JSON parsing.

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [JSONPath crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-jsonpath/CONTEXT.md)
