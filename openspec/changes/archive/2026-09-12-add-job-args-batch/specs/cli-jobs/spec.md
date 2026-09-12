## REMOVED Requirements

### Requirement: batch mode is reserved and rejected

## MODIFIED Requirements

### Requirement: exit-code taxonomy and JSON report

`camel job` SHALL exit 0 when the pipeline completes, 1 when the pipeline
fails, and 2 for load, validation, boot, drain-timeout, shutdown, and
report-write errors; when classes mix, exit 2 SHALL outrank exit 1. These
codes govern command completion only; termination by OS signal remains
platform-defined. For outcomes that reach the send (verdict or timeout)
and for shutdown failures after a recorded verdict, the command SHALL emit
a JSON report (`{document, mode, outcome, terminated_early, duration_ms,
reply?, error?, shutdown_error?}`) to stdout, or to the `--report` path
when given; early exit-2 classes (load, validation, boot) are stderr-only.
When `capture-reply` is set and a reply exchange was returned, the report
SHALL carry the reply body and headers. When a shutdown failure follows a
recorded verdict and the teardown ran with a nonzero budget, the report
SHALL carry the teardown detail in `shutdown_error`, while `error` keeps
the pipeline or timeout verdict; a teardown foregone at zero remaining
budget is stderr-only and SHALL NOT add a `shutdown_error` entry. A
batch-mode drain that completes within the overall timeout reports outcome
`Completed` with mode `batch`.

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
- **THEN** the report carries the shutdown detail in `shutdown_error`
  and the process exits 2

#### Scenario: failed verdict plus shutdown failure keeps both details

- **GIVEN** a job whose pipeline failed and whose teardown also fails
- **WHEN** the report is emitted
- **THEN** `error` holds the pipeline failure, `shutdown_error` holds the
  teardown detail, and the process exits 2

## ADDED Requirements

### Requirement: supported job execution modes

The `mode` key SHALL accept `one-shot` and `batch`. Any other value SHALL
be rejected at load with an unsupported-mode error naming both accepted
values, and the process SHALL exit with code 2 before any boot.

#### Scenario: unsupported mode is rejected at load

- **GIVEN** a job document declaring `mode: stream` (or any value other
  than `one-shot` and `batch`)
- **WHEN** `camel job` loads the document
- **THEN** loading fails with an unsupported-mode error naming the
  accepted values and the process exits with code 2 before any boot

### Requirement: arg flag header injection

`camel job` SHALL accept a repeatable `--arg NAME=VALUE` flag. Each pair
SHALL inject one string header onto the trigger exchange at send time,
applied after the document's `send.headers`, so a CLI value overrides a
document header with the same name. When the same name repeats on the
command line, the last occurrence SHALL win. Values SHALL be raw strings
with no interpolation. A malformed pair (no `=`, or an empty name) SHALL
fail as a usage error with exit 2 before any boot. The flag SHALL work in
both execution modes.

#### Scenario: single and repeated args reach the route

- **GIVEN** a job document whose target route records exchange headers to
  a `mock:` endpoint
- **WHEN** `camel job` runs the document with `--arg name=John` and
  `--arg tier=gold`
- **THEN** the job exits 0 and the recorded exchange carries headers
  `name=John` and `tier=gold`

#### Scenario: arg overrides a colliding document header

- **GIVEN** a job document whose `send.headers` declares `name: Doc` and
  whose target route records exchange headers to a `mock:` endpoint
- **WHEN** `camel job` runs the document with `--arg name=Cli`
- **THEN** the job exits 0 and the recorded header `name` holds `Cli`

#### Scenario: malformed arg is a usage error

- **GIVEN** an `--arg` value with no `=` (for example `nameonly`) or an
  empty name (for example `=value`)
- **WHEN** `camel job` runs the document with that flag value
- **THEN** the process exits 2 with a usage error before any boot

### Requirement: batch mode drains until empty

A `mode: batch` job SHALL boot the same composition root, apply the same
send-target validation, start all document routes, and send the same
single trigger exchange as a one-shot job, including the `seda:`
`waitForTaskToComplete=Always` rewrite and `--arg` header injection.
After the trigger send the job SHALL drain: it SHALL wait until every
`seda:` consumer queue of the document's routes is empty, then emit the
JSON report with outcome `Completed` and mode `batch`, and exit 0. A
batch document with no `seda:` consumer routes SHALL complete immediately
after the trigger send. Zero-depth observations made before the trigger
send completes SHALL NOT satisfy the drain. The mandatory overall
`timeout` SHALL still bound boot, send, drain, and teardown; expiry
reports outcome `Timeout` with exit 2. A trigger-send pipeline failure
reports `Failed` with exit 1 under the existing taxonomy.

#### Scenario: batch drains a fan-out pipeline then exits 0

- **GIVEN** a batch job whose target route fans messages out through one
  or more `seda:` queues to worker routes
- **WHEN** `camel job` runs the document
- **THEN** the job waits until every document `seda:` queue is empty, the
  report shows outcome `Completed` with mode `batch`, and the process
  exits 0

#### Scenario: batch does not complete while work is in flight

- **GIVEN** a batch job whose worker route holds each exchange in flight
  for a short delay before recording it to a `mock:` endpoint
- **WHEN** the drain observes a queue-depth sample of zero while the
  exchange is still in flight
- **THEN** the job does not complete from that sample: exit happens only
  after the worker recorded the exchange

#### Scenario: batch overall timeout expiry

- **GIVEN** a batch job whose traffic does not quiesce within the
  declared `timeout`
- **WHEN** the deadline expires
- **THEN** the JSON report carries outcome `Timeout` with a
  drain-timeout-class error and the process exits with code 2

#### Scenario: batch works with arg injection

- **GIVEN** a batch job whose worker routes record exchange headers to a
  `mock:` endpoint
- **WHEN** `camel job` runs the document with `--arg batch-id=42`
- **THEN** the job drains, exits 0, and the recorded exchanges carry
  header `batch-id=42`
