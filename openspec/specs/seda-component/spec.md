# seda-component Specification

## Purpose
TBD - created by archiving change seda-restart-fix. Update Purpose after archive.
## Requirements
### Requirement: SEDA Single-mode consumer restartability

The SEDA component SHALL restore the Single-mode queue receiver into the
endpoint state when its consumer stops — clearing the `active` flag BEFORE
publishing the restored receiver — so that a consumer subsequently created for
the same endpoint name can start without error. Envelopes still inside the
receiver's queue when restoration occurs SHALL remain queued and be delivered
by the restarted consumer's forwarders; envelopes already dequeued by a
forwarder or in flight through the pipeline at stop time retain the existing
best-effort shutdown behavior.

#### Scenario: Single-mode stop then fresh-consumer start

- **GIVEN** a Single-mode SEDA endpoint (default `multipleConsumers=false`) whose
  consumer has started and then stopped
- **WHEN** a new consumer instance for the same endpoint name calls `start()`
- **THEN** `start()` returns `Ok(())` (no "already has a registered consumer" error) and
  `has_active_consumers()` returns true

#### Scenario: Buffered envelopes survive the restart cycle

- **GIVEN** a Single-mode SEDA endpoint whose consumer is stopped while one or more
  envelopes remain inside the receiver's queue
- **WHEN** a new consumer instance starts on the same endpoint
- **THEN** the restarted consumer's forwarder delivers each still-queued envelope to
  the consumer context

#### Scenario: Repeated restart cycles

- **GIVEN** a Single-mode SEDA endpoint
- **WHEN** the stop/start cycle is repeated several times on fresh consumer instances
- **THEN** every start succeeds and the endpoint never reports
  "already has a registered consumer"

#### Scenario: Concurrent-consumer restart

- **GIVEN** a Single-mode SEDA endpoint configured with `concurrentConsumers=4` whose
  consumer has started and then stopped
- **WHEN** a new consumer instance for the same endpoint starts
- **THEN** the start succeeds and spawns exactly four forwarder tasks, and envelopes
  sent after the restart are delivered

#### Scenario: Producer fencing tracks the active consumer

- **GIVEN** a Single-mode SEDA endpoint whose consumer is stopped (no active consumer)
- **WHEN** a producer sends to the endpoint
- **THEN** the send is fenced with a no-active-consumers error, and after a new consumer
  starts the same send path succeeds

#### Scenario: Route-level restart with default options

- **GIVEN** a CamelContext with a consumer route `from seda:out` (default
  `multipleConsumers=false`) and a producer route sending to `seda:out`
- **WHEN** the consumer route is stopped and restarted, then the producer route sends
- **THEN** the exchange flows through the restarted route's pipeline (the exchange that
  motivated the divert-restart workaround in `route_interception_test.rs`)

#### Scenario: Fanout mode unaffected

- **GIVEN** a Fanout SEDA endpoint (`multipleConsumers=true`)
- **WHEN** a consumer stops and a new consumer starts
- **THEN** behavior is unchanged: the new consumer receives a fresh subscriber queue and
  fanout delivery continues

### Requirement: SEDA consumer startup activation handshake

SEDA consumers SHALL use the Explicit startup mode: `start()` SHALL
signal readiness through `ConsumerContext::mark_ready()` only after
the endpoint's consumer-activation state is published (Single mode:
the `active` flag is stored and the receiver is taken; Fanout mode:
the subscriber is registered), and every error path before that
point SHALL return `Err` without signalling readiness. Route startup
SHALL await this handshake, so a caller of `CamelContext::start()`
observes a fully activated SEDA consumer set when startup returns
`Ok`.

#### Scenario: Startup handshake awaits consumer activation

- **GIVEN** a route with a SEDA consumer started through the normal
  context startup path
- **WHEN** `ctx.start()` returns `Ok`
- **THEN** the SEDA consumer's activation state is already published
  (Single: `active` is true; Fanout: subscriber registered) and a
  producer send that starts immediately after the return value is
  enqueued without observing the pre-enqueue
  "no active consumers" gate

#### Scenario: No pre-activation message-loss window

- **GIVEN** a producer that sends to the SEDA endpoint in the same
  task that awaited `ctx.start()`
- **WHEN** the send executes after startup returned `Ok`
- **THEN** the send succeeds without retry or probe synchronization;
  the pre-enqueue gate passes on the first attempt

#### Scenario: Startup failure propagates before readiness

- **GIVEN** a SEDA consumer whose `start()` returns `Err` before
  signalling readiness (for example, the endpoint already has a
  registered consumer)
- **WHEN** route startup awaits the handshake
- **THEN** the error surfaces as a route-start failure and startup
  does not hang

#### Scenario: Restart cycle keeps the handshake truthful

- **GIVEN** a Single-mode SEDA endpoint whose consumer stopped and a
  fresh consumer instance for the same endpoint starts again
- **WHEN** the fresh consumer's `start()` completes successfully
- **THEN** readiness was signalled after the new `active` flag was
  stored, and buffered envelopes that survived the restart flow
  without any test-side probe

