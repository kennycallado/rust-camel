# 449-simple-exception — spec delta

## MODIFIED Requirements

### Requirement: Simple bounded scalar expression grammar

The Simple predicate grammar SHALL parse bounded scalar expressions with the
precedence chain `||` < `&&` < comparison < sum < product < unary minus <
atom. Comparison operators are `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`;
a comparison SHALL NOT chain (at most one comparison per level). Sum
operators are `+` and `-` (left-associative); the product operator is `*`;
unary minus negates its operand; parentheses group sub-expressions. Atoms
are the existing literals, lookups (`header.x`, `property.x`,
`exchangeProperty.x`, `body`, `body.<path>`, `exception`,
`exception.message`),
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

#### Scenario: Exception atom renders kind-prefixed message

- **GIVEN** an expression `${exception}` and the exchange carries an
  `HttpOperationFailed` error whose Display text is
  `HTTP GET http://svc/api failed: 500 Internal Server Error`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the string
  `HttpOperationFailed: HTTP GET http://svc/api failed: 500 Internal Server Error`
- **AND** the result SHALL NOT contain the error's response body payload or any
  Debug formatting

### Requirement: Template slots parse bounded expressions and unknown languages fail at evaluation

A `${...}` slot SHALL parse with the atom grammar (`body`, `header.`,
`body.<path>`, `exchangeProperty.`, `exception`, `exception.message`, `${language:expr}`
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

#### Scenario: Exception slot without an error is null

- **GIVEN** an expression `${exception}` and the exchange carries no error
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `Value::Null`
