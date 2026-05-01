# camel-language-api

> Language trait API for rust-camel (Expression, Predicate)

## Overview

`camel-language-api` defines the core traits for the rust-camel Languages subsystem, inspired by [Apache Camel's Language SPI](https://camel.apache.org/manual/language.html).

## Traits

| Trait | Description |
|-------|-------------|
| `Language` | Factory that produces `Expression` and `Predicate` instances from scripts |
| `Expression` | Evaluates a script against an `Exchange`, returning a `serde_json::Value` |
| `Predicate` | Evaluates a script against an `Exchange`, returning a `bool` |
| `MutatingExpression` | Evaluates a script against a `&mut Exchange`, propagating changes back |
| `MutatingPredicate` | Like `Predicate` but receives `&mut Exchange`; reserved for future use |

## Error Handling

All operations return `Result<_, LanguageError>` with variants:
- `ParseError` — invalid expression syntax
- `EvalError` — runtime evaluation failure
- `UnknownVariable` — referenced variable not found
- `NotFound` — language not registered
- `AlreadyRegistered` — language already registered under this name
- `NotSupported` — language does not implement mutating expressions/predicates

## Usage

```rust
use camel_language_api::{Language, Expression, Predicate, LanguageError};
use camel_api::exchange::Exchange;

fn evaluate_with(lang: &dyn Language, exchange: &Exchange) -> Result<(), LanguageError> {
    let expr = lang.create_expression("${header.name}")?;
    let value = expr.evaluate(exchange)?;

    let pred = lang.create_predicate("${header.type} == 'order'")?;
    let matches = pred.matches(exchange)?;

    Ok(())
}
```

## License

Apache-2.0

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-language-api = "*"
```
