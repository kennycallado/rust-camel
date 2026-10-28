## ADDED Requirements

### Requirement: Simple bounded scalar expression grammar

The Simple predicate grammar SHALL parse bounded scalar expressions with the
precedence chain `||` < `&&` < comparison < sum < product < unary minus <
atom. Comparison operators are `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`;
a comparison SHALL NOT chain (at most one comparison per level). Sum
operators are `+` and `-` (left-associative); the product operator is `*`;
unary minus negates its operand; parentheses group sub-expressions. Atoms
are the existing literals, lookups (`header.x`, `property.x`,
`exchangeProperty.x`, `body`, `body.<path>`, `exception.message`),
`${language:expr}` delegation, and calls to exactly `int`, `string`,
`default`, `min`, `max`. A bare identifier that names no lookup and no
accepted call SHALL be a parse error; in particular `now()` SHALL be a parse
error — callers inject timestamps through the Exchange
(`CamelMessageTimestamp` -> `nowMs`). Integer literals SHALL be accepted in
u64 range; a unary minus whose operand is the literal `9223372036854775808`
SHALL produce i64::MIN, and any other literal outside i64 range SHALL be a
parse error. Float literals, `/`, `%`, string concatenation with `+`,
ternary conditionals, `in`, `??`, exponentiation, shifts, and JSON or map
construction SHALL be parse errors.

#### Scenario: Precedence and parentheses

- **GIVEN** the predicate
  `int(header.a) + 2 * 3 >= int(header.b) - 1 && property.c || property.d`
- **WHEN** the predicate is created
- **THEN** creation SHALL succeed and the AST SHALL group `2 * 3` before the
  sum, the sums before the comparison, the comparison before `&&`, and `&&`
  before `||`
- **AND** `(int(header.a) + 2) * 3` SHALL change the grouping accordingly

#### Scenario: Comparison does not chain

- **GIVEN** the predicate `1 < 2 < 3`
- **WHEN** the predicate is created
- **THEN** creation SHALL fail with a parse error

#### Scenario: Deferred constructs are parse errors

- **GIVEN** predicates using `1.5`, `7 / 2`, `10 % 3`, `'a' + 'b'`,
  `x ? y : z`, `n in (1, 2)`, `a ?? b`, or `2 ** 3`
- **WHEN** each predicate is created
- **THEN** each creation SHALL fail with a parse error naming the deferred
  construct

#### Scenario: Bare identifier and clock call are rejected

- **GIVEN** the predicates `x == 1` and `now() > 1000`
- **WHEN** each predicate is created
- **THEN** each creation SHALL fail with a parse error
- **AND** `property.x == 1` SHALL still parse

#### Scenario: Accepted calls parse

- **GIVEN** the predicate
  `min(int(header.base), int(header.cap)) <= int(property.nowMs) && default(property.giveUpAt, '0') != 'null'`
- **WHEN** the predicate is created
- **THEN** creation SHALL succeed

#### Scenario: Integer literal range boundaries

- **GIVEN** the literals `9223372036854775807`, `-9223372036854775808`,
  `9223372036854775808`, and `99999999999999999999`
- **WHEN** predicates using each literal are created
- **THEN** `9223372036854775807` SHALL parse to i64::MAX
- **AND** `-9223372036854775808` SHALL parse to i64::MIN via unary minus
- **AND** `9223372036854775808` outside a unary minus SHALL be a parse error
- **AND** `99999999999999999999` SHALL be a parse error
- **WHEN** the predicates `9223372036854775807 + 1` and
  `-9223372036854775808 - 1` evaluate
- **THEN** both SHALL fail with `EvalFailure(Arithmetic)`

### Requirement: Simple integer coercion and arithmetic semantics

The `int(E)` call SHALL return the operand when it is an i64, a u64 that
fits i64, or a string matching `[+-]?[0-9]+` with no whitespace; it SHALL
fail with `ConversionError` for floats, booleans, containers, malformed
strings, and integers outside i64 range (including u64 values above
i64::MAX), and SHALL NEVER substitute a silent 0. The `string(E)` call
SHALL return strings unchanged, SHALL serialize numbers and booleans to
their decimal and `true`/`false` forms, SHALL map null to `""`, and SHALL
fail with `ConversionError` for containers. The `default(a, b)` call SHALL
evaluate `b` lazily — only when `a` yields null, `""`, or `"null"` — SHALL
propagate errors from `a`, and SHALL treat `false` and `0` as present
values. The `min(E, E)` and `max(E, E)` calls and the `+`, `-`, `*`, and
unary `-` operators SHALL operate on i64 with checked arithmetic; overflow
or underflow SHALL fail with `EvalFailure(Arithmetic)`. Arithmetic,
unary minus, `min`, and `max` operands SHALL accept exactly the `int()`
representations (i64, fitting u64, strict integer strings); every other
operand SHALL fail with `TypeMismatch`. String concatenation with `+` is
deferred: when both `+` operands are string literals this SHALL be a parse
error; dynamic operands are validated at evaluation by the operand rule
above, which still admits strict integer strings (numeric-string
arithmetic).

#### Scenario: Integer conversion from numeric strings

- **GIVEN** header `count` with value `"42"` and property `nowMs` with the
  epoch-ms string `"1750000000000"`
- **WHEN** `int(header.count)` and `int(property.nowMs)` evaluate
- **THEN** both SHALL return the exact i64 values 42 and 1750000000000

#### Scenario: Malformed and out-of-range conversion is a catchable error

- **GIVEN** header `v` with value `"abc"`, `" 7"`, or `"1.5"`, header `u`
  with value `"18446744073709551615"`, property `b` with boolean value
  `true`, and property `u64val` holding a native u64 number value above
  i64::MAX
- **WHEN** `int(...)` evaluates each operand
- **THEN** each evaluation SHALL fail with `ConversionError`
- **AND** no evaluation SHALL return 0

#### Scenario: Checked arithmetic catches overflow

- **GIVEN** property `n` with value `"9223372036854775807"`
- **WHEN** `int(property.n) + 1` evaluates
- **THEN** the evaluation SHALL fail with `EvalFailure(Arithmetic)`

#### Scenario: Arithmetic rejects non-integer operands

- **GIVEN** header `f` with value `"1.5"` and property `flag` with boolean
  value `true`
- **WHEN** `header.f * 2` and `property.flag + 1` evaluate
- **THEN** both SHALL fail with `TypeMismatch`

#### Scenario: String conversion rules

- **GIVEN** header `h` with string value `"x"`, header `n` with numeric
  value `42`, property `flag` with boolean value `true`, an unset header,
  and property `arr` holding a JSON array value
- **WHEN** `string(...)` evaluates each
- **THEN** the results SHALL be `"x"`, `"42"`, `"true"`, `""`, and a
  `ConversionError` respectively

#### Scenario: Default is lazy and treats null-like values as missing

- **GIVEN** property `giveUpAt` unset, and header `flag` with value `""`,
  `"null"`, `"0"`, and `false` in turn, and header `bad` = `"abc"`
- **WHEN** `default(property.giveUpAt, '100')` evaluates, the unset case
  SHALL evaluate to `"100"`
- **AND** `default(header.flag, 'x')` SHALL select `'x'` for `""` and
  `"null"`, and SHALL select the operand for `"0"` and `false`
- **AND** `default(header.flag, int(header.bad))` SHALL succeed while `flag`
  is present (the fallback is not evaluated) and SHALL fail with
  `ConversionError` when `flag` is unset
- **AND** an erroring first operand SHALL propagate its error

#### Scenario: Min and max select on i64

- **GIVEN** headers `base` = `"500"` and `cap` = `"3000"`
- **WHEN** `min(int(header.base) * 2, int(header.cap))` evaluates
- **THEN** the result SHALL be the i64 value 1000
- **WHEN** `max(int(header.base), int(header.cap))` evaluates
- **THEN** the result SHALL be the i64 value 3000

### Requirement: Simple comparison and ordering semantics

Equality SHALL keep the existing rules: null equals null; values of
incompatible types compare unequal without an error; a numeric string
coerces to a number for comparison with exact integer equality; `!=` SHALL
be the exact complement of `==`. Unset (null) SHALL NOT equal the empty
string. Ordering comparisons (`>`, `<`, `>=`, `<=`) with a null operand
SHALL return false. Two strings that both parse as strict integers SHALL
compare numerically; any other two strings SHALL compare lexicographically.
A numeric string and a number SHALL compare as exact integers; incompatible
operand types or non-integer numeric strings in mixed comparisons SHALL
fail with `TypeMismatch`. Strict boolean predicate results and
short-circuit truthiness for `&&` and `||` SHALL stay unchanged.

#### Scenario: Equality never throws on type mismatch

- **GIVEN** property `flag` with boolean value `true` and header `ok` with
  string value `"true"`
- **WHEN** `property.flag == header.ok` evaluates
- **THEN** the result SHALL be false with no error
- **AND** `property.flag != header.ok` SHALL be true

#### Scenario: Null equality and the unset-vs-empty distinction

- **GIVEN** header `a` unset and header `b` with value `""`
- **WHEN** `header.a == header.a`, `header.a == header.b`, and
  `header.a != header.b` evaluate
- **THEN** the results SHALL be true, false, and true respectively

#### Scenario: Numeric string coercion is exact beyond float precision

- **GIVEN** header `big` with value `"9007199254740993"` and header `n` with
  value `"42"`
- **WHEN** `header.big == 9007199254740993` and `header.n == 43` evaluate
- **THEN** the results SHALL be true and false respectively
- **AND** `header.big == 9007199254740992` SHALL be false — the comparison
  SHALL NOT round through f64

#### Scenario: Ordering rules

- **GIVEN** header `n` unset, headers `a` = `"10"` and `b` = `"9"`, headers
  `w` = `"apple"` and `v` = `"9"`, header `dec` = `"1.5"`, and header
  `flag` = `true`
- **WHEN** `header.n > 1`, `header.a > header.b`, `header.w > header.v`,
  `header.dec > 1`, and `header.flag > 0` evaluate
- **THEN** the results SHALL be false (null), true (numeric strings compare
  numerically), true (non-numeric strings compare lexicographically), a
  `TypeMismatch` error (non-integer string against a number), and a
  `TypeMismatch` error (boolean against a number) respectively

#### Scenario: Strict boolean results stay

- **GIVEN** the predicate `int(header.n) + 1` with header `n` = `"41"`
- **WHEN** the predicate is evaluated
- **THEN** evaluation SHALL fail with a type-mismatch error because the
  result is a number, not a boolean
- **AND** the predicate `false && header.missing > 1` SHALL return false
  without evaluating the right side

### Requirement: Simple evaluation resource limits

Simple SHALL enforce parse-time limits: source at most 16 KiB, at most 512
AST nodes, and parse-time nesting depth at most 64 with parentheses counted
toward the nesting bound; violations SHALL be parse errors. Simple SHALL
enforce evaluation-time limits with one shared budget per source
evaluation: charged work of one unit per evaluated AST node, one unit per
template slot, and one unit per language delegation, capped at 4096 units;
and produced output capped at 1 MiB. Because the grammar has no repetition
construct, the work ceiling is defense in depth; a source at the 512-node
limit SHALL evaluate within the budget. Limit violations SHALL fail with
`EvalFailure(Limit)`. All Simple diagnostics — parse, conversion,
arithmetic, type-mismatch, limit, and delegation failures — SHALL be
payload-free: they SHALL carry counts, kinds, and positions, never exchange
values.

#### Scenario: Parse-time limits

- **GIVEN** a predicate source larger than 16 KiB, an AST larger than 512
  nodes, and an expression with 65 nested parentheses in turn
- **WHEN** each is created
- **THEN** each creation SHALL fail with a parse error naming the limit
- **AND** an expression with 64 nested parentheses SHALL parse successfully

#### Scenario: Output limit at evaluation

- **GIVEN** a header `big` whose value is larger than 1 MiB
- **WHEN** the template `x${header.big}` evaluates
- **THEN** the evaluation SHALL fail with `EvalFailure(Limit)`
- **AND** the error detail SHALL contain no exchange data

#### Scenario: Node-limited source evaluates within the work budget

- **GIVEN** a valid template whose AST has close to 512 nodes (including
  slot and delegation units in the charge)
- **WHEN** the template evaluates
- **THEN** evaluation SHALL succeed with charged work under the 4096-unit
  ceiling

### Requirement: Template slots parse bounded expressions and unknown languages fail at evaluation

A `${...}` slot SHALL parse with the atom grammar (`body`, `header.`,
`body.<path>`, `exchangeProperty.`, `exception.message`, `${language:expr}`
delegation) AND with the bounded scalar expression grammar (`int`,
`string`, `default`, `min`, `max`, arithmetic, comparisons, logical
operators, parentheses). Slots SHALL NOT nest. A slot that names nothing
SHALL be a compile-time parse error. A slot with a valid lowercase language
prefix whose language is not registered SHALL be created successfully and
SHALL fail at evaluation time with an evaluation error, not at compile
time. Existing slot escaping and whole-slot native-value semantics SHALL
stay unchanged.

#### Scenario: Unrecognized slot is a compile error

- **GIVEN** an expression `${unknown}`
- **WHEN** the expression is created
- **THEN** creation SHALL fail with a parse error

#### Scenario: Unknown language is an evaluation failure

- **GIVEN** an expression `${nope:x}` where `nope` is not a registered
  language
- **WHEN** the expression is created
- **THEN** creation SHALL succeed
- **WHEN** the expression is evaluated
- **THEN** evaluation SHALL fail with an evaluation error

#### Scenario: Slot expression evaluates

- **GIVEN** property `nowMs` = `"1750000000000"` and header `delay` = `"500"`
- **WHEN** the template
  `retry-at-${int(property.nowMs) + int(header.delay)}` evaluates
- **THEN** the result SHALL be the string `retry-at-1750000000500`

#### Scenario: Nested slots are a parse error

- **GIVEN** the template `${default(header.x, '${header.y}')}`
- **WHEN** the template is created
- **THEN** creation SHALL fail with a parse error

#### Scenario: Whole-slot expressions keep native results

- **GIVEN** the source `${int(header.n) * 2}` with header `n` = `"21"`
- **WHEN** the source is evaluated as a whole-slot expression
- **THEN** the result SHALL be the number 42, not the string `"42"`

## REMOVED Requirements

### Requirement: Template slots parse atoms and unknown languages fail at evaluation

**Reason:** superseded by "Template slots parse bounded expressions and
unknown languages fail at evaluation", which extends slot parsing from atoms
to the bounded scalar expression grammar while preserving every existing
guarantee (unknown slot compile error, unknown-language evaluation failure,
escaping, whole-slot native values).
