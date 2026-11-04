## ADDED Requirements

### Requirement: Rhai limit and timeout details name the tripped limit and its configured bound

The rhai language SHALL attach to every limit-class `EvalFailure.detail`
a stable, operand-free string of the form `<limit-name> <bound>
exceeded`, where `<limit-name>` is lowercase-kebab and matches the
`RhaiLimitsConfig` config key, and `<bound>` is the resolved configured
value (rust-camel default when unconfigured). The mapping SHALL be:
`ErrorTooManyOperations` → `max-operations {max_operations} exceeded`;
`ErrorDataTooLarge` → by exact engine label (the label SHALL be used for
classification only and never rendered): `Length of string` →
`max-string-size {max_string_size} exceeded`; `Size of array/BLOB` or
`Size of BLOB` → `max-array-size {max_array_size} exceeded`;
`Size of object map` → `max-map-size {max_map_size} exceeded`; `ErrorStackOverflow` → `max-call-levels {max_call_levels}
exceeded`. The execution-timeout `Elapsed` mapping SHALL attach
`execution-timeout {execution_timeout_ms}ms exceeded` (class `Timeout`
unchanged); this exact-detail and unchanged-class assertion SHALL cover
all three `Elapsed` mapping sites: ordinary expression and predicate
evaluation, grouped predicate `first_match`, and mutating `evaluate`.
When rhai cannot distinguish which of two depth limits tripped at
compile time, the `ParseError.reason` SHALL name both candidate limits
and both resolved bounds — `max-expression-depth {d} or
max-function-expression-depth {f} exceeded` — listing the configured
candidates without claiming both tripped. When a variant carries no
mappable bound (unconfigured variables/modules limits, unrecognized
data-too-large label), the detail SHALL name the limit only and SHALL
NOT invent a number: all other `ErrorDataTooLarge` labels SHALL produce
exactly `data-size limit exceeded`; defensive variants SHALL produce
exactly `max-variables exceeded` and `max-modules exceeded`. The rhai
engine label on `ErrorDataTooLarge` SHALL be used for classification
only and never rendered. Error classes and public error enums SHALL NOT
change; details SHALL contain no exchange data, no script-operated
values, and no raw rhai payload text.

#### Scenario: Operations trip names the limit and the custom bound

- **GIVEN** a rhai expression configured with `max_operations: Some(500)`
- **WHEN** a loop script exhausts the operations budget and the error is mapped
- **THEN** the `EvalFailure.detail` SHALL be exactly
  `max-operations 500 exceeded` and the class SHALL be `Limit`

#### Scenario: Data-too-large trips map the label to the config key

- **GIVEN** a rhai expression configured with `max_map_size: Some(50)`
  (and separately `max_array_size` / `max_string_size`)
- **WHEN** a script trips the respective size check
- **THEN** the detail SHALL be exactly `max-map-size 50 exceeded`,
  `max-array-size {n} exceeded`, or `max-string-size {n} exceeded`,
  matching the tripped check, with class `Limit`

#### Scenario: Call levels trip names the bound

- **GIVEN** a rhai expression configured with `max_call_levels: Some(8)`
- **WHEN** a recursive script function exceeds the call-level budget
- **THEN** the `EvalFailure.detail` SHALL be exactly
  `max-call-levels 8 exceeded` with class `Limit`

#### Scenario: Timeout names the configured duration

- **GIVEN** a rhai expression configured with `execution_timeout_ms: Some(50)`
  and an operations budget high enough that the timeout wins
- **WHEN** an infinite loop script is evaluated
- **THEN** the error SHALL keep class `Timeout` and carry detail exactly
  `execution-timeout 50ms exceeded`

#### Scenario: Default bound appears in the detail

- **GIVEN** a rhai language with default limits (`RhaiLanguage::new()`)
- **WHEN** a script exhausts the default operations budget
- **THEN** the detail SHALL be exactly `max-operations 100000 exceeded`

#### Scenario: Compile-time depth failures are exact and independently tested

- **GIVEN** a rhai expression configured with `max_expression_depth:
  Some(4)` (function depth left non-binding at its default), and
  separately a script whose function body trips
  `max_function_expression_depth: Some(3)` (top-level depth left
  non-binding)
- **WHEN** each script is compiled (create time)
- **THEN** each compilation SHALL return a `ParseError` whose reason is
  exactly `max-expression-depth {d} or max-function-expression-depth
  {f} exceeded` with the resolved values of both settings, and the
  error SHALL remain a `ParseError`

#### Scenario: Unrecognized labels and defensive variants stay name-only

- **GIVEN** the label mapper is exercised with the four recognized
  labels (`Length of string`, `Size of array/BLOB`, `Size of BLOB`,
  `Size of object map`), unknown labels containing category keywords
  plus a secret sentinel, and the defensive variables/modules variants
- **WHEN** each is mapped
- **THEN** recognized labels SHALL produce the contracted
  `<limit-name> {bound} exceeded` string; unknown labels SHALL produce
  exactly `data-size limit exceeded`; defensive variants SHALL produce
  exactly `max-variables exceeded` and `max-modules exceeded`; every
  mapped error SHALL keep class `Limit` and none SHALL contain the
  sentinel or the raw label text

#### Scenario: Details stay operand-free

- **GIVEN** a script whose tripped values (map keys, loop counters)
  contain distinctive text
- **WHEN** any covered limit trips
- **THEN** the rendered detail SHALL equal the contracted string exactly
  (exact-equality asserts no exchange data, script source, or rhai
  payload text is appended)
