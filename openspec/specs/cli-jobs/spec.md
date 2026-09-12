# cli-jobs Specification

## Purpose
TBD - created by archiving change cli-jobs. Update Purpose after archive.
## Requirements
### Requirement: execute-section document parsing

The `execute:` section SHALL parse from the reserved `*.job.yaml` /
`*.job.yml` document family, SHALL be mutually exclusive with the
`scenario:` section and the unit-tier vocabulary (`inputs`, `expects`,
`intercepts`, `beans`, `repositories`, `sequence`, `settle`, `env`), and
SHALL declare exactly one route source (`routeFiles`, `routeFilesFromRoot`,
or `routes`) under the family conflict rule. The section SHALL carry a
mandatory `mode`, a mandatory positive humantime `timeout`, and exactly
one `send` action whose target uses the `direct:` or `seda:` scheme. The
document MAY declare an optional top-level `description` (string), shown
by job listing and rejected nowhere. Bodies SHALL be restricted to
string, object, and array forms. A document named `*.test.yaml` /
`*.test.yml` that declares `execute:` SHALL fail to load under
`camel job` with an error directing the author to rename the file to the
`.job.yaml` suffix.

#### Scenario: valid one-shot document parses

- **GIVEN** a `*.job.yaml` document whose only sections are `execute:`
  (mode `one-shot`, a `timeout`, one `direct:` send) and one route
  source key
- **WHEN** `camel job` loads the document
- **THEN** parsing succeeds and the route source resolves under the
  family semantics (`routeFiles` against the document directory,
  `routeFilesFromRoot` against the nearest ancestor `Camel.toml`)

#### Scenario: execute is mutually exclusive with scenario and expects

- **GIVEN** a `*.job.yaml` document declaring `execute:` together with
  `scenario:` or any unit-tier section such as `expects:`
- **WHEN** the document is loaded by `camel job`
- **THEN** loading fails with a mutual-exclusion error naming the mixed
  sections (test vocabulary does not belong in a job document)

#### Scenario: test-suffix document declaring execute is rejected

- **GIVEN** a `*.test.yaml` document declaring `execute:`
- **WHEN** `camel job` is asked to load it, or `camel test` dispatches
  it
- **THEN** `camel job` fails the load with an error directing the author
  to rename the file to `.job.yaml`, `camel test` refuses the document
  with a pointer to `camel job`, and the process exits with code 2

#### Scenario: missing or invalid timeout is rejected

- **GIVEN** a one-shot document whose `execute:` section omits `timeout`
  or declares a non-positive or unparsable value
- **WHEN** `camel job` loads the document
- **THEN** loading fails with an error naming `execute.timeout`, and the
  process exits with code 2

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

### Requirement: exit-code taxonomy and JSON report

`camel job` SHALL exit 0 when the pipeline completes, 1 when the
pipeline fails, and 2 for load, validation, boot, interruption,
drain-timeout, shutdown, and report-write errors; when classes mix, exit
2 SHALL outrank exit 1. A first SIGINT or SIGTERM SHALL cancel an in-flight
send or batch drain, perform bounded teardown, and report outcome
`Interrupted`. A second SIGINT or SIGTERM received during that teardown SHALL
force-exit with code 1. The signal streams SHALL be armed before config
loading so signals during boot are buffered rather than default-killing the
process. For outcomes that reach the send (verdict, interruption, or
timeout), and for shutdown failures after a recorded verdict, the command
SHALL emit a JSON report
(`{document, mode, outcome, terminated_early, duration_ms, reply?, error?, shutdown_error?}`)
to stdout, or to the `--report` path when given; early exit-2 classes remain
stderr-only. When `capture-reply` is set and a reply exchange is returned,
the report SHALL carry the reply body and headers. A batch drain timeout
SHALL report outcome `Timeout` and preserve the existing exit-2 behavior.
When a signal event is ready at the same poll point as send completion or
deadline expiry, signal handling SHALL win and report `Interrupted`. The
implementation SHALL use signal-first selection ordering to make this
precedence deterministic.

#### Scenario: first signal interrupts an active job

- **GIVEN** a `camel job` process has booted and its send is in flight
- **WHEN** the operator sends SIGINT or SIGTERM once
- **THEN** the send is cancelled, teardown runs under at least
  `MIN_SHUTDOWN_BUDGET`, the report outcome is `Interrupted`, and the
  process exits with code 2

#### Scenario: signal during boot is buffered

- **GIVEN** a `camel job` process is loading config or booting components
- **WHEN** the operator sends SIGINT or SIGTERM
- **THEN** the signal is handled by the job contract after boot reaches its
  signal-aware phase, rather than default-killing the process

On non-Unix platforms, the portable Ctrl+C listener is installed when first
polled; the boot-buffer guarantee applies to Unix SIGINT/SIGTERM streams.

#### Scenario: second signal force-exits during teardown

- **GIVEN** the first signal was consumed and bounded job teardown is running
- **WHEN** a second SIGINT or SIGTERM arrives before teardown completes
- **THEN** the process exits with code 1 without waiting for teardown

#### Scenario: interruption keeps apparatus precedence

- **GIVEN** a first signal interrupts a job and teardown also reports an error
- **WHEN** the JSON report is emitted
- **THEN** outcome remains `Interrupted`, shutdown detail is recorded, and
  the process exits with code 2

#### Scenario: signal interrupts batch drain

- **GIVEN** a batch job's send completed and its drain wait is still active
- **WHEN** the operator sends SIGINT or SIGTERM once
- **THEN** the drain wait is cancelled, teardown runs under the existing
  batch overall-deadline budget without a floor, the report outcome is
  `Interrupted`, and the process exits with code 2

#### Scenario: completed job with captured reply

- **GIVEN** a job completes and `capture-reply` is enabled
- **WHEN** the report is emitted
- **THEN** it contains outcome `Completed`, the captured reply body and
  headers, no `error` or `shutdown_error` for successful teardown, and the
  process exits 0

#### Scenario: failed pipeline exits 1

- **GIVEN** the target route pipeline fails during execution
- **WHEN** the report is emitted
- **THEN** it contains outcome `Failed` with a non-empty error and the
  no shutdown detail when teardown succeeds, and the process exits 1

#### Scenario: timeout and shutdown failure preserve precedence

- **GIVEN** send or batch drain exceeds the mandatory overall timeout, or
  teardown fails after a recorded verdict
- **WHEN** the report is emitted
- **THEN** timeout reports outcome `Timeout`, shutdown failure records
  `shutdown_error` only when teardown had a non-zero remaining budget, no
  shutdown detail when teardown succeeds, and either case exits 2

#### Scenario: shutdown failure after a verdict preserves both details

- **GIVEN** a pipeline verdict was recorded and teardown fails with a
  non-zero remaining budget
- **WHEN** the report is emitted
- **THEN** `error` preserves the pipeline or timeout verdict,
  `shutdown_error` carries teardown detail, and the process exits 2

#### Scenario: signal-first tie is deterministic

- **GIVEN** a signal and send completion become ready at the same poll point
- **WHEN** the command selects its next operation
- **THEN** the signal branch wins and the report outcome is `Interrupted`

#### Scenario: zero-budget shutdown failure is stderr-only

- **GIVEN** the overall deadline has already expired and teardown has no
  remaining budget
- **WHEN** shutdown reports its expected zero-budget failure
- **THEN** the report keeps outcome `Timeout`, the shutdown detail is written
  to stderr without replacing the verdict, and the process exits 2

#### Scenario: batch drain completion reports Completed

- **GIVEN** a batch job's send completes and all expected SEDA queues drain
- **WHEN** the report is emitted
- **THEN** it contains mode `batch`, outcome `Completed`, and the process
  exits 0

#### Scenario: shutdown failure after a verdict forces exit 2

- **GIVEN** a job whose pipeline completed but whose teardown fails or
  exceeds its budget
- **WHEN** the report is emitted
- **THEN** the report carries the shutdown detail in `shutdown_error` and
  the process exits 2

#### Scenario: failed verdict plus shutdown failure keeps both details

- **GIVEN** a job whose pipeline failed and whose teardown also fails
- **WHEN** the report is emitted
- **THEN** `error` holds the pipeline failure, `shutdown_error` holds the
  teardown detail, and the process exits 2

### Requirement: job discovery and no-argument listing

`Camel.toml` SHALL accept a top-level `[jobs]` table with one key,
`dir` (default `"jobs"` when the table is absent); unknown keys inside
`[jobs]` SHALL be rejected. The `dir` value SHALL resolve against the
`Camel.toml` root, never the process working directory. A bare document
argument `camel job <name>` (no path separator, no recognised document
suffix) SHALL resolve to exactly `{jobs.dir}/<name>.job.yaml`; the
resolution SHALL NOT probe alternate suffixes, and a miss SHALL fail
with one error naming the probed file. An argument that contains a path
separator or ends in a recognised document suffix is an explicit path
and SHALL always win over bare-name resolution. A job document SHALL
NOT inherit route files from `routes/` discovery: the explicit route
source remains mandatory, and `[jobs].dir` governs where job documents
live, never where a job's routes come from. `camel job` invoked with no
document argument SHALL list the jobs directory to stdout — one line per
job: the name with the `.job.yaml`/`.job.yml` suffix stripped, plus the
document's `description:` or `(no description)` — read via a cheap
probe parse that does not run the full document grammar. The listing
name is display-only: the bare token `camel job <name>` resolves
`<name>.job.yaml` only, so a listed `<name>.job.yml` is reachable only
by explicit path. A multiline `description:` SHALL render on one line
with embedded newlines replaced by spaces. An unparseable sibling SHALL
be listed as `(unparseable)` and SHALL NOT abort the listing. An empty
or absent jobs directory SHALL print a friendly creation hint to stdout
and exit 0. A genuine usage error (for example `--report` with no
document and no listing intent) SHALL exit 2. Listing output and the
JSON run report never co-occur: listing happens only when no document
was given.

#### Scenario: lists found jobs with exit 0

- **GIVEN** a jobs directory containing `create-user.job.yaml` with
  `description: create a user via direct:in` and `reindex.job.yaml`
  with no `description:` key
- **WHEN** `camel job` runs with no document argument
- **THEN** stdout lists both names with the suffix stripped, the
  description or `(no description)` per job, and the process exits 0

#### Scenario: empty or absent directory is exit 0

- **GIVEN** a project whose jobs directory is empty or does not exist
- **WHEN** `camel job` runs with no document argument
- **THEN** a friendly message naming the directory and the
  `<name>.job.yaml` convention prints to stdout, and the process exits 0

#### Scenario: bare-name resolution is deterministic

- **GIVEN** `{jobs.dir}/foo.job.yaml` exists
- **WHEN** `camel job foo` runs from any subdirectory of the project
- **THEN** the document at the Camel.toml-root-anchored
  `{jobs.dir}/foo.job.yaml` is loaded
- **WHEN** `camel job bar` runs and no `bar.job.yaml` exists there
- **THEN** the run fails with one error naming `bar.job.yaml` and the
  directory searched, and exits 2

#### Scenario: explicit path wins over bare-name resolution

- **GIVEN** a document at `ops/one-shot.job.yml` (explicit `.job.yml`
  spelling)
- **WHEN** `camel job ops/one-shot.job.yml` runs
- **THEN** the explicit path is used as-is, without jobs-directory
  probing

#### Scenario: unparseable sibling does not abort listing

- **GIVEN** a jobs directory containing one valid job and one file
  whose YAML cannot be probed
- **WHEN** `camel job` runs with no document argument
- **THEN** the listing completes, showing the valid job and the sibling
  as `(unparseable)`, and the process exits 0

#### Scenario: listed job.yml is not bare-name resolvable

- **GIVEN** a jobs directory containing `legacy.job.yml` only
- **WHEN** `camel job` runs with no document argument
- **THEN** the listing shows `legacy`
- **WHEN** `camel job legacy` runs
- **THEN** the run fails with the bare-name miss error naming
  `legacy.job.yaml`, and exits 2

#### Scenario: multiline description renders on one line

- **GIVEN** a job whose `description:` is a YAML block scalar spanning
  multiple lines
- **WHEN** `camel job` runs with no document argument
- **THEN** the job's listing line shows the description on a single
  line with embedded newlines replaced by spaces

#### Scenario: jobs dir anchors at the Camel.toml root

- **GIVEN** a project with `Camel.toml` at the root, a root `jobs/`
  directory, and the shell working directory in a nested subdirectory
- **WHEN** `camel job` runs with no document argument
- **THEN** the root `jobs/` directory is listed (not `./jobs/` relative
  to the working directory)

#### Scenario: route source is never defaulted to routes discovery

- **GIVEN** `{jobs.dir}/foo.job.yaml` declares no route source key
- **WHEN** `camel job foo` runs
- **THEN** the load fails with the route-source error and exit 2, and
  no `routes/` glob fallback occurs

#### Scenario: report flag without a document is a usage error

- **GIVEN** `camel job --report out.json` with no document argument
- **WHEN** the command runs
- **THEN** the process exits 2 with a usage error, and no listing is
  printed

### Requirement: one-shot send with load-gated route startup

`camel job` SHALL boot the real composition root (the `camel run` seams),
SHALL force `auto_startup = true` on every discovered route in the job
document (including routes configured `autoStartup: false`), and SHALL
reject a send target with no matching consumer route at load, as well as a
send target whose base is ambiguous across multiple consumer routes. Route
side-effect safety SHALL come from the fail-closed consumer scheme
allowlist at load: every consumer entry point in a job document is
internal by construction, while producer and sink `to:` endpoints remain
unrestricted. The send target SHALL remain the single entry point. The job
SHALL send exactly one exchange to the document's target and SHALL wrap
send, drain, and teardown in the mandatory overall `timeout` (anchored at
process start, covering boot). For `seda:` targets the send SHALL carry
`waitForTaskToComplete=Always` so the producer awaits the pipeline result
regardless of exchange pattern.

#### Scenario: all document routes start

- **GIVEN** a job document whose routes include the send target's
  `direct:` consumer route, a helper `direct:` consumer route the target
  hops to (configured `autoStartup: false`), and one unrelated `seda:`
  consumer route
- **WHEN** the job starts the context
- **THEN** every document route's consumer starts (including the
  `autoStartup: false` helper), and the send target
  remains the sole entry point the job injects into

#### Scenario: direct helper route participates end to end

- **GIVEN** a job document whose target route hops with
  `to: direct:enrich` to a second route that transforms the exchange
- **WHEN** `camel job` runs the document
- **THEN** the hop resolves: the helper route started, its consumer is
  registered, and the job completes with exit 0

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

