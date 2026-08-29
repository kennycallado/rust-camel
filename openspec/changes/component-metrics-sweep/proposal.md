# Proposal: component-metrics-sweep

## What

Complete ADR-0012 error-path metric wiring across all components. The
2026-08-26 audit (rc-q25t) is STALE — surrealdb, ws, master, sql, and
direct gained wiring after it was written. This change begins with a
FRESH AUDIT artifact enumerating the authoritative per-component gap
list, then wires every gap (or drops dead fields where no
taxonomy-eligible site exists).

## Why

- bd rc-bfnw + rc-q25t (P2, epic rc-hrm1 residue). ADR-0012 Phase B
  (00952a95) wired only the 17-site log-policy ledger; components
  outside it kept `RuntimeObservability`/`MetricsCollector` fields with
  stale "Phase B will use this" comments and zero production calls.
  seda, wasm, opensearch, template still have ZERO calls; cxf,
  validator, xslt have exactly one site each; the remaining components'
  site-completeness is unverified against their failure-path surface.
- Dashboards are blind to these components' failures regardless of
  collector wiring (collector bugs were fixed by rc-cizb/rc-685y).

## What Changes

- `audit.md` in the change dir (pre-plan gate, design D1): fresh gap
  table (component → eligible failure sites per ADR-0012 categories
  (b′)/(e) → wired sites → gaps / drop-dead-field + stale-comment
  enumeration), produced by an auditable rg recipe.
- Wiring: each gap gets `increment_errors(route_id,
  "<cat>:<component>:<site>")` with label matching
  `^(b-prime|e):[a-z][a-z0-9-]*:[a-z][a-z0-9-]+$` (ADR-0012 §labels;
  (b′)/(e) only — (g) takes health-check signals, out of scope).
- Dead fields with zero eligible sites are REMOVED (field + plumbing +
  stale deferral comments; public signatures preserved).
- Per-component tests asserting emission (pattern: existing component
  test suites with a recording collector).

## Impact

- Affected: `crates/components/*` (seda, wasm, opensearch, template
  certain; others per audit), their tests. No trait changes
  (MetricsCollector already supports increment_errors).
- specs: `metrics-collection-wiring` gains one requirement (error-path
  completeness).
- Success-path telemetry (rc-6s6h) and wasm RegistryComponentContext
  NoOp (rc-66he) are OUT of scope — separate changes under the epic.
