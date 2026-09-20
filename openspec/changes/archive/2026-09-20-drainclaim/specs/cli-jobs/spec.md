## MODIFIED Requirements

### Requirement: batch mode drains until empty

A `mode: batch` job SHALL boot the same composition root (including the
job boot projection), apply the same send-target validation, start all
document routes, and send one trigger exchange. It SHALL retain the same
declared-argument validation, defaulting, interpolation, and legacy
no-`args:` header behavior as one-shot execution.
For a `seda:` target it SHALL rewrite `waitForTaskToComplete` to `Always` so
the send is synchronous.
After the trigger send it SHALL wait until the context-global
in-flight counter (`CamelContext::total_in_flight()`) reads zero, then
emit the existing JSON report with outcome `Completed` and mode `batch`,
and exit 0. The counter covers every exchange accepted through a counted
path (seda enqueue, channel dispatch, inline dispatch); the raw-sender
exception is documented in the observability spec. The drain verdict
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
- **WHEN** it runs without `--arg tier=...`
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

- **GIVEN** a batch job whose worker routes record exchange headers to a
  `mock:` endpoint
- **WHEN** `camel job` runs the document with `--arg batch-id=42`
- **THEN** the job drains, exits 0, and the recorded exchanges carry
  header `batch-id=42`
