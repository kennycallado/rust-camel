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
| `properties` | Map | Exchange properties as a key-value map |

## Read-only API (`create_expression` / `create_predicate`)

| Function | Description |
|----------|-------------|
| `header("name")` | Look up a header value by name |
| `set_header("key", value)` | Set a header visible within the same script evaluation (not propagated back) |
| `property("name")` | Look up an exchange property by name |
| `set_property("key", value)` | Set a property visible within the same script evaluation (not propagated back) |

> **Note:** `set_header` and `set_property` in read-only mode are local to the script evaluation. Changes are **not** propagated back to the Exchange.

## Mutating API (`create_mutating_expression`)

Use `create_mutating_expression` when you need script changes to persist back to the Exchange. The mutating engine uses **direct map assignment** instead of helper functions:

```rhai
headers["tenant"] = "acme";        // set header — propagated back
properties["trace"] = "enabled";   // set property — propagated back
body = "new content";              // set body — propagated back
let v = headers["existing"];       // read header
```

Changes are applied atomically: if the script throws an error, all modifications are rolled back and the Exchange is restored to its pre-execution state.

The `.script()` builder method uses this API automatically:

```rust
RouteBuilder::from("direct:input")
    .script("rhai", r#"headers["tenant"] = "acme"; body = body + "_processed""#)
    .to("log:result")
    .route_id("script-example")
    .build()?;
```

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
