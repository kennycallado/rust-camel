# Simple

A lightweight expression and predicate language for header, body, property, and exception access. It uses `${...}` interpolation syntax and supports compound predicates with `&&` and `||`, plus bounded integer scalar expressions with explicit conversions and defaults.

```rust,ignore
{{#include ../../../examples/language-simple/src/main.rs:setup}}
```

```rust,ignore
{{#include ../../../examples/language-simple/src/main.rs:route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: language-simple-demo
  from: timer:tick?period=800&repeatCount=8
  steps:
    - set_header:
        key: type
        value: order
    - set_header:
        key: priority
        value: high
    - set_header:
        key: approved
        value: "true"
    - set_body:
        simple: "${header.type}"
    - filter:
        simple: "${body} != null"
        steps:
          - to: log:body-present?showBody=true&showHeaders=true
    - filter:
        simple: "${header.type} == 'order'"
        steps:
          - to: log:orders?showBody=true&showHeaders=true
    - filter:
        simple: "${header.type} == 'order' && ${header.priority} == 'high'"
        steps:
          - to: log:high-priority-orders?showBody=true&showHeaders=true
    - filter:
        simple: "${header.approved} == 'true'"
        steps:
          - to: log:approved-orders?showBody=true&showHeaders=true
```

</details>

Simple parses its source once into an AST and reuses it for each Exchange. You build predicates and expressions up front with `create_predicate` and `create_expression`, then move them into route closures. Predicates return a boolean for filter and choice steps. Expressions return a `Value` for enrichment and transformation. The included example constructs four predicates and one expression before the route.

The language evaluates `${header.x}`, `${body}`, and `${exchangeProperty.y}` against the Exchange. Missing headers, properties, and exception lookups evaluate to `Value::Null`. Simple also supports language delegation through `${lang:expr}`, which resolves the target language at evaluation time.

## Expression and predicate modes

Simple has two parsing modes. `create_expression` uses **template mode**. In template mode the source is literal text plus `${...}` slots. Text outside a slot stays verbatim, so `=`, `<`, `null`, `true`, quotes, and whitespace are never re-parsed as predicate grammar. `create_predicate` uses **predicate mode**, the comparison grammar (`==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`, `&&`, `||`) plus bounded scalar expressions. Predicate mode is unchanged by template mode.

A predicate must evaluate to a real boolean. Any other result is a `type mismatch: expected bool` error. This includes `null`, an empty string, and the string `"true"`. Simple does not apply truthiness at the predicate boundary. Inside `&&` and `||` it does apply a truthiness rule to the operands: null and empty strings are false. That rule never changes the final predicate result.

`==` compares by value and coerces between a string and a number. It parses the string as a strict integer, so `${header.count} == 10` matches when the header is the string `"10"`. It does not coerce a string to a boolean. `${header.approved} == true` is false when the header is the string `"true"`. Compare against `'true'` or store a boolean header instead.

In template mode a source that is exactly one `${...}` slot returns the slot's existing value. Numbers, arrays, or `null` from a header or body-field slot stay native. A whole `${body}` slot renders a JSON body as serialized text. Any other source returns a string. A source with no slot is raw text: `'abc'`, `true`, `42`, and `null` evaluate to the strings `'abc'`, `true`, `42`, and `null`. A missing slot in a mixed template renders as an empty string. A whole missing slot returns `null`.

A `${` that is never closed is a compile-time parse error, never literal text: `Got ${name` fails at creation with a message naming `unclosed interpolation` and the byte offset of the `${` in the trimmed source. An empty `${}` slot and a `${` nested inside slot content are parse errors too. A lone `$`, `{`, or `}` outside a `${` stays verbatim literal text.

Simple template mode is Simple's own literal-plus-`${}` grammar. It is not the [MiniJinja](minijinja.md) template engine (ADR-0047) and not route-template `{{...}}` substitution (ADR-0008).

A bare top-level string value in a route file that contains `${...}` compiles to a Simple template. Only a top-level string is gated. A string nested inside an object or array literal stays literal. See [Basic steps](../yaml-dsl/step-verbs/basic.md) and [Environment variable interpolation](../configuration/env-interpolation.md).

`$$` is not a Simple-template escape. At the Simple source level, `$${header.h}` is a literal `$` followed by the live `${header.h}` slot. The route file's environment pass turns each `$$` into `$` before Simple sees the source. Therefore `$${header.h}` in a route file reaches Simple as the whole slot `${header.h}` and keeps its native value. To render `$` plus a slot, write `$$$${header.h}`. To emit a literal `${...}` that must not be evaluated, use an explicit `language:` and `source:` value with a quoted rhai or js string, and escape environment or argument tokens as `$$` (see [Environment variable interpolation](../configuration/env-interpolation.md)).

## Bounded scalar expressions

Simple parses bounded integer scalar expressions in predicate mode and in `${...}` template slots. An expression is a pure function of the Exchange. Simple has no statements, loops, variables, or mutation, and no external engine.

### Grammar and precedence

The precedence chain, from lowest to highest:

| Level | Operators or form | Notes |
| --- | --- | --- |
| logical or | `\|\|` | Short-circuits. Left operand truthiness decides. |
| logical and | `&&` | Short-circuits. Left operand truthiness decides. |
| comparison | `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains` | One comparison per level; comparisons do not chain. |
| sum | `+`, `-` | Left-associative. Integer operands only. |
| product | `*` | Integer operands only. |
| unary minus | `-E` | Negates one integer operand. |
| atom | literals, lookups, calls, delegation, `( E )` | Parentheses group and count toward the depth limit. |

Lookups are `header.x`, `property.x`, `exchangeProperty.x`, `body`, `body.<path>`, `exception`, and `exception.message`. A lookup key may contain hyphens when each hyphen is directly followed by a letter or an underscore and the key is written without spaces (`header.some-key`), in standalone and compound positions. A hyphen followed by a digit or whitespace is the subtraction operator, so `header.n-1` is `header.n - 1`. `header.some - key` is therefore a parse error, and the message names the unspaced form. Atoms also include `${language:expr}` delegation and the five calls below.

`exception` renders `<variant_name>: <Display>`, for example `HttpOperationFailed: HTTP GET http://svc/api failed: 500 Internal Server Error`; `exception.message` renders the error's `Display` text alone. Both are `Value::Null` when the exchange carries no error, and the rendering never includes the HTTP response body or other payload data beyond the message.

A bare identifier that names no lookup and no accepted call is a parse error. `x == 1` is therefore rejected; write `property.x == 1`. `now()` is a parse error because Simple has no clock. Inject a timestamp through the Exchange, for example by mapping the `CamelMessageTimestamp` header to the `nowMs` property, then evaluate `int(property.nowMs)`.

Integer literals are parsed in u64 range. A unary minus over the literal `9223372036854775808` produces `i64::MIN`. Any other literal outside i64 range is a parse error.

### The five calls

| Call | Arity | Behaviour |
| --- | --- | --- |
| `int(E)` | 1 | Converts to i64. Accepts i64, a u64 that fits i64, and a strict integer string `[+-]?[0-9]+`. Fails with `ConversionError` for everything else. Never returns a silent 0. |
| `string(E)` | 1 | Returns strings unchanged, serializes numbers to decimal and booleans to `true`/`false`, maps null to `""`, and fails with `ConversionError` for containers. |
| `default(a, b)` | 2 | Returns `a` unless `a` is null, `""`, or `"null"`. Then returns `b`. `b` is evaluated lazily. Errors from `a` propagate. `false` and `0` are present values. |
| `min(E, E)` | 2 | Converts both operands with the integer rule and returns the smaller i64. |
| `max(E, E)` | 2 | Converts both operands with the integer rule and returns the larger i64. |

A wrong arity is a parse error. Any other function name, including `now`, is a parse error.

### Coercion and arithmetic

| Input | `int(E)` | `string(E)` | Arithmetic, `min`, `max`, unary `-` |
| --- | --- | --- | --- |
| i64 | the value | decimal text | accepted |
| u64 that fits i64 | the value | decimal text | accepted |
| strict integer string | parsed i64 | unchanged | accepted as an integer |
| other string | `ConversionError` | unchanged | `TypeMismatch` |
| boolean | `ConversionError` | `true` or `false` | `TypeMismatch` |
| null | `ConversionError` | `""` | `TypeMismatch` |
| array or object | `ConversionError` | `ConversionError` | `TypeMismatch` |

`int()` requires `[+-]?[0-9]+` with no whitespace. Leading or trailing spaces, decimal points, and values outside i64 range fail. `int()` accepts a u64 only when it fits i64; `u64::MAX` fails.

Arithmetic uses i64 checked operations. Overflow or underflow fails with `EvalFailure(Arithmetic)`. A numeric string is a valid arithmetic operand, so numeric-string arithmetic is explicit and checked. `+` is numeric only; joining two string literals with `+` is a parse error.

### Null, missing, and comparison rules

- A missing header, property, exception lookup, or JSON body field is `Value::Null`.
- `null == null` is true. An unset null value does not equal the empty string.
- Values of incompatible types compare unequal with no error. `!=` is the exact complement of `==`.
- A numeric string coerces to an exact integer for comparison with a number; the comparison never rounds through f64.
- Ordering (`>`, `<`, `>=`, `<=`) with a null operand is false.
- Two strict integer strings compare numerically; any other two strings compare lexicographically.
- For ordering comparisons (`>`, `<`, `>=`, `<=`), a non-integer string or a boolean compared with a number is a `TypeMismatch`.
- A predicate result must be a real boolean. A number, string, or null result is a `TypeMismatch`. `&&` and `||` keep short-circuit truthiness for their operands only.
- Diagnostics are payload-free: they carry counts, kinds, and positions, never Exchange values.

### Limits

| Limit | Value | Failure |
| --- | --- | --- |
| Source size | 16 KiB | parse error |
| AST nodes | 512 | parse error |
| Parse-time nesting depth | 64, parentheses counted | parse error |
| Evaluation work | 4096 units, shared per source evaluation | `EvalFailure(Limit)` |
| Produced output | 1 MiB | `EvalFailure(Limit)` |

Work is one unit per evaluated AST node, one unit per template slot, and one unit per language delegation. The grammar has no repetition construct, so the work ceiling is defense in depth. A source at the 512-node limit evaluates within the budget.

### Corpus examples

A due check compares epoch milliseconds exactly and defaults an empty or `"null"` body to zero:

```text
property.skipHandled != true && int(property.nowMs) >= int(default(body, '0'))
```

A retry deadline renders through a template slot:

```text
retry-at-${string(int(property.nowMs) + int(header.delay))}
```

A limit comparison stays exact beyond f64 precision:

```text
int(header.n) >= int(header.limit)
```

Redis keys use ordinary template slots and need no expression grammar:

```text
112:${property.region}:inbox
${header.inboxKey}:retry-at
112:${property.region}:outbox:attempts
```

For two possibly unset values, use an explicit presence guard. Null stays equal to null, so two matching fallback sentinels are not a guard:

```text
body != null && property.skipXml != null && body == property.skipXml
```

## Deferred constructs

These constructs are parse errors today. None is supported syntax:

- float literals and float arithmetic (`1.5`);
- division `/` and modulo `%`;
- string concatenation with `+`;
- ternary conditionals (`x ? y : z`);
- `in` membership;
- null-coalescing `??`;
- exponentiation `**` and shifts;
- JSON or map construction;
- chained comparisons (`1 < 2 < 3`).

## Use rhai instead

[rhai](rhai.md) remains the scripting path. Keep these steps in rhai:

- capped exponential backoff (`min(base * 2^(n-1), cap)`), which needs exponentiation and shifts;
- string concatenation and dynamic key assembly beyond template slots;
- JSON or map construction;
- mutation of the Exchange;
- clock access;
- float, division, and modulo arithmetic;
- anything beyond the bounded scalar grammar.

Simple evaluates scalar policy and predicates only. It does not implement `MutatingExpression`, and the `script:` surface stays rejected.

Simple is the most common language for predicates in EIP patterns. Use it for flat header and body access in filter, choice, and enrichment steps. It requires no external engine. For structured JSON queries, use [JSONPath](jsonpath.md).

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · [Simple crate](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-simple/CONTEXT.md) · ADR-0092
