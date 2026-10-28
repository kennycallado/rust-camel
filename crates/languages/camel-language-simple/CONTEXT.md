# Simple language

Lightweight implementation of the [Language SPI](../camel-language-api/CONTEXT.md) for headers,
body fields, Exchange properties, exception messages, bounded scalars, predicates, and delegation.

## Compile and evaluation model

`create_expression` and `create_predicate` parse source once into an internal AST.
`SimpleExpression::evaluate` and `SimplePredicate::matches` reuse that AST per Exchange. Simple
therefore caches its parsed form and is not an FC-LANG-RECOMPILE instance.

**Language delegation**: `${lang:expr}` resolves `lang` through `ResolverFn` per evaluation and
asks that Language to create and evaluate an Expression, so a registry change is not masked by a
stale binding. The delegated Expression is not cached.
_Avoid_: static language binding, cached delegated expression

## Trust boundary

Simple source is trusted operator configuration; Exchange data and errors are untrusted. The
parser keeps them separate; delegation passes nested source to the selected Language's own limits.

## Expression and predicate modes

`create_expression` parses in TEMPLATE mode. `create_predicate` parses predicate mode: the
comparison grammar plus the bounded scalar expression grammar. Predicate mode is unchanged by
template mode.

**Template mode**: source is literal text plus `${...}` slots. Text outside a slot stays verbatim
(`=`, `<`, `null`, `true`, quotes, whitespace); a slot-free source is raw text. A slot accepts
atom lookups and the bounded scalar grammar, with no nested slots. Exactly one whole `${...}`
source returns its native value; `${body}` alone serializes a JSON body; other sources return a
string. An unclosed `${` is a compile-time parse error naming its byte offset; an empty `${}` and
a nested `${` are errors too. Template mode is NOT MiniJinja (ADR-0047) or route templates.
_Avoid_: MiniJinja, Jinja, route templates, `{{...}}`

**Predicate mode**: comparison grammar (`==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`, `&&`, `||`)
plus bounded scalar expressions. `matches` requires a real `Value::Bool`; `null`, `""`, `"true"`,
and other values are `LanguageError::TypeMismatch`. No truthiness coercion.
_Avoid_: truthiness, `as_bool` coercion, non-boolean predicate

## Bounded scalar expressions

Precedence, lowest to highest: `||` < `&&` < comparison < sum < product < unary minus < atom.
Comparisons do not chain. Sums are left-associative `+` and `-`; product is `*`; parentheses group
and count toward depth. Atoms are literals, lookups, `${language:expr}`, parentheses, and exactly
five calls: `int(E)`, `string(E)`, `default(a, b)`, `min(E, E)`, and `max(E, E)`. Wrong arity or
any other function name is a parse error. A bare non-lookup identifier is a parse error, as is
`now()`: Simple has no clock. Callers inject timestamps through the Exchange (`nowMs` property).

`int` accepts i64, a u64 that fits i64, and strict integer strings `[+-]?[0-9]+`; every other
operand is `ConversionError`, never a silent 0. `string` keeps strings, serializes numbers and
booleans, maps null to `""`, and rejects containers. `default` returns its first operand unless it
is null, `""`, or `"null"`, evaluates the fallback lazily, propagates first-operand errors, and
treats `false` and `0` as present. Arithmetic, unary minus, `min`, and `max` use the same operand
representations as `int` and i64 checked arithmetic; overflow is `EvalFailure(Arithmetic)`, other
operand kinds are `TypeMismatch`. `+` is numeric only. Integer literals are accepted in u64 range;
`-9223372036854775808` is `i64::MIN`; any other literal outside i64 range is a parse error.

Deferred: floats, `/`, `%`, string concat `+`, ternary, `in`, `??`, exponentiation, shifts, JSON
or map construction. Capped backoff, dynamic assembly, JSON work, mutation, clock stay in rhai.

### Limits

| Limit | Value | Failure |
| --- | --- | --- |
| Source size | 16 KiB | parse error |
| AST nodes | 512 | parse error |
| Parse depth | 64, parentheses counted | parse error |
| Evaluation work | 4096 units, shared per source | `EvalFailure(Limit)` |
| Output size | 1 MiB | `EvalFailure(Limit)` |

Work is one unit per evaluated AST node, one per template slot, and one per language delegation.
All Simple diagnostics are payload-free: counts, kinds, and positions, never Exchange values.

## Null and comparison semantics

- Missing headers, properties, exception messages, and JSON body fields are `Value::Null`. A
  body-field path against a non-JSON body is `Value::Null`.
- Null interpolation adds an empty string. A whole-slot missing value is `Value::Null`.
- `&&` and `||` use Simple truthiness for operands: null and empty strings are false, booleans
  keep their value, everything else is true. The final predicate result has no such coercion.
- `==` compares by value and coerces a numeric string to an exact integer (and the reverse). It
  never rounds through f64 and does not coerce string to boolean, so `${header.approved} == true`
  is false for the string `"true"`. Compare against `'true'` or store a boolean header.
- `null == null` is true; null does not equal `""`. `!=` is the exact complement of `==`.
  Incompatible types compare unequal without an error.
- Ordering and `contains` with null are false. Two strict integer strings compare numerically;
  other strings compare lexicographically; a non-integer string or boolean against a number is
  `TypeMismatch`.

`default` treats `null`, `""`, and `"null"` as missing. Elsewhere `"null"` is a present value, so
guard body shape explicitly with `body != '' && body != 'null'`.

## Authority

- ADR-0092 defines the bounded scalar grammar, coercion, and limits; normative delta:
  `openspec/changes/simple-scalar-expressions/specs/language-value-boundary/spec.md`.
- `camel-language-api` owns the `Language`, `Expression`, and `Predicate` contracts; ADR-0004
  governs hot-reload pipeline snapshot semantics.
