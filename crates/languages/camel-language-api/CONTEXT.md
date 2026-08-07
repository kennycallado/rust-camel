# Language SPI

The contract crate for the [Languages](../CONTEXT.md) bounded context. It defines shared
`LanguageError` and traits used by `js`, `jsonpath`, `xpath`, `simple`, `rhai`, and `minijinja`.
It owns no engine: each language crate implements these traits.

> **Scope boundary.** The domain concepts (Language, Expression, Predicate, MutatingExpression,
> MutatingPredicate) are glossed once in the parent [`crates/languages/CONTEXT.md`](../CONTEXT.md).
> This file documents the **SPI-contract** angle — trait object-safety, the sync/async split, the
> default `NotSupported` behavior, and the error type — without re-defining those terms.

## `#[non_exhaustive]` posture

ADR-0049 places this contract crate in its mandatory scope. The single `pub enum`,
`LanguageError`, carries `#[non_exhaustive]`. New contract enums use `#[non_exhaustive]` from
birth. Compliance is enforced by `cargo xtask lint-non-exhaustive`.

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
`EvalError(String)` (runtime evaluation failure), `UnknownVariable(String)` (missing variable),
`NotFound(String)` (unregistered language), and `NotSupported { feature, language }` (the default
for unimplemented mutating variants). `LanguageError::eval_error` attaches the expression text
to an `EvalError` (`error.rs:26`).
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
