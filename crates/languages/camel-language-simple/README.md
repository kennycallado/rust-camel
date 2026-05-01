# camel-language-simple

> Simple Language interpreter for rust-camel YAML DSL

## Overview

`camel-language-simple` implements [Apache Camel's Simple Language](https://camel.apache.org/components/latest/languages/simple-language.html) — a lightweight expression language for accessing exchange data and building predicates in route definitions.

## Supported Expressions

| Syntax | Description |
|--------|-------------|
| `${header.name}` | Access a message header by name |
| `${body}` | Access the message body as text |
| `${body.field}` | Access JSON body field (dot-notation) |
| `${body.nested.field}` | Access nested JSON fields |
| `${body.array.0}` | Access array element by index |
| `${exchangeProperty.name}` | Access an exchange property |
| `true` / `false` | Boolean literals |
| `null` | Null literal |

> **Note:** Body field access (`${body.field}`) only works on `Body::Json` and returns `Value::Null` for other body types or missing fields.

## Supported Operators

| Operator | Example | Description |
|----------|---------|-------------|
| `==` | `${header.type} == 'order'` | Equality |
| `!=` | `${header.type} != null` | Inequality |
| `>` | `${header.age} > 18` | Greater than (numeric) |
| `<` | `${header.age} < 100` | Less than (numeric) |
| `>=` | `${header.count} >= 5` | Greater than or equal |
| `<=` | `${header.count} <= 10` | Less than or equal |
| `contains` | `${body} contains 'hello'` | String contains |
| `&&` | `${header.type} == 'order' && ${header.priority} == 'high'` | Logical AND (short-circuit) |
| `\|\|` | `${header.type} == 'order' \|\| ${header.type} == 'invoice'` | Logical OR (short-circuit) |

## Compound Predicates

Combine multiple conditions with `&&` (AND) and `||` (OR):

| Expression | Description |
|------------|-------------|
| `${body} != null && ${body} != ''` | Body is present and non-empty |
| `${header.type} == 'order' \|\| ${header.type} == 'invoice'` | Type is order or invoice |
| `${body.status} == 'active' && ${body.score} > 50` | Status is active AND score > 50 |

Precedence: `&&` binds tighter than `||`. `${a} \|\| ${b} && ${c}` is parsed as `${a} \|\| (${b} && ${c})`.

## Null Semantics

| Expression | Result | Notes |
|------------|--------|-------|
| `${body}` with empty exchange | `Value::Null` | Empty body is null, not empty string |
| `${header.missing}` | `Value::Null` | Missing headers return null |
| `${body} != null` | false when body is empty, true when body has content | Use to check if body exists |
| `null > 5` | false | Null is not comparable for ordering |
| `null == null` | true | Null equals null |
| `null contains 'x'` | false | Null is not searchable |

## Boolean Comparisons

Compare JSON boolean fields with `true`/`false` literals:

```yaml
- filter:
    language: simple
    source: "${body.active} == true"
```

## Language Delegation

Delegate expression evaluation to another registered language using `${lang:expr}` syntax:

| Syntax | Description |
|--------|-------------|
| `${jsonpath:$.store.book[0].title}` | Evaluate a JSONPath expression |
| `${xpath://user/name}` | Evaluate an XPath expression |
| `${constant:hello}` | Evaluate a constant expression |

The language name must be registered in the `CamelContext` language registry. When used outside a context (standalone), delegation returns an error.

## Usage

```rust
use camel_language_simple::SimpleLanguage;
use camel_language_api::Language;

let lang = SimpleLanguage::new();

// Create a predicate
let pred = lang.create_predicate("${header.type} == 'order'").unwrap();

// Create an expression
let expr = lang.create_expression("${header.name}").unwrap();
```

## Registration

`SimpleLanguage` is registered by default in `CamelContext` under the name `"simple"`:

```rust
let ctx = CamelContext::new();
let lang = ctx.resolve_language("simple").unwrap();
```

## License

Apache-2.0

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-language-simple = "*"
```
