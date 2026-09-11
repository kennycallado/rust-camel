# cli-jobs Specification

## Purpose
TBD - created by archiving change cli-jobs. Update Purpose after archive.
## Requirements
### Requirement: execute-section document parsing

The `execute:` section SHALL parse from the reserved `*.test.yaml` /
`*.test.yml` document family, SHALL be mutually exclusive with the
`scenario:` section and the unit-tier vocabulary (`inputs`, `expects`,
`intercepts`, `beans`, `repositories`, `sequence`, `settle`, `env`), and
SHALL declare exactly one route source (`routeFiles`, `routeFilesFromRoot`,
or `routes`) under the family conflict rule. The section SHALL carry a
mandatory `mode`, a mandatory positive humantime `timeout`, and exactly
one `send` action whose target uses the `direct:` or `seda:` scheme.
Bodies SHALL be restricted to string, object, and array forms.

#### Scenario: valid one-shot document parses

- **GIVEN** a `*.test.yaml` document whose only sections are `execute:`
  (mode `one-shot`, a `timeout`, one `direct:` send) and one route
  source key
- **WHEN** `camel job` loads the document
- **THEN** parsing succeeds and the route source resolves under the
  family semantics (`routeFiles` against the document directory,
  `routeFilesFromRoot` against the nearest ancestor `Camel.toml`)

#### Scenario: execute is mutually exclusive with scenario and expects

- **GIVEN** a document declaring `execute:` together with `scenario:` or
  any unit-tier section such as `expects:`
- **WHEN** the document is loaded by `camel job` or dispatched by
  `camel test`
- **THEN** loading fails with a mutual-exclusion error naming the mixed
  sections, and `camel test` refuses the document with a pointer to
  `camel job`

#### Scenario: missing or invalid timeout is rejected

- **GIVEN** a one-shot document whose `execute:` section omits `timeout`
  or declares a non-positive or unparsable value
- **WHEN** `camel job` loads the document
- **THEN** loading fails with an error naming `execute.timeout`, and the
  process exits with code 2

### Requirement: batch mode is reserved and rejected

The `mode` key SHALL accept only `one-shot` in v1. The value `batch`
SHALL parse but SHALL be rejected at load with an error stating that
batch mode is not available yet; any other value SHALL be rejected as an
unsupported mode.

#### Scenario: batch mode is rejected at load

- **GIVEN** a job document declaring `mode: batch`
- **WHEN** `camel job` loads the document
- **THEN** loading fails with a "batch mode is not available yet" error
  and the process exits with code 2 before any boot

### Requirement: fail-closed consumer scheme allowlist

At load time, `camel job` SHALL reject documents whose discovered route
definitions consume (`from:` URI) from any scheme outside the job-safe
allowlist `{direct, seda, log, mock}`. Producer and sink `to:` URIs
SHALL NOT be restricted.

#### Scenario: non-job-safe consumer scheme is rejected

- **GIVEN** a job document whose routes include a route with
  `from: kafka:topic` (or any scheme outside the allowlist)
- **WHEN** `camel job` loads the routes
- **THEN** the document is rejected with an error naming the offending
  route, its from-URI, and the allowlist, and the process exits with
  code 2

#### Scenario: producers are unrestricted

- **GIVEN** a job document whose target route consumes from
  `direct:in` and contains `to: http://...` and `to: log:out` steps
- **WHEN** `camel job` loads the routes
- **THEN** the consumer gate passes and the job runs

### Requirement: one-shot send with side-effect-safe route startup

`camel job` SHALL boot the real composition root (the `camel run`
seams), SHALL force `auto_startup = false` on every discovered route
except the send target's consumer route (forced on), and SHALL reject a
send target with no matching consumer route at load, as well as a send
target whose base is ambiguous across multiple consumer routes. It
SHALL send exactly one exchange to the document's target and SHALL wrap
send, drain, and teardown in the mandatory overall `timeout` (anchored
at process start, covering boot). For `seda:` targets the send SHALL
carry `waitForTaskToComplete=Always` so the producer awaits the
pipeline result regardless of exchange pattern.

#### Scenario: only the target route starts

- **GIVEN** a job document whose routes include the send target's
  `direct:` consumer route and one unrelated `seda:` consumer route
- **WHEN** the job starts the context
- **THEN** only the target route's consumer is started; the unrelated
  consumer route stays stopped for the whole job

#### Scenario: seda send is synchronous

- **GIVEN** a job document targeting `seda:work` whose route pipeline
  fails during execution
- **WHEN** the send completes
- **THEN** the failure surfaces as the job's outcome (exit 1, outcome
  `Failed`), not a fire-and-forget `Completed`

#### Scenario: overall timeout expiry

- **GIVEN** a job whose send does not complete within the declared
  `timeout`
- **WHEN** the deadline expires
- **THEN** the JSON report carries outcome `Timeout` with a
  drain-timeout-class error and the process exits with code 2

### Requirement: exit-code taxonomy and JSON report

`camel job` SHALL exit 0 when the pipeline completes, 1 when the
pipeline fails, and 2 for load, validation, boot, drain-timeout,
shutdown, and report-write errors; when classes mix, exit 2 SHALL
outrank exit 1. These codes govern command completion only; termination
by OS signal remains platform-defined. For outcomes that reach the send
(verdict or timeout)
and for shutdown failures after a recorded verdict, the command SHALL
emit a JSON report
(`{document, mode, outcome, terminated_early, duration_ms, reply?,
error?}`) to stdout, or to the `--report` path when given; early
exit-2 classes (load, validation, boot) are stderr-only. When
`capture-reply` is set and a reply exchange was returned, the report
SHALL carry the reply body and headers.

#### Scenario: completed job with captured reply

- **GIVEN** a job document targeting a `direct:` route that rewrites the
  body, with `capture-reply: true`
- **WHEN** `camel job` runs the document
- **THEN** the process exits 0 and the report shows outcome
  `Completed` with the reply body produced by the route

#### Scenario: failed pipeline exits 1

- **GIVEN** a job document whose target route fails during execution
- **WHEN** `camel job` runs the document
- **THEN** the process exits 1 and the report shows outcome `Failed`
  with a non-empty error

#### Scenario: shutdown failure after a verdict forces exit 2

- **GIVEN** a job whose pipeline completed but whose teardown fails or
  exceeds its budget
- **WHEN** the report is emitted
- **THEN** the report error carries the shutdown detail and the process
  exits 2

