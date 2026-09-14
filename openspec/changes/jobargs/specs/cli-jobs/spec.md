## ADDED Requirements

### Requirement: declared job arguments

A job document MAY contain a top-level `args:` mapping as a sibling of
`execute:`. Argument names SHALL match `[A-Za-z_][A-Za-z0-9_]*`. Each argument declaration SHALL accept only `required` (boolean,
default `false`), `default` (string), and `description` (string). The
document and each declaration SHALL reject unknown fields. Version 1 SHALL
support string values only.

#### Scenario: valid declaration is parsed

- **GIVEN** a `.job.yaml` document with `args: {name: {required: true, description: "Customer name"}}`
- **WHEN** the document is loaded
- **THEN** the declaration is accepted as a top-level sibling of `execute:`

#### Scenario: malformed declaration is rejected

- **GIVEN** an argument declaration containing `requried: true`
- **WHEN** the document is loaded
- **THEN** loading fails with an unknown-field diagnostic and exit 2 before boot

### Requirement: declared argument validation

When `args:` is present, `--arg NAME=VALUE` SHALL name a declared argument.
Unknown names SHALL fail with a diagnostic naming the name. A required argument
without a default SHALL fail when omitted. Optional arguments with defaults
SHALL receive their default. Explicit values SHALL override defaults. All such
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
`headers`, and `timeout`; unresolved names SHALL fail with exit 2. Normal jobs
accept declared values through `--arg`; compiled artifacts use embedded
defaults and reject required declarations without defaults at startup while
retaining their existing narrow argument surface.

#### Scenario: all job fields interpolate

- **GIVEN** declared arguments `target: {default: "direct:in"}`, `text: {default: "hello"}`, `header: {default: "gold"}`, and `wait: {default: "30s"}` referenced in `to`, `body`, `headers`, and `timeout`
- **WHEN** the job runs
- **THEN** each reference resolves to the same string value before field validation

#### Scenario: compiled artifact matches normal job

- **GIVEN** one job document declaring `value: {default: "hello"}` compiled into an artifact and run normally
- **WHEN** both runs resolve `${arg:value}`
- **THEN** both paths produce identical interpolated route and message data, the artifact rejects `--arg value=other`, and a compiled declaration with `required: true` and no default exits 2

### Requirement: legacy argument compatibility

When a document has no `args:` block, undeclared `--arg NAME=VALUE` pairs
SHALL continue to inject string headers after document headers. The command
SHALL emit a deprecation note identifying the legacy behavior. This path SHALL
remain available in both execution modes.

#### Scenario: no declaration preserves header injection

- **GIVEN** a job document with no top-level `args:` block
- **WHEN** it runs with `--arg name=John`
- **THEN** `name=John` reaches the trigger exchange header and stderr contains the deprecation note

### Requirement: declared arguments replace legacy header injection

The existing `arg flag header injection` requirement SHALL be modified so
`--arg` pairs inject headers only when the document has no top-level `args:`
block. Declared documents SHALL resolve values through their declarations and
interpolation surface instead of implicitly creating headers.

#### Scenario: declared argument is not an implicit header

- **GIVEN** a document declaring `name` and referencing `${arg:name}` in its body
- **WHEN** it runs with `--arg name=John`
- **THEN** the body receives `John` and no automatic `name` header is added

## MODIFIED Requirements

### Requirement: arg flag header injection

`camel job` SHALL accept a repeatable `--arg NAME=VALUE` flag. When the job
document has no top-level `args:` block, each pair SHALL inject one string
header onto the trigger exchange at send time, applied after document headers;
repeated names use the last value, values remain raw strings with no
interpolation, and a CLI value overrides a colliding document header. When
`args:` is present, each pair SHALL instead satisfy a declared argument and
SHALL NOT inject an implicit header. Malformed pairs and declared-argument
validation failures SHALL exit 2 before boot. The flag SHALL work in both
execution modes on the legacy path.

#### Scenario: declared and legacy paths differ

- **GIVEN** one document without `args:` and one document declaring `name`
- **WHEN** both run with `--arg name=John`
- **THEN** the first sends a `name=John` header and the second resolves only its declared interpolation

#### Scenario: legacy header precedence remains stable

- **GIVEN** a no-`args:` document with `send.headers.name: Doc`
- **WHEN** it runs with repeated `--arg name=First --arg name=Last`
- **THEN** the trigger exchange carries raw string header `name=Last`

#### Scenario: legacy single and repeated args reach route

- **GIVEN** a no-`args:` job whose target route records exchange headers
- **WHEN** it runs with `--arg name=John --arg tier=gold`
- **THEN** the recorded headers are `name=John` and `tier=gold`, with the last occurrence winning

#### Scenario: malformed legacy arg is a usage error

- **GIVEN** an `--arg` value with no `=` or with an empty name
- **WHEN** `camel job` parses the flag
- **THEN** it exits 2 before boot with a usage error

### Requirement: batch mode drains until empty

A `mode: batch` job SHALL boot the same composition root, apply the same
send-target validation, start all document routes, and send one trigger
exchange. It SHALL retain the same declared-argument validation, defaulting,
interpolation, and legacy no-`args:` header behavior as one-shot execution.
For a `seda:` target it SHALL rewrite `waitForTaskToComplete` to `Always` so
the send is synchronous.
After the trigger send it SHALL drain every `seda:` consumer queue until empty,
then emit the existing JSON report with outcome `Completed` and mode `batch`,
and exit 0. A batch job with no `seda:` consumer routes completes immediately.
Observations before the trigger send completes do not satisfy the drain. The
declared timeout bounds boot, send, drain, and teardown; expiry reports
`Timeout` with exit 2, while trigger-send pipeline failure reports `Failed`
with exit 1.

#### Scenario: batch applies declared defaults

- **GIVEN** a batch job declaring `tier: {default: gold}` and using `${arg:tier}`
- **WHEN** it runs without `--arg tier=...`
- **THEN** the default is resolved before the trigger send and the batch drains normally

#### Scenario: batch drains fan-out work

- **GIVEN** a batch job whose trigger route fans out through one or more `seda:` queues
- **WHEN** the job runs
- **THEN** it waits for in-flight work and queue emptiness, reports `Completed` with mode `batch`, and exits 0

#### Scenario: batch timeout covers drain

- **GIVEN** a batch job whose `seda:` work does not drain before `timeout`
- **WHEN** the deadline expires
- **THEN** it reports `Timeout` and exits 2

### Requirement: argument validation exit status

Every argument parse, declaration, unknown-name, missing-required, or
interpolation validation failure SHALL exit 2. No new exit code SHALL be added.

#### Scenario: malformed CLI argument exits 2

- **GIVEN** `--arg nameonly`
- **WHEN** the CLI parses arguments
- **THEN** it exits 2 before boot
