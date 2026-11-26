# Design: direct-concurrent-admission

## Approach

Delete the producer-side admission permit. `DirectProducer` loses its
`semaphore` field; the channel path in `call` submits through
`ctx.send_and_wait` without acquiring anything. The single timed section
(`lib.rs:472`) already spans lookup, wait, and execution, so producer deadline
and timeout error are preserved by construction.

This is the smallest correct fix because the dispatch-time probe (below) shows
both channel-path populations already have the admission semantics the spec
requires, provided by the target route's drain topology. The producer permit
gated submission queueing only; it never gated pipeline execution.

Caveat carried into docs and tests: the producer deadline and error are
unchanged. More calls can reach the bounded route queue before timing out.
Once enqueued, an exchange can still execute after its caller times out
(`lib.rs:468-472`; the channel path's reply wait drops the receiver without
retracting the envelope).

## Dispatch-time probe (verified on main `a13416220`)

Question: is the consumer kind (Sequential vs Concurrent) observable at
dispatch? Findings, with evidence:

1. The core publishes the inline dispatcher capability only for non-Concurrent
   AND non-aggregate-split topologies
   (`crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs:396-408`).
2. The effective concurrency model is resolved at route start
   (`route_controller_trait.rs:348-349`), but it is NOT published on the
   `ConsumerContext` and NOT stored in `DirectEntry`
   (`crates/components/camel-direct/src/lib.rs:46-50`). At dispatch the
   producer sees only `dispatcher: Option<...>`.
3. `dispatcher: None` is an ambiguous proxy: it covers Concurrent consumers
   AND capability-unavailable Sequential entries (aggregate-split). So the
   kind is NOT observable at dispatch without a new api-layer capability.
4. Both `None` populations serialize (or parallelize) correctly without any
   producer-side permit:
   - Concurrent: the route controller spawns one task per envelope, bounded by
     the route `max` semaphore (`route_controller_trait.rs:456-479,539`). Two
     submitted exchanges run in parallel up to `max`.
   - Sequential (channel arm): ONE drain task runs a `rx.recv()` loop and
     awaits each pipeline call before the next receive
     (`route_controller_trait.rs:617-747`). The mpsc channel preserves
     arrival order, so exactly one pipeline execution is in flight, in channel
     arrival order. The resume path shares this topology
     (`route_controller.rs:1189-1332`).
   - Aggregate-split: the aggregate engine drives pre/agg/post from the
     channel; fragments are processed one at a time by the engine's consumer
     task (`route_controller_trait.rs:411-438`; spec scenario `:52-62` stays
     satisfied). Aggregate-split is a topology, not a concurrency model.

Per the order: because the kind is not observable at dispatch, choose the
smallest correct alternative — remove the permit, and let the mechanisms that
already own admission keep it. A per-consumer-kind permit or endpoint-kind
lookup would need a new `ConsumerContext`/`DirectEntry` field across
camel-component-api + camel-core + camel-direct to distinguish the two `None`
populations — and the distinction would change nothing, because both behave
correctly unserialized at the producer.

## Why this cannot reorder Sequential admission

FIFO means arrival at the target channel, not producer invocation or caller
return order. Removing the permit can change when calls reach that channel,
but the drain still processes input envelopes serially in channel order
(Sequential channel arm `route_controller_trait.rs:617-747`; aggregate engine
fragment processing `route_controller.rs:1189-1332`). Aggregate-split routes
process fragments serially; this does not imply FIFO completion across
buckets.

- Inline Sequential: the dispatcher's admission mutex serializes dispatches
  (`crates/camel-core/src/lifecycle/adapters/inline_dispatcher.rs:44-46,96,163`).
  This change does not touch it. Test `sequential_consumer_keeps_fifo` (renamed
  from `concurrent_producers_serialized_fifo`) pins it deterministically.
- Channel-path Sequential (capability-unavailable, aggregate-split): the
  single drain task executes one envelope at a time in channel arrival order.
  The removed permit only delayed SUBMISSION of the second call; execution
  order was already channel arrival order. Test
  `sequential_channel_consumer_keeps_fifo` pins the end-to-end trace.
- Concurrent: overlap is the declared intent of `concurrency: concurrent`,
  bounded by the route `max`.

## Affected crates

- `camel-direct`: remove `semaphore` field, its construction
  (`lib.rs:272`), clone arm (`:400`), and channel-path acquire (`:517-521`);
  update the stale comments (`:427-431` poll_ready rationale stays but drops
  the permit wording, `:512-515` channel-path comment names the real
  mechanism). Tests: delete `call_blocks_on_semaphore_until_release` and
  `call_pending_when_all_permits_held`; rename
  `concurrent_producers_serialized_fifo` to `sequential_consumer_keeps_fifo`
  with the deterministic admission-order assertion; update the
  `direct_producer` helper and any remaining `semaphore` field references.
- `camel-test`: new `tests/direct_concurrent_admission_test.rs` with three
  route-level tests (below), driven through the harness's paused clock
  (`CamelTestContext::builder()...build()` returning a `TimeController`, the
  existing harness pattern) so the 200 ms sleeps and short timeouts are
  virtual time and the assertions are deterministic.
- `docs`: `docs/src/components/direct.md:97` documents the permit; rewrite the
  paragraph to the permit-free channel path and carry the deadline caveat:
  more calls can reach the bounded route queue before timing out; once
  enqueued, an exchange can still execute after its caller times out.

## Architecture boundaries

Component-internal change plus one integration test file. No camel-component-api
change, no camel-core change, no DSL change. The inline dispatcher, cycle/depth
guard, timeout parity construction sites, and the b′ emission site are
untouched.

## Tests

Named exactly (order requirement + D5 finding):

1. `concurrent_consumer_parallel_admission` — camel-test route-level, paused
   clock. One producer resolved through the harness context, two clones (the
   shared `to:`-step shape), target route `from("direct:work")` with
   `.concurrent(2)` and a 200 ms sleep step; both calls join with elapsed
   virtual time under 350 ms (200 ms sleeps; red pre-fix at ~400 ms virtual).
   Assert both pipeline executions can be in flight before either completes.
2. `sequential_consumer_keeps_fifo` — camel-direct unit (rename of
   `concurrent_producers_serialized_fifo`). Pin and poll the four call futures
   once in `p0`–`p3` order BEFORE driving them to completion (spawn order is
   not admission order; the pin-and-poll makes the arrival order explicit),
   then drive to completion. Retain the paired begin/end (no interleaving)
   checks and assert the exact admission order `["p0","p1","p2","p3"]`.
3. `direct_timeout_still_counts_admission_wait` — camel-test route-level,
   paused clock. Concurrent route `direct:work` with `max = 1`; park the
   first exchange in its pipeline; submit a second through a clone of the
   same producer with a short `timeout_ms`; assert it fails with
   `CamelError::ProcessorError("direct:work call timed out")` while only the
   first exchange has entered the pipeline. Release the first and observe the
   second enter the pipeline (enqueued work still executes after its caller
   timed out).
4. `sequential_channel_consumer_keeps_fifo` — camel-test route-level, paused
   clock. A real Sequential aggregate-split route (`from("direct:agg")` with a
   top-level `aggregate` compiled as pre/agg/post); park `p0` in its
   pre-stage; submit `p1`, `p2` through clones of one producer and poll for
   their arrival; assert only `p0` has entered the pre-stage before release;
   after release, assert the exact pre-stage trace
   `begin:p0, end:p0, begin:p1, end:p1, begin:p2, end:p2`.
