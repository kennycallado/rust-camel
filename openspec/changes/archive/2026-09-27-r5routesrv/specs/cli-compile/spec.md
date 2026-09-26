## ADDED Requirements

### Requirement: Long-running route-server artifacts

A compiled route artifact SHALL run like `camel run` in a deployment
posture: after successful boot it SHALL keep running — with every
listener the embedded documents and embedded configuration declare
bound and serving — until the first stop signal. The SIGINT and SIGTERM
streams SHALL be armed before boot so a signal arriving during boot is
buffered and consumed by the shutdown wait rather than default-killing
the process. The first SIGINT or SIGTERM SHALL start a graceful
shutdown: in-flight listener work is given the configured drain budget
to complete, then listeners close, and the process exits 0, writing the
completed route report when `--report` was given; work still in flight
when the budget expires is dropped. A second SIGINT or SIGTERM received
while teardown is still running SHALL force-exit with code 1 without
waiting for teardown — the same first-graceful/second-force SIGNAL
contract `camel run` and `camel job` implement (outcome codes differ by
kind: a signal-stopped route exits 0; a job interruption keeps the job
taxonomy codes). A signal burst of identical signals MAY coalesce
before delivery; the force-exit hatch MUST accept either signal.
Compiled job artifacts SHALL keep the bounded exit-after-completion
behavior and SHALL NOT serve until a signal. For a signed artifact,
envelope verification SHALL finish before any listener binds: a failed
or missing required envelope exits 2 with no port ever bound.

#### Scenario: Route artifact serves its declared listener until SIGTERM

- **GIVEN** a compiled route artifact whose document declares a REST
  listener on a free local port
- **WHEN** the artifact starts and an HTTP request is sent to the
  listener after boot completes
- **THEN** the listener answers with the route's response while the
  process keeps running, and the artifact's `--manifest` output lists
  the listener

#### Scenario: First signal drains gracefully and exits 0

- **GIVEN** a serving compiled route artifact started with
  `--report <path>` and a listener request in flight whose route
  completes within the configured drain budget
- **WHEN** SIGTERM arrives while the request is in flight
- **THEN** the in-flight request completes inside the drain budget,
  listeners close, the process exits 0, and the report file contains
  `{"kind":"route","status":"completed","error":null}`

#### Scenario: Second signal force-exits during teardown

- **GIVEN** a booting compiled route artifact receives SIGINT followed
  by SIGTERM, both buffered before the shutdown wait begins
- **WHEN** boot finishes and the buffered signals are consumed — the
  first starting graceful teardown, the second already queued for the
  force-exit guard
- **THEN** the process exits with code 1 without waiting for teardown
  to complete

#### Scenario: Route artifact signal during boot is buffered

- **GIVEN** a compiled route artifact is loading configuration or
  booting components
- **WHEN** SIGTERM arrives mid-boot
- **THEN** the signal is buffered, boot reaches the shutdown wait, and
  the process shuts down gracefully with exit 0 instead of dying to the
  default disposition

#### Scenario: Deployment-equivalence with camel run

- **GIVEN** the same listener-bearing route document run once via
  `camel run --routes <doc> --no-watch` and once as a compiled artifact
- **WHEN** the listener is queried and SIGTERM is sent in both runs
- **THEN** both runs answer the query before the signal and exit 0
  after it — identical serve, drain, and exit behavior

#### Scenario: Job artifacts stay bounded

- **GIVEN** a compiled job artifact whose embedded routes include a
  direct consumer
- **WHEN** the artifact runs
- **THEN** it completes its bounded send and exits with the job outcome
  taxonomy codes, never serving until a signal

#### Scenario: Envelope verification precedes listener binding

- **GIVEN** a compiled route artifact whose manifest marks the detached
  signature envelope required and whose envelope bytes were corrupted
  after signing, leaving the artifact trailer itself valid
- **WHEN** the artifact starts
- **THEN** it exits 2 naming the failed signature-verification step and
  no declared listener port is ever bound
