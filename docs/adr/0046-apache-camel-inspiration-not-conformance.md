# ADR-0046: Apache Camel as design-inspiration corpus, not conformance authority

**Date:** 2026-07-17
**Status:** Accepted
**Amends:** none
**Cross-refs:** epic rc-ca8z (the positioning decision this ADR codifies at the architectural level), ADR-0019 (ExceptionDisposition — basis for D2 divergences), ADR-0024 (PipelineOutcome — replaces CamelError::Stopped), ADR-0025 (outcome-aware structural EIPs), ADR-0032 (Exchange-data trust boundary), ADR-0033 (security defaults — policy-ADR precedent)

## Decision

Apache Camel is a **design-inspiration corpus**, NOT a conformance authority. Decisions about what an EIP should do are designed against the project ADRs, not against the behavior observed in Camel. Camel stays valuable because it encodes 20 years of real production edge cases. But it is **input to design**, not an acceptance spec.

### Consultation protocol (mandatory for new EIPs or major redesigns)

When you design or implement a new EIP, or substantially redesign an existing one:

1. **Trigger by divergence density.** Apply the full protocol **only if** the EIP touches at least one ADR that breaks conformance with Camel. Operational markers:
   - Stateful EIP (aggregation, correlation, repositories)
   - Timing with completion or timeout (aggregate completion, resequencer)
   - Divergent control-flow (ADR-0019 ExceptionDisposition, ADR-0024/0025 PipelineOutcome, Stop EIP)
   - Trust-boundary impact (ADR-0032 — untrusted data in sinks or numeric decisions)
   - Backpressure or admission (ADR-0044 — Camel has no `poll_ready`)

   For stateless EIPs that are nearly identical to Camel (Filter, Content-Based Router, Throttle, SetBody/SetHeader), a pure **coverage audit** is enough. You do not need to read Camel.

2. **Dose: 3 tests, not 5.** Read 3 representative tests from `apache/camel/<comp>/src/test/java/...`. Stop when 2 consecutive tests add no new scenario. A full table of 5 or more tests gives flat marginal value.

3. **Classify while you read.** No separate tabulation phase. For each test, extract (a) the scenario, (b) the EIP invariant exercised, and (c) the decision: same or diverges. Document divergences inline in the EIP ADR (if one exists), or in the crate `CONTEXT.md` (if it is cross-EIP).

4. **Native tests, not translations.** Write tests with the project harness (`CamelTestContext`, `MockEndpoint`, etc.) that assert **our** semantics. Never translate asserts literally. Literal translation produces invalid green or spurious red.

5. **KPI: divergences-documented/EIP, not bugs/hour.** Bugs found by this protocol are bugs a coverage audit would also find. The irreplaceable value is the **divergences forced into documentation**. Those appear only when you read the feature space of Camel that we deliberately do not implement.

## Context

Epic `rc-ca8z` fixes the positioning: "a distinct cloud-native runtime with EIP vocab compat ONLY, not a drop-in replacement". The operational consequence — "what does a dev do when they ask whether an EIP should behave like Camel" — was not codified. Without codification, two risks:

1. **Drift by inertia:** a dev ports tests by habit. This produces invalid green (the test passes for the wrong reason) or spurious red (the test fails on behavior our ADRs declare correct).
2. **Memory loss:** divergence decisions are made implicitly and lost in context compaction. They leave cognitive debt for the next contributor.

The spike `rc-spt-camel-splitter-spike` (branch `spike/rc-spt-camel-splitter-spike`, commit `8d31e74a`) produced concrete evidence:

- **2 divergences forced into documentation** (D1 `parallelAggregate()` does not apply architecturally — `join_all` is sequential; D2 aggregation receives `Err(e)` in a `Vec`, not an Exchange with an attached exception — ADR-0019). Both are decisions that **only reading Camel reveals**.
- **1 real bug** (G3 `CAMEL_SPLIT_SIZE` never set on the last streaming fragment). This is a coverage gap a coverage audit would find.
- **3 pinned pre-existing invariants** (G1 unique IDs per fragment, G2 split JSON array, streaming semantics). Coverage, not bugs.

The spike also confirms that an automatic `cargo xtask port-camel-test` would: produce invalid green on `parallelAggregate()` (semantics that do not exist in rust-camel); produce spurious red on `testSplitterWithException` (Camel passes the failed exchange to the strategy; we return `Err` in the Vec); and lose G1 and G3 (asserts with no direct translation). **Automatic porting institutionalizes the error of confusing "Camel does X" with "X is correct".**

## Consequences

### Positive

- Divergence decisions survive context compaction once they land in tracked ADRs or `CONTEXT.md`.
- A new dev does not ask "why does rust-camel not have `parallelAggregate()`?". The answer lives in the EIP ADR or CONTEXT.
- Predictable design cost: 3 tests per divergent EIP, no more.
- Measurable and honest KPI: divergences-documented, not false coverage positives.

### Negative

- Design time per divergent EIP (reading, classification, documentation). Accepted as the cost of being a reinvention, not a port.
- Risk of over-applying the protocol to stateless EIPs where an audit was enough. The density trigger mitigates this, but it needs judgment.

## Scope (not retrospective)

The protocol applies to **new EIPs or major redesigns after this ADR**. It does not apply retrospectively to stable ADRs (for example ADR-0006 Script EIP, ADR-0019 error handling). For existing EIPs, consulting Camel is optional, and only when a concrete design question emerges.

## Anti-patterns

1. **"Camel does X, therefore X is correct."** Our ADRs prove otherwise. ADR-0024 calls the `CamelError::Stopped`/HTTP-204 model a **bug** that Camel-the-design induces. ADR-0032 calls the Camel trust model a rejected security risk. Camel is a starting point, not an oracle.
2. **Translate asserts literally.** `expectedBodiesReceived(...)` in an error-handling test assumes the Processor-chain model. The equivalent assert in rust-camel depends on the applied `ExceptionDisposition`. It is not a translation; it is a re-derivation.
3. **Treat ported green as coverage.** A ported test that passes through accidental coupling to semantics we do not share does not validate the correct invariant.
4. **Automate porting "to scale".** Scaling the error does not correct it. The discipline of deciding divergence per EIP is not parallelizable or automatable. It is the design work that makes rust-camel a reinvention and not a port.
5. **Document divergences in ephemeral docs.** Spike docs live under `docs/*` (gitignored by policy). Documented divergences must land in **tracked docs** (ADRs, `CONTEXT.md`, or as `notes` in bd referencing the ADR or CONTEXT), not in gitignored artifacts.

## Rejected alternatives

- **`cargo xtask port-camel-test`:** rejected on the Splitter spike evidence (see Context). It would produce invalid green, spurious red, and loss of non-translatable invariants.
- **Unified conformance TCK:** none exists for Apache Camel, and it would break the positioning decision of epic rc-ca8z.
- **Forbid reading Camel:** excessive. It loses the real value (20 years of production edge cases). The protocol captures that value without becoming an acceptance spec.
- **Ad-hoc policy without an ADR:** each dev decides alone. This reproduces the two Context risks (drift and memory loss).

## Measurement

The KPI `divergences-documented/EIP` applies as follows:

- For each EIP under the protocol, register findings in bd with `discovered-from: <EIP-issue>` and label them `divergence`, `gap-coverage`, or `pin-invariant`.
- Close the bd once the divergence lands in a tracked doc (new ADRs, amendments, or `CONTEXT.md` updates).
- Protocol health metric: ratio `divergences / (divergences + gaps)` per EIP. If it tends to 0, the EIP did not diverge and the protocol over-invested. Next time, apply a coverage audit.

## Evidence

- **Splitter spike:** branch `spike/rc-spt-camel-splitter-spike`, commit `8d31e74a`. Spike doc (gitignored): `docs/spikes/camel-splitter-conformance-spike.md`.
- **Oracle consultation (e_opus):** 2 passes, session `ses_08fc0fd19ffei7uuZcFoOrbnyq`. Verdict: the protocol is validated by divergences (D1/D2), not by bugs (G3 — coverage).
- **bd follow-ups:** `rc-0dgq` (D2 doc in `crates/camel-processor/CONTEXT.md`).

## Self-grill record

**Questions generated:**

1. [glossary] Does "Camel inspiration corpus" use terms that collide with CONTEXT-MAP entries?
2. [sharpen] How do we operationalize "divergence density" so it is not subjective?
3. [scenario] Does the protocol apply retrospectively to stable ADRs (for example Script EIP ADR-0006)?
4. [cross-ref] Is the spike evidence traceable or ephemeral?

**Answers (with citations):**

1. [glossary] No CONTEXT-MAP entry covers Camel as authority. "Documentation Authority & Refresh" (`CONTEXT-MAP.md:127-152`) lists source code, then ARCHITECT.md, then CONTEXT-MAP, then README. Camel is outside the list. The ADR is consistent: it codifies that Camel stays outside the authority order.
2. [sharpen] Operationalized with 5 markers: stateful, timing/completion, divergent control-flow (ADR-0019/0024/0025), trust-boundary (ADR-0032), backpressure (ADR-0044). At least 1 marker triggers the full protocol; 0 markers triggers a pure audit. Reflected in Decision section 1.
3. [scenario] Not retrospective. Clarified in the "Scope" section. ADR-0006 does not require re-applying the protocol. The protocol applies to new EIPs or major redesigns after this ADR.
4. [cross-ref] The spike doc is gitignored (`docs/*` policy, verified at `.gitignore:3`). Detected drift: divergences documented in spike docs would be lost. Fix: "Anti-patterns 5" forces divergences to land in tracked docs. rc-0dgq is already open for D2.

**Outcome:** refine (applied)
**Self-grill mode:** self-grill-proposals skill
