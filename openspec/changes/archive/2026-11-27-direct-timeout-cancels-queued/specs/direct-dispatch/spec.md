## MODIFIED Requirements

### Requirement: No producer-side serialization for Concurrent consumers

Calls through one direct producer to a route using the Concurrent drain SHALL
be able to overlap, subject to channel backpressure and the route's configured
concurrency limit. Producer `timeout_ms` SHALL still cover channel submission
and the reply wait. A caller that abandons the wait (its `timeout_ms` elapses)
SHALL cancel its channel-path dispatch at the next step boundary: a step
already running completes, no later step runs, and a dispatch that never
entered the pipeline never starts. The inline path runs the pipeline on the
caller's task, so an abandoned inline dispatch is dropped at once instead of
reaching a boundary.

#### Scenario: Concurrent consumer parallel admission

- **GIVEN** a route `from("direct:work")` declared
  `concurrency: concurrent` with `max >= 2` whose pipeline sleeps 200 ms,
  and one `DirectProducer` for `direct:work` (shared by clones, as one
  `to:` step shares its producer state)
- **WHEN** two exchanges are dispatched in parallel through that one
  producer
- **THEN** both pipeline executions SHALL be able to be in flight before
  either completes (test: `concurrent_consumer_parallel_admission`)

#### Scenario: Timeout still counts the channel-path wait

- **GIVEN** a Concurrent `direct:name` consumer whose route is configured
  with `max = 1` and holds a first dispatch in flight whose caller's
  `timeout_ms` also elapses, and a producer configured with a short
  `timeout_ms`
- **WHEN** a second dispatch waits for the route and the wait exceeds
  `timeout_ms`
- **THEN** the second dispatch SHALL fail with the canonical timeout error
  text while waiting and SHALL never enter the pipeline, and after release
  neither abandoned exchange reaches a later step
  (test: `direct_timeout_still_counts_admission_wait`)

#### Scenario: An abandoned channel-path dispatch stops at the next step boundary

- **GIVEN** a dispatch already inside a step on the channel path when its
  caller's `timeout_ms` elapses, and a following step in the same route
- **WHEN** the running step finishes
- **THEN** the exchange SHALL NOT be dispatched to the following step, while
  the finished step's effects stand
  (test: `abandoned_dispatch_stops_before_the_next_step`)
