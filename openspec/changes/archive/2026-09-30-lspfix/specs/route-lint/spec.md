# Delta: route-lint

## MODIFIED Requirements

### Requirement: camel test documents skipped with info diagnostic

This requirement carves an exception into "`camel lint` CLI runs the engine
and exits by severity": for reserved documents, the CLI does not run the
engine. The `camel lint` CLI subcommand SHALL skip a file whose name ends in
`.test.yaml`, `.test.yml`, `.job.yaml`, or `.job.yml` (a camel test or job
document per ADR-0062), using the reserved-suffix predicate re-exported by
`camel-dsl` (defined in `camel-api`, see "Path-aware engine lint skips
reserved-suffix documents"), and SHALL emit a one-line info diagnostic
stating the file is a reserved document, exiting 0. No error SHALL be
reported for reserved documents.

#### Scenario: explicit test document linted by path is skipped with info line

- **GIVEN** a routes directory containing `routes/demo.yaml` and `routes/demo.test.yaml`
- **WHEN** `camel lint routes/demo.test.yaml` runs
- **THEN** no lint rules run, and the output contains one info line naming `demo.test.yaml` as a skipped camel test document

#### Scenario: no schema diagnostics for test documents

- **GIVEN** a test document whose `expects` and `inputs` keys do not conform to the route schema
- **WHEN** `camel lint` runs on it
- **THEN** no R-SCHEMA or other rule diagnostics are emitted for the test document

#### Scenario: job document linted by path is skipped with info line

- **GIVEN** a routes directory containing `routes/demo.job.yaml`
- **WHEN** `camel lint routes/demo.job.yaml` runs
- **THEN** no lint rules run, and the output contains one info line naming `demo.job.yaml` as a skipped reserved document, exiting 0

## ADDED Requirements

### Requirement: Path-aware engine lint skips reserved-suffix documents with info diagnostic

The `LintEngine` SHALL expose `lint_with_path(&self, source: &str, path:
Option<&Path>) -> Vec<Diagnostic>`. When `path` is `Some` and its file name
ends in `.test.yaml`, `.test.yml`, `.job.yaml`, or `.job.yml`, the engine
SHALL run no rules and return exactly one `Diagnostic` with the stable code
`R-RESERVED`, severity `Info`, a span at offset 0, and a message stating the
file is a reserved document owned by `camel test` or `camel job`. When `path`
is `None` or not reserved-suffixed, `lint_with_path` SHALL behave exactly as
the string-only `lint(source)`, which SHALL delegate to
`lint_with_path(source, None)` so existing callers see byte-identical
diagnostics. The suffix decision SHALL consume the single predicate defined
in `camel-api` (re-exported by `camel-dsl::discovery`) — the engine SHALL NOT
keep a private suffix copy.

#### Scenario: reserved test suffix skips all rules

- **GIVEN** the engine and a path `routes/hello.test.yaml` with test-document content (`routeFiles`, `inputs`, `expects`)
- **WHEN** `lint_with_path(source, Some(path))` runs
- **THEN** the result is exactly one diagnostic: code `R-RESERVED`, severity `Info`, and zero Error/Warning diagnostics

#### Scenario: every reserved suffix variant skips

- **GIVEN** paths ending in `.test.yaml`, `.test.yml`, `.job.yaml`, and `.job.yml`
- **WHEN** `lint_with_path` runs for each with non-route content
- **THEN** each result is the single `R-RESERVED` Info diagnostic and no rule (R-SYN, R-SCHEMA, …) diagnostics

#### Scenario: ordinary path lints as before

- **GIVEN** the engine, a path `routes/hello.yaml`, and a source with a schema violation
- **WHEN** `lint_with_path(source, Some(path))` runs
- **THEN** the diagnostics are identical to `lint(source)` on the same text

#### Scenario: no-path call is unchanged

- **GIVEN** the engine and any source
- **WHEN** `lint(source)` runs
- **THEN** the diagnostics equal `lint_with_path(source, None)`, including for reserved-suffix-looking content, because no file context exists
