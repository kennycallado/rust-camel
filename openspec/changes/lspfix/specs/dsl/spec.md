# Delta: dsl

## MODIFIED Requirements

### Requirement: Reserved test suffix in route discovery

Route discovery SHALL treat file names ending in `.test.yaml` or `.test.yml`
as camel test documents, never as route documents. When a wildcard pattern
(default glob, `Camel.toml` `routes` entry, or `--routes` value) matches a
reserved-suffixed file (`.test.yaml`/`.test.yml`/`.job.yaml`/`.job.yml`),
discovery SHALL skip the file with no error. When an explicit pattern with
no glob metacharacters names a reserved-suffixed file, discovery SHALL fail
with a `ReservedDocumentSuffix` error whose message names the file and names
`camel test` or `camel job` as the correct action. The suffix predicates
(`is_test_document`, `is_job_document`, `is_reserved_document`) SHALL be
defined in `camel-api` (`reserved_suffix` module — the crate visible to both
sides of the lint/runtime hex boundary) and re-exported by
`camel-dsl::discovery`, preserving the single source of truth consumed by
the CLI (run, watch, lint), discovery internals, and the `camel-lint`
engine. No consumer SHALL keep a private suffix copy.

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
- **THEN** discovery fails with a `ReservedDocumentSuffix` error naming `demo.test.yaml` and instructing the user to run `camel test` instead

#### Scenario: literal job document naming errors

- **GIVEN** an invocation `camel run --routes jobs/nightly.job.yaml`
- **WHEN** discovery runs
- **THEN** discovery fails with a `ReservedDocumentSuffix` error naming `nightly.job.yaml` and instructing the user to run `camel job` instead

#### Scenario: test-json names stay governed by JSON gating

- **GIVEN** a file named `routes/x.test.json` matched by `routes/*.json`
- **WHEN** discovery runs
- **THEN** the file is not treated as test-suffixed (test documents are YAML-only) and the existing JSON pattern gating applies unchanged

#### Scenario: re-export keeps the discovery path stable

- **GIVEN** consumers importing `camel_dsl::discovery::is_reserved_document` (CLI lint/test/job, corpus gates, integration harness)
- **WHEN** the predicate definition moves to `camel-api` with a `camel-dsl` re-export
- **THEN** all existing import paths and behavior stay unchanged, and the camel-lint engine consumes the same predicate without depending on `camel-dsl`
