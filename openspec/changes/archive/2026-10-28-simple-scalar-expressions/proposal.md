# Proposal: simple-scalar-expressions

## Why

camel-112's inbox/outbox ticks run ~25 rhai steps per non-empty tick because Simple
cannot do integer arithmetic, coercion, or null-safe defaults. Rhai is the fast
deterministic scripting path today (8.4 us/eval after rc-rn6v1), but scalar policy
belonging in Simple forces users onto a scripting engine for want of `int()`,
`default()`, and `+`. Simple is the fast path (~8 us/choice); it should cover
bounded integer scalar policy without becoming a scripting language.

Corpus: `.opencode/fleet/inbox/reference/camel-112-simple-corpus-2026-10-28.md`.
Binding ruling: e_gpt 2026-10-28 (recorded in the mission order and bd rc-p8aqm) —
bounded integer expressions + explicit defaults, read-only, no clock.

## What Changes

- Parser (`camel-language-simple`): precedence grammar `||` < `&&` < comparison
  (`== != > < >= <= contains`, non-chaining) < sum (`+ -`) < product (`*`) <
  unary `-` < atom; parenthesised expressions; calls `int(E)`, `string(E)`,
  `default(E,E)`, `min(E,E)`, `max(E,E)`; template slots accept full `${E}`
  expressions. Parse-time limits: 16 KiB source, 512 AST nodes, 64 depth.
- Evaluator/lib: i64 checked arithmetic (`EvalFailure(Arithmetic)` on overflow),
  strict `int()` conversion (`ConversionError`, never silent 0), lazy
  `default(a,b)` treating null/`""`/`"null"` as missing, `string()` rules,
  min/max, null-safe equality and ordering tables, 4096-step work limit and
  1 MiB output limit (`EvalFailure(Limit)`), payload-free diagnostics.
- Preserved: template/predicate modes, escaping, `${lang:...}` delegation,
  strict boolean predicate results, short-circuit truthiness, current equality
  rules (null == null; incompatible types compare false, no error),
  `script:` rejection (Simple stays read-only).
- Deferred: float, `/`, `%`, string `+`, ternary, `in`, `??`, pow/shift,
  JSON/map construction. `now()` rejected (inject via Exchange:
  `CamelMessageTimestamp` -> `nowMs`). No `property()`/`header()` aliases.
- Docs: ADR-0092, `docs/src/languages/simple.md`.
- Perf guard: choice <= 9 us, 25-step compute <= 250 us, <= 10% regression vs
  recorded baseline.

Affected crates: `camel-language-simple` (parser, evaluator, lib, benches),
`camel-core` (corpus-level tests if route-shape coverage needs it), docs.

## Acceptance criteria

- Corpus coverage: every corpus case SUPPORTED by the ruling has a test
  (including `""`/`"null"`/unset, `int("abc")` error, epoch-ms range,
  type-mismatch `==`, `default` laziness, overflow, short-circuit, strict
  boolean boundary). The deferred cases (capped backoff `2^(n-1)`, string
  `+` key construction, JSON construction) have tests documenting they stay
  in rhai or are parse errors in Simple.
- Migration coverage: a representative `CamelRedis.Key/Index/Timeout`
  rewrite as `set_header` templates with `${property.region}` has a test
  (the ~one-third corpus slice that needs no new syntax).
- `cargo test -p camel-language-simple -p camel-core` (touched suites) green;
  fmt; clippy `-D warnings`; xtask lints (unwrap, test-sleep, unbounded-wait,
  glob-imports, unsafe-safety, missing-docs-ratchet); doc build; `mdbook test`;
  `openspec validate --all --strict`.
- Bench gates recorded in the park report with numbers.

## Risk budget

Acceptable: integer-only arithmetic (float deferred), `null == null` staying
true (corpus's two-unset guard case migrates to explicit presence guards or
distinct non-colliding sentinels — documented in ADR-0092). Out of bounds: mutation in Simple, clock functions,
turning Simple into a scripting language, changing rhai, workspace/bridge bump.

Bd: rc-p8aqm.
