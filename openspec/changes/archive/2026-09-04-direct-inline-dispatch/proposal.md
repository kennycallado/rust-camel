# Proposal: direct-inline-dispatch

Bd: rc-wijd — camel-direct per-message task handoff.

## Why

Every `direct:` hop in rust-camel pays up to 4 scheduler wakeups across two
channel round-trips: (1) the producer's `mpsc + oneshot` into the
DirectConsumer loop task, then (2) that loop's `send_and_wait`, a second
`mpsc + oneshot` into the route-controller pipeline task
(`crates/components/camel-direct/src/lib.rs`, `camel-core` route controller).
JVM Camel `direct:` is a synchronous in-thread procedure call with zero
wakeups.

Recorded baseline (era-2 run `20260903T084658Z`, `benchmarks/records/`):
split-aggregate `rust-camel-lib` m2 = 1,592,506 ns/tick (~15.9 us/fragment)
vs `node-native` 95,409 ns (~0.95 us) — a 16.7x gap on a route whose hot
loop is 100 `direct:` hops per tick. Any user route through `direct:` pays
this tax.

## What Changes

- Add an inline-dispatch fast path for `direct:`: when the target consumer
  is live, ready, and effectively `ConcurrencyModel::Sequential`, the
  producer executes the consumer pipeline inline in its own task — no
  mandatory inter-task channel handoff or its associated wakeups.
- Introduce an opaque `InlineRouteDispatcher` capability, implemented by
  camel-core as an adapter and requested only by `DirectConsumer`.
  `SyncBoxProcessor` is NOT exposed through the generic consumer SPI.
- Keep the channel path as fallback: `Concurrent` model routes and
  capability-unavailable cases dispatch as today.
- Serialize concurrent producers targeting a Sequential route with one
  registry-owned FIFO admission permit, then dispatch inline.
- Guard the inline path: cycle re-entry (`direct:a -> b -> a`) fails
  immediately; inline depth caps at 64; the task yields at least once every
  32 completed inline hops.
- Keep `timeout_ms` semantics on both paths (same default, error, boundary).
- Inline work belongs to both the caller's and the consumer's cancellation
  domains (ADR-0043): consumer drain grace, then `pipeline_cancel`, then
  `CamelError::ConsumerStopping`.
- Phase 4 (independent): remove the aggregator's per-fragment
  `serde_json::to_string` of a constant correlation key and redundant key
  clones (`crates/camel-processor/src/aggregator.rs`).

Excluded: the cli-vs-lib startup gap (separate bd), SEDA/`vm:` changes, and
any generic inline-dispatch SPI for all components.

## Acceptance criteria

- `camel-bench` criterion bench `direct_hop` (one dispatch through a
  no-op consumer pipeline): at least 5x faster after Phase 3 against its
  Phase 0 baseline (order-of-magnitude aspiration; node-native scenario
  ceiling 0.95 us/fragment for context).
- A `direct:` cycle terminates with an error before an external deadline —
  never succeeds, hangs, or overflows the stack; depth 65 returns
  `CamelError::ProcessorError`.
- All existing camel-direct tests pass unchanged (timeout, startup
  handshake, `failIfNoConsumers`, duplicate consumer rejection).
- Hexagonal architecture boundary test passes; no `SyncBoxProcessor` in
  public generic SPI.
- All AGENTS.md quality gates green.

## Risk budget

Acceptable: cyclic routes change from deadlock-until-timeout to immediate
error; concurrent-producer ordering changes from semaphore race to FIFO.
Out of bounds: breaking `ConsumerContext` for other components, changing
SEDA semantics, dropping the timeout contract, exposing raw processors.

## Affected crates

- camel-component-api: `InlineRouteDispatcher` capability trait.
- camel-core: adapter (pipeline snapshot, admission, cancellation scope).
- camel-direct: registry entries, producer dispatch, consumer publishing.
- camel-processor: aggregator correlation-key trim (Phase 4).
- camel-bench: new `direct_hop` criterion bench; Phase 0 baseline +
  per-phase re-runs (the cross-framework `benchmarks/` harness is not part
  of this change).
