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

### Requirement: typed SEDA no-active-consumers gate classification

The SEDA crate SHALL classify a pipeline error as the no-active-consumers
gate only when the error carries the crate's typed gate rejection marker —
an endpoint-creation failure whose source chain, within the classifier's
bounded walk depth of 8 source hops, contains the marker type. The marker
type SHALL NOT be constructible or nameable outside the SEDA crate, and
the source handle carrying it SHALL NOT be cloneable or extractable
outside camel-api, so foreign code cannot forge gate provenance. Message
text SHALL play no part in classification. Every endpoint-creation
failure that lacks the gate marker AND the terminal-config marker —
whatever its text — SHALL report `is_no_active_consumers_gate` as false
and `is_direct_startup_race` as true (retryable), except the terminal-config
carrier defined by the typed terminal-config-error classification
requirement. The crate SHALL construct every gate rejection through one
in-crate constructor, and the rejection's outer detail SHALL stay
byte-identical to the historical wording (`SEDA endpoint '<name>' has no
active consumers` single mode, `SEDA endpoint '<name>' has no active
subscribers` fanout mode); the marker's own Display is non-canonical
diagnostic text (it never equals a gate message and never participates
in classification). A marker deeper than 8 source hops SHALL NOT classify
the error as the gate (bounded walk; tested at and beyond the limit).

#### Scenario: genuine single-mode gate fails fast

- **GIVEN** a SEDA single-mode endpoint with no active consumer
- **WHEN** a producer send rejects through the gate constructor with the
  typed marker in the source chain
- **THEN** `is_no_active_consumers_gate` reports true and
  `is_direct_startup_race` reports false (fail fast, no retry)

#### Scenario: genuine fanout-mode gate fails fast

- **GIVEN** a SEDA fanout-mode endpoint with no active subscriber, rejected
  at the pre-enqueue gate or at the subscriber-list check
- **WHEN** the producer send rejects through the gate constructor
- **THEN** `is_no_active_consumers_gate` reports true and
  `is_direct_startup_race` reports false

#### Scenario: gate marker deeper in the source chain still classifies

- **GIVEN** an endpoint-creation failure whose source chain contains an
  intermediate wrapper error that in turn sources the typed gate marker,
  with the marker within 8 source hops
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports true and
  `is_direct_startup_race` reports false

#### Scenario: marker beyond the walk limit stays retryable

- **GIVEN** an endpoint-creation failure whose source chain carries the
  typed gate marker deeper than 8 source hops
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true (bounded walk, boundary tested)

#### Scenario: foreign message containing the consumer wording stays retryable

- **GIVEN** a foreign component failure rendered as a plain
  `EndpointCreationFailed` whose text contains "has no active consumers"
  as a substring
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true (retryable within the bounded
  window)

#### Scenario: foreign message containing the subscriber wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text contains
  "has no active subscribers" as a substring
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: byte-exact imitation of the consumer wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text is byte-identical
  to the canonical single-mode gate message
  (`SEDA endpoint 'q' has no active consumers`) but carries no typed
  marker
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: byte-exact imitation of the subscriber wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text is byte-identical
  to the canonical fanout-mode gate message
  (`SEDA endpoint 'q' has no active subscribers`) but carries no typed
  marker
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: decorated gate wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text is the canonical
  gate message with extra prefix, suffix, or case modification
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: typed endpoint failure without the gate marker stays retryable

- **GIVEN** an endpoint-creation failure variant carrying a source chain
  that does not contain the gate marker (a foreign or future
  source-preserving endpoint failure, including one whose source is a
  terminal-config marker of a FOREIGN crate)
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true

#### Scenario: other SEDA endpoint-creation failures stay retryable

- **GIVEN** a SEDA endpoint-creation failure with any other wording
  (queue-full, enqueue or fanout timeout, or a passthrough creation
  failure)
- **WHEN** the classification runs
- **THEN** `is_no_active_consumers_gate` reports false and
  `is_direct_startup_race` reports true (documented residual, unchanged;
  the multipleConsumers configuration rejection moved to the terminal-config
  classification — see the typed terminal-config-error requirement)

#### Scenario: canonical wording and aliases are preserved

- **GIVEN** a gate rejection produced at any of the three gate sites
- **WHEN** the error renders and is inspected
- **THEN** the outer detail is byte-identical to the historical wording,
  `variant_name()` reports `EndpointCreationFailed`, and `classify()`
  reports `endpoint`

### Requirement: typed SEDA terminal-config-error classification

The SEDA crate SHALL classify a pipeline error as a terminal
configuration error only when the error carries the crate's typed
terminal-config marker — an `EndpointCreationFailedWithSource` whose
source chain, within the classifier's bounded walk depth of 8 source
hops, contains the marker type. The marker type SHALL NOT be
constructible or nameable outside the SEDA crate. Message text SHALL
play no part in classification. The crate SHALL construct every
terminal-config rejection through in-crate constructors, and each
rejection's outer detail SHALL stay byte-identical to the historical
wording: the multipleConsumers+wait rejection
(`multipleConsumers=true with waitForTaskToComplete != Never is not
supported — a single request cannot have N valid replies without
aggregator semantics`) and the endpoint-config-conflict rejection
(the `is_compatible_with` incompatibility detail, `endpoint '<name>'
already exists with different config: <diffs>`, produced when an
endpoint is requested with a config incompatible with the existing
endpoint state of the same name). The marker SHALL cover both terminal
variants — the multipleConsumers+wait conflict and the endpoint config
conflict. The marker's own Display SHALL be non-canonical diagnostic
text that never equals a canonical rejection message.
`is_seda_terminal_config_error` SHALL report true for an error carrying
either marker variant within the walk limit, and
`is_direct_startup_race` SHALL report false for the same (both
combinations are deterministic — no consumer timing and no retry can
ever satisfy them), and true when the marker sits deeper than 8 source
hops (bounded walk; boundary tested at the limit and beyond). The plain
`EndpointCreationFailed` variant SHALL never classify as a
terminal-config error.

#### Scenario: genuine multipleConsumers+wait reject fails fast

- **GIVEN** a SEDA fanout endpoint (`multipleConsumers=true`) with an
  active consumer and a producer whose `waitForTaskToComplete` is not
  `Never`
- **WHEN** the producer send rejects through the terminal-config
  constructor
- **THEN** `is_seda_terminal_config_error` reports true and
  `is_direct_startup_race` reports false (terminal, no retry)

#### Scenario: genuine endpoint config conflict fails fast

- **GIVEN** a SEDA endpoint state registered for name `<n>` with one
  config, and an endpoint creation request for the same name with an
  incompatible config (for example a different `size`)
- **WHEN** `create_endpoint` rejects through the config-conflict
  constructor
- **THEN** `is_seda_terminal_config_error` reports true and
  `is_direct_startup_race` reports false (deterministic conflict, no
  retry on any arm)

#### Scenario: config-conflict rejection detail stays byte-identical

- **GIVEN** an endpoint request whose config conflicts with the existing
  endpoint state (`size: 10 vs 5` diff)
- **WHEN** the rejection renders
- **THEN** the outer detail reads exactly `endpoint '<name>' already
  exists with different config: size: 10 vs 5` (the historical
  `is_compatible_with` wording, unchanged by the typed carrier)

#### Scenario: foreign imitation of the config wording stays retryable

- **GIVEN** a plain `EndpointCreationFailed` whose text byte-matches the
  canonical config-conflict wording
- **WHEN** the classification runs
- **THEN** `is_seda_terminal_config_error` reports false and
  `is_direct_startup_race` reports true (typed provenance only)

#### Scenario: typed source without the terminal-config marker stays retryable

- **GIVEN** an `EndpointCreationFailedWithSource` whose source chain
  carries a foreign error type instead of the marker
- **WHEN** the classification runs
- **THEN** `is_seda_terminal_config_error` reports false and
  `is_direct_startup_race` reports true

#### Scenario: terminal-config marker at exactly the walk limit classifies

- **GIVEN** an endpoint-creation failure whose source chain carries the
  typed terminal-config marker at exactly 8 source hops
- **WHEN** the classification runs
- **THEN** `is_seda_terminal_config_error` reports true and
  `is_direct_startup_race` reports false (inclusive limit)

#### Scenario: terminal-config marker beyond the walk limit stays retryable

- **GIVEN** an endpoint-creation failure whose source chain carries the
  typed terminal-config marker deeper than 8 source hops
- **WHEN** the classification runs
- **THEN** `is_seda_terminal_config_error` reports false and
  `is_direct_startup_race` reports true (bounded walk, boundary tested)

