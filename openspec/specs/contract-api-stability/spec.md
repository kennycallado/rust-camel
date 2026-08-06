# contract-api-stability Specification

## Purpose
TBD - created by archiving change non-exhaustive-contract-enforcement. Update Purpose after archive.
## Requirements
### Requirement: xtask lint-non-exhaustive enforces ADR-0049

The workspace SHALL provide a `cargo xtask lint-non-exhaustive` command that
fails when a `pub enum` in a contract crate lacks both `#[non_exhaustive]` and
an `exhaustive-by-contract` note, mirroring the
`lint-unwrap`/`lint-secrets`/`lint-log-levels` pattern.

#### Scenario: Lint passes on compliant crates

- **GIVEN** every `pub enum` in the three contract crates carries
  `#[non_exhaustive]` or the exception note
- **WHEN** `cargo xtask lint-non-exhaustive` is run
- **THEN** it exits 0 and prints "lint-non-exhaustive: OK (no violations)"

#### Scenario: Lint fails on a non-compliant enum

- **GIVEN** a `pub enum` in a contract crate that has neither
  `#[non_exhaustive]` nor the exception note
- **WHEN** `cargo xtask lint-non-exhaustive` is run
- **THEN** it exits non-zero and reports the offending file:line

#### Scenario: Lint rejects an invalid exception marker

- **GIVEN** a `pub enum` in a contract crate preceded by a marker that is NOT a
  valid exception — a plain `// exhaustive-by-contract` comment (non-rustdoc),
  an empty rationale (`/// exhaustive-by-contract:` with nothing after the
  colon), or a marker detached from the item
- **WHEN** `cargo xtask lint-non-exhaustive` is run
- **THEN** it exits non-zero (the marker does not satisfy ADR-0049 §Rule 3)

#### Scenario: Lint ignores authoring and schema crates

- **GIVEN** a `pub enum` in `camel-dsl`, `camel-config`, `camel-cli`, or
  `camel-builder` without `#[non_exhaustive]`
- **WHEN** `cargo xtask lint-non-exhaustive` is run
- **THEN** it is not flagged (out of scope per ADR-0049 §Scope)

#### Scenario: AGENTS.md documents the new gate

- **GIVEN** the AGENTS.md QUALITY GATES block
- **WHEN** it is inspected
- **THEN** a `lint-non-exhaustive` step is listed alongside
  `lint-unwrap`/`lint-secrets`/`lint-log-levels`

