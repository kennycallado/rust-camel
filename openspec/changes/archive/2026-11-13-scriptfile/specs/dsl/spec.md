## ADDED Requirements

### Requirement: Script file source on expression objects

Every DSL expression object (the `script` step, `filter`/`when` predicate
blocks, `loop-while` expressions, and value-source expressions) SHALL accept
`source_file` as an alternative to the inline `source` pair. The object SHALL
define exactly one source form: `language`+`source`, `source_file` (with
optional `language`), `simple`, `rhai`, `jsonpath`, or `xpath`. Setting both
`source` and `source_file` SHALL fail with a named error naming the
expression context, and setting no source form SHALL fail with the existing
alternatives error extended to name `source_file`.

#### Scenario: source_file alone is accepted

- **GIVEN** a route file containing `script: { source_file: "hello.rhai" }`
  next to a non-empty `hello.rhai`
- **WHEN** the route file is loaded
- **THEN** the expression compiles with the file's verbatim contents as its
  source

#### Scenario: source and source_file together are rejected

- **GIVEN** an expression object with both `source` and `source_file` set
- **WHEN** the route file is loaded
- **THEN** load fails before any file IO, naming the expression context,
  both field names, and the resolved `source_file` path

#### Scenario: no source form is rejected

- **GIVEN** an expression object with neither `source` nor `source_file` nor
  a shorthand field
- **WHEN** the route file is loaded
- **THEN** load fails with the exactly-one alternatives error naming
  `source_file` as an accepted form

#### Scenario: shorthand field and source_file together are rejected

- **GIVEN** an expression object with `rhai` and `source_file` both set
- **WHEN** the route file is loaded
- **THEN** load fails because exactly two source forms are defined

### Requirement: Language inference from script file extension

When `source_file` is set without `language`, the language SHALL be inferred
from the resolved file's extension: `.rhai` maps to `rhai`, `.js` to `js`,
`.jsonpath` to `jsonpath`, `.xpath` to `xpath`, and `.jinja` and `.jinja2`
to `minijinja`. An explicit `language:` SHALL always
win over inference. An unknown extension without `language:` SHALL fail with
a named error that includes the resolved path and the extension.

#### Scenario: extension infers the language

- **GIVEN** `script: { source_file: "scripts/enrich.rhai" }` with no
  `language:` field
- **WHEN** the route file is loaded
- **THEN** the expression compiles with language `rhai`

#### Scenario: explicit language wins over inference

- **GIVEN** `script: { language: simple, source_file: "notes.txt" }`
- **WHEN** the route file is loaded
- **THEN** the expression compiles with language `simple` and the file's
  contents, with no extension error

#### Scenario: unknown extension without language is a named error

- **GIVEN** `script: { source_file: "scripts/req.xyz" }` with no `language:`
- **WHEN** the route file is loaded
- **THEN** load fails with an error naming the resolved path and `xyz`

### Requirement: Script file resolution and loading

Relative `source_file` paths SHALL resolve against the directory of the
route file that references them (YAML and JSON). The Rust DSL file
constructor SHALL resolve relative paths against the process CWD. Absolute
paths SHALL be accepted unchanged. The file SHALL be read once at load time,
verbatim (no env interpolation or templating of contents), and a file whose
contents are empty after trimming SHALL be rejected. A missing or unreadable
file SHALL fail with a named error that includes the resolved absolute path.

#### Scenario: relative path resolves against the route file directory

- **GIVEN** `routes/routes.yaml` referencing `source_file: "scripts/x.rhai"`
  and the script at `routes/scripts/x.rhai`
- **WHEN** the route file is loaded
- **THEN** the resolved script path is `routes/scripts/x.rhai` and its
  contents are used as the source

#### Scenario: missing file names the resolved path

- **GIVEN** a route file referencing `source_file: "scripts/absent.rhai"`
- **WHEN** the route file is loaded
- **THEN** load fails with an error containing the resolved path
  `scripts/absent.rhai` joined under the route file's directory

#### Scenario: empty file is rejected

- **GIVEN** a route file referencing a script file that contains only
  whitespace
- **WHEN** the route file is loaded
- **THEN** load fails with a named error that includes the resolved path

#### Scenario: file contents are read verbatim

- **GIVEN** a script file containing `${env:SECRET}` inside its body
- **WHEN** the route file is loaded without that env var set
- **THEN** load succeeds and the expression source is the literal
  `${env:SECRET}` text (interpolation never reaches file contents)

#### Scenario: source_file inside a route template resolves after substitution

- **GIVEN** a route template defined in `templates/payment.yaml` whose steps
  contain an expression with `source_file: "{{script_name}}"`, and a route
  instance in `routes/orders.yaml` that instantiates the template with
  `script_name: fraud-check.rhai`
- **WHEN** the route set is loaded
- **THEN** the substituted path resolves against the template document's
  directory (`templates/fraud-check.rhai`), the resolved script registers as
  an instance dependency, and a subsequent edit to
  `templates/fraud-check.rhai` alone changes the instance's change-detection
  hash

### Requirement: Rust DSL file-based expression constructor

The Rust DSL SHALL provide `LanguageExpressionDef::file(path)` constructing
an expression from a file with the language inferred from the extension, and
`LanguageExpressionDef::file_with_language(path, language)` constructing one
with an explicit language. Both SHALL read the file at construction time,
apply the same empty-file and missing-file named errors as the DSL, and
resolve relative paths against the process CWD.

#### Scenario: file constructor infers the language

- **GIVEN** a non-empty `scripts/x.rhai` relative to the process CWD
- **WHEN** `LanguageExpressionDef::file("scripts/x.rhai")` is called
- **THEN** it returns an expression with language `rhai` and the file's
  contents

#### Scenario: file constructor fails closed on a missing file

- **GIVEN** no file at the given path
- **WHEN** `LanguageExpressionDef::file` is called
- **THEN** it returns a named error containing the resolved path

### Requirement: Script contents participate in reload change detection

Route discovery SHALL fold the contents of every referenced script file into
the route document's source hash, so a route set whose only change is a
referenced script file yields a different source hash and is reloaded
instead of skipped.

#### Scenario: script-only edit changes the source hash

- **GIVEN** a loaded route referencing `scripts/x.rhai`
- **WHEN** only `scripts/x.rhai` is edited and discovery runs again
- **THEN** the route's new source hash differs from the committed hash
