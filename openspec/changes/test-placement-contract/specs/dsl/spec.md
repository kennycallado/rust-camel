# dsl Delta

## ADDED Requirements

### Requirement: Reserved test suffix in route discovery

Route discovery SHALL treat file names ending in `.test.yaml` or `.test.yml`
as camel test documents, never as route documents. When a wildcard pattern
(default glob, `Camel.toml` `routes` entry, or `--routes` value) matches a
test-suffixed file, discovery SHALL skip the file with no error. When an
explicit pattern with no glob metacharacters names a test-suffixed file,
discovery SHALL fail with a `ReservedTestSuffix` error whose message names
the file and states the `camel test` command as the correct action. The
suffix predicate SHALL live in `camel-dsl` and SHALL be the single source of
truth consumed by the CLI (run, watch) and lint.

#### Scenario: wildcard glob skips colocated test document

- **GIVEN** a directory containing `routes/demo.yaml` and `routes/demo.test.yaml`
- **WHEN** discovery runs with pattern `routes/*.yaml`
- **THEN** `demo.yaml` loads as a route and `demo.test.yaml` is skipped with no error

#### Scenario: explicit Camel.toml routes entry skips test document

- **GIVEN** `Camel.toml` with `routes = ["routes/*.yaml"]` and a colocated `routes/demo.test.yaml`
- **WHEN** `camel run` starts
- **THEN** discovery skips the test document and startup succeeds

#### Scenario: explicit no-wildcard naming errors

- **GIVEN** an invocation `camel run --routes routes/demo.test.yaml`
- **WHEN** discovery runs
- **THEN** discovery fails with a `ReservedTestSuffix` error naming `demo.test.yaml` and instructing the user to run `camel test` instead

#### Scenario: test-json names stay governed by JSON gating

- **GIVEN** a file named `routes/x.test.json` matched by `routes/*.json`
- **WHEN** discovery runs
- **THEN** the file is not treated as test-suffixed (test documents are YAML-only) and the existing JSON pattern gating applies unchanged
