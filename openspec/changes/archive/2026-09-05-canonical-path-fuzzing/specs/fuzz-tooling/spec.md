## ADDED Requirements

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

## MODIFIED Requirements

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
