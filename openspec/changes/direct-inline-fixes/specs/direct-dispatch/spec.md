## MODIFIED Requirements

### Requirement: Inline dispatch selection for direct endpoints

The direct component SHALL execute the target consumer's pipeline inline in
the producer's task, with no mandatory inter-task channel handoff, when the
target consumer is live, ready, and effectively
`ConcurrencyModel::Sequential`. A consumer whose effective model is
Concurrent, or whose registry entry carries no dispatcher capability, SHALL
dispatch through the channel path. An aggregate-split route (a top-level
`aggregate` with timeout or force-completion, compiled as pre/agg/post
split pipelines) SHALL NOT publish a dispatcher capability — its entry
SHALL dispatch through the channel path, where the aggregate engine drives
the split pipelines; its `managed.pipeline` is an identity shell and SHALL
NOT be exposed to inline execution. A dispatch rejected for invalid
recursion (cycle or depth overflow) SHALL error and SHALL NOT fall back to
the channel path.

#### Scenario: Sequential consumer dispatches inline

- **GIVEN** a live and ready `direct:name` consumer whose effective
  concurrency model is Sequential, registered in the `DirectRegistry`
- **WHEN** a `DirectProducer` for `direct:name` dispatches an exchange
- **THEN** the consumer pipeline SHALL execute in the producer's task
  without an intermediate channel round-trip, and the reply SHALL return
  on the same task

#### Scenario: Concurrent consumer falls back to the channel path

- **GIVEN** a live and ready `direct:name` consumer whose effective
  concurrency model is Concurrent
- **WHEN** a `DirectProducer` for `direct:name` dispatches an exchange
- **THEN** the dispatch SHALL go through the channel path, with the
  pre-623cca62 consumer-driven semantics

#### Scenario: Capability-unavailable consumer falls back to the channel path

- **GIVEN** a live and ready `direct:name` consumer whose registry entry
  carries no dispatcher capability
- **WHEN** a `DirectProducer` for `direct:name` dispatches an exchange
- **THEN** the dispatch SHALL go through the channel path and the
  channel-path result (transformed exchange or error) SHALL propagate
  unchanged to the producer

#### Scenario: Aggregate-split route never publishes the dispatcher

- **GIVEN** a route `from("direct:agg-in").aggregate(...)` compiled as a
  top-level aggregate split (pre/agg/post pipelines, timeout or
  force-completion)
- **WHEN** the controller starts or resumes that route
- **THEN** the route SHALL NOT publish an inline dispatcher capability for
  its `direct:agg-in` entry, and producers dispatching to it SHALL take
  the channel path
- **AND** N fragments sent via `to("direct:agg-in")` SHALL be processed
  and aggregated into exactly one reply (end-to-end correctness)

#### Scenario: Stopped consumer is not inline-eligible

- **GIVEN** a previously inline-eligible `direct:name` consumer that has
  been stopped
- **WHEN** a `DirectProducer` for `direct:name` dispatches an exchange
- **THEN** the dispatch SHALL NOT execute inline; it SHALL fail or fall
  back per the existing stopped-consumer semantics

#### Scenario: Resumed consumer republishes the inline capability

- **GIVEN** a stopped-and-resumed `direct:name` consumer whose topology
  is inline-eligible
- **WHEN** the controller completes the resume
- **THEN** a fresh dispatcher capability SHALL be published, and
  subsequent dispatches execute inline

#### Scenario: Missing consumer keeps fail-if-no-consumers behavior

- **GIVEN** no consumer registered for `direct:name` and
  `failIfNoConsumers` enabled
- **WHEN** a `DirectProducer` for `direct:name` dispatches an exchange
- **THEN** the dispatch SHALL fail with the existing no-consumer error,
  unchanged from the channel-only implementation

## ADDED Requirements

### Requirement: Operator-visible failure signal for direct dispatches

Every unhandled direct-dispatch failure SHALL emit the b′ failure signal
(pipeline error metrics increment) exactly once per failing dispatch
invocation, through a context-threaded metrics handle — NOT a handle owned
by the producing route's build — so the emission is observable even when
the producing route's own pipeline tracing is unwired. Unhandled failure
covers: the initial registry lookup failure (no consumer registered, with
`failIfNoConsumers=false`), admission failure of an active inline
dispatcher, and an in-pipeline error returned by an active inline
dispatcher. A downstream lookup error returned through an already-selected
dispatcher is an in-pipeline error of that dispatcher's invocation.
`ConsumerStopping` surrender SHALL NOT emit (stop-time surrender is not
an operator-visible failure). Channel-path dispatches keep their existing
emission site and count, unchanged. An emission MUST NOT be produced both
by the dispatch mechanism and by a wired producing route's traced wrapper
for the same unhandled failure when the wrapper is absent from the
observed path (no double counting on the component path); when the
producing route IS traced, the traced wrapper's own recording is additive
pipeline telemetry, not the component b′ signal.

#### Scenario: Initial lookup failure is visible with unwired producer pipeline

- **GIVEN** a started context whose producing route
  `from("direct:entry").to("direct:missing?failIfNoConsumers=false")` has
  pipeline tracing unwired (recording lifecycle registered after routes,
  no pipeline tracer), and a recording collector registered before start
  completes
- **WHEN** one exchange runs through `direct:entry` and the dispatch to
  `direct:missing` fails at registry lookup
- **THEN** the collector observes exactly one
  `increment_errors` (the b′ signal) — this is the existing test
  `metrics_wiring_test::late_registration_after_routes_observed`,
  currently red

#### Scenario: Dispatch timeout emits once

- **GIVEN** an inline-eligible `direct:name` consumer whose dispatch
  expires the single timed section covering lookup, admission, and
  execution (e.g. a pipeline slower than the configured timeout)
- **WHEN** the dispatch fails with the timeout error
- **THEN** exactly one `increment_errors` is recorded for that dispatch
  invocation

#### Scenario: In-pipeline error emits once

- **GIVEN** an inline-eligible `direct:name` consumer whose pipeline
  returns an unhandled error for the dispatched exchange
- **WHEN** the dispatch completes with that error
- **THEN** exactly one `increment_errors` is recorded for that dispatch
  invocation

#### Scenario: Wired producing route does not double-count the component signal

- **GIVEN** a producing route whose pipeline tracing IS wired and whose
  dispatch fails (non-ConsumerStopping)
- **WHEN** the failure surfaces
- **THEN** the component b′ signal is recorded exactly once (the traced
  wrapper's own pipeline-error recording is separate telemetry and does
  not suppress or duplicate the component signal)

#### Scenario: ConsumerStopping emits nothing

- **GIVEN** an inline dispatch that fails with `ConsumerStopping` (target
  consumer stopping)
- **WHEN** the failure surfaces to the producer
- **THEN** zero `increment_errors` are recorded for the surrender

#### Scenario: Channel-path emission unchanged

- **GIVEN** a Concurrent `direct:name` consumer dispatched through the
  channel path whose send returns an unhandled error
- **WHEN** the failure surfaces
- **THEN** the b′ signal is recorded by the existing channel-path
  emission site, with the same count as before this change
