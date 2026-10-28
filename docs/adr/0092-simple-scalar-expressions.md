# ADR-0092: Bounded scalar expressions in Simple

- Status: Accepted (decided 2026-10-28)
- Source: bd `rc-p8aqm`; camel-112 Simple corpus
  (`camel-112-simple-corpus-2026-10-28.md`)
- Ruling: e_gpt, 2026-10-28 (mission order 428)
- Cross-refs: ADR-0090 (pre-1.0 compatibility), ADR-0091 (Rust style),
  language value boundary spec
  `openspec/changes/simple-scalar-expressions/specs/language-value-boundary/spec.md`

## Context

camel-112 runs about 25 rhai steps per non-empty inbox/outbox tick. Most
of these steps do scalar policy: compare an epoch timestamp, add a retry
delay, apply a numeric fallback, or build a Redis key. Simple cannot
express these operations. It has no integer arithmetic, no numeric
coercion, and no explicit default. Users therefore move scalar policy into
rhai, a scripting engine, only to obtain `int()`, `default()`, and `+`.

Simple is the fast deterministic path at about 8 us per choice. It parses
once and evaluates against the Exchange with no external engine. Rhai is
also fast, but it carries a larger grammar and a scripting surface. Scalar
policy that fits Simple should stay in Simple.

The camel-112 corpus has three parts. Section 1 holds due checks, limit
comparisons, and retry deadlines. Section 2 holds Redis key construction
that existing template syntax already supports. Section 3 holds null-safe
defaults and presence guards. Section 1 is the forcing case for this ADR.

## Decision

Simple accepts bounded integer scalar expressions in predicate mode and in
`${...}` template slots. It stays a read-only expression language, not a
scripting language.

### Bounded integer expressions with explicit defaults

The language adds `int`, `string`, `default`, `min`, and `max` calls,
integer arithmetic, comparisons, and parentheses. It does not add
statements, loops, variables, or mutation. Every expression is a pure
function of the Exchange.

### Grammar and precedence

The precedence chain is, from lowest to highest:

```text
||  <  &&  <  comparison  <  sum  <  product  <  unary minus  <  atom
```

- Comparison operators are `==`, `!=`, `>`, `<`, `>=`, `<=`, and
  `contains`. A comparison does not chain. At most one comparison appears
  per level.
- Sum operators are `+` and `-`, left-associative.
- The product operator is `*`.
- Unary minus negates its operand.
- Parentheses group sub-expressions and count toward the depth limit.

Atoms are literals, lookups, `${language:expr}` delegation, and the five
calls. Lookups are `header.x`, `property.x`, `exchangeProperty.x`, `body`,
`body.<path>`, and `exception.message`. A bare identifier that names no
lookup and no accepted call is a parse error.

Integer literals are accepted in u64 range. A unary minus over the literal
`9223372036854775808` produces `i64::MIN`. Any other literal outside i64
range is a parse error. This rule keeps `-9223372036854775808` expressible
without a silent wrap.

### The five calls

| Call | Arity | Behaviour |
| --- | --- | --- |
| `int(E)` | 1 | Returns an i64 from an i64, a fitting u64, or a strict integer string `[+-]?[0-9]+`. Fails with `ConversionError` for every other value. It never returns a silent 0. |
| `string(E)` | 1 | Returns strings unchanged. Serializes numbers to decimal and booleans to `true`/`false`. Maps null to `""`. Fails with `ConversionError` for containers. |
| `default(a, b)` | 2 | Returns `a` unless `a` is null, `""`, or `"null"`. Then returns `b`. `b` is lazy. Errors from `a` propagate. `false` and `0` are present values. |
| `min(E, E)` | 2 | Converts both operands with the integer rule and returns the smaller i64. |
| `max(E, E)` | 2 | Converts both operands with the integer rule and returns the larger i64. |

A wrong arity is a parse error. Any other function name, including `now`,
is a parse error.

### Template slots

A `${...}` slot accepts the full bounded scalar expression grammar in
addition to the atom lookups. Slots do not nest. A slot that names nothing
is a compile-time parse error. A slot with a valid lowercase language
prefix and an unregistered language is created successfully and fails at
evaluation. Escaping and whole-slot native values stay unchanged: a source
that is exactly one `${E}` slot returns the native value, for example the
number 42, not the string `"42"`.

### Integer coercion and checked arithmetic

`int()` accepts i64, a u64 that fits i64, and strict integer strings with no
whitespace. It rejects floats, booleans, containers, malformed strings, and
integers outside i64 range with `ConversionError`. It never substitutes 0.

Arithmetic, unary minus, `min`, and `max` accept exactly the same operand
representations. Every other operand fails with `TypeMismatch`. Integer
strings remain valid arithmetic operands, so numeric-string arithmetic is
explicit and checked.

Arithmetic uses i64 checked operations. Overflow or underflow fails with
`EvalFailure(Arithmetic)`. Two string literals joined with `+` are a parse
error because string concatenation is deferred.

### Defaults and missing values

Missing means null, `""`, or `"null"`. `default(a, b)` evaluates `b` only
when `a` is missing. The default is therefore lazy, and an erroring `b`
stays dormant while `a` is present. An error from `a` propagates. The
values `false` and `0` are present and do not trigger the default.

### Equality

Equality keeps the existing rules:

- `null == null` is true.
- Values of incompatible types compare unequal with no error.
- `!=` is the exact complement of `==`.
- An unset null value does not equal the empty string.
- A numeric string coerces to a number for comparison with exact integer
  equality. The comparison never rounds through f64.
- A non-parsing string against a number compares unequal with no error.

### Ordering

Ordering operators are `>`, `<`, `>=`, and `<=`. The table is:

| Left operand | Right operand | Result |
| --- | --- | --- |
| null | any | `false` |
| any | null | `false` |
| strict integer string | strict integer string | numeric i64 comparison |
| other string | other string | lexicographic comparison |
| strict integer string | number | exact integer comparison |
| non-integer string | number | `TypeMismatch` |
| number | number | i64 comparison |
| boolean or container | any | `TypeMismatch` |

Ordering never returns true for a null operand.

### Strict booleans

A predicate result must be a real boolean. A number, string, or null result
is a `TypeMismatch` error. Logical `&&` and `||` keep their short-circuit
truthiness for operands. That rule never changes the final predicate
result.

### Limits

Simple enforces these limits:

| Limit | Value | Failure |
| --- | --- | --- |
| Source size | 16 KiB | parse error |
| AST nodes | 512 | parse error |
| Parse depth | 64 (parentheses counted) | parse error |
| Evaluation work | 4096 units, shared per source | `EvalFailure(Limit)` |
| Output size | 1 MiB | `EvalFailure(Limit)` |

Work is one unit per evaluated AST node, one unit per template slot, and one
unit per language delegation. The grammar has no repetition construct, so
the work ceiling is defense in depth. A source at the 512-node limit
evaluates within the budget.

### Payload-free diagnostics

Every Simple diagnostic is payload-free. Parse, conversion, arithmetic,
type-mismatch, limit, and delegation failures carry counts, kinds, and
positions. They never carry exchange values. This rule holds on every
error path, including delegation wrapping.

## Rulings

- **`now()` is rejected.** Simple has no clock. A parse error names the
  unknown function. Callers inject a timestamp through the Exchange: map
  the `CamelMessageTimestamp` header to the `nowMs` property, then evaluate
  `int(property.nowMs)`.
- **Simple stays read-only.** Simple does not implement
  `MutatingExpression`. The `script:` surface stays rejected. Simple
  evaluates values and predicates and never writes to the Exchange.
- **Capped backoff stays in rhai.** The shape
  `min(base * 2^(n-1), cap)` needs exponentiation and shifts. Both are
  deferred. Users keep this step in rhai. `min` and `max` cover the clamp
  only.
- **No `property()` or `header()` aliases.** The existing `property.x`,
  `header.x`, and `body` lookups suffice. Aliases would double the grammar
  surface and add no capability.
- **Two-unset equality keeps `null == null` true.** The corpus case
  `body == property.skipXml` is true when both sides are unset. The binding
  ruling does not change equality. Callers use explicit presence guards:
  `body != null && property.skipXml != null && body == property.skipXml`.
  Two `default(x, '')` fallbacks are not a guard because both sides still
  match when both are absent.

## Deferred

Each item stays out of Simple until a later decision:

- **Float literals and float arithmetic.** Integer policy is the forcing
  case; floats add rounding rules and a wider coercion table.
- **Division `/`.** It adds divide-by-zero and truncation rules.
- **Modulo `%`.** It adds sign rules and zero-divisor rules.
- **String concatenation with `+`.** Key construction uses templates;
  `+` stays numeric to keep one meaning for one operator.
- **Ternary conditionals.** `default` and `&&`/`||` cover the corpus; a
  ternary adds a second control-flow form.
- **`in`.** Collection membership needs collection literals, which are
  deferred.
- **`??`.** `default` already covers null-like values with explicit
  semantics.
- **Exponentiation and shifts.** They exist for capped backoff, which
  stays in rhai.
- **JSON and map construction.** Simple is read-only and has no container
  literals; JSON work stays in rhai or a data format.
- **Bracket and quoted key lookup.** `header["some-key"]` does not exist in
  Simple. Hyphenated keys are addressed by the unspaced key-continuation rule
  and the spaced-hyphen diagnostic (bd rc-e2p0n), so no grammar surface is
  added.

## Migration guidance

### Corpus section 1: due checks and limits

A due check moves from rhai to Simple without new syntax beyond this ADR:

```text
property.skipHandled != true && int(property.nowMs) >= int(default(body, '0'))
```

An empty or `"null"` body defaults `at` to 0. The epoch-ms range stays exact
because the comparison uses i64, not f64. A limit comparison is:

```text
int(header.n) >= int(header.limit)
```

A retry deadline is a template:

```text
retry-at-${string(int(property.nowMs) + int(header.delay))}
```

### Corpus section 2: key construction

Redis keys already use template slots. Set them with `set_header` templates
and no new grammar:

```text
112:${property.region}:inbox
${header.inboxKey}:retry-at
112:${property.region}:outbox:attempts
```

### Corpus section 3: defaults and null safety

Use `default(...)` for a numeric fallback. Use explicit string guards for
body shape: `body != '' && body != 'null'`. An unset property compared to a
boolean is false with no error. Short-circuit order protects later
comparisons: `header.outcome == 'unknown' && int(header.n) >= int(header.limit)`.

For two possibly unset values, add the presence guard from the rulings
section. Do not rely on two matching fallback sentinels.

### What still forces rhai

Keep these in rhai:

- capped backoff (`2^(n-1)` and shifts);
- string concatenation and dynamic key assembly beyond templates;
- JSON or map construction;
- float, division, and modulo arithmetic;
- mutation of the Exchange;
- clock access;
- anything beyond the bounded grammar.

## Consequences

- Simple covers bounded integer scalar policy. camel-112 can remove most of
  its ~25 rhai steps per tick.
- The language stays deterministic, read-only, and fast. There is no clock,
  no mutation, and no external engine.
- The grammar surface stays small. Deferred constructs have named reasons
  and named escape hatches.
- No SPI change is needed. The error classes `Arithmetic`, `Limit`,
  `ConversionError`, and `TypeMismatch` already exist in
  `camel-language-api`.
- The limits bound parse cost, evaluation cost, and output size. Diagnostics
  stay payload-free.
- rhai remains the scripting path. The boundary is explicit: scalar policy
  in Simple, scripting in rhai.
