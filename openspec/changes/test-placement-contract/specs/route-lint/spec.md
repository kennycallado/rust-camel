# route-lint Delta

## ADDED Requirements

### Requirement: camel test documents skipped with info diagnostic

This requirement carves an exception into "`camel lint` CLI runs the engine
and exits by severity": for camel test documents, the CLI does not run the
engine. The `camel lint` CLI subcommand SHALL skip a file whose name ends
in `.test.yaml` or `.test.yml`, using the suffix predicate exported by
`camel-dsl`, and SHALL emit a one-line info diagnostic stating the file is
a camel test document, exiting 0. The `camel-lint` engine SHALL NOT change
its input contract (engine receives source text; the predicate is applied
by `camel-cli` before invoking the engine). No error SHALL be reported for
test documents.

#### Scenario: explicit test document linted by path is skipped with info line

- **GIVEN** a routes directory containing `routes/demo.yaml` and `routes/demo.test.yaml`
- **WHEN** `camel lint routes/demo.test.yaml` runs
- **THEN** no lint rules run, and the output contains one info line naming `demo.test.yaml` as a skipped camel test document

#### Scenario: no schema diagnostics for test documents

- **GIVEN** a test document whose `expects` and `inputs` keys do not conform to the route schema
- **WHEN** `camel lint` runs on it
- **THEN** no R-SCHEMA or other rule diagnostics are emitted for the test document
