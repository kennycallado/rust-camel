## MODIFIED Requirements

### Requirement: Rhai JSON errors follow the existing classifier

An unavailable operation SHALL be classified by the engine's existing
classifier. `JsonNumber` arithmetic (`+`, `+=`) SHALL fail as
`function-not-found`; `values` and `merge` SHALL fail as `function-not-found`;
an unsupported `for` iteration SHALL fail as `runtime` (ErrorFor). Every
registered wrapper comparison SHALL fail as `type-mismatch`.

A `parse_json` call whose argument is not a string SHALL resolve and SHALL fail
as `type-mismatch`, naming `string` as the expected type and the argument's
static Rhai type name (`blob`, `map`, `array`, `json value`, `json number`,
`i64`, `f64`, `bool`, `()`, `char`, `FnPtr`, `Instant`) as the actual type, at
the script call position. Such a call SHALL NEVER be classified as
`function-not-found`. The `parse_json` exact-type guards and the wrapper
comparisons are the only failing overloads added.

A size or depth violation SHALL be `limit`. A malformed parse
SHALL be `parse` with no input text and no parser line or column. A setter type
failure SHALL be `type-mismatch` and SHALL NEVER be `ErrorIndexingType`, because
Rhai silently discards indexing errors during index-chain write-back. No error
SHALL contain exchange data.

#### Scenario: Arithmetic and container helpers are function-not-found

- **GIVEN** a `JsonNumber` and a parsed object
- **WHEN** a script evaluates `j + 1`, `j += 1`, `obj.values()`, or
  `obj.merge(x)`
- **THEN** each SHALL fail with a `function-not-found` error

#### Scenario: Non-string parse_json input is a typed mismatch

- **GIVEN** a script step `let data = parse_json(body)` where `body` binds to a
  `Body::Bytes` (Rhai `Blob`) or a `Body::Json` object (native map)
- **WHEN** the step evaluates
- **THEN** it SHALL fail with `expected string, actual blob` or
  `expected string, actual map` at the position of the `parse_json` call
- **AND** the error SHALL NEVER be `function-not-found`
- **AND** the error text SHALL contain no body bytes, no parsed values, and no
  blob hex

#### Scenario: Iteration is a runtime error

- **GIVEN** a parsed object
- **WHEN** a script iterates it with `for`
- **THEN** the step SHALL fail with a `runtime` error (ErrorFor)

#### Scenario: Setter failure propagates

- **GIVEN** a parsed object
- **WHEN** a script assigns a function pointer to a key
- **THEN** the step SHALL fail with a `type-mismatch` error; the failure SHALL
  NOT be an `ErrorIndexingType` and SHALL NOT be discarded

#### Scenario: Parse error is redacted

- **GIVEN** a script that calls `parse_json` on malformed text holding secret
  data
- **WHEN** the script runs
- **THEN** the error SHALL NOT contain the input text or a parser line/column;
  it SHALL carry only the class and the script call position
