## ADDED Requirements

### Requirement: Simple date sub-language provides UTC wall-clock formatting

The system SHALL parse `${date:now:<pattern>}`, `${date:now}`, and
`${date:header.<name>:<pattern>}` as native Simple slots using a documented Java
`SimpleDateFormat` subset (`y`, `M`, `d`, `H`, `m`, `s`, `S`, `X`, `'` quoting). The clock SHALL
be UTC. `${date:now}` without a pattern SHALL use the ISO-8601 default
`yyyy-MM-dd'T'HH:mm:ss.SSSX`. The special payload `${date:now:millis}` SHALL evaluate to epoch
milliseconds as a number. `date:` slots SHALL be usable in template slots and predicates alike.
The `S` field SHALL follow Java minimum-width integer milliseconds (count = minimum digits,
zero-padded, never truncated), and every `X` count (1–3) SHALL render `Z` for the UTC clock,
matching the JDK's zero-offset ISO-8601 rendering.

#### Scenario: ISO-8601 default formats a fixed instant

- **GIVEN** the pattern compiler and a fixed instant at epoch millisecond `1700000123456`
- **WHEN** the default pattern formats that instant
- **THEN** the output SHALL be `2023-11-14T22:15:23.456Z`

#### Scenario: Pattern matrix formats a fixed instant

- **GIVEN** the fixed instant at epoch millisecond `1700000123456` (wall time
  `2023-11-14T22:15:23.456Z`)
- **WHEN** the patterns `yyyy-MM-dd`, `HH:mm:ss`, `yyyy-MM-dd HH:mm:ss.SSS`, `yy`,
  `HH:mm:ssXXX`, and `'T'HH` are applied
- **THEN** the outputs SHALL be `2023-11-14`, `22:15:23`, `2023-11-14 22:15:23.456`, `23`,
  `22:15:23Z`, and `T22` respectively

#### Scenario: Millis width follows Java minimum-width semantics

- **GIVEN** the fixed instant at epoch millisecond `1700000123045` (millisecond field `45`)
- **WHEN** the patterns `S`, `SS`, and `SSS` are applied to the `HH:mm:ss.SSS`-style fraction
  position
- **THEN** the millisecond rendering SHALL be `45`, `45`, and `045` respectively
- **AND** for the fixed instant `1700000123004` (millisecond field `4`) the pattern `SS` SHALL
  render `04`

#### Scenario: millis payload evaluates to a number

- **GIVEN** a compiled expression `${date:now:millis}`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be a `Value::Number` whose magnitude is within the plausible
  epoch-millisecond window around evaluation time

#### Scenario: Live now output has the ISO shape

- **GIVEN** a compiled expression `${date:now}`
- **WHEN** the expression is evaluated twice
- **THEN** each output SHALL match the ISO-8601 UTC shape
  `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$`
- **AND** no assertion SHALL compare raw wall-clock values for equality or order, and no test
  SHALL sleep

#### Scenario: Date header variant formats an epoch-millis header

- **GIVEN** a header `ts` holding the number `1700000123456` and the expression
  `${date:header.ts:yyyy-MM-dd}`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `2023-11-14`
- **AND** a missing header SHALL evaluate to `null`
- **AND** a non-numeric header value SHALL fail with a conversion error that excludes the value

### Requirement: Unsupported date patterns are rejected at compile time

The system SHALL validate `date:` pattern text at expression creation time. An unsupported
pattern letter or construct SHALL fail `create_expression`/`create_predicate` with a
`LanguageError::ParseError` naming the offending construct, and format characters SHALL never be
silently dropped. `date:` SHALL NOT travel the language-delegate path, so rejection is a compile
error, not an evaluation-time unknown-language failure. An unknown `date:` command SHALL fail at
creation with a diagnostic naming the accepted commands (`now`, `header.<name>`).

#### Scenario: Unsupported pattern letters fail at creation through the SPI

- **GIVEN** the expressions `${date:now:yyyy-MM-dd G}` and `${date:now:yyyy-MM-dd z}`
- **WHEN** each expression is created through `create_expression`
- **THEN** each creation SHALL fail with a parse error naming the unsupported letter
- **AND** no expression object SHALL be returned

#### Scenario: Unterminated quote fails at creation through the SPI

- **GIVEN** the expression `${date:now:yyyy-MM-dd'THH:mm}`
- **WHEN** the expression is created through `create_expression`
- **THEN** creation SHALL fail with a parse error about the unterminated quote

#### Scenario: Unknown date command diagnostic names the accepted commands

- **GIVEN** the expression `${date:week:yyyy-MM-dd}`
- **WHEN** the expression is created
- **THEN** creation SHALL fail with a parse error whose message names `now` and `header.` as
  the accepted commands

#### Scenario: Bad pattern in a predicate fails at creation

- **GIVEN** the predicate `${date:now:yyyy-MM-dd G} > ${header.low}`
- **WHEN** the predicate is created through `create_predicate`
- **THEN** creation SHALL fail with a parse error naming `G`

#### Scenario: Bare now() call stays rejected

- **GIVEN** the predicate `now() > 1000`
- **WHEN** the predicate is created
- **THEN** creation SHALL still fail with a parse error — the `date:` sub-language does not add a
  `now()` function to the scalar grammar (ADR-0092, as amended by mission 469)

### Requirement: Rhai now_ms is the explicit wall-clock accessor

The system SHALL register a `now_ms()` function on every Rhai engine that returns wall-clock epoch
milliseconds as `i64`. The value SHALL be storable to exchange slots (unlike `timestamp()`, whose
`Instant` result the outbound converter refuses). The function SHALL always be registered;
adoption is opt-in at the call site. `timestamp()` SHALL keep its monotonic `Instant` semantics,
and documentation SHALL distinguish the two, noting that `now_ms()` is wall clock and may step
backward on clock adjustments (NTP). Existing scripts that never call `now_ms()` SHALL keep
deterministic behavior.

#### Scenario: now_ms returns a plausible epoch millisecond count

- **GIVEN** a Rhai expression `now_ms()`
- **WHEN** it is evaluated twice
- **THEN** each result SHALL be an integer within the plausible epoch-millisecond window captured
  around the two evaluations (with bounded scheduling slack)
- **AND** no assertion SHALL require the second value to be greater than or equal to the first

#### Scenario: now_ms is storable outbound

- **GIVEN** a mutating script `headers["t"] = now_ms();`
- **WHEN** the script is evaluated
- **THEN** the header `t` SHALL hold a number and no outbound refusal SHALL fire

### Requirement: MiniJinja templates have no wall-clock access

The system SHALL NOT expose wall-clock time to MiniJinja templates. Templates SHALL render only
exchange-derived context; a route needing a timestamp SHALL compute it with Simple `date:` or
Rhai `now_ms()` and pass it as a header or property. The sandbox surface SHALL not grow.

#### Scenario: No now function or value exists in templates

- **GIVEN** a template `<p>{{ now() }}</p>` (autoescape-wrapped as the engine requires)
- **WHEN** the expression is created and then evaluated against an empty exchange
- **THEN** creation SHALL succeed and evaluation SHALL fail
- **AND** the failure SHALL classify as `EvalFailure` with the existing unknown-function class
  (function-not-found) and a payload-free diagnostic that does not echo template content
- **AND** the documented alternative is passing a computed timestamp via a header or property

## MODIFIED Requirements

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
stay unchanged. The `date:` prefix SHALL be reserved as the native
wall-clock sub-language and SHALL NOT take the delegate path: `date:` slots
SHALL be fully validated at compile time per the "Simple date sub-language"
and "Unsupported date patterns" requirements, even though a literal delegate
language named `date` is not registered.

#### Scenario: Unrecognized slot is a compile error

- **GIVEN** an expression `${unknown}`
- **WHEN** the expression is created
- **THEN** creation SHALL fail with a parse error

#### Scenario: Unknown language is an evaluation failure

- **GIVEN** an expression `${nope:x}` where `nope` is not a registered
  language and is not the reserved `date:` prefix
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

#### Scenario: Reserved date prefix is the compile-time exception

- **GIVEN** an expression `${date:now:yyyy-MM-dd}`
- **WHEN** the expression is created
- **THEN** creation SHALL NOT defer to the delegate path
- **AND** the slot SHALL compile as a native `date` sub-language expression
- **AND** an invalid `date:` payload SHALL be a compile-time parse error, not an
  evaluation-time unknown-language failure
