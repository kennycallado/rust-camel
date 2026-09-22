## ADDED Requirements

### Requirement: dynamic declared-argument flags

`camel job` SHALL accept one `--<name> VALUE` flag for each argument
declared in the job document's top-level `args:` block. The flag name
SHALL be the declared argument name verbatim (underscore spelling,
long-only; no dash alias, no derived short flag, no partial-match
inference). Both `--name VALUE` and `--name=VALUE` SHALL be accepted.
Dynamic flags SHALL be accepted only AFTER the positional document
reference (explicit path or bare name); a dynamic flag before the
document reference SHALL fail with the standard unknown-argument
diagnostic. Repeating the same dynamic flag SHALL keep the last
occurrence. Dynamic flags SHALL lower into the same NAME=VALUE pair
channel `--arg` uses, before coercion, defaults, required checks, and
interpolation, so a run with `--name world` SHALL behave identically
to `--arg name=world` (same resolved values, same interpolation, same
report bytes, same exit code). Values of declared `int`, `enum`, and
untyped arguments SHALL flow through the existing typed coercion
unchanged. An undeclared flag, a dash-alias spelling of a declared
underscore name (`--user-name` for `user_name`), a partial prefix of
a declared flag (`--user` for `--user_name`), and any short spelling
SHALL each fail (exit 2) with clap's unknown-argument diagnostic —
no inference, no aliases — with the suggestion hint computed against
the declared flag set. Invocations without dynamic
flags SHALL behave identically to before this requirement existed.

#### Scenario: declared string flag behaves identically to arg

- **GIVEN** a job document declaring `name: {required: true}` with
  `${arg:name}` referenced in its send body
- **WHEN** it runs once with `camel job <doc> --name world` and once
  with `camel job <doc> --arg name=world`
- **THEN** both runs exit 0, interpolate `world` identically, and
  produce the same report

#### Scenario: both value forms accepted

- **GIVEN** a job document declaring `name`
- **WHEN** it runs with `--name=world`
- **THEN** the run is indistinguishable from `--name world`

#### Scenario: last occurrence wins within the form

- **GIVEN** a job document declaring `name`
- **WHEN** it runs with `--name first --name second`
- **THEN** the resolved value is `second`, matching the `--arg`
  repeat rule

#### Scenario: typed coercion applies to flag values

- **GIVEN** a job document declaring `count: {type: int}`
- **WHEN** it runs with `--count notanint`
- **THEN** it exits 2 before boot with the existing coercion
  diagnostic naming `count`, `int`, and the raw value

#### Scenario: undeclared flag fails with a hint

- **GIVEN** a job document declaring only `name`
- **WHEN** it runs with `--nmae x`
- **THEN** it exits 2 with clap's unknown-argument diagnostic for
  `--nmae` including a suggestion pointing at `--name`

#### Scenario: alias spellings are rejected

- **GIVEN** a job document declaring `user_name`
- **WHEN** it runs with `--user-name x`, with `--user x`, or with a
  short spelling of the flag
- **THEN** each run exits 2 with the unknown-argument diagnostic; the
  verbatim underscore long form is the only accepted spelling

#### Scenario: bare-name invocation carries dynamic flags

- **GIVEN** a discovered job `myjob` under the configured jobs root
  declaring `name`
- **WHEN** `camel job myjob --name world` runs
- **THEN** the job resolves through bare-name discovery and the run
  behaves as the explicit-path form

#### Scenario: flags before the document reference fail

- **WHEN** `camel job --name world <doc>` runs
- **THEN** it exits 2 with the standard unknown-argument diagnostic
  for `--name`; the supported shape is document first, flags after

#### Scenario: static flags keep their meaning after the document

- **GIVEN** invocations placing `--arg`, `--help`/`-h`, `--report`,
  or `--config` after the document reference — before or after a
  dynamic flag
- **WHEN** the run is parsed
- **THEN** each flag keeps its pre-existing meaning (`--arg` pairs,
  manual job help, report path, config load): flags clap consumes
  statically apply directly; flags captured into the dynamic tail
  (which begins at the first unknown token) are recovered by the
  tail re-parse, and a tail `--config` is honored before job-root
  resolution with the last occurrence on argv winning

#### Scenario: terminator tokens are never flags

- **GIVEN** a `--` terminator anywhere after the `job` subcommand
  token
- **WHEN** any token follows it in what would be the dynamic tail
- **THEN** those tokens are treated as literal values, never parsed
  as dynamic or static flags, and the first one fails as an
  unexpected positional (exit 2) — clap terminator semantics
  preserved

### Requirement: bool flag spellings

A declared `bool` argument SHALL be settable as bare `--flag` (value
true) and as bare `--no-flag` (value false); when neither appears the
document `default` SHALL apply, or the argument stays unset when no
default exists. The value-taking forms `--flag=false` and
`--no-flag=false` SHALL be hard errors (exit 2) whose diagnostic names
the accepted spellings `--flag`, `--no-flag`, and the pair form
`--arg flag=false`. A stray token after a bool flag (`--flag false`)
SHALL fail (exit 2) rather than be consumed as the flag's value.
Supplying both `--flag` and `--no-flag` in one invocation SHALL fail
(exit 2) naming the argument; it SHALL NOT resolve by last-wins.
`--no-<name>` SHALL be rejected as an unknown flag for every declared
non-bool argument. Lowered bool values SHALL pass through the existing
bool coercion unchanged.

#### Scenario: bare flag sets true

- **GIVEN** a job document declaring `verbose: {type: bool}` with
  `${arg:verbose}` in its send body
- **WHEN** it runs with `--verbose`
- **THEN** the body interpolates canonical `true` and the exit code
  is 0

#### Scenario: absent bool applies the document default

- **GIVEN** a job document declaring `verbose: {type: bool, default:
  "false"}`
- **WHEN** it runs without any `verbose` flag
- **THEN** `${arg:verbose}` resolves to `false` through the default
  path exactly as before

#### Scenario: negated spelling sets false

- **GIVEN** a job document declaring `verbose: {type: bool, default:
  "true"}`
- **WHEN** it runs with `--no-verbose`
- **THEN** the resolved value is canonical `false`

#### Scenario: value form is rejected naming the spellings

- **GIVEN** a job document declaring `verbose: {type: bool}`
- **WHEN** it runs with `--verbose=false` or with `--no-verbose=false`
- **THEN** it exits 2 before boot and the diagnostic names
  `--verbose`, `--no-verbose`, and `--arg verbose=false`

#### Scenario: stray token after a bool flag fails

- **GIVEN** a job document declaring `verbose: {type: bool}`
- **WHEN** it runs with `--verbose false`
- **THEN** it exits 2; `false` is not consumed as the flag's value

#### Scenario: contradictory spellings conflict

- **GIVEN** a job document declaring `verbose: {type: bool}`
- **WHEN** it runs with `--verbose --no-verbose`
- **THEN** it exits 2 naming `verbose` and both spellings

#### Scenario: negation of a non-bool argument is unknown

- **GIVEN** a job document declaring `count: {type: int}` and no bool
  arguments
- **WHEN** it runs with `--no-count`
- **THEN** it exits 2 with the unknown-argument diagnostic for
  `--no-count`

### Requirement: cross-form argument conflicts

The same argument key supplied through BOTH a dynamic flag and an
`--arg NAME=VALUE` pair in one invocation SHALL fail (exit 2) naming
the key and both forms. It SHALL NOT resolve by last-wins across
forms. Within a single form the existing last-wins rules SHALL apply.

#### Scenario: same key through both forms fails

- **GIVEN** a job document declaring `name`
- **WHEN** it runs with `--name a --arg name=b`, or with the forms in
  the opposite order (`--arg name=b --name a`)
- **THEN** it exits 2 before boot and the diagnostic names `name`,
  the `--name` form, and the `--arg` form, regardless of order

### Requirement: dynamic flags require an args block

A dynamic flag passed to a job document WITHOUT a top-level `args:`
block SHALL fail (exit 2) with a diagnostic naming the flag and
pointing at the two remedies: declaring an `args:` block or using
`--arg`. The dynamic flag SHALL NOT fall back to legacy header
injection and SHALL NOT trigger the legacy `--arg` deprecation note;
the deprecation note SHALL keep applying to `--arg` pairs on
undeclared documents exactly as before.

#### Scenario: dynamic flag on an undeclared document fails

- **GIVEN** a job document with no `args:` block
- **WHEN** it runs with `--name x`
- **THEN** it exits 2 before boot; the diagnostic names `--name`,
  mentions the `args:` block and `--arg`, and no header injection or
  deprecation note occurs

### Requirement: reserved argument names

A job document declaring a top-level argument named `help`, `config`,
`report`, or `arg` SHALL fail document load (exit 2) with a
reserved-name diagnostic naming the argument and the static flag it
collides with. The rejection SHALL occur in BOTH execution parsing
and `camel job <name> --help` parsing, and in `camel compile`'s
declaration validation, before any flag parsing of the document's
arguments.

#### Scenario: every reserved name fails execution

- **GIVEN** job documents each declaring one argument named `help`,
  `config`, `report`, or `arg`
- **WHEN** each job runs
- **THEN** each run exits 2 at load with a diagnostic naming the
  argument as a reserved argument name

#### Scenario: reserved name fails help identically

- **GIVEN** the same documents
- **WHEN** `camel job <name> --help` runs for each
- **THEN** each exits 2 with the same reserved-name diagnostic

#### Scenario: reserved name fails compile validation

- **GIVEN** a job document declaring `args: {config: {required: true}}`
- **WHEN** `camel compile` validates its declarations
- **THEN** compilation exits 2 with the reserved-name diagnostic and
  produces no artifact

## MODIFIED Requirements

### Requirement: job-scoped help

`camel job <NAME> --help` SHALL render the job's declared interface from
the parsed job document instead of clap's subcommand help. The output SHALL
show the resolved document's file stem, the document `description` (or
`(no description)`), the execution mode, the send target as authored, and
the declared arguments ordered lexically by argument name, one line per
argument, each carrying the argument name, the declared type rendered as
`string` (also when `type:` is omitted), `int`, `bool`, or `enum[...]`
with its comma-separated member list, a `required`/`optional` marker, the
`default` value when one is declared, and the argument `description` when
present. When at least one argument is declared, the `Arguments:` table
SHALL carry exactly one additional note line stating that each argument
is settable as `--<name> <VALUE>`, bool arguments as bare `--<name>` /
`--no-<name>`, in addition to `--arg <name>=<value>`; the note's exact
wording SHALL be pinned byte-exact by tests. Documents with no declared
arguments SHALL keep printing `(no arguments)` with no note. Each
argument SHALL render on one line: any maximal run of CR and LF
characters inside a `default` value or `description` SHALL render as a
single space. The type column SHALL pad to the widest rendered type in
that job, using the same per-job width strategy as the name column. A
document whose top-level `args:` block is absent or empty SHALL print
`(no arguments)` in place of the argument table. The `job` subcommand
SHALL suppress clap's automatic `--help`/`-h` handling so a present
positional name always reaches the job-scoped path; `--help` without a
name SHALL print the `camel job` usage text, and bare `camel job` SHALL
keep the discovery listing. Help SHALL be a pure projection: it SHALL NOT
boot the route pipeline, write a report, or install signal handlers, and
it SHALL take precedence over `--report` when both are passed. The help
parse SHALL enforce the same structural and declaration checks as
execution parsing (suffix contract, section exclusivity, strict serde
shape, route-source conflict, per-argument declaration validation
including `type` grammar and typed-default coercion, mode spelling,
`timeout` presence) and SHALL
NOT resolve `--arg` pairs, apply defaults, interpolate, or validate
execution values (timeout duration, send scheme). Resolution, structural,
and declaration failures SHALL exit 2 with the existing loud diagnostics.

#### Scenario: nested job help shows the invocable path

- **GIVEN** `daily/ingest.job.yaml` exists nested in a configured root
- **WHEN** `camel job daily/ingest --help` runs
- **THEN** the first stdout line is `daily/ingest.job.yaml` — the same
  spelling the listing shows — and the exit code is 0

#### Scenario: help with a job name renders the declared interface

- **GIVEN** a discovered job whose document declares `args:` with a
  required argument with a description and an optional argument with a
  default
- **WHEN** `camel job <name> --help` runs
- **THEN** stdout shows the document's file stem, the document
  description, the mode, the send target, and one line per declared
  argument with name, the declared type, `required`/`optional`, the
  default when declared, and the description when present, and the exit
  code is 0

#### Scenario: arguments table notes the flag spellings

- **GIVEN** a discovered job whose document declares at least one
  argument
- **WHEN** `camel job <name> --help` runs
- **THEN** the `Arguments:` table includes the one note line naming
  the `--<name> <VALUE>`, `--<name>` / `--no-<name>`, and
  `--arg <name>=<value>` spellings, and the exit code is 0

#### Scenario: help renders the enum member list

- **GIVEN** a discovered job declaring `tier: {type: "enum[bronze,gold]"}`
- **WHEN** `camel job <name> --help` runs
- **THEN** the `tier` row's type column shows `enum[bronze,gold]` and the
  exit code is 0

#### Scenario: help type column aligns across rows

- **GIVEN** a discovered job declaring `count: {type: int}` and `tier: {type: "enum[bronze,gold]"}`
- **WHEN** `camel job <name> --help` runs
- **THEN** both rows render their type column padded to the widest type (`enum[bronze,gold]`), so the `required`/`optional` markers start at the same column on every row, and the exit code is 0

#### Scenario: help with a malformed document fails loud

- **GIVEN** a discovered job document that fails structural or declaration
  parsing (for example an unknown field or a malformed argument
  declaration)
- **WHEN** `camel job <name> --help` runs
- **THEN** stderr carries the parse diagnostic (not clap help) and the
  exit code is 2

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
  send target, and the line `(no arguments)` under `Arguments:` with no
  note line, and the exit code is 0

#### Scenario: help does not execute the job

- **GIVEN** a discovered job document with a valid `execute:` section
- **WHEN** `camel job <name> --help` runs
- **THEN** no route boots, no report file is written, no signal handlers
  are installed, and the process exits 0 after printing the interface

#### Scenario: help with an unknown job name fails loud

- **GIVEN** no job resolves under the configured `[jobs].dirs` roots
- **WHEN** `camel job <name> --help` runs
- **THEN** stderr carries the existing not-found diagnostic and the
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
