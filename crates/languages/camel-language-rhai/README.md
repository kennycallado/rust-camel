# camel-language-rhai

> Rhai scripting language for rust-camel

## Overview

`camel-language-rhai` provides a [Rhai](https://rhai.rs/) scripting language integration for rust-camel, similar to Apache Camel's Groovy or JavaScript language support. Rhai is an embedded scripting language for Rust with no external dependencies.

## Available Variables

Scripts have access to these variables injected from the Exchange:

| Variable | Type | Description |
|----------|------|-------------|
| `body` | String | Message body as text |
| `headers` | Map | Message headers as a key-value map |

## Available Functions

| Function | Description |
|----------|-------------|
| `header("name")` | Look up a header value by name |
| `set_header("key", value)` | Set a header; visible within the same script evaluation but **not** propagated back to the Exchange |
| `property("name")` | Look up an exchange property by name |
| `set_property("key", value)` | Set a property; visible within the same script evaluation but **not** propagated back to the Exchange |

> **Note:** `set_header` and `set_property` allow scripts to pass values between
> statements within a single evaluation (e.g. set a header then read it back later
> in the same script). Changes do **not** persist after the script completes because
> expressions receive a read-only `&Exchange` reference.

## Usage

```rust
use camel_language_rhai::RhaiLanguage;
use camel_language_api::Language;

let lang = RhaiLanguage::new();

// Predicate using header lookup
let pred = lang.create_predicate(r#"header("type") == "order""#).unwrap();

// Expression with string concatenation
let expr = lang.create_expression(r#"body + " processed""#).unwrap();

// Full Rhai scripting
let expr = lang.create_expression(r#"
    if header("priority") == "high" {
        "URGENT: " + body
    } else {
        body
    }
"#).unwrap();
```

## Registration

Register manually in `CamelContext` (not included by default to avoid the Rhai dependency):

```rust
use camel_language_rhai::RhaiLanguage;

let mut ctx = CamelContext::new();
ctx.register_language("rhai", Box::new(RhaiLanguage::new())).unwrap();
```

## License

Apache-2.0
