# tar-splitter Specification

## Purpose
TAR and TAR.GZ stream splitting. The stream-split format contract and compiler
expose TAR and TAR.GZ alongside the preserved ZIP behavior.

## Requirements
### Requirement: stream split format selection

The system SHALL expose TAR and TAR.GZ in the existing stream-split format contract and compiler while preserving ZIP behavior.

#### Scenario: compiler selects TAR format

- **GIVEN** a stream split configuration naming TAR
- **WHEN** the route is compiled
- **THEN** the compiler constructs the TAR splitter without changing ZIP selection or semantics

