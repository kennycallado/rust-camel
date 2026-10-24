# Simple

A lightweight expression and predicate language for header, body, property, and exception access. It uses `${...}` interpolation syntax and supports compound predicates with `&&` and `||`.

```rust,ignore
{{#include ../../../examples/language-simple/src/main.rs:setup}}
```

```rust,ignore
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
        simple: "${header.approved} == 'true'"
        steps:
          - to: log:approved-orders?showBody=true&showHeaders=true
```

</details>

Simple parses its source once into an AST and reuses it for each Exchange. You build predicates and expressions up front with `create_predicate` and `create_expression`, then move them into route closures. Predicates return a boolean for filter and choice steps. Expressions return a `Value` for enrichment and transformation. The included example constructs four predicates and one expression before the route.

The language evaluates `${header.x}`, `${body}`, and `${exchangeProperty.y}` against the Exchange. Missing headers, properties, and exception messages evaluate to `Value::Null`. Simple also supports language delegation through `${lang:expr}`, which resolves the target language at evaluation time.

## Expression and predicate modes

Simple has two parsing modes. `create_expression` uses **template mode**. In template mode the source is literal text plus `${...}` slots. Text outside a slot stays verbatim, so `=`, `<`, `null`, `true`, quotes, and whitespace are never re-parsed as predicate grammar. `create_predicate` uses **predicate mode**, the comparison grammar (`==`, `!=`, `>`, `contains`, `&&`, `||`). Predicate mode is unchanged by template mode.

A predicate must evaluate to a real boolean. Any other result is a `type mismatch: expected bool` error. This includes `null`, an empty string, and the string `"true"`. Simple does not apply truthiness at the predicate boundary. Inside `&&` and `||` it does apply a truthiness rule to the operands: null and empty strings are false. That rule never changes the final predicate result.

`==` compares by value and coerces between a string and a number. It parses the string as a number, so `${header.count} == 10` matches when the header is the string `"10"`. It does not coerce a string to a boolean. `${header.approved} == true` is false when the header is the string `"true"`. Compare against `'true'` or store a boolean header instead.

In template mode a source that is exactly one `${...}` slot returns the slot's existing value. Numbers, arrays, or `null` from a header or body-field slot stay native. A whole `${body}` slot renders a JSON body as serialized text. Any other source returns a string. A source with no slot is raw text: `'abc'`, `true`, `42`, and `null` evaluate to the strings `'abc'`, `true`, `42`, and `null`. A missing slot in a mixed template renders as an empty string. A whole missing slot returns `null`.

A `${` that is never closed is a compile-time parse error, never literal text: `Got ${name` fails at creation with a message naming `unclosed interpolation` and the byte offset of the `${` in the trimmed source. An empty `${}` slot and a `${` nested inside slot content are parse errors too. A lone `$`, `{`, or `}` outside a `${` stays verbatim literal text.

Simple template mode is Simple's own literal-plus-`${}` grammar. It is not the [MiniJinja](minijinja.md) template engine (ADR-0047) and not route-template `{{...}}` substitution (ADR-0008).

A bare top-level string value in a route file that contains `${...}` compiles to a Simple template. Only a top-level string is gated. A string nested inside an object or array literal stays literal. See [Basic steps](../yaml-dsl/step-verbs/basic.md) and [Environment variable interpolation](../configuration/env-interpolation.md).

`$$` is not a Simple-template escape. At the Simple source level, `$${header.h}` is a literal `$` followed by the live `${header.h}` slot. The route file's environment pass turns each `$$` into `$` before Simple sees the source. Therefore `$${header.h}` in a route file reaches Simple as the whole slot `${header.h}` and keeps its native value. To render `$` plus a slot, write `$$$${header.h}`. To emit a literal `${...}` that must not be evaluated, use an explicit `language:` and `source:` value with a quoted rhai or js string, and escape environment or argument tokens as `$$` (see [Environment variable interpolation](../configuration/env-interpolation.md)).

Simple is the most common language for predicates in EIP patterns. Use it for flat header and body access in filter, choice, and enrichment steps. It requires no external engine. For structured JSON queries, use [JSONPath](jsonpath.md).

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [Simple crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-simple/CONTEXT.md)
