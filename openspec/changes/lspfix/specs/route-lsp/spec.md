# Delta: route-lsp

## ADDED Requirements

### Requirement: Reserved-suffix documents publish no error diagnostics

The server SHALL derive the document's file path from its URI
(`Url::to_file_path`, which percent-decodes) on every lint site — `didOpen`,
`didSave`, and the debounced `didChange` task — and call
`LintEngine::lint_with_path` with it. For URIs whose path ends in
`.test.yaml`, `.test.yml`, `.job.yaml`, or `.job.yml`, the published
diagnostics SHALL contain zero `Error`-severity entries; the engine's single
`R-RESERVED` Info diagnostic is published. The reserved-suffix skip applies
to the whole lint: for a reserved path no rule runs, so syntax-broken
reserved documents produce `R-RESERVED` (not R-SYN) and the server still
does not panic — the "Partial and malformed input never panics the server"
requirement keeps its no-panic guarantee, while its R-SYN reporting clause
is scoped to non-reserved documents. For non-file URIs (`untitled:`,
`http:`, …) where no path can be derived, the server SHALL lint the full
document exactly as before. The server SHALL NOT implement its own suffix
check; the skip comes from the engine.

#### Scenario: didOpen of a colocated test document is error-free

- **GIVEN** a server with the production engine and a `file://…/routes/hello.test.yaml` URI whose text is a test document (`routeFiles`, `inputs`, `expects`)
- **WHEN** the client sends `didOpen`
- **THEN** the published diagnostics are exactly one `R-RESERVED` `Info` diagnostic and zero `Error`-severity entries

#### Scenario: didChange on a job document stays error-free through the debounce path

- **GIVEN** an opened `file://…/routes/nightly.job.yaml` document
- **WHEN** the client sends `didChange` with a full-replacement job-document body and the debounce window elapses
- **THEN** the published diagnostics contain no `Error`-severity entries

#### Scenario: didSave of a reserved document stays error-free

- **GIVEN** an opened `file://…/routes/hello.test.yaml` document
- **WHEN** the client sends `didSave`
- **THEN** the republished diagnostics contain no `Error`-severity entries

#### Scenario: malformed YAML at a reserved path skips R-SYN

- **GIVEN** a server and a `file://…/routes/broken.test.yaml` URI whose text is invalid YAML (`not: [a, route`)
- **WHEN** the client sends `didOpen`
- **THEN** the published diagnostics are the single `R-RESERVED` Info diagnostic with no R-SYN or other rule diagnostics, and the server does not panic

#### Scenario: non-file URI keeps full lint

- **GIVEN** a document opened with an `untitled:` URI containing a route with a schema violation
- **WHEN** the server lints it
- **THEN** the rule diagnostics are produced as before (no path context, no reserved skip)
