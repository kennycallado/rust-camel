## ADDED Requirements

### Requirement: WIT package version

The system SHALL declare `package camel:plugin@1.0.0;` in every WIT file
that belongs to the `camel:plugin` package.

#### Scenario: Canonical WIT files carry the version

- **GIVEN** the four canonical WIT files in `crates/camel-wit/wit/`
- **WHEN** a build or test reads any of them
- **THEN** each file's first non-comment, non-blank line contains
  `package camel:plugin@1.0.0;`

#### Scenario: Host-copy WIT files carry the version

- **GIVEN** the three host-copy WIT files in
  `crates/components/camel-component-wasm/wit/`
- **WHEN** `cargo build -p camel-component-wasm` runs
- **THEN** the build succeeds and each file declares
  `package camel:plugin@1.0.0;`

#### Scenario: Example WIT files carry the version

- **GIVEN** the eight example `.wit` files across `examples/`
- **WHEN** any example is compiled
- **THEN** each file declares `package camel:plugin@1.0.0;`

### Requirement: No WIT-006 markers

The system SHALL NOT contain any `TODO(WIT-006)` markers in source files
after versioning is applied.

#### Scenario: Negative search for WIT-006

- **GIVEN** the full source tree excluding `target/`, `.worktrees/`,
  `docs/archived/`, and `docs/audits/` (historical audit records are
  immutable snapshots)
- **WHEN** `grep -r 'TODO(WIT-006)'` runs
- **THEN** zero matches are found

### Requirement: camel-wit zero-dependency leaf

The system SHALL keep `camel-wit` free of runtime dependencies after dead
code removal.

#### Scenario: Cargo.toml has no dependencies

- **GIVEN** `crates/camel-wit/Cargo.toml` after dead code removal
- **WHEN** the `[dependencies]` section is read
- **THEN** it is empty or absent (zero entries)

#### Scenario: No external caller references removed symbols

- **GIVEN** the full source tree excluding `camel-wit/src/lib.rs` itself
- **WHEN** `grep` searches for `WitHost`, `APPLICATION_JSON`,
  `TEXT_PLAIN`, `APPLICATION_OCTET_STREAM`, `TEXT_HTML`, `APPLICATION_XML`,
  `APPLICATION_FORM_URLENCODED`, and `wit_dir`
- **THEN** zero matches are found outside `camel-wit/src/lib.rs`

### Requirement: Cross-crate WIT comparison test is unconditional

The system SHALL enforce WIT file consistency between canonical and host
copies without an existence-guard escape hatch, covering all three
host-copy files.

#### Scenario: No if-exists guard in comparison test

- **GIVEN** the `test_host_wit_matches_canonical` test in
  `camel-wit/src/lib.rs`
- **WHEN** the test source is read
- **THEN** it does NOT contain `if !.*exists` or `if !.*exists { return; }`
  patterns

#### Scenario: All three host-copy files compared

- **GIVEN** the `test_host_wit_matches_canonical` test
- **WHEN** the test runs
- **THEN** it compares `camel-plugin.wit`, `camel-bean.wit`, AND
  `camel-source.wit` between canonical and host directories (not just one
  file)

#### Scenario: Hard-fail on missing host directory

- **GIVEN** the host-copy WIT directory at
  `crates/components/camel-component-wasm/wit/`
- **WHEN** the comparison test runs and the directory does not exist
- **THEN** the test fails (panics or assertion error) rather than silently
  passing via an early return
