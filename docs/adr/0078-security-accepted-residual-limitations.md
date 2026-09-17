# ADR-0078: Security Accepted-Residual Limitations (2026-08-31 Audit)

- Status: Accepted (decided 2026-08-31, canonized 2026-09-17)
- Source: security audit 2026-08-31 (unversioned, `docs/audits`)
- Ruling: e_gpt ruling of 2026-09-16 (ADR-0076 adjacent). Accepted risks
  are decisions. Decisions go to ADRs.
- Companion: ADR-0077 section "Consequences" (already excludes the JS
  sandbox from OOM fuzzing because of F3-2)

## Context

The 2026-08-31 security audit fixed every actionable finding in-tree, each
with an adversarial regression test. Five acceptance-class items remained
with no ADR home. The 2026-09-16 ruling requires each accepted risk to live
in an ADR. This ADR records those five items with their acceptance
rationale. It adds no new mitigations and reopens no decision.

## Decision

All five items below are **accepted as residual risk (2026-08-31 audit)**.

1. **F3-2, JS (Boa) heap amplification.** Boa 0.21 exposes no heap cap. A
   script such as `'x'.repeat(2**31)` in an in-process context can exhaust
   process memory. The CPU, loop, recursion, and source-size limits bound
   time, not memory. Acceptance rationale: script source is operator
   config. Untrusted JavaScript must use the out-of-process `function:`
   path. A Boa heap-limit API does not exist yet. Residual exposure: an
   operator-supplied script can crash the process with out-of-memory.

2. **F3-3, timed-out scripts cannot be cancelled.** `tokio::time::timeout`
   abandons a `spawn_blocking` task but does not kill it. With
   operator-raised operation limits, a CPU-bound script holds
   blocking-pool threads past the wall-clock timeout. Acceptance
   rationale: this is an inherent tokio limitation. Default limits trip in
   milliseconds. Residual exposure: blocking-pool threads stay occupied
   after a timeout when limits are raised.

3. **F6-4, aggregator force-complete drop under late-channel pressure.**
   The force-complete path emits through a bounded late channel (capacity
   256). When the channel is full, it drops the exchange and logs a warn.
   Acceptance rationale: accepted divergence D-A3 (ADR-0046 family,
   documented in `crates/camel-processor/CONTEXT.md`, pinned by test).
   Residual exposure: exchange loss during force-complete under
   late-channel pressure.

4. **F6-5, configuration foot-guns.** `max_buckets(0)` is accepted and
   denies every exchange. SQL `One` and `List` modes materialize
   operator-bounded result sets. Acceptance rationale: availability-only
   and operator-triggered. Residual exposure: operators can degrade their
   own routes through configuration. No integrity or confidentiality impact.

5. **F6-6, no global cross-route concurrency semaphore.** Total
   concurrency equals the sum of the configured route and component
   limits. No knob bounds the sum. Acceptance rationale: bounded by
   deployment sizing. This is an architectural decision. Residual
   exposure: aggregate load can exceed any single route cap. The operator
   must size the deployment.

## Source accounting note

The audit header counts 22 findings as 17 fixed, 3 accepted, and 2
deferred. The audit sections enumerate a different split. Section 2 lists
21 fixed finding IDs. F4-5 and F4-6 share one heading. Section 3 lists 2
accepted items. Section 4 lists 4 deferred items. The header severity
subtotals also disagree with the section headings. The header says one HIGH
fixed finding. Section 2 lists F4-1 and F6-1 as HIGH, plus F6-2 as
MEDIUM-HIGH. This ADR records the discrepancy as found and does not
resolve it. The five canonized items span sections 3 and 4. Each carries
acceptance rationale in the source.

## Consequences

- No mitigation work follows from this ADR. Reopening any item requires a
  new decision that supersedes this ADR.
- F3-2 has one named external unblock condition: a Boa heap-limit API. The
  audit names no unblock condition for F3-3. It records F3-3 as an inherent
  tokio limitation.
- The audit source stays unversioned per the `docs/audits` ruling. This
  ADR is the durable record of the five acceptances and of the accounting
  discrepancy.
