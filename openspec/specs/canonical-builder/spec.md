# canonical-builder Specification

## Purpose
TBD - created by archiving change audit-fix-docdrift-t1-baseline. Update Purpose after archive.
## Requirements
### Requirement: Canonical spec version references in error messages

The canonical route builder SHALL emit error messages that reference the
current canonical spec version. Per ADR-0016 the current version is v2; the
superseded v1 strings are stale and SHALL be corrected.

#### Scenario: unsupported step reports current version

- **GIVEN** a declarative route contains a `processor` step, which the
  canonical builder rejects
- **WHEN** `build_canonical()` compiles the route
- **THEN** the error message references `canonical v2`, not `canonical v1`

#### Scenario: unsupported split aggregation reports current version

- **GIVEN** a declarative route uses a custom split aggregation strategy that
  the canonical builder rejects
- **WHEN** `build_canonical()` compiles the route
- **THEN** the error message references `canonical v2`, not `canonical v1`

#### Scenario: test assertions track the corrected version

- **GIVEN** the canonical builder error-message strings are corrected to v2
- **WHEN** `cargo test -p camel-builder` runs
- **THEN** the assertions in `tests/canonical_spec_test.rs` and the inline
  tests in `lib.rs` pass against the v2 strings

