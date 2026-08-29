# Design: component-metrics-sweep

## Context

ADR-0012 requires `increment_errors` at every (b′)/(e) failure site
(signal replacement for downgraded `error!`); (g) sites take
`HealthCheckRegistry::force_unhealthy_for_route` instead and are OUT of
scope. Phase A plumbed
`RuntimeObservability` into all Endpoint impls; Phase B wired only the
log-policy ledger. The 2026-08-26 audit is stale: current prod-call
counts (conductor-verified 2026-08-28): seda/wasm/opensearch/template 0;
cxf/validator/xslt 1 each; surrealdb 1, ws 2, master 2, sql 5, direct 3
(completeness unverified).

## Goals / Non-Goals

- Goals: every taxonomy-eligible failure site — categories (b′) and
  (e) ONLY — emits `increment_errors` with a valid label; dead
  observability fields removed where no eligible site exists; fresh
  audit artifact is the single source of truth for what gets wired.
- Non-Goals: success-path vocabulary (rc-6s6h); wasm
  RegistryComponentContext NoOpMetrics (rc-66he); (g)-category wiring
  (health-check signal, existing state unchanged); MetricsCollector
  trait changes.

## Decisions

### D1 — Audit as a PRE-PLAN gate, audit-anchored tasks

Sequence: spec-bless (this document) → execute the fixed audit recipe →
author the final audit-anchored tasks.md → plan-bless. The audit
(conductor-run between the blessings, committed as `audit.md` in the
change dir) enumerates per component the (b′) and (e) surface. (b′)
discovery is SEMANTIC, not single-API: every consumer-side failure that
is locally terminal — `send_and_wait` Err not forwarded to a caller who
can absorb it, `ctx.send` errors consumed locally (e.g. seda
lib.rs:754-756), post-dispatch side-effect failures — plus (e) every
accept/retry loop failure. (g) creation failures are NOT in the metrics
audit (health-check category, out of scope). Recipe: rg sweep for
candidate APIs (`send_and_wait`, `.send(`, accept/retry loops) ∪ sites
already annotated `// log-policy: outside-contract`, then MANUAL
semantic review per hit: is the Err forwarded to a handler who can own
it (→ not b′) or locally terminal (→ b′)? Subtract wired
`increment_errors` sites; output gaps + drop-candidates as file:line +
category rows with the semantic verdict recorded. Wiring tasks reference audit rows by
component — the audit IS the task-level site enumeration (no
placeholders: each row is concrete). Plan-bless sees the FINAL tasks.md
plus the committed audit.md; it never references rows that do not exist.

### D2 — Wire-or-drop criterion, public-API-preserving

A component with at least one eligible site: wire ALL gaps, keep the
field. A component whose stored field has zero eligible sites AND no live
observability/delegation use after analysis (OQ-5 ratification: exec
and llm fields carry live success-path metrics; master's runtime is
live delegation — RETAINED, none is a drop): REMOVE the stored field,
its clone/manual-Clone plumbing, and its stale deferral comments — but
PRESERVE public signatures (public
constructor/trait parameters stay, binding to `_runtime` style unused
params) unless an explicit API-break review approves removal. Dead
weight lies (stale "Phase B / Phase-5 / deferred / read later" comments
are the debt being repaid); each drop row in the audit enumerates the
associated stale comments so their removal is verifiable per component,
not by one grep phrase.

### D3 — Test pattern: every wired row asserts

Every wired audit row maps to an executable test asserting the EXACT
label string via a recording collector (in-tree pattern:
camel-component-api `test_support` / existing component suites);
table-driven per component where the component already parameterizes
failure modes. A row may be waived only by recording in audit.md: the
technical blocker, the seam attempted, the alternate verification (e.g.
compile-level label-regex assertion), and an explicit plan-bless
approval of the waiver.

### D4 — Phasing (audit already committed pre-plan)

- Phase 1: seda + the three certain-dead (wasm*, opensearch, template) —
  *wasm here means its producer/consumer wiring sites only;
  RegistryComponentContext NoOp stays (rc-66he).
- Phase 2: the partial five (cxf, validator, xslt, sql, direct) +
  completeness verification of surrealdb/ws/master + any audit-added
  components (mcp/log/mock/timer/cron/xj/file/controlbus if the audit
  finds eligible unwired sites).
Both phases CONSUME the committed audit.md; no task produces it.

AS-BUILT RATIFICATION (post-audit): the audit reshaped the groups —
validator/xslt/sql/direct are COMPLETE (no task), and timer/file/keycloak
gained wire rows. Phases as built: Phase 1 = wire G1-G5 (seda consumer,
timer, ws, file, keycloak label); Phase 2 = drops D1-D3 and D5-D8 (D4 master superseded to RETAINED at plan-bless: live delegation plumbing). Ordering: T1
wires the seda consumer field BEFORE Task 12 drops the seda producer field —
required, since the drop's Kept-verdict depends on the consumer being
wired (G1).

## Risks / Trade-offs

- Audit subjectivity at (b′) boundaries (absorbed-vs-not): mitigated by
  cross-checking existing `// log-policy:` annotations — sites already
  annotated outside-contract/handler-owned are pre-categorized.
- Public-API preservation on drops leaves unused `_runtime` params
  (temporary mild ugliness) — removal is a separate API-break decision.
- Wide diff across many crates: each component is an independent task
  with its own test; failures stay local.

## Migration Plan

None — metric additions are additive; dead-field removals are internal.

## Open Questions

None.
