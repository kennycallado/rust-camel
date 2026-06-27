# Language SPI

The contract crate for the [Languages](../CONTEXT.md) bounded context. Defines the traits every
expression/predicate language implements (`js`, `jsonpath`, `xpath`, `simple`, `rhai`) and the
shared `LanguageError`. It owns no engine: each language crate implements these traits.

> **Scope boundary.** The domain concepts (Language, Expression, Predicate, MutatingExpression,
> MutatingPredicate) are glossed once in the parent [`crates/languages/CONTEXT.md`](../CONTEXT.md).
> This file documents the **SPI-contract** angle — trait object-safety, the sync/async split, the
> default `NotSupported` behavior, and the error type — without re-defining those terms.

## Language

**Language (trait)**:
The factory SPI (`lib.rs`). `name()` returns the registration key; `create_expression` /
`create_predicate` compile a script string. `create_mutating_expression` /
`create_mutating_predicate` default to `Err(NotSupported)` so a language opts in to mutation rather
than being forced to implement it.
_Avoid_: language registry, language plugin (the trait is the contract, not the registration)

**Expression (trait)** / **Predicate (trait)**:
Object-safe, `async` (`#[async_trait]`) read-only evaluators: `Expression::evaluate` returns a
`Value`; `Predicate::matches` returns `bool`. Both take `&Exchange` — they must not mutate it.
_Avoid_: sync expression (the contract is async even when an impl is internally synchronous)

**MutatingExpression / MutatingPredicate (traits)**:
The opt-in mutating variants taking `&mut Exchange`; body/header/property changes propagate back.
`MutatingPredicate` is reserved (no current language implements it).
_Avoid_: side-effecting expression (use the precise trait name)

**LanguageError**:
The shared error enum (`error.rs`): `ParseError { expr, reason }` (compile/parse failure),
`EvalError(String)` (runtime evaluation failure), `NotSupported { feature, language }` (the default
for unimplemented mutating variants). Helper `EvalError::in_expr` attaches the expression text.
_Avoid_: ScriptError, eval failure (use LanguageError and its variant names)

## Example dialogue

> "My language is synchronous internally. Do I implement an async trait?"
> "Yes — `Expression` and `Predicate` are `#[async_trait]` by contract. Wrap your synchronous
> evaluation in the async method; the async shape lets the runtime treat all languages uniformly."
>
> "I only support read-only evaluation. What do I do about the mutating traits?"
> "Nothing — `create_mutating_expression` / `create_mutating_predicate` default to
> `Err(LanguageError::NotSupported)`. You opt in only when your language can mutate the Exchange."
>
> "Where do per-language value-coercion or null-handling rules belong?"
> "If a leaf language has semantics that diverge from this SPI (coercion, null handling, security
> constraints), give that leaf crate its own CONTEXT.md per the coverage policy; otherwise the parent
> languages/CONTEXT.md is sufficient."
