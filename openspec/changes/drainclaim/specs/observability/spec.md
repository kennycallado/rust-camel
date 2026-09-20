## ADDED Requirements

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

#### Scenario: raw sender fast path stays uncounted

- **GIVEN** an http/grpc/ws/master per-request task that submits an
  envelope through `ConsumerContext::sender()`
- **WHEN** the envelope moves through the route pipeline
- **THEN** the counter does not include it (documented exception; the
  exchange is observable only through the component's own signals)
