# credential-lint Specification

## Purpose
TBD - created by archiving change audit-fix-secret-leak-lint. Update Purpose after archive.
## Requirements
### Requirement: Credential derive consistency lint

The `lint-secrets` xtask SHALL inspect struct and enum definitions for
ADR-0051 derive consistency using AST parsing. A type annotated with
`/// ADR-0051 credential boundary: <classification>` must comply with
the derive rules for its classification.

#### Scenario: manual-redaction type derives Debug

- **GIVEN** a struct annotated `/// ADR-0051 credential boundary: manual-redaction`
- **WHEN** the struct has `#[derive(Debug)]`
- **THEN** the lint reports a violation with the file path, line number, and rule name `credential-derive: manual-redaction forbids Debug`

#### Scenario: manual-redaction type derives Serialize

- **GIVEN** a struct annotated `/// ADR-0051 credential boundary: manual-redaction`
- **WHEN** the struct has `#[derive(Serialize)]`
- **THEN** the lint reports a violation with rule name `credential-derive: manual-redaction forbids Serialize`

#### Scenario: manual-redaction type correctly redacted

- **GIVEN** a struct annotated `/// ADR-0051 credential boundary: manual-redaction`
- **WHEN** the struct has no `#[derive(Debug)]` and no `#[derive(Serialize)]`
- **THEN** the lint reports no violation for that struct

#### Scenario: redacting-wrapper derives Debug safely

- **GIVEN** a struct annotated `/// ADR-0051 credential boundary: redacting-wrapper`
- **WHEN** the struct has `#[derive(Debug)]` but no `#[derive(Serialize)]`
- **THEN** the lint reports no violation

#### Scenario: redacting-wrapper derives Serialize

- **GIVEN** a struct annotated `/// ADR-0051 credential boundary: redacting-wrapper`
- **WHEN** the struct has `#[derive(Serialize)]`
- **THEN** the lint reports a violation with rule name `credential-derive: redacting-wrapper forbids Serialize`

#### Scenario: protocol-dto derives Serialize safely

- **GIVEN** a struct annotated `/// ADR-0051 credential boundary: protocol-dto`
- **WHEN** the struct has `#[derive(Serialize)]` but no `#[derive(Debug)]`
- **THEN** the lint reports no violation

#### Scenario: protocol-dto derives Debug

- **GIVEN** a struct annotated `/// ADR-0051 credential boundary: protocol-dto`
- **WHEN** the struct has `#[derive(Debug)]`
- **THEN** the lint reports a violation with rule name `credential-derive: protocol-dto forbids Debug`

#### Scenario: Zeroizing field without classification

- **GIVEN** a struct with a field of type `Zeroizing<String>` and no `/// ADR-0051 credential boundary:` annotation
- **WHEN** the lint inspects the struct
- **THEN** the lint reports a violation with rule name `credential-derive: Zeroizing field requires manual-redaction classification`

#### Scenario: qualified-path Zeroizing field without classification

- **GIVEN** a struct with a field of type `zeroize::Zeroizing<String>` and no `/// ADR-0051 credential boundary:` annotation
- **WHEN** the lint inspects the struct
- **THEN** the lint reports a violation with rule name `credential-derive: Zeroizing field requires manual-redaction classification`

#### Scenario: Zeroizing field with manual-redaction classification

- **GIVEN** a struct with a field of type `Zeroizing<String>` annotated `/// ADR-0051 credential boundary: manual-redaction`
- **WHEN** the struct has no `#[derive(Debug)]` and no `#[derive(Serialize)]`
- **THEN** the lint reports no violation

#### Scenario: unannotated type with credential-suggesting field name

- **GIVEN** a struct with a field named `client_key_path` and no `/// ADR-0051 credential boundary:` annotation
- **WHEN** the lint inspects the struct
- **THEN** the lint reports no violation (field-name heuristic is not used)

#### Scenario: multiline derive attribute

- **GIVEN** a struct annotated `/// ADR-0051 credential boundary: manual-redaction`
- **WHEN** the derive attribute spans multiple lines (e.g., `#[derive(\n    Debug,\n    Clone,\n)]`)
- **THEN** the lint correctly detects the `Debug` derive and reports a violation

#### Scenario: unannotated type without Zeroizing fields

- **GIVEN** a struct or enum with no `/// ADR-0051 credential boundary:` annotation and no `Zeroizing<...>` fields
- **WHEN** the lint inspects the struct
- **THEN** the lint reports no violation (classification is opt-in, not inferred)

### Requirement: Closed-vocabulary classification enforcement

The `lint-secrets` xtask SHALL reject unknown, malformed, or conflicting
duplicate classifications. Only `manual-redaction`, `redacting-wrapper`,
and `protocol-dto` are valid classifications.

#### Scenario: unknown classification value

- **GIVEN** a struct annotated `/// ADR-0051 credential boundary: unknown-value`
- **WHEN** the lint inspects the struct
- **THEN** the lint reports a violation with rule name `credential-derive: unknown classification 'unknown-value'`

#### Scenario: malformed classification attribute

- **GIVEN** a struct annotated `/// ADR-0051 credential boundary:` with no classification value after the colon
- **WHEN** the lint inspects the struct
- **THEN** the lint reports a violation with rule name `credential-derive: malformed classification (missing value)`

#### Scenario: conflicting duplicate classification

- **GIVEN** a struct with two doc comments: `/// ADR-0051 credential boundary: manual-redaction` and `/// ADR-0051 credential boundary: protocol-dto`
- **WHEN** the lint inspects the struct
- **THEN** the lint reports a violation with rule name `credential-derive: conflicting duplicate classifications`

### Requirement: Parse-failure hard-fail

The `lint-secrets` xtask SHALL report a hard failure when `syn::parse_file`
cannot parse a `.rs` source file. Parse failures must not silently skip
enforcement.

#### Scenario: unparseable source file

- **GIVEN** a `.rs` file with invalid Rust syntax that `syn::parse_file` rejects
- **WHEN** the lint scans the workspace
- **THEN** the lint reports the parse error with file path and error message, and exits non-zero

### Requirement: lint-secrets exit code

The `lint-secrets` xtask SHALL exit with a non-zero status code when any
credential derive violation is found, consistent with existing sink-pattern
violations.

#### Scenario: violations present

- **GIVEN** one or more credential derive violations exist in the workspace
- **WHEN** `cargo xtask lint-secrets` runs
- **THEN** the command exits with status code 1 and prints each violation

#### Scenario: no violations

- **GIVEN** zero credential derive violations and zero sink-pattern violations
- **WHEN** `cargo xtask lint-secrets` runs
- **THEN** the command exits with status code 0 and prints `lint-secrets: OK`

