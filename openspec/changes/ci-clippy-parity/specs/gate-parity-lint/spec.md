## ADDED Requirements

### Requirement: Registry-to-workflow parity lint

The `lint-gate-parity` xtask command SHALL statically verify that the
command of each gate in its checked set equals its mapped workflow
step's `run:` block: `clippy` maps to `.github/workflows/ci.yml`, job
`quality`, step `clippy`; `doc-build` maps to `.github/workflows/docs.yml`,
job `validate`, step `Build Rustdoc`. A gate missing from the
`AGENTS.md ## QUALITY GATES` registry or missing its mapped workflow
step SHALL be a violation. Comparison SHALL normalize each side into an
ordered list of statements — a line ending in `\` joins the next line,
any other newline separates statements, `#` comments are stripped,
whitespace runs collapse, quotes and command separators are preserved —
and SHALL compare the statement lists element-by-element. A `NAME=value`
statement prefix on the registry side SHALL be satisfied when the mapped
step's effective environment (step `env:` over job `env:` over
workflow-level `env:` over unset) supplies the same value, and SHALL be
a violation otherwise; the remaining command text SHALL still match.
The command SHALL be pure file parsing: no cargo builds, no new
dependencies, millisecond runtime. On parity it SHALL print
`lint-gate-parity: OK (<n> gates in parity)` and exit 0; on drift it
SHALL exit 1 with a paste-ready block naming the gate and workflow file
and printing the expected (registry) and actual (workflow) statements,
or the expected and actual effective environment values with their
source.

#### Scenario: Clean tree passes

- **GIVEN** the repository tree where the registry `clippy` and
  `doc-build` commands match their mapped workflow steps, with
  `docs.yml` supplying `RUSTDOCFLAGS="-D warnings"` at workflow level
- **WHEN** `cargo xtask lint-gate-parity` runs
- **THEN** it prints `lint-gate-parity: OK (2 gates in parity)` and
  exits 0

#### Scenario: Clippy flag drift fails with expected and actual

- **GIVEN** a workflow whose `clippy` step lost `--all-targets` while
  the registry keeps it
- **WHEN** `cargo xtask lint-gate-parity` runs
- **THEN** it exits 1 with a message naming gate `clippy` and
  `ci.yml`, and prints the expected registry statements and the actual
  workflow statements ready to paste

#### Scenario: Missing mapped step fails

- **GIVEN** a workflow set where the mapped job has no step named
  `clippy`
- **WHEN** `cargo xtask lint-gate-parity` runs
- **THEN** it exits 1 reporting gate `clippy` as missing its mapped
  workflow step

#### Scenario: Dropped command separator fails

- **GIVEN** a workflow step whose `run:` block merges two of the four
  clippy invocations into one line (separator lost)
- **WHEN** `cargo xtask lint-gate-parity` runs
- **THEN** it exits 1, because statement boundaries are preserved and
  the statement lists differ in length or content

#### Scenario: Comment-only difference passes

- **GIVEN** a workflow step whose `run:` block differs from the
  registry command only in comments or line wrapping
- **WHEN** `cargo xtask lint-gate-parity` runs
- **THEN** no violation is reported, because normalization strips
  comments and joins continuations within statements

#### Scenario: Inherited workflow environment satisfies the registry prefix

- **GIVEN** the registry `doc-build` command carries the prefix
  `RUSTDOCFLAGS="-D warnings"` and `docs.yml` sets the same value at
  workflow level with a bare `cargo doc` step
- **WHEN** `cargo xtask lint-gate-parity` runs
- **THEN** no violation is reported for the environment, and the bare
  command statements match

#### Scenario: Missing or overridden environment fails

- **GIVEN** the registry `doc-build` prefix `RUSTDOCFLAGS="-D warnings"`
  with a workflow that neither sets `RUSTDOCFLAGS` at any level nor
  sets it to a different value
- **WHEN** `cargo xtask lint-gate-parity` runs
- **THEN** it exits 1 printing the expected effective value, the actual
  effective value, and its source (workflow, job, step, or unset)

#### Scenario: Parser unit tests cover fixtures

- **GIVEN** in-module fixture registry blocks and workflow job/step
  texts
- **WHEN** the parity check under test runs against the fixtures
- **THEN** parity, clippy drift, doc-build crate-set drift, missing
  step, separator loss, comment-only, inherited environment, and
  missing-or-overridden environment cases each produce the verdict
  their scenario declares
