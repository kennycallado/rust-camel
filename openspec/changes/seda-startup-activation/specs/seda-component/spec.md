# seda-component Specification (delta)

## ADDED Requirements

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
