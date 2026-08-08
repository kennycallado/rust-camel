# ignore-test-policy Specification

## Purpose
TBD - created by archiving change audit-fix-ignore-test-policy. Update Purpose after archive.
## Requirements
### Requirement: `#[ignore]` reason vocabulary

Every `#[ignore]` test attribute in source files under `crates/` and
`examples/` SHALL carry a reason string (`#[ignore = "..."]`) whose
value starts with one of exactly two prefixes drawn from a closed
vocabulary. Each prefix has a specific delimiter and requires a
non-empty detail string:

| Prefix | Grammar | Meaning |
|--------|---------|---------|
| `requires pre-built ` | `requires pre-built <non-empty artifact detail>` | buildable artifact covered by CI job |
| `slow test: ` | `slow test: <non-empty description>` | slow but self-contained |

The prefix match is case-sensitive (all lowercase). A near-prefix that
does not exactly match one of the two (e.g. `requires livewire`,
`Requires pre-built`, `requires prebuilt`) SHALL be rejected.

External-service tests (requiring Kafka, Redis, OpenSearch, K8s, etc.)
SHALL NOT use `#[ignore]`. They MUST follow the established
`camel-test` + `--features integration-tests` + testcontainers pattern.
The lint SHALL treat `requires live` as a migration error, not an
accepted prefix.

Files under `scripts/`, `bridges/`, `target/`, and `.worktrees/` are
excluded from the scan — these directories contain meta-tooling, build
infrastructure, or generated code, not component test code.

#### Scenario: Bare `#[ignore]` is rejected

- **GIVEN** a Rust source file under `crates/` containing `#[ignore]`
  without a `= "..."` reason string
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation on that line and exits non-zero

#### Scenario: Reason with valid `requires pre-built` passes

- **GIVEN** a Rust source file containing
  `#[ignore = "requires pre-built guest wasm"]` in a file listed in
  the allowlist
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports no violation for that line

#### Scenario: Reason with valid `slow test:` passes

- **GIVEN** a Rust source file containing
  `#[ignore = "slow test: file polling (run with --ignored)"]`
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports no violation for that line

#### Scenario: Reason with unrecognized prefix is rejected

- **GIVEN** a Rust source file containing
  `#[ignore = "because reasons"]`
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation naming the allowed prefixes and
  exits non-zero

#### Scenario: `requires live` is a migration error

- **GIVEN** a Rust source file containing
  `#[ignore = "requires live Kafka at localhost:9092"]`
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a migration error instructing the
  contributor to move the test to `camel-test` behind
  `--features integration-tests` with testcontainers (see ADR-0054)

#### Scenario: Near-prefix typo is rejected

- **GIVEN** a Rust source file containing
  `#[ignore = "requires livewire foo"]`
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation — `requires livewire` is not a
  valid prefix

#### Scenario: Case-sensitive prefix matching

- **GIVEN** a Rust source file containing
  `#[ignore = "Requires pre-built wasm"]` (capital R)
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation — all prefixes MUST be lowercase

#### Scenario: Wrong delimiter for `requires pre-built` is rejected

- **GIVEN** a Rust source file containing
  `#[ignore = "requires pre-built: guest wasm"]` (colon instead of
  space after prefix)
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation — `requires pre-built` requires
  a space delimiter, not a colon

#### Scenario: `slow test` without colon delimiter is rejected

- **GIVEN** a Rust source file containing
  `#[ignore = "slow test file polling"]` (missing colon)
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation — `slow test` requires a colon
  delimiter (`slow test: <detail>`)

#### Scenario: Prefix without detail is rejected

- **GIVEN** a Rust source file containing
  `#[ignore = "requires pre-built"]`
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation — the prefix MUST be followed by
  a non-empty detail string

#### Scenario: Meta-tooling directories are excluded

- **GIVEN** a Rust source file under `scripts/xtask/src/` containing
  bare `#[ignore]`
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint does not scan that file and reports no violation

### Requirement: `requires pre-built` allowlist coupling

A `#[ignore]` test whose reason starts with `requires pre-built` SHALL
only be permitted if the test file containing it is listed in
`scripts/xtask/allowlist-ignore.txt`. The allowlist SHALL be
bidirectionally verified:

1. **Forward check:** every `requires pre-built` test must be in the
   allowlist.
2. **Reverse check:** every allowlist entry must (a) be a direct-child
   `.rs` file of `crates/components/camel-component-wasm/tests/` (not
   nested), (b) exist on disk, and (c) contain at least one
   `requires pre-built` test. The filename (without `.rs`) uniquely maps
   to a cargo integration-test target name.
3. **Mixed-reason invariant:** every `#[ignore]` test in an allowlisted
   file MUST use the `requires pre-built` prefix.

The CI `wasm-integration` job SHALL consume the same allowlist to
determine which test files require pre-built WASM artifacts.

#### Scenario: `requires pre-built` in allowlist passes

- **GIVEN** a test file
  `crates/components/camel-component-wasm/tests/source_integration.rs`
  containing `#[ignore = "requires pre-built guest wasm"]`
- **AND** the file path is listed in
  `scripts/xtask/allowlist-ignore.txt`
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports no violation

#### Scenario: `requires pre-built` not in allowlist is rejected

- **GIVEN** a test file `crates/foo/tests/bar.rs` containing
  `#[ignore = "requires pre-built artifact"]`
- **AND** the file path is NOT listed in
  `scripts/xtask/allowlist-ignore.txt`
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation

#### Scenario: Allowlist entry outside WASM scope is rejected

- **GIVEN** `scripts/xtask/allowlist-ignore.txt` contains an entry for
  `crates/some-other-crate/tests/foo.rs`
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation

#### Scenario: Stale allowlist entry is rejected

- **GIVEN** `scripts/xtask/allowlist-ignore.txt` contains a path that
  exists but no longer has any `requires pre-built` tests
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation

#### Scenario: Mixed-reason file in allowlist is rejected

- **GIVEN** an allowlisted test file contains both
  `#[ignore = "requires pre-built guest wasm"]` and
  `#[ignore = "slow test: foo"]`
- **WHEN** `cargo xtask lint-ignore` runs
- **THEN** the lint reports a violation

### Requirement: CI `wasm-integration` job

The CI pipeline SHALL include a `wasm-integration` job that builds all
WASM guest crates for the `wasm32-wasip2` target, and runs the
buildable `#[ignore]` tests from `camel-component-wasm`. The tests
resolve guest `.wasm` artifacts from the Cargo target directory
(`target/wasm32-wasip2/debug/`), so no manual artifact copy is needed.
The job SHALL derive test targets from `allowlist-ignore.txt` (not
hardcoded) via `cargo test -p camel-component-wasm --test <target> --
--ignored`, ensuring the allowlist-to-CI contract is bidirectional.

#### Scenario: WIT version breakage fails CI via compilation

- **GIVEN** a PR that changes the `camel:plugin` WIT package version in
  a way that breaks the guest's `wit_bindgen::generate!` macro
- **WHEN** the `wasm-integration` CI job's build step runs
- **THEN** the guest fails to compile against the new bindings and the
  build step exits non-zero

#### Scenario: WIT version breakage fails CI via runtime test

- **GIVEN** a PR that changes the WIT interface in a way that compiles
  but changes the runtime contract
- **WHEN** the `wasm-integration` CI job's test step runs
- **THEN** the `--ignored` WASM tests fail due to host-guest contract
  mismatch

#### Scenario: Green on unmodified main

- **GIVEN** the `main` branch with all WASM guests compiling cleanly
- **WHEN** the `wasm-integration` CI job runs
- **THEN** all `--ignored` WASM tests pass

### Requirement: CI integration tests for external services

The CI pipeline SHALL run the existing `camel-test` testcontainers-based
integration tests (Kafka, Redis, OpenSearch, Kubernetes) in the
`full-tests-linux` job via `cargo test -p camel-test --features
integration-tests --test <target>`. These tests self-provision their
dependencies via testcontainers and require Docker (already available
in that job).

#### Scenario: External-service integration tests run in CI

- **GIVEN** the `full-tests-linux` CI job
- **WHEN** the job executes
- **THEN** `cargo test -p camel-test --features integration-tests --test
  kafka_test`, `redis_test`, `opensearch_test`, and `kubernetes_test`
  all run and pass on unmodified main

### Requirement: `lint-ignore` in quality gates

The `lint-ignore` command SHALL be registered in both `AGENTS.md`
QUALITY GATES and the `ci.yml` quality job, alongside the existing
`lint-unwrap`, `lint-secrets`, and `lint-non-exhaustive` gates.

#### Scenario: Quality gate runs lint-ignore

- **GIVEN** the `ci.yml` quality job definition
- **WHEN** the job executes
- **THEN** `cargo xtask lint-ignore` runs and fails the job if any
  violation is found

