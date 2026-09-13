## ADDED Requirements

### Requirement: Documentation matches current behavior

Repository documentation SHALL describe the current behavior, source layout,
feature configuration, and component metric labels without changing runtime
behavior.

#### Scenario: DSL comments explain format-specific null handling

- **GIVEN** the route AST comment describes `parameters: null`
- **WHEN** a reader follows the explanation for serde_json and noyalib
- **THEN** it states that serde_json rejects null up front and noyalib routes
  YAML null through `deserialize_map` to `visit_map` over an empty access,
  while direct-deserializer tests pin the `visit_unit`/`visit_none` arms

#### Scenario: DSL comments do not claim an existing type is omitted

- **GIVEN** `ResequenceStreamYaml` is defined in `route_ast.rs`
- **WHEN** the surrounding comments are read
- **THEN** no comment claims that `ResequenceStreamYaml` is intentionally
  omitted

#### Scenario: Testing build table distinguishes feature sets

- **GIVEN** a reader uses `docs/src/testing/index.md`
- **WHEN** the reader compares the no-default-features and default builds
- **THEN** the table identifies the featureless build separately and reflects
  the current default integration-http and integration-sql features

#### Scenario: Processor catalog citations are source-accurate

- **GIVEN** a processor catalog entry cites `src/lib.rs:N`
- **WHEN** the corresponding module declaration is inspected
- **THEN** N is the actual declaration line for that module

#### Scenario: Component context pages document current b-prime labels

- **GIVEN** the WS, file, or timer component context page is read
- **WHEN** its documented metric labels are compared with source
- **THEN** it documents `b-prime:ws:message-dispatch`,
  `b-prime:file:poll-send`, or `b-prime:timer:fire-send` respectively

#### Scenario: Docs-only scope excludes test harness extraction

- **GIVEN** the docsweep diff is reviewed
- **WHEN** changed paths are enumerated
- **THEN** no component test harness or production implementation is changed
