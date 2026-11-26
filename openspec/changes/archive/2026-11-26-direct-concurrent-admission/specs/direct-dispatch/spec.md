## ADDED Requirements

### Requirement: No producer-side serialization for Concurrent consumers

Calls through one direct producer to a route using the Concurrent drain SHALL
be able to overlap, subject to channel backpressure and the route's configured
concurrency limit. Producer `timeout_ms` SHALL still cover channel submission
and the reply wait.

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
  with `max = 1` and holds a first dispatch in flight, and a producer
  configured with a short `timeout_ms`
- **WHEN** a second dispatch waits for the route and the wait exceeds
  `timeout_ms`
- **THEN** the second dispatch SHALL fail with the canonical timeout error
  text while waiting, and the first exchange SHALL still be the only one
  that entered the pipeline; after release, the second exchange SHALL still
  execute (test: `direct_timeout_still_counts_admission_wait`)

## MODIFIED Requirements

### Requirement: Admission serialization for concurrent inline producers

The direct registry SHALL serialize concurrent dispatches targeting the same
Sequential endpoint so at most one dispatch per endpoint executes at a time
and producers are admitted in arrival order. Inline dispatches serialize
through the dispatcher's FIFO admission permit. On the channel path,
Sequential input envelopes SHALL be processed one at a time in target-channel
arrival order. No producer-side admission permit participates on either path.

#### Scenario: Concurrent producers are admitted FIFO

- **GIVEN** multiple producers dispatching concurrently to the same
  Sequential `direct:name` endpoint
- **WHEN** their inline dispatches execute
- **THEN** exactly one dispatch SHALL be in flight at a time and
  completions SHALL occur in FIFO admission order
  (test: `sequential_consumer_keeps_fifo`)
