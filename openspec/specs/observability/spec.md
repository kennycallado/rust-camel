# observability Specification

## Purpose
Runtime observability. Compiled `To` steps retain their declared URI metadata,
per-step durations export with `route` and `to_uri` labels, bridge RPCs
propagate W3C trace context, bridges emit structured JSON logs, and the
runtime counts accepted-not-completed exchanges.
## Requirements
### Requirement: Compiled To steps retain declared URI metadata

The Runtime SHALL retain the declared URI of each `To` step as immutable
compile-time metadata on its compiled process step, while leaving metadata
absent for steps that are not `To` steps.

#### Scenario: To step retains the authored URI

- **GIVEN** a route contains `To("direct:orders")`
- **WHEN** the route is compiled
- **THEN** the corresponding compiled process step exposes `direct:orders` as
  its declared URI metadata

#### Scenario: Non-To step has no URI metadata

- **GIVEN** a route contains a processor or structural step that is not `To`
- **WHEN** the route is compiled
- **THEN** its compiled process step has no declared URI metadata

### Requirement: OTEL exports per-step duration by declared URI

When duration metrics are enabled, the Runtime SHALL record a
`step_duration_secs` histogram for each call-time `To` step with `route` and
`to_uri` labels, using the existing dynamic-label metrics collector contract.
The histogram records elapsed call-time attempts for both successful and failed
processor results, but never records readiness attempts.

#### Scenario: Call-time To duration is recorded

- **GIVEN** duration metrics are enabled and a compiled `To("direct:orders")`
  step completes a call
- **WHEN** the tracing processor records step metrics
- **THEN** it records `step_duration_secs` with labels `route` equal to the
  route identifier and `to_uri` equal to `direct:orders`

#### Scenario: Duration lever disables the new histogram

- **GIVEN** duration metrics are disabled
- **WHEN** a `To` step completes a call
- **THEN** no `step_duration_secs` histogram is recorded

#### Scenario: Readiness failure does not create call-time duration

- **GIVEN** a producer fails during `poll_ready`
- **WHEN** readiness metrics are recorded
- **THEN** no `step_duration_secs` histogram is recorded, while existing
  readiness exchange/error metrics retain their current behavior

### Requirement: Bridge RPC trace context propagation

The Rust bridge client SHALL inject the current camel-otel trace context as
W3C `traceparent` gRPC metadata on every RPC it sends to a Java bridge
(Send, Subscribe, Health), reusing camel-otel propagation, and SHALL NOT
inject the header when no valid trace context exists. Metadata already
carrying a `traceparent` SHALL NOT be overwritten.

#### Scenario: Active context reaches every RPC shape

- **GIVEN** a camel-otel context with a valid span context is current at RPC
  time (ambient for Subscribe/Health, exchange-carried for Send)
- **WHEN** the bridge client issues Send, Subscribe, or Health
- **THEN** each request carries `traceparent` matching
  `00-<32hex trace id>-<16hex span id>-<2hex flags>` with the context's
  trace and span ids

#### Scenario: No-tracing fallback injects nothing

- **GIVEN** no valid camel-otel context exists at RPC time
- **WHEN** any bridge RPC is issued
- **THEN** no `traceparent` metadata is present and the RPC completes with
  existing semantics

#### Scenario: Bridge surfaces the received traceparent

- **GIVEN** a request with `traceparent` metadata arrives at the JMS bridge
- **WHEN** the interceptor processes the call and the service handles it
- **THEN** the bridge logs the received value at info level via
  java.util.logging and the value arrives intact through the bridge service

### Requirement: Bridge structured JSON logs

The Java bridges SHALL emit log records as structured JSON objects on
stdout through the existing java.util.logging-to-stdout pipeline, without
swapping the log framework.

#### Scenario: Log records render as JSON lines

- **GIVEN** a bridge runs with the JSON console formatter enabled
- **WHEN** any log record is emitted
- **THEN** stdout receives a single line that is a valid JSON object
  carrying at least timestamp, level, logger name, and message

#### Scenario: Ready-protocol line stays untouched

- **GIVEN** a bridge starts up
- **WHEN** the PortAnnouncer emits its readiness line
- **THEN** stdout still receives the exact `{"status":"ready",...}` JSON
  object, unaffected by the log formatter

### Requirement: Context in-flight claim counts accepted-not-completed exchanges

The runtime SHALL maintain one context-global counter of exchanges that
are accepted but not yet completed. Claims (`InFlightClaim`) may be
minted only at the acceptance boundaries: seda producer enqueue (queue
residency), `ConsumerContext::send` / `send_and_wait` dispatch
(non-seda), and inline-route dispatch. A claim SHALL decrement the
counter exactly once when dropped, and every release path — normal
pipeline completion, dispatch push failure, queued-envelope drop at route
stop, pipeline task abort, pipeline panic, and pipeline readiness failure
— SHALL drop the claim. Fanout SHALL split one claim per subscriber copy
so each copy counts and releases independently. `CamelContext` SHALL
expose the counter through `total_in_flight()`, a single atomic load: a
read of zero is a linearizable statement that no exchange accepted
through a counted path is awaiting completion. During any handoff between
boundaries, the acquiring claim SHALL be attached before the releasing
claim drops, so no instant of a counted exchange's lifecycle is
uncovered. The http/grpc/ws/master raw `sender()` fast path is a
documented exception: envelopes submitted through it carry no claim and
are not counted.

#### Scenario: seda queue residency is counted

- **GIVEN** a started seda consumer route and an idle counter
- **WHEN** a producer enqueues one envelope and the counter is read
  before the route's pipeline dequeues it
- **THEN** `total_in_flight()` is at least 1 for that exchange

#### Scenario: pipeline residency releases on completion

- **GIVEN** a route whose pipeline parks each exchange on a deterministic
  barrier
- **WHEN** an exchange is dispatched and the barrier is held
- **THEN** `total_in_flight()` is at least 1; after the barrier releases
  and the pipeline completes, it returns to 0

#### Scenario: dispatch handoff never uncovers an exchange

- **GIVEN** an exchange forwarded from a seda queue into its consumer
  route
- **WHEN** the counter is observed at any instant between the forwarder
  receiving the envelope and the route pipeline completing it
- **THEN** the exchange holds at least one live claim throughout

#### Scenario: dispatch push failure rolls the claim back

- **GIVEN** a consumer route whose dispatch channel is closed
- **WHEN** `ConsumerContext::send` attempts the push and fails
- **THEN** the claim attached to the rejected envelope is released and
  `total_in_flight()` returns to its prior value

#### Scenario: queued-envelope drop at route stop releases the claim

- **GIVEN** a stopped route whose dispatch channel still holds queued
  envelopes with attached claims
- **WHEN** the channel drains as part of teardown
- **THEN** each dropped envelope releases its claim and the counter
  returns to 0

#### Scenario: pipeline readiness failure releases the claim

- **GIVEN** a route pipeline task that has dequeued an envelope and holds
  its claim
- **WHEN** the pipeline's service readiness check (`ready_with_backoff`)
  fails and the task exits without processing
- **THEN** the held claim is released and `total_in_flight()` returns to 0

#### Scenario: aborted or panicked pipeline releases its claim

- **GIVEN** a route pipeline holding a live claim for one exchange
- **WHEN** the pipeline task is aborted, or the pipeline panics while
  processing
- **THEN** the claim is released exactly once and `total_in_flight()`
  returns to 0 (no leaked increment)

#### Scenario: inline dispatch is counted

- **GIVEN** a route with an inline fast path (zero-handoff `direct:`
  dispatch)
- **WHEN** a producer dispatches an exchange through the
  `InlineRouteDispatcher` and the pipeline parks on a deterministic
  barrier
- **THEN** `total_in_flight()` is at least 1 for that exchange and
  returns to 0 when the dispatch future completes

#### Scenario: fanout splits one claim per subscriber copy

- **GIVEN** a seda endpoint in fanout mode with two active subscribers
- **WHEN** a producer enqueues one exchange
- **THEN** the counter holds two claims (one per copy), and each
  subscriber's pipeline completion releases exactly its own claim

#### Scenario: raw sender fast path mints at acceptance

- **GIVEN** an http/grpc/ws/master consumer that submits envelopes
  through the raw `ConsumerContext::sender()` fast path, with the
  counter captured at consumer start via
  `ConsumerContext::in_flight_counter()`
- **WHEN** the component dequeues an accepted wire message at its
  acceptance point (http `RequestEnvelope` dequeue, grpc transport recv
  or streaming chunk acceptance, ws frame acceptance, master epoch
  bridge dequeue for uncounted delegate envelopes)
- **THEN** the constructed envelope carries a claim minted from that
  counter, `total_in_flight()` includes it from acceptance through
  pipeline completion, and every rejection/early-exit path releases it
  by drop

#### Scenario: master delegate bridge never double counts

- **GIVEN** a master delegate whose synthetic `ConsumerContext` has the
  route counter installed (so the delegate's own `send()` mints at
  bridge-channel entry)
- **WHEN** the epoch bridge forwards the already-counted envelope to
  the route pipeline
- **THEN** the bridge mints nothing (the existing claim passes through
  untouched) and the counter holds exactly one claim for that envelope

### Requirement: Branch-exchange construction splits the in-flight claim

The system SHALL provide `Exchange::branch_clone()` in `camel-api` as
the sole constructor for fanout branch copies produced by cloning a
source `Exchange`. For a source exchange carrying an in-flight claim,
`branch_clone()` SHALL mint one sibling claim via
`InFlightClaim::split()` onto the copy, so each detached branch copy
counts on the context-global gauge and releases independently on
completion; the source's own claim SHALL remain live and unchanged.
`Exchange::clone` SHALL keep setting `in_flight_claim: None` and
`InFlightClaim` SHALL remain non-`Clone`. Fresh-construction fanout
sites that assemble a new `Exchange` from parsed entry data and carry
no relation to a claimed parent (e.g. tar/archive per-entry exchanges)
are outside the helper's scope. The split-fragment construction seam
is IN scope: `fragment_exchange` (`camel-api/src/splitter.rs`) and any
equivalent segment/streaming fragment builder SHALL split the parent's
carried claim onto every fragment, because each fragment is a fanout
branch exchange of the parent. Every
fanout clone site under `crates/camel-processor/src` and every
exchange-clone site under `crates/camel-core/src/lifecycle/adapters/`
(including `step_compilers/splitting.rs`, `exchange_uow.rs`, and
`route_compiler.rs` boundary/error snapshots) SHALL construct branch
copies through `branch_clone()`. A site MAY keep plain `clone()` only
when the copy provably completes before the source's claim drops and
never escapes the current claim scope, with the justification recorded
in the implementing task report.

#### Scenario: branch copy carries a split claim

- **GIVEN** an exchange holding one live `InFlightClaim` against a
  gauge with total 1
- **WHEN** `branch_clone()` produces a copy
- **THEN** the gauge total is 2, the copy carries its own claim, and
  the original claim is still attached to the source

#### Scenario: sibling releases independently

- **GIVEN** a source exchange and one `branch_clone()` copy with gauge
  total 2
- **WHEN** the copy's claim is dropped
- **THEN** the gauge total is 1 while the source claim remains live,
  and drops to 0 only when the source claim is dropped

#### Scenario: plain clone carries no claim

- **GIVEN** an exchange holding one live claim
- **WHEN** `Exchange::clone` produces a copy
- **THEN** the gauge total is 1 and the copy carries no claim

#### Scenario: detached fanout copy keeps the drain gauge nonzero

- **GIVEN** a route with a wiretap (or divert) branch and a
  deterministic barrier parked inside the branch consumer only
- **WHEN** the main exchange completes while the branch copy is still
  parked behind the barrier
- **THEN** `total_in_flight()` is at least 1 until the barrier
  releases and the branch copy completes, and returns to 0 afterwards

#### Scenario: multicast branch copy outlives the parent claim

- **GIVEN** a route with a multicast of two recipients where one
  recipient fire-and-forget dispatches its copy to a downstream
  consumer route that parks it on a deterministic barrier, and the
  other recipient returns immediately
- **WHEN** the multicast aggregation completes, the parent pipeline
  replies, and the parent's own claim is released
- **THEN** `total_in_flight()` is at least 1 — held only by the
  branch copy's split claim — until the barrier releases and the copy
  completes, and returns to 0 afterwards

#### Scenario: split segment copy outlives the parent claim

- **GIVEN** a splitter route whose per-segment copy is handed to a
  downstream consumer route that parks on a deterministic barrier
- **WHEN** the splitter completes and the parent's own claim is
  released while the segment copy is still parked
- **THEN** `total_in_flight()` is at least 1 — held only by the
  segment copy's split claim — until the barrier releases, and
  returns to 0 after the copy completes

