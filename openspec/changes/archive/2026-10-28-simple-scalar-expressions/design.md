# Design: simple-scalar-expressions

## Approach

Extend the existing Simple AST and recursive-descent parser with a precedence
climbing layer, and the evaluator with i64 scalar semantics. No new crate; no
SPI change (`LanguageError::EvalFailure` classes `Arithmetic`/`Limit` and
`ConversionError`/`TypeMismatch` already exist in `camel-language-api`).

- **Parser** (`parser.rs`): keep template/predicate mode split. Predicate mode
  gains precedence climbing: `or := and (|| and)*`, `and := cmp (&& cmp)*`,
  `cmp := sum (cmpop sum)?` (at most ONE comparison — non-chaining),
  `sum := product ((+|-) product)*`, `product := unary (* unary)*`,
  `unary := - unary | atom`, `atom := literal | lookup | slot | call | (E)`.
  Calls are exactly `int`, `string`, `default`, `min`, `max`; any other
  function name (including `now`) and any bare identifier is a parse error.
  Integer literals lex in u64 range; a unary minus over the literal
  `9223372036854775808` produces i64::MIN, any other out-of-i64-range
  literal is a parse error (this keeps `-9223372036854775808` expressible);
  float literals (`1.5`, `1e3`) are parse errors ("floats are deferred").
  Parentheses count toward a parse-time nesting bound of 64 so deeply nested
  parens cannot bypass the AST depth limit.
  Template slots (`${...}`) accept the same expression grammar (no nested
  slots); a whole-slot source keeps returning the native value. Parse-time
  limits: source <= 16 KiB, <= 512 AST nodes, depth <= 64 (ParseError).
- **Evaluator** (`evaluator.rs`, `lib.rs`): new AST nodes evaluate with the
  ruling's tables. `int(E)`: i64 passes, fitting u64, `[+-]?[0-9]+` strings
  (no whitespace); floats/bools/containers/out-of-range ->
  `ConversionError`. Arithmetic, unary `-`, `min`, and `max` accept exactly
  the `int()` operand representations (i64, fitting u64, strict integer
  strings) and reject everything else with `TypeMismatch`; i64 checked,
  overflow -> `EvalFailure(Arithmetic)`.
  `string(E)`: strings unchanged, numbers/bools serialized, null -> `""`,
  containers -> `ConversionError`. `default(a,b)`: lazy — evaluate `b` only
  when `a` yields null/`""`/`"null"`; `a`'s errors propagate; `false`/`0`
  are present. Equality keeps current rules (null == null true; incompatible
  types false without error; numeric-string coercion with exact integer
  comparisons). Ordering: null operand -> false; numeric strings accepted;
  incompatible/lossy -> `TypeMismatch`. Work limit 4096 steps and output
  limit 1 MiB -> `EvalFailure(Limit)`. Diagnostics stay payload-free
  (fixed strings, no exchange data).

## Affected crates

- `camel-language-simple`: parser, evaluator, lib, benches, tests; CONTEXT.md refresh.
- `camel-core`: corpus-shaped route tests only if unit tests cannot express them.
- `docs`: ADR-0092, `docs/src/languages/simple.md`.
- `.github/workflows/ci.yml`: bench-smoke guard step (mirrors the rhai
  `eval_overhead` precedent: absolute env gates + committed baseline file).

## Architecture boundaries

Languages context only. Simple remains read-only: no `MutatingExpression`
opt-in, `script:` still rejected loudly (existing spec scenario stays green).
No Runtime/DSL/Components/Services/Functions changes. Simple is NOT a
scripting language: no statements, no loops, no clock, no JSON construction.

## Phases

### Phase 1: Grammar + evaluation core
- **Goal:** Simple parses and evaluates bounded integer expressions per the ruling.
- **Dependencies:** none (existing parser/evaluator structure).
- **Externally-visible types/interfaces:** new AST variants; no SPI change;
  existing `create_expression`/`create_predicate` signatures unchanged.
- **Deliverable:** parser + evaluator + lib changes, corpus tests green.
- **Exit-criteria:** `cargo test -p camel-language-simple` green (incl. all
  corpus cases); fmt + clippy clean on touched crates.

### Phase 2: Docs, ADR, perf guard
- **Goal:** durable documentation and the performance gate.
- **Dependencies:** Phase 1 semantics frozen.
- **Externally-visible types/interfaces:** none (docs + bench only).
- **Deliverable:** ADR-0092, `docs/src/languages/simple.md`, guard bench +
  baseline file + bench-smoke wiring.
- **Exit-criteria:** `mdbook test` green; doc build green; bench gates pass
  (choice <= 9 us, 25-step <= 250 us, <= 10% vs recorded pre-change baseline).

## Alternatives considered

- **Full expression language (float, pow, ternary, JSON)** — rejected by the
  binding ruling: keeps Simple bounded and fast; capped backoff (`2^(n-1)`)
  stays in rhai until a future decision.
- **Native `now()`** — rejected by the ruling: callers inject epoch ms via the
  Exchange (`CamelMessageTimestamp` -> `nowMs` property).
- **`property()`/`header()` aliases** — rejected by the ruling: existing
  `property.x`/`header.x`/`body` lookups suffice; aliases double the grammar
  surface.
- **Two-unset equality guard (`null != null`)** — rejected: the binding ruling
  keeps `null == null` true. camel-112's `body == property(skipXml)` case
  must therefore use explicit presence guards
  (`body != null && property.skipXml != null && body == property.skipXml`)
  or distinct non-colliding sentinels — two `default(x, '')` fallbacks still
  match when both sides are absent (ADR-0092 migration guidance).
