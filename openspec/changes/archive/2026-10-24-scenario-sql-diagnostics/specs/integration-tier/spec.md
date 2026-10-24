## ADDED Requirements

### Requirement: Scenario console SQLx verbosity

When the scenario harness owns the tracing subscriber, its console formatter SHALL emit SQLx events at INFO and higher severities.
It SHALL omit SQLx DEBUG and TRACE events from the console.
Other targets SHALL retain their existing TRACE-through-ERROR console output.
The harness SHALL apply this filter only to the formatter and SHALL preserve every event in the capture layer.
It SHALL NOT introduce an ambient configuration read for this policy.

#### Scenario: ordinary SQL statements do not reach the console

- **GIVEN** a SQL prepare action that emits SQLx DEBUG statement events
- **WHEN** `camel test` runs the document
- **THEN** the action succeeds and the console omits those statement events

#### Scenario: filtered SQL events still reach log assertions

- **GIVEN** a SQL action, `logs.noLevelAbove: trace`, and ambient `RUST_LOG=off`
- **WHEN** SQLx emits a DEBUG query event during the action
- **THEN** the document fails with a logs verdict naming `DEBUG sqlx::query`
- **AND** the console formatter omits the raw SQL statement

#### Scenario: warning events remain visible

- **GIVEN** a SQLx event at WARN severity
- **WHEN** the scenario harness receives it
- **THEN** the event remains eligible for console output and captured assertions

### Requirement: SQL scenario action labels

The CLI SHALL label a SQL prepare action as `scenario[i] sql`, where `i` is its zero-based scenario index.

#### Scenario: SQL preparation has a specific result row

- **WHEN** the first action prepares SQL state and the second action validates that state
- **THEN** their result rows are `scenario[0] sql` and `scenario[1] validate`
