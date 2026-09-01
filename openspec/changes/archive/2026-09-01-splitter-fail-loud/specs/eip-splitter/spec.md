## ADDED Requirements

### Requirement: Split fails loud on wrong body type

The splitter SHALL return a typed error when the exchange body type does not match the split expression input contract. The error SHALL be `CamelError::TypeConversionFailed`. This SHALL apply to the eager splitter service, the split segment, and the declarative split compiler.

#### Scenario: BodyLines rejects a JSON body

- **GIVEN** an exchange with `Body::Json` and the `body_lines` split expression
- **WHEN** the splitter runs
- **THEN** it returns `TypeConversionFailed` and the message contains `body_lines`, the received type name `json`, the expected `text`, and the phrase `add an unmarshal step before split`

#### Scenario: BodyJsonArray rejects a text body

- **GIVEN** an exchange with `Body::Text` and the `body_json_array` split expression
- **WHEN** the splitter runs
- **THEN** it returns `TypeConversionFailed` and the message contains `body_json_array`, the received type name `text`, the expected `json (array)`, and the phrase `add an unmarshal step before split`

#### Scenario: BodyJsonArray rejects a non-array JSON body

- **GIVEN** an exchange with `Body::Json(Object)` and the `body_json_array` split expression
- **WHEN** the splitter runs
- **THEN** it returns `TypeConversionFailed` and the message contains `json (non-array)` and the unmarshal phrase

#### Scenario: Declarative split matches the eager splitter

- **GIVEN** a declarative split step whose expression evaluates to a `Value` that is neither `String` nor `Array`
- **WHEN** the compiled split segment runs
- **THEN** the outcome is `Failed` with `TypeConversionFailed`, the message contains `declarative split`, the received value type name, the expected `text or array`, and the unmarshal phrase, and the original exchange is not cloned as a single fragment

#### Scenario: Split segment error is a failure, not a stop

- **GIVEN** a split segment whose expression returns the typed error
- **WHEN** the segment runs
- **THEN** the outcome is `Failed` and carries the typed error

### Requirement: Empty content keeps pass-through

The splitter SHALL return the original exchange unchanged when the body is empty or the split yields zero fragments over correct-type content.

#### Scenario: Empty body passes through

- **GIVEN** an exchange with `Body::Empty`
- **WHEN** the splitter runs with any built-in expression
- **THEN** it returns the original exchange with success semantics

#### Scenario: Empty text yields zero fragments

- **GIVEN** an exchange with `Body::Text("")` and the `body_lines` expression
- **WHEN** the splitter runs
- **THEN** it returns the original exchange unchanged

#### Scenario: Empty array yields zero fragments

- **GIVEN** an exchange with `Body::Json([])` and the `body_json_array` expression
- **WHEN** the splitter runs
- **THEN** it returns the original exchange unchanged

### Requirement: Streaming splitter uses the typed error

The streaming splitter SHALL return `TypeConversionFailed` with the mandated message when the body is not `Body::Stream`.

#### Scenario: Streaming splitter rejects a non-stream body

- **GIVEN** an exchange with `Body::Text` and the streaming splitter
- **WHEN** the splitter pulls its first item
- **THEN** it returns `TypeConversionFailed` and the message contains `streaming split`, the received type name `text`, the expected `stream`, and the phrase `add an unmarshal step before split`

### Requirement: Split error diagnostics carry no payload

A split type error SHALL name the expression kind, the received body variant, the expected type, and an unmarshal remediation hint. The error message SHALL NOT contain body content.

#### Scenario: Error message omits payload bytes

- **GIVEN** an exchange whose body holds the marker string `SECRET-8f31a` and a mismatched split expression
- **WHEN** the split fails
- **THEN** the error message contains the four diagnostic fields and does not contain `SECRET-8f31a`
