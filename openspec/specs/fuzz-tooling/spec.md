# fuzz-tooling Specification

## Purpose
TBD - created by archiving change fuzzing-mutation-tooling. Update Purpose after archive.
## Requirements
### Requirement: Fuzz execution isolation

The `cargo xtask fuzz` subcommand SHALL run fuzz targets only inside a
linked git worktree and SHALL direct every build, corpus, and crash-artifact
write to `target-fuzz/` inside that worktree. The crash-artifact location
SHALL be pinned with `-artifact_prefix` so libFuzzer never writes to
`fuzz/artifacts/`.

#### Scenario: refused in the main checkout

- **GIVEN** the current directory resolves to the main checkout, where
  `git rev-parse --git-dir` equals `git rev-parse --git-common-dir`
- **WHEN** the operator runs `cargo xtask fuzz dsl_yaml`
- **THEN** the command exits non-zero with a one-line reason that names the
  worktree requirement, and no build or fuzz process starts

#### Scenario: isolated target directory in a worktree

- **GIVEN** the operator works in a linked worktree
- **WHEN** `cargo xtask fuzz dsl_yaml --time 60` completes
- **THEN** all new files live under `<worktree>/target-fuzz/`, and the main
  checkout's `./target` directory shows no new writes

#### Scenario: crash artifacts stay inside target-fuzz

- **GIVEN** a fuzz run in a linked worktree that finds a crash
- **WHEN** libFuzzer writes the crash artifact
- **THEN** the artifact lands under `<worktree>/target-fuzz/artifacts/<target>/`
  because the wrapper passed `-artifact_prefix`, and no `fuzz/artifacts/`
  directory is created

### Requirement: dsl_yaml fuzz harness

The `dsl_yaml` fuzz target SHALL feed arbitrary bytes as UTF-8 text to
`camel_dsl::yaml` route parsing with
`camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD` (`128 * 1024`)
and `SecurityCompileContext::default()` as arguments, and SHALL hold the
invariant that parsing never panics.

#### Scenario: malformed YAML returns an error instead of panicking

- **GIVEN** the fuzzer generates truncated, deeply nested, or alias-heavy
  YAML bytes
- **WHEN** the harness converts the bytes to a string and parses them
- **THEN** the parse call returns either `Ok` or `Err`, and the fuzz target
  reports no crash

#### Scenario: invalid UTF-8 input is skipped

- **GIVEN** the fuzzer generates byte sequences that are not valid UTF-8
- **WHEN** the harness runs
- **THEN** the harness returns early without calling the parser and reports
  no crash

### Requirement: Committed seed corpus

The repository SHALL commit a curated seed corpus for `dsl_yaml` derived
from existing camel-dsl adversarial test inputs and audit regression
inputs, and the xtask wrapper SHALL copy the seeds into the corpus
directory before the first run.

#### Scenario: first run starts from committed seeds

- **GIVEN** a worktree where `target-fuzz/corpus/dsl_yaml/` does not exist
- **WHEN** the operator runs `cargo xtask fuzz dsl_yaml`
- **THEN** the wrapper creates the corpus directory, copies every committed
  seed file into it, and the fuzz run reports those inputs as covered

#### Scenario: seed set contains the audit's adversarial shapes

- **GIVEN** the committed seed directory `fuzz/seeds/dsl_yaml/`
- **WHEN** the seed files are inspected
- **THEN** the set includes at least one YAML alias-bomb input, one
  deep-nesting input beyond the parser's recursion budget, and one input
  taken from an existing camel-dsl malformed-route test

### Requirement: Crash minimization and promotion

When a fuzz run finds a crash, the xtask wrapper SHALL minimize the input
with `cargo +nightly fuzz tmin <target> <artifact>`, report the minimized
input path, and instruct the operator to add a committed Rust regression
test. Raw crash artifacts SHALL NOT be committed.

#### Scenario: injected panic is caught and minimized

- **GIVEN** a harness build that contains an intentional panic on a known
  input shape
- **WHEN** the fuzzer finds the crash
- **THEN** the wrapper runs `tmin`, prints the minimized artifact path under
  `target-fuzz/`, prints the regression-test promotion instruction, exits
  non-zero so the crash is not silently swallowed, and no crash file is
  added to git

### Requirement: Toolchain presence guards

The xtask wrapper SHALL verify that the nightly toolchain and cargo-fuzz
are installed before building, and SHALL fail with an install hint
otherwise.

#### Scenario: cargo-fuzz missing

- **GIVEN** a machine without `cargo-fuzz` installed
- **WHEN** the operator runs `cargo xtask fuzz dsl_yaml`
- **THEN** the command exits non-zero, prints the `cargo install cargo-fuzz`
  hint plus the nightly toolchain requirement, and starts no build

### Requirement: Production workspace isolation

The `fuzz/` crate SHALL be excluded from the production workspace, and the
root `Cargo.lock` SHALL gain no entries from this change. The change SHALL
NOT add `fuzz` or `xtask fuzz` to the QUALITY GATES block in `AGENTS.md`.

#### Scenario: workspace membership stays unchanged

- **GIVEN** the root `Cargo.toml` after this change
- **WHEN** `cargo metadata` runs at the workspace root
- **THEN** no `camel-fuzz` package appears as a workspace member, and the
  root `Cargo.lock` is byte-identical to its pre-change state

#### Scenario: quality gates stay untouched

- **GIVEN** the `AGENTS.md` QUALITY GATES block after this change
- **WHEN** the gate list is inspected
- **THEN** it contains no entry that invokes `fuzz` or `xtask fuzz`, so
  neither tool can block a build or a merge

