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

## Usage

```rust
use camel_language_simple::SimpleLanguage;
use camel_language_api::Language;

let lang = SimpleLanguage;

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
