# Design: correlationfix

## Approach

Unify on an explicit correlation source (investigation Option A). The
post-fix authoring contract for the YAML `aggregate` verb:

1. `header` is OPTIONAL at parse (`#[serde(default)]` → `""`);
   `correlation_key` stays optional.
2. Validation requires ONE OF the two sources, non-empty: header-only,
   expression-only, or both. Empty `correlation_key` strings are rejected.
3. When both are present, `correlation_key` (expression correlation)
   OVERRIDES `header`. This mirrors `compile_canonical_aggregate`
   (compile.rs:738-744) exactly, and matches builder canonicalization,
   which emits `header = expr` + `correlation_key = Some(expr)` for
   `Expression` configs (camel-builder lib.rs:1197-1219) — both-present is
   representable in every valid canonical round-trip, so rejecting it would
   create a NEW split-brain instead of fixing one.
4. One shared mapping helper (`fn aggregate_correlation(header: &str,
   correlation_key: Option<&str>) -> CorrelationStrategy`) in compile.rs is
   used by both `compile_aggregate_step` and `compile_canonical_aggregate`,
   so the two paths cannot drift again. Lowering runs after
   `validate_route`, which gates both entry points (compile.rs:249, :327).

API addition: `AggregatorConfigBuilder::correlate_by_expr(expr, language)`
on camel-api — override-setter semantics mirroring `correlate_by` (:328-333):
sets `correlation = Expression`; `header_name` is left untouched (same
precedent as the canonical path, which bootstraps with `correlate_by(header)`
then overrides `agg_config.correlation`; runtime reads `.correlation`, not
`.header_name`). The DSL lowers an empty header as a no-op bootstrap that
the expression override immediately replaces.

`compile_aggregate_step` additionally: wires `correlation_key` through the
helper (deleting the stale NOTE at :1839-1841), keeps the
`completion_predicate` builder-path rejection, and updates that error
message (it currently claims the builder path does not lower correlation
keys — no longer true after this change).

Canonical shape is UNCHANGED: `CanonicalAggregateSpec.header: String` stays
required in serialized form; expression-only authoring serializes with
`header: ""`, which recompiles to `Expression` under override-wins.
`CanonicalRouteSpec::validate_contract` (camel-api/src/runtime.rs:466-472)
currently rejects ANY empty aggregate header — it must adopt the same
one-of rule: empty `header` is legal exactly when `correlation_key` is
present and non-empty, and an empty `correlation_key` is rejected even
beside a non-empty header. Without this, expression-only canonical routes
and hot reload would break at contract validation.

Schema: `AggregateData.header` gains a serde default → schemars output
shifts → regenerate via `cargo xtask schema` (tool run, not edit).

## Affected crates

- `camel-api` (crates/camel-api/src/aggregator.rs): additive builder setter
  `correlate_by_expr` + unit test; runtime.rs `validate_contract`
  aggregate rule becomes one-of (empty header legal only with non-empty
  `correlation_key`; empty `correlation_key` rejected). No signature breaks.
- `camel-dsl`: route_ast.rs (`header` serde default), compile.rs (shared
  helper, builder-path wiring, validation rule, predicate error message),
  yaml.rs unchanged (field already flows through), tests + schema regen.
- `camel-builder`: round-trip parity tests only (canonicalize_aggregate
  behavior unchanged).
- docs: step-verbs.md aggregate table (header optional + precedence),
  aggregator.md expression-correlation paragraph.

## Architecture boundaries

DSL → API compile-time dependency only; the runtime (camel-processor)
already evaluates header, expression, and function correlation strategies —
no data-plane change. Operator config stays on the trusted side of the
ADR-0032 exchange-data trust boundary; the expression is evaluated by the
pre-existing language registry sink, unchanged. Canonical serialization
stays byte-compatible for previously-valid canonical routes.

Single-phase change (S/M, per investigation); no `## Phases` section.

Excluded scope unchanged (no runtime processor work, no canonical
serialization shape change, no camel-cli, no scripts/xtask edits).
`validate_contract` is the only camel-api behavior change beyond the
additive setter.

## Alternatives considered

- Option B (forbid expression correlation in the normal DSL): rejected —
  the field is an exposed canonical and DSL contract, runtime and reverse
  mapping already support it.
- Option C (fail loudly, keep split paths): rejected as final design —
  containment only, leaves normal/canonical semantics divergent.
- Reject-when-both-present instead of override-wins: rejected — breaks
  every `Expression` builder→canonical round-trip (canonicalize emits both
  fields) and contradicts pinned canonical-path behavior.

## Investigation findings (durable landing, rc-q8ng)

Investigation 91 (fleet inbox `q8ng-findings.md`, 2026-09; superseded copy —
this section is the tracked landing). Executive finding:
`aggregate.correlation_key` is accepted by the declarative model and
preserved by canonical lowering, but silently discarded by builder lowering
(compile.rs:1828-1877 uses `def.header` only; note at :1839 said
intentional) while validation requires the field (:2108-2114). A
valid-looking YAML aggregate either fails validation when the key is
omitted, or passes and executes with wrong correlation semantics when it is
present.

Reproduction path: YAML parse copies the value into
`AggregateStepDef.correlation_key` (yaml.rs:1128-1147; model.rs:394-400) →
normal compilation never reads it → runtime receives
`CorrelationStrategy::HeaderName` instead of the authored expression.
Canonical lowering preserves it (compile.rs:1680-1699) and
`compile_canonical_aggregate` sets `Expression { expr, language: "simple" }`
(:738-744). `camel-api/src/aggregator.rs:21-30` defines
`CorrelationStrategy::Expression`; the builder had only header setters
(:205-226, :327-333) — the missing builder operation was the enabling gap.

Blast radius: camel-dsl compile/validate (primary); model/route_ast/yaml
(contract source); camel-processor (runtime consumer — no gap found);
camel-builder lib.rs:1194-1244 (reverse canonical mapping); tests at
compile.rs ~4351 (`test_aggregate_requires_correlation_key`), ~3419-3549,
~4145 (canonical), ~2556-2641 (YAML canonical), camel-builder ~3638,
~3911-4019 (canonicalization); docs step-verbs.md:281-288 (documented
header required + correlation_key optional — conflicted with validation),
aggregator.md:29 (header-only builder semantics).

Root cause class: implementation drift caused by incomplete contract
design — three-way disagreement (validator mandates expression, builder
ignores expression, canonical honors expression). Oracle (e_opus,
rc-90ez Q6 secondary): independently classified the same three-way
disagreement, recommended Option A with S/M size and no cross-crate
signature break beyond the additive builder operation.

Minimum regression matrix (all covered by scenarios in the delta spec):
expression via normal lowering → `Expression`; expression via canonical
lowering → identical; header-only follows the chosen contract (accepted,
header-based); missing both fails with a stable config error;
builder→canonical round trip preserves expression correlation;
both-present follows documented precedence (expression overrides).
