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

The repository SHALL commit a curated seed corpus per target under
`fuzz/seeds/<target>/`, and the xtask wrapper SHALL copy the seeds into
that target's corpus directory before the first run. `dsl_yaml` seeds
derive from existing camel-dsl adversarial test inputs and audit
regression inputs. Valid `dsl_json` seeds derive from real canonical
JSON documents — the JSON arms of parity test cases, camel-dsl JSON test
fixtures, and documents valid against `schemas/dsl/route-schema.json`;
malformed or adversarial `dsl_json` seeds are JSON-native fixtures; no
`dsl_json` seed SHALL be a mechanical YAML→JSON conversion.
`dsl_template` seeds derive from template parser and materializer test
fixtures. `dsl_parity` seeds derive from documents valid in both formats
plus malformed-in-both shapes. Targets SHALL NOT share corpus
directories.

#### Scenario: first run starts from committed seeds

- **GIVEN** a worktree where `target-fuzz/corpus/<target>/` does not
  exist
- **WHEN** the operator runs `cargo xtask fuzz <target>`
- **THEN** the wrapper creates the corpus directory, copies every
  committed seed file into it, and the fuzz run reports those inputs as
  covered

#### Scenario: seed set contains the audit's adversarial shapes

- **GIVEN** the committed seed directory `fuzz/seeds/dsl_yaml/`
- **WHEN** the seed files are inspected
- **THEN** the set includes at least one YAML alias-bomb input, one
  deep-nesting input beyond the parser's recursion budget, and one input
  taken from an existing camel-dsl malformed-route test

#### Scenario: JSON seeds are target-native, not conversions

- **GIVEN** the committed seed directory `fuzz/seeds/dsl_json/`
- **WHEN** the seed files are inspected against their stated source
- **THEN** valid seeds trace to canonical JSON documents,
  malformed/adversarial seeds trace to JSON-native fixtures, none is a
  mechanical YAML→JSON conversion, and the set includes at least one
  valid minimal route, one deep-nesting document, and one malformed
  document

#### Scenario: parity seeds exercise both front-ends

- **GIVEN** the committed seed directory `fuzz/seeds/dsl_parity/`
- **WHEN** the seed files are parsed by both front-ends
- **THEN** the set includes at least one document both accept, and one
  document both reject

### Requirement: Crash minimization and promotion

When a fuzz run finds a crash, the xtask wrapper SHALL minimize the
input with `cargo +nightly fuzz tmin <target> <artifact>`, forwarding
the fuzz run's `-artifact_prefix` so the minimized output lands in the
same `target-fuzz/artifacts/<target>/` directory the wrapper scans,
report the minimized input path, and instruct the operator to add a
committed Rust regression test. Raw crash artifacts SHALL NOT be
committed, and minimization SHALL NOT write to cargo-fuzz's default
`fuzz/artifacts/` location.

#### Scenario: injected panic is caught and minimized

- **GIVEN** a harness build that contains an intentional panic on a known
  input shape
- **WHEN** the fuzzer finds the crash
- **THEN** the wrapper runs `tmin`, prints the minimized artifact path under
  `target-fuzz/artifacts/<target>/`, prints the regression-test promotion
  instruction, exits non-zero so the crash is not silently swallowed, and
  no crash file is added to git

#### Scenario: tmin writes only to the scanned artifact directory

- **GIVEN** a crash artifact under `target-fuzz/artifacts/<target>/`
- **WHEN** the wrapper runs `tmin` on it
- **THEN** the minimized file appears under `target-fuzz/artifacts/<target>/` and no `fuzz/artifacts/` directory is created

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

### Requirement: dsl_json fuzz harness

The `dsl_json` fuzz target SHALL feed arbitrary bytes as UTF-8 text to
canonical JSON route parsing via
`camel_dsl::json::parse_json_with_threshold_and_security` with
`camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD` and
`SecurityCompileContext::default()` as arguments, and SHALL hold the
invariant that parsing never panics.

#### Scenario: malformed JSON returns an error instead of panicking

- **GIVEN** the fuzzer generates truncated, deeply nested, or
  type-confused JSON bytes
- **WHEN** the harness converts the bytes to a string and parses them
- **THEN** the parse call returns either `Ok` or `Err`, and the fuzz
  target reports no crash

#### Scenario: invalid UTF-8 input is skipped

- **GIVEN** the fuzzer generates byte sequences that are not valid UTF-8
- **WHEN** the harness runs
- **THEN** the harness returns early without calling the parser and
  reports no crash

### Requirement: dsl_template fuzz harness

The `dsl_template` fuzz target SHALL feed arbitrary bytes as UTF-8 text
to JSON template-section parsing via
`camel_dsl::template::json::parse_json_templates` and
`parse_json_templated_routes`. When both sections parse `Ok`, the
harness SHALL, for each parsed instance whose `route_template_ref`
matches a parsed template, call the materializer entry
(`materialize_and_compile`) with the instance's parameters and the
default threshold and security arguments, discarding the result. An
instance whose reference matches no template is an ordinary rejection:
no materialization is attempted. Parsing and materialization SHALL hold
the invariant that they never panic.

#### Scenario: templated document expands without panicking

- **GIVEN** a document whose `templates` and `templated_routes` sections
  both parse and each instance references a parsed template
- **WHEN** the harness materializes each matching instance
- **THEN** materialization returns either `Ok` or `Err`, and the fuzz
  target reports no crash

#### Scenario: instance references a missing template

- **GIVEN** a parsed instance whose `route_template_ref` matches no
  parsed template
- **WHEN** the harness evaluates the instance
- **THEN** the instance is rejected without materialization and the fuzz
  target reports no crash

#### Scenario: malformed template section returns an error instead of panicking

- **GIVEN** the fuzzer generates truncated or placeholder-heavy template
  documents
- **WHEN** the harness parses both sections
- **THEN** the parse calls return either `Ok` or `Err`, and the fuzz
  target reports no crash

#### Scenario: invalid UTF-8 input is skipped

- **GIVEN** the fuzzer generates byte sequences that are not valid UTF-8
- **WHEN** the harness runs
- **THEN** the harness returns early without calling any parser and
  reports no crash

### Requirement: dsl_parity differential harness

The `dsl_parity` fuzz target SHALL feed the same UTF-8 text to both
front-ends in two parts. Part one (panic coverage): the
threshold-and-security parse variants of both front-ends
(`yaml::parse_yaml_with_threshold_and_security`,
`json::parse_json_with_threshold_and_security`, shared threshold and
security arguments) run with their results discarded. Part two
(differential, directional because JSON is a subset of YAML's data
model): the text deserializes with `serde_json` to
`RouteDslRoutes`; on `Err` the input is outside the overlap and the
comparison skips it; on `Ok` the same text deserializes with the YAML
serde front-end to `RouteDslRoutes`, and the harness SHALL panic when
the YAML deserialization returns `Err` or the two `RouteDslRoutes`
disagree at the step layer: differing route counts, a differing
per-route `id` or `from`, or differing `{:#?}` Debug renderings of the
flattened `Vec<&RouteDslStep>` (the pre-converter flattening
`parity_tests.rs` uses — comparing downstream of the shared
`route_dsl_to_declarative_route` converter would normalize away
front-end divergence). Deserialization and comparison SHALL NOT
themselves panic on non-divergent input.

#### Scenario: YAML rejects what JSON accepts

- **GIVEN** an input that `serde_json` deserializes to `RouteDslRoutes`
  while the YAML serde front-end returns `Err`
- **WHEN** the harness compares the two outcomes
- **THEN** the harness panics and the fuzz target reports a crash

#### Scenario: both front-ends accept but deserialize differently

- **GIVEN** an input both serde front-ends deserialize to `RouteDslRoutes`
- **WHEN** the harness compares route counts, per-route `id`/`from`, and
  the flattened step Debug renderings
- **THEN** the harness panics on any difference, and reports no crash
  when all three agree

#### Scenario: JSON rejects the input

- **GIVEN** an input for which the `serde_json` deserialization returns
  `Err` (including YAML-only syntax)
- **WHEN** the harness evaluates the differential comparison
- **THEN** the comparison is skipped and the fuzz target reports no
  crash regardless of the YAML outcome

#### Scenario: invalid UTF-8 input is skipped

- **GIVEN** the fuzzer generates byte sequences that are not valid UTF-8
- **WHEN** the harness runs
- **THEN** the harness returns early without calling any parser and
  reports no crash

