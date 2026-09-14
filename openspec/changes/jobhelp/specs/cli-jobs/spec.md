## ADDED Requirements

### Requirement: job-scoped help

`camel job <NAME> --help` SHALL render the job's declared interface from
the parsed job document instead of clap's subcommand help. The output SHALL
show the resolved document's file stem, the document `description` (or
`(no description)`), the execution mode, the send target as authored, and
the declared arguments ordered lexically by argument name, one line per
argument, each carrying the argument name, the type (`string` in v1), a
`required`/`optional` marker, the `default` value when one is declared,
and the argument `description` when present. Each argument SHALL render
on one line: any maximal run of CR and LF characters inside a `default`
value or `description` SHALL render as a single space. A document whose
top-level `args:` block is absent or empty SHALL print `(no arguments)` in
place of the argument table. The `job` subcommand SHALL suppress clap's automatic
`--help`/`-h` handling so a present positional name always reaches the
job-scoped path; `--help` without a name SHALL print the `camel job` usage
text, and bare `camel job` SHALL keep the discovery listing. Help SHALL be
a pure projection: it SHALL NOT boot the route pipeline, write a report,
or install signal handlers, and it SHALL take precedence over `--report`
when both are passed. The help parse SHALL enforce the same structural and
declaration checks as execution parsing (suffix contract, section
exclusivity, strict serde shape, route-source conflict, per-argument
declaration validation, mode spelling, `timeout` presence) and SHALL NOT
resolve `--arg` pairs, apply defaults, interpolate, or validate execution
values (timeout duration, send scheme). Resolution, structural, and
declaration failures SHALL exit 2 with the existing loud diagnostics.

#### Scenario: help with a job name renders the declared interface

- **GIVEN** a discovered job whose document declares `args:` with a
  required argument with a description and an optional argument with a
  default
- **WHEN** `camel job <name> --help` runs
- **THEN** stdout shows the document's file stem, the document
  description, the mode, the send target, and one line per declared
  argument with name, `string`, `required`/`optional`, the default when
  declared, and the description when present, and the exit code is 0

#### Scenario: help succeeds without satisfied required arguments

- **GIVEN** a schema-valid job document declaring a required argument
  without a default, whose `to` and `timeout` reference `${arg:...}`
  tokens
- **WHEN** `camel job <name> --help` runs without any `--arg`
- **THEN** the interface renders with the `to` token as authored and the
  declared arguments unchanged, and the exit code is 0

#### Scenario: help without an args block prints no arguments

- **GIVEN** a discovered job whose document has no top-level `args:`
  block, or an empty `args: {}` block
- **WHEN** `camel job <name> --help` runs
- **THEN** stdout shows the file stem, the description, the mode, the
  send target, and the line `(no arguments)` under `Arguments:`, and the
  exit code is 0

#### Scenario: help does not execute the job

- **GIVEN** a discovered job document with a valid `execute:` section
- **WHEN** `camel job <name> --help` runs
- **THEN** no route boots, no report file is written, no signal handlers
  are installed, and the process exits 0 after printing the interface

#### Scenario: help with an unknown job name fails loud

- **GIVEN** no job resolves under the configured `[jobs].dirs` roots
- **WHEN** `camel job <name> --help` runs
- **THEN** stderr carries the existing not-found diagnostic and the exit
  code is 2

#### Scenario: help with a malformed document fails loud

- **GIVEN** a discovered job document that fails structural or declaration
  parsing (for example an unknown field or a malformed argument
  declaration)
- **WHEN** `camel job <name> --help` runs
- **THEN** stderr carries the parse diagnostic (not clap help) and the
  exit code is 2

#### Scenario: help flag without a job name prints command usage

- **GIVEN** the `job` subcommand with no positional name
- **WHEN** `camel job --help` runs
- **THEN** stdout shows the `camel job` usage text and the exit code is 0,
  while bare `camel job` still prints the discovery listing

#### Scenario: help takes precedence over report

- **GIVEN** a discovered job document
- **WHEN** `camel job <name> --help --report <file>` runs
- **THEN** the declared interface is printed, no report file is written,
  and the exit code is 0
