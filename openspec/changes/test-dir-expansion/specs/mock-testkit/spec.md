# mock-testkit Delta

## ADDED Requirements

### Requirement: Directory argument expansion

`camel test` SHALL accept directory arguments alongside file
arguments. Each directory argument SHALL expand to the test documents
(`*.test.yaml` / `*.test.yml`, per the reserved-suffix predicate
`camel_dsl::discovery::is_test_document`) contained in it, discovered
recursively. Directory names `target`, `.git`, and `node_modules`
SHALL be skipped at any depth. Within one directory argument the
expanded documents SHALL be ordered by byte-sorted path; across
arguments, CLI argument order SHALL be preserved. A file reached more
than once (explicit argument plus directory expansion) SHALL run once,
at its first occurrence. Plain file arguments SHALL keep their existing
verbatim behavior. A directory argument that expands to zero test
documents SHALL be reported as a misuse error (exit 2 class) naming
the directory, and the remaining arguments SHALL still run.

#### Scenario: directory expands recursively to sorted test documents

- **GIVEN** a directory containing `b.test.yaml`, `a.test.yaml`, and a nested subdirectory containing `c.test.yml`
- **WHEN** `camel test <dir>` runs
- **THEN** the documents run in the order `a.test.yaml`, `b.test.yaml`, `c.test.yml` (byte-sorted within the directory argument)

#### Scenario: excluded directory names are skipped

- **GIVEN** a directory whose `target/` subdirectory contains `gen.test.yaml`
- **WHEN** `camel test <dir>` runs
- **THEN** `gen.test.yaml` does not run

#### Scenario: zero-document directory is a misuse error

- **GIVEN** a directory containing no test documents
- **WHEN** `camel test <empty-dir> <other.test.yaml>` runs
- **THEN** an error naming `<empty-dir>` is reported, `<other.test.yaml>` still runs, and the exit code is 2

#### Scenario: duplicate documents run once

- **GIVEN** a directory containing `a.test.yaml` and the invocation `camel test <dir> <dir>/a.test.yaml`
- **WHEN** the run completes
- **THEN** `a.test.yaml` runs exactly once

#### Scenario: plain file arguments unchanged

- **GIVEN** an explicit file argument with any name
- **WHEN** `camel test <file>` runs
- **THEN** the file is read and parsed exactly as before this requirement (no expansion, no suffix filtering)
