# Design: direct-inline-dispatch

## Approach

The hop tax is two channel round-trips, so the fix collapses them in two
stages (investigation `rc-wijd`, oracle decision record, 2026-09-04).

**Stage A — Hook B (Phase 1).** The producer stops handing off to the
DirectConsumer loop task. `DirectRegistry` entries gain the consumer's
submission context; `DirectProducer::call` submits through
`send_and_wait` directly. One round-trip removed, no SPI change. All
existing semantics (timeout, error propagation, ordering) stay on the
existing channel machinery.

**Stage B — inline dispatcher (Phases 2-3).** camel-component-api declares
an opaque, opt-in `InlineRouteDispatcher` capability. camel-core implements
it as an adapter that owns: the `SharedPipeline` handle (loaded per call,
held through completion — snapshot isolation per ADR-0004), the consumer
route id, the consumer's drain/cancellation state, and one FIFO admission
permit per endpoint. `DirectConsumer` publishes the dispatcher into
`DirectRegistry` next to the channel fallback; the registry never exposes
`SyncBoxProcessor` outside this adapter (hexagonal boundary: contract types
only in camel-component-api; framework coupling stays in camel-core).

`DirectProducer::call` selects inline when the target is live, ready, and
effectively `ConcurrencyModel::Sequential`; otherwise it uses the channel
path. Concurrent producers targeting a Sequential route queue on the
admission permit, then dispatch inline — producer concurrency alone never
selects the fallback. A task-local endpoint stack (established per call via
the `CANCEL_TOKEN`-style `scope` pattern, shared by nested dispatches)
rejects cycle re-entry immediately and caps acyclic inline depth at 64
(`CamelError::ProcessorError`). A per-endpoint atomic hop counter on the
dispatcher yields the executing task at least once per 32 completed inline
hops (cumulative across all dispatches through the endpoint). `timeout_ms`
keeps its default, error text, and boundary on both paths —
the boundary spans registry lookup, admission wait, and pipeline execution
(cooperative: CPU-bound stretches without an await cannot be interrupted,
same as today).

Cancellation is dual-domain (ADR-0043): the dispatcher takes its drain guard
first, then races the WHOLE operation (admission, cohort barrier, readiness,
pipeline under the `CANCEL_TOKEN` scope) against the consumer token in a
biased `tokio::select!` — the consumer arm wins ties and yields
`CamelError::ConsumerStopping`; producer-side cancellation drops the future
without touching the consumer token, and the drain guard decrements exactly
once on every exit path. Timeout and dispatch
errors keep the consumer route as the b′ owner (ADR-0012). Any new contract
enum follows ADR-0049.

**Phase 4 (independent).** The aggregator resolves a constant correlation
key without per-fragment `serde_json::to_string` and drops redundant key
clones (`crates/camel-processor/src/aggregator.rs` ~412-417, 465-475).

**Measurement.** A criterion microbenchmark in the `camel-bench` crate
(`benches/direct.rs`, bench id `direct_hop`: one producer dispatch through a
no-op consumer pipeline and back) measures the hop itself. Phase 0 adds the
bench and saves the criterion baseline `direct-inline-baseline`; Phases 1
and 3 re-run `cargo bench -p camel-bench --bench direct` against it for
attribution (Phase 1 informational; Phase 3 gates on ratio >= 5x). The
cross-framework `benchmarks/` harness is NOT used for local A/B — the
recorded era-2 split-aggregate m2 (16.7x vs node-native) stays as the
ticket's motivating evidence; any canonical harness re-run after merge is a
separate human-operated decision.

## Affected crates

- camel-component-api: `InlineRouteDispatcher` trait (contract layer).
- camel-core: dispatcher adapter, admission permit, dual-domain
  cancellation wiring in the route controller.
- camel-direct: registry entry shape, producer selection, consumer
  publishing.
- camel-processor: aggregator correlation-key fast resolution (Phase 4).
- In-crate regression tests (camel-component-direct, camel-core adapters):
  cycle/depth/fairness/cancellation coverage lives with the code it pins.

## Architecture boundaries

Contract type lives in camel-component-api; the implementation is a
camel-core adapter — domain components stay framework-agnostic (respects
the hexagonal boundary test and the ADR-0001 data/control plane split: the
fast path moves only data-plane exchanges; control-plane lifecycle commands
are unchanged). DSL, services, languages, and functions are untouched.

## Phases

### Phase 0: Guardrails and baseline
- **Goal:** pin current cycle semantics, add the `direct_hop` criterion
  bench, and record the "before" number.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** none.
- **Deliverable:** regression test pinning that a `direct:` cycle never
  succeeds, hangs, or overflows (today: timeout error); the `direct_hop`
  criterion bench with its saved baseline.
- **Exit-criteria:** cycle test green on unmodified code; `cargo bench -p
  camel-bench --bench direct` runs; baseline `direct-inline-baseline` saved
  with the median recorded in `bench/baseline.md`.

### Phase 1: Collapse camel-direct channel (Hook B)
- **Goal:** remove the first round-trip; producer submits via
  `send_and_wait` directly.
- **Dependencies:** Phase 0.
- **Externally-visible types/interfaces:** none.
- **Deliverable:** reworked `DirectRegistry` entry + `DirectProducer`.
- **Exit-criteria:** all camel-direct tests green; bench re-run recorded
  in the change dir (informational attribution — not a gate).

### Phase 2: InlineRouteDispatcher seam
- **Goal:** capability trait + camel-core adapter published by
  DirectConsumer; no producer behavior change yet.
- **Dependencies:** Phase 1.
- **Externally-visible types/interfaces:** `InlineRouteDispatcher` trait
  (camel-component-api).
- **Deliverable:** trait, adapter, registry storage with channel fallback.
- **Exit-criteria:** hexagonal test green; startup-handshake tests green;
  capability absent by default for other components.

### Phase 3: Inline fast path
- **Goal:** producer selects inline dispatch with guards; capability
  publication covers resume_route too (rc-y4vk).
- **Dependencies:** Phase 2.
- **Externally-visible types/interfaces:** none beyond Phase 2.
- **Deliverable:** selection predicate, cycle/depth guard, 32-hop yield,
  dual-domain cancellation, timeout parity, Concurrent fallback.
- **Exit-criteria:** bench gate — criterion `direct_hop` ratio
  (Phase 0 baseline median / Phase 3 median, from criterion estimates.json)
  is at least 5x; cycle test tightened to immediate error; depth-64,
  fairness, and cancellation tests green.

### Phase 4: Aggregator correlation-key trim
- **Goal:** remove per-fragment serialization waste.
- **Dependencies:** none (independently shippable; ordered last).
- **Externally-visible types/interfaces:** none.
- **Deliverable:** constant-key fast resolution, clone reduction.
- **Exit-criteria:** `cargo test -p camel-processor` green; aggregator
  behavior unchanged.

## Alternatives considered

- Raw async call into the pipeline (no capability): bypasses envelope,
  admission, cancellation, and lifecycle accounting — rejected.
- Pooled oneshots / channel tuning: keeps both round-trips — rejected.
- Unbounded channel without oneshot: drops backpressure, keeps wakeups —
  rejected.
- Dedicated dispatcher task: same handoff count, worse — rejected.
- Generic SPI exposing processors to all components: blast radius and leak
  risk — rejected (oracle verdict: direct-only capability).
