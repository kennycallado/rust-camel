# cli-jobs delta — argdrop

Removes the `--arg NAME=VALUE` static flag from the `camel job` CLI
surface. Dynamic `--<name>` flags (from the document's declared
`args:` block) remain the sole CLI surface for declared arguments;
the legacy implicit-header path is removed with the flag.

## REMOVED Requirements

### Requirement: arg flag header injection

Removed with the `--arg` flag: no static pair flag exists on the
`camel job` surface. Declared arguments are supplied through dynamic
`--<name>` flags (see `dynamic declared-argument flags`); document
`send.headers` remain the author-side header mechanism.

### Requirement: legacy argument compatibility

Removed: the legacy implicit-header path (`--arg` pairs as send-time
headers on documents without `args:`) and its deprecation note no
longer exist. rc-uxvm6 (last-wins/empty-value on that path) is moot.

### Requirement: declared arguments replace legacy header injection

Removed as moot: with no `--arg` flag there is no implicit header
injection for declared documents to replace. Declared values resolve
through declarations and interpolation only.

### Requirement: cross-form argument conflicts

Removed as moot: with a single supply form (dynamic flags) there is
no cross-form conflict to reject. Last-wins within one form still
applies (see `dynamic declared-argument flags`).

## MODIFIED Requirements

### Requirement: batch mode drains until empty

A `mode: batch` job SHALL boot the same composition root (including the
job boot projection), apply the same send-target validation, start all
document routes, and send one trigger exchange. It SHALL retain the same
declared-argument validation, defaulting, and interpolation as one-shot
execution.
For a `seda:` target it SHALL rewrite `waitForTaskToComplete` to `Always` so
the send is synchronous.
After the trigger send it SHALL wait until the context-global
in-flight counter (`CamelContext::total_in_flight()`) reads zero, then
emit the existing JSON report with outcome `Completed` and mode `batch`,
and exit 0. The counter covers every exchange accepted through a counted
path (seda enqueue, channel dispatch, inline dispatch, and the
raw-sender acceptance dequeues of http/grpc/ws/master per rc-nftni;
gRPC streaming calls hold their acceptance claim call-scoped, so
inter-chunk idle gaps stay counted); bounded non-exchange residuals
(transport intake, post-completion egress) are documented in the
observability spec. The drain verdict
SHALL be that single atomic read; it SHALL NOT depend on timed gauge
samples, queue-depth labels, or any fixed quiescence window. A batch job
with no `seda:` consumer routes completes as soon as the counter reads
zero after the trigger send. The declared timeout bounds boot, send,
drain, and teardown; expiry reports `Timeout` with exit 2, while
trigger-send pipeline failure reports `Failed` with exit 1. A batch
document whose routes include a scheduled auto-firing consumer
(`timer:`, `cron:`) SHALL be rejected at load by the fail-closed
consumer-scheme gate, so scheduled recurring producers cannot perturb
the drain verdict after the trigger send.

#### Scenario: batch applies declared defaults

- **GIVEN** a batch job declaring `tier: {default: gold}` and using `${arg:tier}`
- **WHEN** it runs without `--tier`
- **THEN** the default is resolved before the trigger send and the batch drains normally

#### Scenario: batch drains a fan-out pipeline then exits 0

- **GIVEN** a batch job whose trigger route fans out through one or more `seda:` queues
- **WHEN** the job runs
- **THEN** it waits for in-flight work to reach zero, reports `Completed` with mode `batch`, and exits 0

#### Scenario: batch does not complete while work is in flight

- **GIVEN** a batch job whose worker route holds each exchange for a short
  delay before recording it to a `file:` sink
- **WHEN** the drain observes the counter while the worker route's
  in-flight count is still nonzero
- **THEN** the job does not complete from that observation: exit happens only
  after the worker recorded the exchange

#### Scenario: a barrier-parked self-feeding route cannot false-complete

- **GIVEN** a batch job whose route cycles
  `from: seda:a -> park on a deterministic barrier -> to: seda:a` forever,
  with an overall timeout allowing several polls while an exchange is
  parked
- **WHEN** the job runs to its deadline
- **THEN** it reports `Timeout` and exits 2; it never reports `Completed`
  while a cycled exchange is parked inside the route

#### Scenario: settled work completes without a quiescence window

- **GIVEN** a batch job whose exchanges have all completed
- **WHEN** the drain polls after the trigger send
- **THEN** the job completes promptly on the first zero read; no fixed
  multi-second quiescence window is imposed before the `Completed` verdict

#### Scenario: the drain verdict is one linearizable read

- **GIVEN** a batch job with work still moving between a seda queue and a
  route pipeline
- **WHEN** the drain samples the counter
- **THEN** the verdict uses a single `total_in_flight()` load that cannot
  observe an intermediate handoff state: any exchange accepted through a
  counted path and mid-lifecycle keeps the read nonzero

#### Scenario: scheduled recurring consumer routes are rejected at load

- **GIVEN** a batch document declaring a route with `from: timer:...` or
  `from: cron:...`
- **WHEN** the job loads the document
- **THEN** the route is rejected by the fail-closed consumer-scheme gate
  before any route starts, and the job fails at load rather than
  admitting a scheduled recurring producer

#### Scenario: batch overall timeout expiry

- **GIVEN** a batch job whose `seda:` work does not drain before `timeout`
- **WHEN** the deadline expires
- **THEN** it reports `Timeout` and exits 2

#### Scenario: batch works with arg injection

- **GIVEN** a batch job declaring `batch_id` whose worker routes
  record the resolved value
- **WHEN** `camel job` runs the document with the dynamic flag
  `--batch_id 42` (the `--arg` form is removed)
- **THEN** the job drains, exits 0, and the recorded exchanges carry
  the resolved `batch_id` value

### Requirement: declared argument validation

When `args:` is present, argument values SHALL be supplied only
through each declared argument's dynamic flag. A required argument
without a default SHALL fail when its dynamic flag is absent.
Optional arguments with defaults SHALL receive their default.
Explicit flag values SHALL override defaults. After name resolution,
required checking, and default application, every typed argument's
resolved value SHALL be coerced to its declared type per the
`typed argument coercion` requirement before interpolation. All such
validation failures SHALL use exit 2.

#### Scenario: required argument is missing

- **GIVEN** a job declaring `name: {required: true}` without a default
- **WHEN** it runs without `--name`
- **THEN** it exits 2 and names `name` in the error

#### Scenario: default applies

- **GIVEN** a job declaring `tier: {default: gold}`
- **WHEN** it runs without `--tier`
- **THEN** `${arg:tier}` resolves to `gold`

#### Scenario: unknown declared argument fails

- **GIVEN** a job declaring only `name`
- **WHEN** it runs with `--tier gold`
- **THEN** it exits 2 and names `tier` in the error

### Requirement: argument interpolation

`${arg:NAME}` SHALL resolve at the same interpolation stage and through the
same scanner as `${env:NAME}`. The argument grammar is exactly
`${arg:NAME}` with an identifier `NAME`; the `${arg:NAME:-fallback}` form is
not supported. Resolved arguments SHALL be available in `to`, `body`,
`headers`, and `timeout`; unresolved names SHALL fail with exit 2. When the
declaration carries a `type`, the reference SHALL substitute the coerced
value's canonical string form; without a `type`, the value substitutes
verbatim (A2 behavior). Normal jobs accept declared values through
dynamic flags; compiled artifacts use embedded defaults — coerced through
the same rules at startup — and reject required declarations without
defaults at startup while retaining their existing narrow argument
surface.

#### Scenario: all job fields interpolate

- **GIVEN** declared arguments `target: {default: "direct:in"}`, `text: {default: "hello"}`, `header: {default: "gold"}`, and `wait: {default: "30s"}` referenced in `to`, `body`, `headers`, and `timeout`
- **WHEN** the job runs
- **THEN** each reference resolves to the same string value before field validation

#### Scenario: compiled artifact matches normal job

- **GIVEN** one job document declaring `value: {default: "hello"}` compiled into an artifact and run normally
- **WHEN** both runs resolve `${arg:value}`
- **THEN** both paths produce identical interpolated route and message data, the artifact rejects `--arg value=other` as an unknown flag, and a compiled declaration with `required: true` and no default exits 2

### Requirement: argument validation exit status

Every argument parse, declaration, missing-required, coercion, or
interpolation validation failure SHALL exit 2. No new exit code
SHALL be added.

#### Scenario: malformed CLI argument exits 2

- **GIVEN** a dynamic flag invocation missing its value (for example `--name` as the last token)
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
present. When at least one argument is declared, the `Arguments:` table
SHALL carry exactly one additional note line stating that each argument
is settable as `--<name> <VALUE>` and bool arguments as bare `--<name>` /
`--no-<name>`; the note's exact
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
NOT apply defaults, interpolate, or validate
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
  the `--<name> <VALUE>` and `--<name>` / `--no-<name>` spellings,
  and the exit code is 0

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
- **WHEN** `camel job <name> --help` runs without any dynamic flag
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

### Requirement: typed argument coercion

When a declaration carries `type:`, the argument's resolved value — the
last dynamic-flag value or the applied `default` — SHALL be coerced
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
SHALL list the allowed members. Missing-required
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
- **WHEN** it runs with `--count 007`
- **THEN** the send target interpolates `7` and the exit code is 0

#### Scenario: int rejects a non-integer value

- **GIVEN** a job declaring `count: {type: int}`
- **WHEN** it runs with `--count abc`
- **THEN** it exits 2 before boot and the error names `count`, the expected type `int`, and `abc`

#### Scenario: int rejects float and whitespace forms

- **GIVEN** a job declaring `count: {type: int}`
- **WHEN** it runs with `--count 3.5` or `--count " 42"`
- **THEN** it exits 2 before boot naming `count` and `int`

#### Scenario: bool canonicalizes case-insensitive defaults

- **GIVEN** a job declaring `verbose: {type: bool, default: "TRUE"}` with `${arg:verbose}` referenced in its `body`
- **WHEN** it runs without a `verbose` flag
- **THEN** the body interpolates the canonical form `true` and the exit code is 0

#### Scenario: bool rejects numeric and unknown default spellings

- **GIVEN** a job declaring `verbose: {type: bool, default: "yes"}`
- **WHEN** the document loads
- **THEN** loading exits 2 naming `verbose`, the expected type `bool`, and the raw value

#### Scenario: bool accepts case-insensitive true and false

- **GIVEN** a job declaring `verbose: {type: bool}` with `${arg:verbose}` referenced in its `body`
- **WHEN** it runs with `--verbose TRUE` and later with `--verbose False`
- **THEN** the body interpolates the canonical form `true`, respectively `false`, and each run exits 0

#### Scenario: bool rejects numeric and unknown spellings

- **GIVEN** a job declaring `verbose: {type: bool}`
- **WHEN** it runs with `--verbose 1`, with `--verbose 0`, or with `--verbose yes`
- **THEN** each run exits 2 before boot and the error names `verbose`, the expected type `bool`, and the raw value

#### Scenario: unknown-name failure precedes coercion

- **GIVEN** a job declaring `count: {type: int}` and nothing else
- **WHEN** it runs with `--ghost 1 --count abc`
- **THEN** it exits 2 naming `ghost` as the unknown argument, not a coercion error for `count`

#### Scenario: enum accepts a member and substitutes it verbatim

- **GIVEN** a job declaring `tier: {type: "enum[bronze,gold]"}` with `${arg:tier}` referenced in its `headers`
- **WHEN** it runs with `--tier gold`
- **THEN** the header interpolates `gold` and the exit code is 0

#### Scenario: enum rejects an outsider and lists the members

- **GIVEN** a job declaring `tier: {type: "enum[bronze,gold]"}`
- **WHEN** it runs with `--tier silver`
- **THEN** it exits 2 before boot and the error names `tier`, `silver`, and lists `bronze` and `gold`

#### Scenario: typed default coerces without a CLI pair

- **GIVEN** a job declaring `count: {type: int, default: "007"}` with `${arg:count}` referenced in its `to`
- **WHEN** it runs without `--count`
- **THEN** the send target interpolates `7` and the exit code is 0

#### Scenario: typed default failing coercion fails load and help

- **GIVEN** a job declaring `count: {type: int, default: "abc"}`
- **WHEN** the job runs, or `camel job <name> --help` runs
- **THEN** both exit 2 with a declaration error naming `count`, `int`, and `abc`

#### Scenario: coerced values reach every interpolation site

- **GIVEN** a job declaring `target: {type: "enum[direct:in,direct:out]"}`, `count: {type: int, default: "7"}`, `verbose: {type: bool}` referenced in `to`, `body`, and `headers`, and `wait: {type: int, default: "30"}` referenced in `timeout: "${arg:wait}s"`
- **WHEN** it runs with `--no-verbose --target direct:out`
- **THEN** every reference resolves to the canonical form before field validation and the exit code is 0

#### Scenario: missing-required failure precedes coercion

- **GIVEN** a job declaring `name: {type: string, required: true}` without a default and `count: {type: int}`
- **WHEN** it runs with `--count abc` and no `--name`
- **THEN** it exits 2 naming `name` as the missing required argument, not a coercion error for `count`

#### Scenario: compiled artifact coerces embedded typed defaults

- **GIVEN** one job document declaring `count: {type: int, default: "007"}` with `${arg:count}` in its `to`, compiled into an artifact and run normally
- **WHEN** both runs resolve `${arg:count}`
- **THEN** both interpolate `7`; and compiling an otherwise identical document whose `default` fails coercion (for example `abc`) exits 2 at compile time and produces no artifact

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
occurrence. Dynamic flags SHALL lower into NAME=VALUE pairs before
coercion, defaults, required checks, and interpolation.
Values of declared `int`, `enum`, and
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
- **WHEN** it runs with `camel job <doc> --name world`
- **THEN** the run exits 0 and interpolates `world` into the body

#### Scenario: both value forms accepted

- **GIVEN** a job document declaring `name`
- **WHEN** it runs with `--name=world`
- **THEN** the run is indistinguishable from `--name world`

#### Scenario: last occurrence wins within the form

- **GIVEN** a job document declaring `name`
- **WHEN** it runs with `--name first --name second`
- **THEN** the resolved value is `second`

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

- **GIVEN** invocations placing `--help`/`-h`, `--report`,
  or `--config` after the document reference — before or after a
  dynamic flag
- **WHEN** the run is parsed
- **THEN** each flag keeps its pre-existing meaning (manual job help,
  report path, config load): flags clap consumes
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
the accepted spellings `--flag` and `--no-flag`. A stray token after a
bool flag (`--flag false`)
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
  `--verbose` and `--no-verbose`

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

### Requirement: dynamic flags require an args block

A dynamic flag passed to a job document WITHOUT a top-level `args:`
block SHALL fail (exit 2) with a diagnostic naming the flag and
pointing at the remedy: declaring an `args:` block. The dynamic flag
SHALL NOT fall back to header injection.

#### Scenario: dynamic flag on an undeclared document fails

- **GIVEN** a job document with no `args:` block
- **WHEN** it runs with `--name x`
- **THEN** it exits 2 before boot; the diagnostic names the flag `name` and
  mentions the `args:` block, and no header injection occurs

### Requirement: reserved argument names

A job document declaring a top-level argument named `help`, `config`,
or `report` SHALL fail document load (exit 2) with a
reserved-name diagnostic naming the argument and the static flag it
collides with. The rejection SHALL occur in BOTH execution parsing
and `camel job <name> --help` parsing, and in `camel compile`'s
declaration validation, before any flag parsing of the document's
arguments.

#### Scenario: every reserved name fails execution

- **GIVEN** job documents each declaring one argument named `help`,
  `config`, or `report`
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
