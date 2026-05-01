# camel-language-jsonpath

> JSONPath Language for rust-camel YAML DSL

## Overview

`camel-language-jsonpath` implements [RFC 9535 JSONPath](https://datatracker.ietf.org/doc/html/rfc9535) — a query language for extracting values from JSON data.

## Supported JSONPath Syntax

| Syntax | Description | Example |
|--------|-------------|---------|
| `$` | Root node | `$` |
| `.field` | Child member | `$.store.name` |
| `['field']` | Child member (quoted) | `$['store']['name']` |
| `[index]` | Array index | `$.items[0]` |
| `[start:end]` | Array slice | `$.items[0:5]` |
| `[*]` | Wildcard (all items) | `$.items[*]` |
| `..field` | Descendant (deep scan) | `$..price` |
| `?()` | Filter expression | `$.items[?(@.price > 10)]` |
| `,` | Union (multiple names/indices) | `$.items[0,2,4]` |
| `()` | Script expression | `$.items[(@.length - 1)]` |

> **Note:** JSONPath expressions operate on the message body. The body must be JSON or coercible to JSON. Text bodies containing valid JSON strings are automatically parsed.

## Feature Enablement

Enable in `Cargo.toml`:

```toml
[dependencies.camel-core]
version = "*"
features = ["lang-jsonpath"]
```

## YAML DSL Usage

### Filter

```yaml
routes:
  - id: "filter-high-price"
    from: "direct:orders"
    filter:
      jsonpath: "$.items[?(@.price > 100)]"
    steps:
      - to: "log:high-value"
```

### Set Header

```yaml
routes:
  - id: "set-header-from-json"
    from: "direct:data"
    set-header:
      name: "customer-id"
      expression:
        language: "jsonpath"
        expression: "$.customer.id"
    steps:
      - to: "log:info"
```

### Set Body

```yaml
routes:
  - id: "set-body-from-json"
    from: "direct:data"
    set-body:
      expression:
        language: "jsonpath"
        expression: "$.result"
    steps:
      - to: "log:info"
```

### Log

```yaml
routes:
  - id: "log-json-value"
    from: "direct:data"
    log:
      message: "Price: ${jsonpath:$.price}"
      logging-level: "info"
```

### Split

```yaml
routes:
  - id: "split-array"
    from: "direct:items"
    split:
      expression:
        language: "jsonpath"
        expression: "$.items[*]"
    steps:
      - to: "log:item"
```

### Choice

```yaml
routes:
  - id: "choice-by-type"
    from: "direct:orders"
    choice:
      - when:
          jsonpath: "$.type == 'premium'"
        steps:
          - to: "log:premium"
      - otherwise:
          steps:
            - to: "log:standard"
```

## Usage

```rust
use camel_language_jsonpath::JsonPathLanguage;
use camel_language_api::Language;

let lang = JsonPathLanguage;

// Create a predicate
let pred = lang.create_predicate("$.items[?(@.price > 10)]").unwrap();

// Create an expression
let expr = lang.create_expression("$.store.name").unwrap();
```

## Registration

`JsonPathLanguage` is registered in `CamelContext` under the name `"jsonpath"` when the `lang-jsonpath` feature is enabled:

```rust
let mut ctx = CamelContext::new();
// JsonPathLanguage available as "jsonpath"
let lang = ctx.resolve_language("jsonpath").unwrap();
```

## License

Apache-2.0

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-language-jsonpath = "*"
```
