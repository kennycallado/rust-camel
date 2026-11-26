# Proposal: direct-concurrent-admission

## Why

`create_producer` hard-codes the admission permit to one
(`crates/components/camel-direct/src/lib.rs:272`, `Semaphore::new(1)`). Every
clone of one `to: "direct:name"` step shares that permit, and the channel path
takes it before `send_and_wait` (`lib.rs:517-521`). A Concurrent consumer has
no dispatcher capability, so it always takes the channel path. Result: two
calls through one producer into a route declared `concurrency: concurrent`
with `max >= 2` still execute one at a time. Declaring concurrency on the
target route has no effect for that caller. Nothing in
`openspec/specs/direct-dispatch/spec.md` requires serializing a Concurrent
consumer; the FIFO admission requirement (`:112-125`) covers Sequential
endpoints only. This is bd `rc-zy47o` (P2, observer finding 5, 2026-11-26).

## What Changes

- Remove the producer-side admission permit from the channel path: the
  `DirectProducer` no longer holds a `Semaphore`, and channel-path dispatches
  submit immediately. Admission semantics stay with the mechanisms that own
  them: the inline dispatcher's FIFO admission mutex for Sequential inline
  dispatches, and the target route's drain topology for channel-path
  dispatches (Sequential: single drain task, arrival order; Concurrent:
  per-envelope tasks bounded by the route `max`).
- Remove the two unit tests whose premise is the permit
  (`call_blocks_on_semaphore_until_release`,
  `call_pending_when_all_permits_held`).
- Add the four named tests: `concurrent_consumer_parallel_admission`
  (route-level, paused clock, red pre-fix),
  `sequential_consumer_keeps_fifo` (rename of
  `concurrent_producers_serialized_fifo` plus a deterministic arrival-order
  assertion), `direct_timeout_still_counts_admission_wait` (route-level,
  Concurrent route with `max = 1`, channel-path timeout parity), and
  `sequential_channel_consumer_keeps_fifo` (route-level, Sequential
  aggregate-split route, channel-arrival-order trace).
- Spec delta to `openspec/specs/direct-dispatch/`: one ADDED requirement (no
  producer-side serialization for Concurrent consumers) and one MODIFIED
  requirement (admission serialization names both mechanisms and pins
  channel-arrival order for Sequential input).
- Update `docs/src/components/direct.md`, which documents the permit today.

Excluded: camel-core dispatch topology, the inline dispatcher, the
`ConsumerContext` API. No api-layer change.

## Acceptance criteria

- `concurrent_consumer_parallel_admission` fails pre-fix (elapsed virtual
  time ~400 ms) and passes post-fix (both executions in flight before either
  completes; 200 ms sleeps on a paused clock, both calls under 350 ms):
  two parallel calls through one producer into a 200 ms Concurrent route
  overlap.
- `sequential_consumer_keeps_fifo` passes: inline FIFO admission keeps exactly
  one dispatch in flight, executes begin/end pairs without interleaving, and
  admits in `p0`–`p3` order.
- `sequential_channel_consumer_keeps_fifo` passes: through clones of one
  producer into a Sequential aggregate-split route, the pre-stage executes
  exactly `begin:p0, end:p0, begin:p1, end:p1, begin:p2, end:p2`.
- `direct_timeout_still_counts_admission_wait` passes: with a Concurrent
  route at `max = 1` holding a first dispatch, a second dispatch with a short
  `timeout_ms` fails with the canonical
  `direct:work call timed out` error while only the first exchange has
  entered the pipeline; after release, the second exchange still executes.
- All existing camel-direct tests, the camel-core direct suite, and the
  AGENTS.md quality gates stay green.

## Risk budget

- The only behavior change is on the channel path. Sequential entries on that
  path (aggregate-split, capability-unavailable) stay serialized by the
  route's single drain task in target-channel arrival order, so their
  processing order does not change. The caveat: the producer deadline and
  error are unchanged. More calls can reach the bounded route queue before
  timing out. Once enqueued, an exchange can still execute after its caller
  times out.
- The permit never gated pipeline execution on any path; it gated submission
  queueing only. Removing it changes queue depth, not ordering guarantees.
- Aggregate-split is a topology, not a concurrency model; its fragments
  process serially, which does not imply FIFO completion across buckets.

Affected crates: `camel-direct`, `camel-test` (tests). bd: rc-zy47o.
