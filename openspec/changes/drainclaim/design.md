# Design: drainclaim

## Approach

One context-global lifecycle counter (`Arc<AtomicU64>`, RMWs at `AcqRel`,
read at `Acquire` — same ordering discipline as seda's `DepthGuard`)
counts accepted-not-completed exchanges. `CamelContext::total_in_flight()`
is a single atomic load: the drain verdict is linearizable by
construction, with no multi-counter co-zero state to manufacture.

**Claim mechanism — envelope-carried RAII.** `ExchangeEnvelope`
(camel-component-api `consumer.rs`) gains
`in_flight_claim: Option<InFlightClaim>`. `InFlightClaim` owns one
increment; `Drop` decrements exactly once; `split()` mints a sibling claim
(+1) for fanout copies. Attach at acceptance:

1. **Seda producer enqueue** (queue residency): `SedaProducer` captures
   the counter once at creation via its existing
   `Arc<dyn RuntimeObservability>` handle and attaches a claim where
   `DepthGuard::count_in` counts the envelope in; fanout attaches one
   claim for the first subscriber copy and `split()`s one per additional
   permit.
2. **`ConsumerContext::send` / `send_and_wait`** (non-seda dispatch): the
   context carries the counter (installed by camel-core at consumer
   start) and attaches a claim to every envelope it constructs.
3. **`InlineRouteDispatcher` direct call**: the camel-core
   `RouteInlineDispatcher` state carries the counter; `dispatch` mints a
   claim and moves it into the returned future, held across the pipeline.

Release at pipeline completion: the pipeline drain sites
(`route_controller_trait.rs` concurrent + sequential paths,
`route_controller.rs` pre-pipeline path) destructure the claim out of
every dequeued envelope and hold it in task scope across the pipeline
await. RAII covers every exit: push-failure rollback (the dropped
envelope releases its claim), queued-envelope drop at route stop,
pipeline-task abort, panic, readiness failure, and normal completion.

**Handoff overlap** falls out of parameter lifetimes with ZERO seda
forwarder changes: `forward_envelope` owns the queue envelope, and its
shell (with the producer's claim) drops after `ctx.send` returns — by
which time the route envelope already carries the new claim. Claims
overlap; there is no uncovered instant on any hop.

**Timer policy.** Scheduled auto-firing consumer routes (`timer:`,
`cron:`) are already rejected at load by the fail-closed
`JOB_SAFE_CONSUMER_SCHEMES` gate (job `document.rs`), for both modes.
This change pins that gate as a drain-soundness precondition — without
it, a scheduled producer could raise the counter after a zero read and
destabilize the verdict — with a spec scenario and a regression test; no
gate behavior changes. Interactive `stream:in` input can still arrive
after a zero read; it remains bounded by the overall deadline backstop,
exactly as today.

**Raw-sender exception.** The http/grpc/ws/master per-request paths that
push envelopes through `ConsumerContext::sender()` construct them without
claims and stay uncounted — a documented spec exception (follow-up
rc-nftni). Compile-compatibility edits in those crates (and
`camel-bench`) are mechanical field additions / pattern ellipses only:
`in_flight_claim: None`, no behavior change.

**Execution-discovered exceptions (same under-count class, out-of-zone
fixes, filed as follow-ups).** Route-level aggregate paths ARE covered —
pending-bucket exchanges carry their claims into the bucket and every
emission/drop path releases exactly once (execution amendment to the
take-and-hold rule, commit f66e69f6). Two stash sites cannot carry
envelope-scoped claims and remain uncounted: (1) resequencer buffers —
exchanges stash behind `dyn ResequencePolicy` + raw `Exchange` channels
(rc-hllkk); (2) size-only aggregators compiled inside pipelines, whose
partial buckets outlive the embedding pipeline (rc-qbigm). Both require
Exchange-carried claims (a camel-api seam) and are excluded from this
change's guarantee, like the raw-sender path.

**Glossary (canonical claim vs stop-bookkeeping).** The canonical claim
is context-global and means accepted-not-completed over the whole
exchange lifecycle (enqueue → dispatch → pipeline completion). It is
distinct from the existing per-route `drain_in_flight`
(`route_helpers.rs`, ADR-0043), which counts dequeued-to-completion for
stop ordering; that counter and `DrainGuard` are untouched. The UoW
per-route `in_flight` counter and all gauges are untouched too.

## Affected crates

- `camel-component-api`: `InFlightClaim` type; envelope field;
  `ConsumerContext` counter + attach; `ComponentContext` /
  `RuntimeObservability` default-`None` `in_flight_counter()` accessor
  (blanket impl forwards; core's controller path overrides).
- `camel-core`: `CamelContext` counter field + `total_in_flight()`;
  counter install at `ConsumerContext` construction and dispatcher state;
  claim take-and-hold at the three pipeline drain sites.
- `camel-component-seda`: producer counter capture + enqueue attach +
  fanout split.
- `camel-cli`: `commands/job/batch.rs` drain rewrite (poll
  `total_in_flight()`, delete probe/streak), `mod.rs` probe registration
  removal.
- Mechanical compile-compat only: `camel-bench`, `camel-component-grpc`,
  `camel-http`, `camel-ws`, `camel-master`, `camel-direct` literal /
  pattern updates (`None`, no behavior change).

## Architecture boundaries

Components stay data-plane: they see an opaque `Arc<AtomicU64>` mint
accessor, no runtime knowledge (hexagonal seam mirrors
`component_metrics_enabled`). `total_in_flight()` is a control-plane read
on `CamelContext` (data/control split per CONTEXT-MAP). ADR-0012: no new
emissions; ADR-0043: `drain_in_flight` untouched; ADR-0064: lean test
set; ADR-0066: no new collector binding.

## Phases

### Phase 1: canonical claim primitive

- **Goal:** every exchange accepted through a counted path holds at least
  one live claim across its whole lifecycle; claims may temporarily
  overlap at handoffs (successor attached before predecessor drops) and
  fanout multiplies claims per subscriber copy.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** `InFlightClaim`;
  `ExchangeEnvelope.in_flight_claim`; `CamelContext::total_in_flight()`;
  `ComponentContext::in_flight_counter()`.
- **Deliverable:** api + core + seda changes with unit tests for every
  release path (push-failure, queued-drop, abort, panic,
  readiness-failure, inline dispatch, fanout split, raw-sender None).
- **Exit-criteria:** a barrier-held exchange keeps `total_in_flight()`
  nonzero and the zero/nonzero predicate is exact through every release
  path (the numeric value may transiently exceed the live-exchange count
  during handoff overlap — that is sound, not a leak); handoff overlap
  proven by a dispatch-boundary test.

### Phase 2: batch drain on the linearizable snapshot

- **Goal:** CLI drains on `total_in_flight() == 0`; heuristic gone.
- **Dependencies:** Phase 1 only.
- **Externally-visible types/interfaces:** none (job CLI behavior per
  spec deltas).
- **Deliverable:** `batch.rs` + `mod.rs` rewrite, timer-gate regression
  test, job tests with deterministic barriers.
- **Exit-criteria:** barrier-parked self-feed never false-completes;
  settled jobs complete without a quiescence window; exit codes, signals,
  reports unchanged.

## Alternatives considered

- **Composite per-route counters + live depth (mission 144):** rejected —
  two counters read together can interleave to a manufactured co-zero.
- **Per-route counter with unconditional enablement:** same co-zero
  exposure across routes, plus per-route fanout ambiguity.
- **Edge-triggered gauge emissions:** concurrent RMWs can emit
  out-of-order snapshots, producing a false zero.
- **New `RuntimeQuery` variants:** the job CLI owns the context
  in-process; a direct read suffices (YAGNI).
