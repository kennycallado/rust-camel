# Design: aggregator-adr0046-divergences

## Approach

Pure documentation + native-test change. The spike (`rc-mybm-spike` KB source)
already classified every divergence with code-level evidence and the forcing
ADR. Implementation is mechanical: translate each classified divergence into
(a) one CONTEXT.md sub-section and (b), where the divergence is behaviorally
testable, one native test that pins OUR semantics. D-A4 and G-A1 are
mechanism/knob notes covered by existing tests, so they add documentation
only. No architectural decision remains open at implementation time.

The documentation lives in `crates/camel-processor/CONTEXT.md` under a new
top-level section "Aggregator EIP divergences from Apache Camel (ADR-0046
protocol)". This co-locates with the existing Splitter divergence block
("Aggregation contract (divergence from Apache Camel)") so a reader sees all
aggregation-family divergences together. The Splitter block stays as-is
(that is D2 for the Split EIP specifically); the new block covers the
Aggregate EIP itself.

Native tests extend the existing inline `#[cfg(test)] mod tests` in
`crates/camel-processor/src/aggregator.rs` (the established harness there —
`new_test_svc`, `config_size`, `make_exchange`). No new test file is created;
the precedent (Splitter) added no test, but the divergences here are richer
and benefit from semantic pins.

## Affected crates

- **camel-processor**: `CONTEXT.md` (5 divergence sub-sections + 1 gap note);
  `src/aggregator.rs` (native test additions in the existing test module).
  References `camel-api/src/aggregator.rs` type definitions via the normal
  cross-crate dependency (no modification of those types from this crate).
- **camel-api**: `src/aggregator.rs` modified — a `compile_fail` doctest on the
  `AggregationFn` alias (Task 2) and a typed-variant unit test
  `test_da5_validate_returns_typed_missing_memory_bound_variant` (Task 5). No
  behavior or signature change; only doc-comments and test code are added.

## Architecture boundaries

Respected trivially — this change touches only documentation and tests in the
processor crate. It crosses no Runtime/DSL/Component/Language/Function
boundary. The divergences themselves are forced by boundaries that already
exist:

- D-A1, D-A2 → forced by the `camel-api` `AggregationFn` contract shape
  (binary `(Exchange, Exchange) -> Exchange`: no null oldExchange on the first
  bucket message, no `Result` return to signal failure).
- D-A3 → forced by the nonblocking `force_complete_all() -> ()` signature
  (it cannot return completed exchanges inline) plus the bounded `late_tx`
  mpsc channel that drains them into `post_pipeline`.
- D-A4 → forced by the existing timeout/task-cap/TTL configuration contracts
  (`CompletionCondition::Timeout`, `max_timeout_tasks`, `bucket_ttl`): the
  per-bucket task + sweep + cap IS the configured mechanism, not a missing
  central scheduler.
- D-A5 → forced by ADR-0033 (security defaults: `validate()` rejects
  unbounded configs).

No boundary is moved or created by this change.

## Phases

Omitted — single-phase. All six deliverables are coherent documentation+test
additions across two crates sharing one goal (land the ADR-0046 Aggregator
divergence inventory). Task 1 is a prerequisite for Tasks 2–6: it creates the
CONTEXT.md section scaffold (header + preamble + D-A1 subsection) that Tasks
2–6 append their subsections to. The conductor dispatches sequentially in
numbered order `1→2→3→4→5→6`. This intra-phase ordering does NOT make the
change multi-phase (the unit of blessing is the whole 6-task plan); it is a
PHASE 3 dispatch constraint, consistent with single-phase status. (Absence of
"Phases" here AND absence of `## Phase N` headings in `tasks.md` together
signal single-phase per the conductor-light contract.)

## Alternatives considered

- **New ADR per fundamental divergence.** Rejected: none of the 5 divergences
  is fundamental. Each is forced by an existing ADR (0019/0033/0044) or by the
  Tower contract shape that those ADRs already codify. A new ADR would
  duplicate existing decisions. If plan-bless finds one IS fundamental,
  escape to PHASE 1.
- **Amend ADR-0046 inline with the Aggregator findings.** Rejected:
  ADR-0046 is the protocol definition, not a per-EIP findings log. Per-EIP
  divergences belong in the crate CONTEXT.md (ADR-0046 §Anti-pattern §5).
- **Separate test file `aggregator_divergence_tests.rs`.** Rejected: the
  existing inline test module in `aggregator.rs` is the established home and
  shares the `new_test_svc` harness. A new file would duplicate harness setup.
- **Translate Camel assertions literally.** Rejected per ADR-0046
  §Anti-pattern §2 — literal translation produces invalid-green or spurious-red.
  Tests assert OUR semantics.
