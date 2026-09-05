# direct-dispatch Specification

## Purpose
TBD - created by archiving change direct-inline-dispatch. Update Purpose after archive.
## Requirements
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

### Requirement: Cycle rejection and inline depth cap

The inline path SHALL detect re-entry of any endpoint already active on the
current task's inline stack and fail immediately with an error; it SHALL
reject acyclic inline dispatch exceeding depth 64 with
`CamelError::ProcessorError`. The inline path SHALL NOT fall back to the
channel path on cycle or depth rejection.

#### Scenario: Direct cycle terminates with an immediate error

- **GIVEN** routes `direct:a -> direct:b` and `direct:b -> direct:a`, both
  Sequential and inline-eligible
- **WHEN** a dispatch enters `direct:a`
- **THEN** the cycle SHALL be rejected with an error before an external
  deadline — the dispatch SHALL NOT succeed, hang until timeout, or
  overflow the stack

#### Scenario: Depth cap rejects at 65

- **GIVEN** an acyclic chain of 65 inline-eligible `direct:` hops on one
  task
- **WHEN** the 65th hop dispatches
- **THEN** the dispatch SHALL fail with `CamelError::ProcessorError`

### Requirement: Admission serialization for concurrent inline producers

The direct registry SHALL serialize concurrent inline dispatches targeting
the same Sequential endpoint through one FIFO admission permit, so at most
one inline dispatch per endpoint is in flight and producers are admitted
in arrival order.

#### Scenario: Concurrent producers are admitted FIFO

- **GIVEN** multiple producers dispatching concurrently to the same
  Sequential `direct:name` endpoint
- **WHEN** their inline dispatches execute
- **THEN** exactly one dispatch SHALL be in flight at a time and
  completions SHALL occur in FIFO admission order

### Requirement: Inline timeout parity

The inline path SHALL honor `timeout_ms` with the same default value, error
text, and timeout boundary as the channel path; the boundary SHALL span
registry lookup, admission wait, and pipeline execution on both paths.
Inline timeout enforcement SHALL be cooperative: a CPU-bound stretch
without an await point cannot be interrupted.

#### Scenario: Configured timeout fires on the inline path

- **GIVEN** an inline-eligible `direct:name` consumer whose pipeline parks
  longer than the configured `timeout_ms`
- **WHEN** a `DirectProducer` dispatches inline
- **THEN** the dispatch SHALL fail with the same timeout error text as the
  channel path

#### Scenario: Timeout boundary covers lookup, admission, and execution

- **GIVEN** an inline-eligible endpoint with `timeout_ms` configured
- **WHEN** an inline dispatch exceeds `timeout_ms` across registry lookup,
  admission wait, or pipeline execution
- **THEN** the dispatch SHALL fail with the channel-path timeout error text

#### Scenario: Default timeout unchanged on the inline path

- **GIVEN** an inline dispatch with no explicit `timeout_ms`
- **WHEN** the timeout applies
- **THEN** the effective default value SHALL equal the channel-path default

### Requirement: Dual-domain cancellation for inline dispatch

Inline dispatch SHALL belong to both the producer's and the consumer's
cancellation domains: the consumer route's in-flight count SHALL be
incremented for the duration of the inline call, consumer stop SHALL apply
drain grace before its pipeline cancellation ends stragglers with
`CamelError::ConsumerStopping`, and the count SHALL be decremented exactly
once per dispatch.

#### Scenario: Consumer stop during inline dispatch

- **GIVEN** an inline dispatch in flight against `direct:name`
- **WHEN** the consumer route stops
- **THEN** the stop SHALL wait for drain grace, then cancel the in-flight
  inline dispatch with `CamelError::ConsumerStopping`

#### Scenario: In-flight count decremented exactly once

- **GIVEN** an inline dispatch that completes, fails, or is cancelled
- **WHEN** the dispatch finishes by any path
- **THEN** the consumer route's in-flight count SHALL be decremented
  exactly once

#### Scenario: Producer cancellation does not stop the consumer

- **GIVEN** an inline dispatch in flight and the producer's task cancelled
- **WHEN** the dispatch unwinds
- **THEN** the consumer route's in-flight count SHALL be decremented exactly
  once and the consumer route SHALL continue running

#### Scenario: Restart uses fresh cancellation state

- **GIVEN** a consumer route stopped while inline dispatches were in flight
  and then restarted
- **WHEN** new inline dispatches begin after restart
- **THEN** they SHALL operate on fresh cancellation tokens and an in-flight
  count of zero

### Requirement: Pipeline snapshot isolation per inline dispatch

Each inline dispatch SHALL load one pipeline snapshot at call entry and
SHALL hold that snapshot through completion of the call, so a concurrent
pipeline swap cannot change the processor chain mid-dispatch (snapshot
isolation pattern, ADR-0004).

#### Scenario: Snapshot held through completion

- **GIVEN** an inline dispatch in flight and a pipeline swap for the target
  route completing during that dispatch
- **WHEN** the inline dispatch continues
- **THEN** it SHALL keep executing the snapshot loaded at call entry until
  it completes

### Requirement: Fairness yield on long inline chains

The inline path SHALL yield the executing task at least once per 32
completed inline hops, so a long synchronous chain cannot monopolize a
worker thread without an await point.

#### Scenario: 100 sequential dispatches yield repeatedly

- **GIVEN** 100 sequential non-nested inline dispatches through one
  endpoint on one task (for example, split fragments each sent to
  `direct:agg-in`) with no natural await points
- **WHEN** the dispatches execute
- **THEN** the task SHALL yield at least three times (at least once per 32
  completed hops, counted cumulatively across the dispatches on that task)

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

