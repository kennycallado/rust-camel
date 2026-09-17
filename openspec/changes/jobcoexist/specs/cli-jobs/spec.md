## MODIFIED Requirements

### Requirement: one-shot send with load-gated route startup

`camel job` SHALL boot the real composition root (the `camel run` seams)
except the process-scoped diagnostic surfaces: before context
configuration the job boot projection SHALL remove the durable runtime
journal and SHALL replace the ambient observability configuration with
defaults, for every job form including compiled artifacts. The job SHALL
force `auto_startup = true` on every discovered route in the job
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

#### Scenario: ambient diagnostics are projected away

- **GIVEN** an ambient `Camel.toml` enabling `[runtime_journal]`,
  `[observability.otel]`, `[observability.prometheus]`, and
  `[observability.health]`
- **WHEN** `camel job` boots
- **THEN** the job opens no journal, initializes no OTel providers, and
  registers no Prometheus or health listener, while component, security,
  repository, platform, bean, supervision, timeout, and log-level
  configuration still applies

### Requirement: batch mode drains until empty

A `mode: batch` job SHALL boot the same composition root (including the
job boot projection), apply the same send-target validation, start all
document routes, and send one trigger exchange. It SHALL retain the same
declared-argument validation, defaulting, interpolation, and legacy
no-`args:` header behavior as one-shot execution.
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

#### Scenario: batch drains a fan-out pipeline then exits 0

- **GIVEN** a batch job whose trigger route fans out through one or more `seda:` queues
- **WHEN** the job runs
- **THEN** it waits for in-flight work and queue emptiness, reports `Completed` with mode `batch`, and exits 0

#### Scenario: batch does not complete while work is in flight

- **GIVEN** a batch job whose worker route holds each exchange in flight
  for a short delay before recording it to a `mock:` endpoint
- **WHEN** the drain observes a queue-depth sample of zero while the
  exchange is still in flight
- **THEN** the job does not complete from that sample: exit happens only
  after the worker recorded the exchange

#### Scenario: batch overall timeout expiry

- **GIVEN** a batch job whose `seda:` work does not drain before `timeout`
- **WHEN** the deadline expires
- **THEN** it reports `Timeout` and exits 2

#### Scenario: batch works with arg injection

- **GIVEN** a batch job whose worker routes record exchange headers to a
  `mock:` endpoint
- **WHEN** `camel job` runs the document with `--arg batch-id=42`
- **THEN** the job drains, exits 0, and the recorded exchanges carry
  header `batch-id=42`

## ADDED Requirements

### Requirement: job boot coexistence (isolated one-shot operator path)

The argv `camel job` path is the isolated one-shot operator path: a job
SHALL coexist with any process sharing its ambient configuration on the
projected process-scoped diagnostic surfaces — durable runtime journal,
OTel providers, Prometheus and health listeners. The job boot projection
SHALL apply after ambient configuration is parsed and validated, so
malformed configuration still fails loud with exit 2 before any boot. The
projection SHALL modify exactly two fields — `runtime_journal` (removed)
and `observability` (replaced with defaults) — and SHALL NOT remove or
alter components, repositories, security, platform, beans, supervision,
timeouts, or log level. No flag or environment variable SHALL re-enable
the suppressed surfaces on the job path. Ambient repository backends
(`idempotent_repo`, `cache_repo`) remain config-driven shared-file
surfaces: they are NOT projected, and concurrent processes configuring
persistent repository backends may still contend for those files.

#### Scenario: job coexists with a live server on shared ambient config

- **GIVEN** a live `camel run` holding the shared runtime journal lock and
  the shared Prometheus and health ports from an ambient config, and a
  `camel job` run in the same directory with that same config
- **WHEN** the job executes
- **THEN** it exits 0 with a JSON report, starts no second diagnostic
  listener, and writes nothing to the shared journal

#### Scenario: two concurrent jobs on one pod contend on nothing projected

- **GIVEN** two `camel job` processes running concurrently — their
  overlapping executions forced by a route that holds each exchange for a
  bounded delay — against the same ambient config whose repositories use
  in-memory defaults
- **WHEN** both execute
- **THEN** both exit 0 with reports and neither opens a journal or binds a
  diagnostic listener

#### Scenario: malformed ambient config still fails loud

- **GIVEN** an ambient `Camel.toml` with malformed journal or observability
  configuration
- **WHEN** `camel job` loads it
- **THEN** loading fails with the existing configuration diagnostic and
  exit 2; projection never masks a configuration defect

#### Scenario: projection allowlist is exact

- **GIVEN** a fully populated configuration with every field set away from
  its default, including components, repositories, security, platform,
  beans, supervision, timeouts, and log level
- **WHEN** the job boot projection runs
- **THEN** only `runtime_journal` and `observability` change; every other
  field is identical to the input
