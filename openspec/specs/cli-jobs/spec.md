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

