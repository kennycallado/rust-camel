## ADDED Requirements

### Requirement: typed argument coercion

When a declaration carries `type:`, the argument's resolved value — the
last `--arg NAME=VALUE` pair or the applied `default` — SHALL be coerced
to the declared type at resolution, before interpolation. Coercion SHALL
produce the value's canonical string form: `int` accepts an optional sign
followed by decimal digits only (no surrounding whitespace, i64 range) and
canonicalizes as plain decimal (`007` becomes `7`, `+5` becomes `5`);
`bool` accepts `true` and `false` case-insensitively — `1`, `0`, and every
other spelling are rejected — and canonicalizes lowercase; `enum`
validates exact, case-sensitive membership and canonicalizes as the
matched member verbatim; `string` (and any declaration without `type:`)
keeps the value verbatim. A coercion failure SHALL exit 2 before boot,
naming the argument, the expected type, and the raw value; an enum failure
SHALL list the allowed members. Unknown-name and missing-required
failures SHALL take precedence over coercion. A typed declaration whose
`default` fails coercion SHALL fail document load with the same
declaration-error class in BOTH execution parsing and `--help` parsing.
Compiling a job document SHALL run the argument-declaration checks —
`type` grammar and typed-default coercion included — and exit 2 without
producing an artifact on failure; no other execution-value validation
SHALL run at compile time. Compiled artifacts SHALL coerce embedded
typed defaults through the same rules at startup.

#### Scenario: int canonical form substitutes

- **GIVEN** a job declaring `count: {type: int}` with `${arg:count}` referenced in its `to`
- **WHEN** it runs with `--arg count=007`
- **THEN** the send target interpolates `7` and the exit code is 0

#### Scenario: int rejects a non-integer value

- **GIVEN** a job declaring `count: {type: int}`
- **WHEN** it runs with `--arg count=abc`
- **THEN** it exits 2 before boot and the error names `count`, the expected type `int`, and `abc`

#### Scenario: int rejects float and whitespace forms

- **GIVEN** a job declaring `count: {type: int}`
- **WHEN** it runs with `--arg count=3.5` or `--arg count=" 42"`
- **THEN** it exits 2 before boot naming `count` and `int`

#### Scenario: bool accepts case-insensitive true and false

- **GIVEN** a job declaring `verbose: {type: bool}` with `${arg:verbose}` referenced in its `body`
- **WHEN** it runs with `--arg verbose=TRUE` and later with `--arg verbose=False`
- **THEN** the body interpolates the canonical form `true`, respectively `false`, and each run exits 0

#### Scenario: bool rejects numeric and unknown spellings

- **GIVEN** a job declaring `verbose: {type: bool}`
- **WHEN** it runs with `--arg verbose=1`, with `--arg verbose=0`, or with `--arg verbose=yes`
- **THEN** each run exits 2 before boot and the error names `verbose`, the expected type `bool`, and the raw value

#### Scenario: enum accepts a member and substitutes it verbatim

- **GIVEN** a job declaring `tier: {type: "enum[bronze,gold]"}` with `${arg:tier}` referenced in its `headers`
- **WHEN** it runs with `--arg tier=gold`
- **THEN** the header interpolates `gold` and the exit code is 0

#### Scenario: enum rejects an outsider and lists the members

- **GIVEN** a job declaring `tier: {type: "enum[bronze,gold]"}`
- **WHEN** it runs with `--arg tier=silver`
- **THEN** it exits 2 before boot and the error names `tier`, `silver`, and lists `bronze` and `gold`

#### Scenario: typed default coerces without a CLI pair

- **GIVEN** a job declaring `count: {type: int, default: "007"}` with `${arg:count}` referenced in its `to`
- **WHEN** it runs without `--arg count=...`
- **THEN** the send target interpolates `7` and the exit code is 0

#### Scenario: typed default failing coercion fails load and help

- **GIVEN** a job declaring `count: {type: int, default: "abc"}`
- **WHEN** the job runs, or `camel job <name> --help` runs
- **THEN** both exit 2 with a declaration error naming `count`, `int`, and `abc`

#### Scenario: unknown-name failure precedes coercion

- **GIVEN** a job declaring `count: {type: int}` and nothing else
- **WHEN** it runs with `--arg ghost=1 --arg count=abc`
- **THEN** it exits 2 naming `ghost` as the unknown argument, not a coercion error for `count`

#### Scenario: coerced values reach every interpolation site

- **GIVEN** a job declaring `target: {type: "enum[direct:in,direct:out]"}`, `count: {type: int, default: "7"}`, `verbose: {type: bool}` referenced in `to`, `body`, and `headers`, and `wait: {type: int, default: "30"}` referenced in `timeout: "${arg:wait}s"`
- **WHEN** it runs with `--arg verbose=false --arg target=direct:out`
- **THEN** every reference resolves to the canonical form before field validation and the exit code is 0

#### Scenario: missing-required failure precedes coercion

- **GIVEN** a job declaring `name: {type: string, required: true}` without a default and `count: {type: int}`
- **WHEN** it runs with `--arg count=abc` and no `--arg name=...`
- **THEN** it exits 2 naming `name` as the missing required argument, not a coercion error for `count`

#### Scenario: compiled artifact coerces embedded typed defaults

- **GIVEN** one job document declaring `count: {type: int, default: "007"}` with `${arg:count}` in its `to`, compiled into an artifact and run normally
- **WHEN** both runs resolve `${arg:count}`
- **THEN** both interpolate `7`; and compiling an otherwise identical document whose `default` fails coercion (for example `abc`) exits 2 at compile time and produces no artifact

## MODIFIED Requirements

### Requirement: supported job execution modes

A job document SHALL contain exactly one top-level `execute:` mapping. The
mapping's `mode` key SHALL accept `one-shot` and `batch`. Any other value
SHALL be rejected at load with an unsupported-mode error naming both
accepted values, and the process SHALL exit with code 2 before any boot.
Execution sequences, named execution registries, and executable job
classes, traits, or registries are PERMANENT NON-GOALS.

#### Scenario: unsupported mode is rejected at load

- **GIVEN** a job document declaring `mode: stream` (or any value other
  than `one-shot` and `batch`)
- **WHEN** `camel job` loads the document
- **THEN** loading fails with an unsupported-mode error naming the
  accepted values and the process exits with code 2 before any boot

#### Scenario: execute sequence is rejected at load

- **GIVEN** a job document whose top-level `execute:` value is a sequence
  of execution mappings
- **WHEN** `camel job` loads the document
- **THEN** parsing fails with a loud document diagnostic and the process
  exits with code 2 before any boot

### Requirement: declared job arguments

A job document MAY contain a top-level `args:` mapping as a sibling of
`execute:`. Argument names SHALL match `[A-Za-z_][A-Za-z0-9_]*`. Each
argument declaration SHALL accept only `required` (boolean, default
`false`), `default` (string), `description` (string), and `type` (string,
optional). The `type` value SHALL be one of `string`, `int`, `bool`, or
`enum[...]`: `enum[...]` is a single string scalar holding a comma-
separated member list where each member is trimmed, non-empty, unique
after trimming, and free of `,`, `[`, `]`, CR, and LF; the list SHALL
declare at least one member. Any other `type` value — unknown word,
non-string scalar, malformed enum grammar — SHALL fail document load with
a declaration diagnostic and exit 2. A `type` declaring `string`, or an
omitted `type`, SHALL keep the A2 string behavior unchanged. A typed
declaration whose `default` fails coercion SHALL fail document load with
the same declaration-error class (shared by execution and `--help`
parsing). The document and each declaration SHALL reject unknown fields.

#### Scenario: valid declaration is parsed

- **GIVEN** a `.job.yaml` document with `args: {name: {required: true, description: "Customer name"}}`
- **WHEN** the document is loaded
- **THEN** the declaration is accepted as a top-level sibling of `execute:`

#### Scenario: malformed declaration is rejected

- **GIVEN** an argument declaration containing `requried: true`
- **WHEN** the document is loaded
- **THEN** loading fails with an unknown-field diagnostic and exit 2 before boot

#### Scenario: each type spelling is accepted

- **GIVEN** a `.job.yaml` document declaring one argument per type spelling: `a: {type: string}`, `b: {type: int}`, `c: {type: bool}`, and `d: {type: "enum[x,y]"}`
- **WHEN** the document is loaded
- **THEN** all four declarations are accepted and `a` behaves identically to an untyped declaration

#### Scenario: unknown type word is rejected

- **GIVEN** an argument declaration containing `type: flot`
- **WHEN** the document is loaded
- **THEN** loading fails with a declaration diagnostic naming the argument and `flot`, and exit 2

#### Scenario: malformed enum grammar is rejected

- **GIVEN** argument declarations containing `type: "enum[]"`, `type: "enum[a,,b]"`, or `type: "enum[a,a]"`
- **WHEN** the document is loaded
- **THEN** loading fails with a declaration diagnostic naming the argument and the malformed value, and exit 2

#### Scenario: enum members are trimmed and case is preserved

- **GIVEN** an argument declaration containing `type: "enum[gold, silver]"`
- **WHEN** the document is loaded
- **THEN** the declared members are `gold` and `silver`, and a later value `Gold` fails membership exactly

### Requirement: declared argument validation

When `args:` is present, `--arg NAME=VALUE` SHALL name a declared argument.
Unknown names SHALL fail with a diagnostic naming the name. A required argument
without a default SHALL fail when omitted. Optional arguments with defaults
SHALL receive their default. Explicit values SHALL override defaults. After
name resolution, required checking, and default application, every typed
argument's resolved value SHALL be coerced to its declared type per the
`typed argument coercion` requirement before interpolation. All such
validation failures SHALL use exit 2.

#### Scenario: unknown declared argument fails

- **GIVEN** a job declaring only `name`
- **WHEN** it runs with `--arg tier=gold`
- **THEN** it exits 2 and names `tier` in the error

#### Scenario: required argument is missing

- **GIVEN** a job declaring `name: {required: true}` without a default
- **WHEN** it runs without `--arg name=...`
- **THEN** it exits 2 and names `name` in the error

#### Scenario: default applies

- **GIVEN** a job declaring `tier: {default: gold}`
- **WHEN** it runs without `--arg tier=...`
- **THEN** `${arg:tier}` resolves to `gold`

### Requirement: argument interpolation

`${arg:NAME}` SHALL resolve at the same interpolation stage and through the
same scanner as `${env:NAME}`. The argument grammar is exactly
`${arg:NAME}` with an identifier `NAME`; the `${arg:NAME:-fallback}` form is
not supported. Resolved arguments SHALL be available in `to`, `body`,
`headers`, and `timeout`; unresolved names SHALL fail with exit 2. When the
declaration carries a `type`, the reference SHALL substitute the coerced
value's canonical string form; without a `type`, the value substitutes
verbatim (A2 behavior). Normal jobs accept declared values through
`--arg`; compiled artifacts use embedded defaults — coerced through the
same rules at startup — and reject required declarations without defaults
at startup while retaining their existing narrow argument surface.

#### Scenario: all job fields interpolate

- **GIVEN** declared arguments `target: {default: "direct:in"}`, `text: {default: "hello"}`, `header: {default: "gold"}`, and `wait: {default: "30s"}` referenced in `to`, `body`, `headers`, and `timeout`
- **WHEN** the job runs
- **THEN** each reference resolves to the same string value before field validation

#### Scenario: compiled artifact matches normal job

- **GIVEN** one job document declaring `value: {default: "hello"}` compiled into an artifact and run normally
- **WHEN** both runs resolve `${arg:value}`
- **THEN** both paths produce identical interpolated route and message data, the artifact rejects `--arg value=other`, and a compiled declaration with `required: true` and no default exits 2

### Requirement: argument validation exit status

Every argument parse, declaration, unknown-name, missing-required,
coercion, or interpolation validation failure SHALL exit 2. No new exit
code SHALL be added.

#### Scenario: malformed CLI argument exits 2

- **GIVEN** `--arg nameonly`
- **WHEN** the CLI parses arguments
- **THEN** it exits 2 before boot

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
present. Each argument SHALL render on one line: any maximal run of CR and
LF characters inside a `default` value or `description` SHALL render as a
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
`timeout` presence) and SHALL NOT resolve `--arg` pairs, apply defaults,
interpolate, or validate execution values (timeout duration, send
scheme). Resolution, structural, and declaration failures SHALL exit 2
with the existing loud diagnostics.

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
