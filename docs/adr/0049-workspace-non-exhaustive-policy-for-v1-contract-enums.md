# ADR-0049: Workspace `#[non_exhaustive]` Policy for v1.0 Contract Enums

**Date:** 2026-08-05
**Status:** Accepted
**References:** ADR-0002, ADR-0011, ADR-0016, ADR-0024, ADR-0025, ADR-0045
**Origin:** camel-api quality audit (`docs/audits/modules/camel-api-quality-2026-08-05.md`, finding I2 / DP-6)

## Decision

**Public contract enums in the workspace's contract crates are `#[non_exhaustive]` by
default, applied before the 1.0 API freeze.**

### Scope

The policy binds the three **public contract crates** already named in
`CONTEXT-MAP.md` "CONTEXT.md coverage policy":

- `camel-api`
- `crates/components/camel-component-api`
- `crates/languages/camel-language-api`

Within these crates, the policy applies to a **contract enum**: any `pub enum` that an
out-of-crate implementer or caller matches against as part of the stable contract. This
is exactly the surface that turns an additive change (a new variant) into a breaking
change (an external `match` that no longer compiles).

Concretely, the initial application set (the enums that must gain `#[non_exhaustive]` — the
mechanical fix is finding I2, executed via the code stream, not this ADR):

| Crate | Enum | Site | Contract role |
|---|---|---|---|
| camel-api | `RuntimeCommand` | `runtime.rs:444` | CQRS command (ADR-0002) |
| camel-api | `RuntimeQuery` | `runtime.rs:565` | CQRS query (ADR-0002) |
| camel-api | `RuntimeCommandResult` | `runtime.rs:542` | CQRS result (ADR-0002) |
| camel-api | `RuntimeQueryResult` | `runtime.rs:579` | CQRS result (ADR-0002) |
| camel-api | `RuntimeEvent` | `runtime.rs:587` | lifecycle event (ADR-0002) |
| camel-api | `CanonicalStepSpec` | `runtime.rs:113` | versioned route contract (ADR-0011/0016) |
| camel-api | `CanonicalSplitExpressionSpec` | `runtime.rs:180` | versioned route contract |
| camel-api | `CanonicalSplitAggregationSpec` | `runtime.rs:199` | versioned route contract |
| camel-api | `CanonicalAggregateStrategySpec` | `runtime.rs:217` | versioned route contract |
| camel-api | `CanonicalConcurrencySpec` | `runtime.rs:281` | versioned route contract |
| camel-component-api | `ConsumerStartupMode` | `consumer.rs:38` | component contract |
| camel-component-api | `ConcurrencyModel` | `consumer.rs:374` | component contract |
| camel-language-api | `LanguageError` | `error.rs:4` | error contract |

`CamelError` and `ConfigValidationError` (`camel-api/src/error.rs`) already carry
`#[non_exhaustive]` and are guarded by exhaustive `variant_name()` tests — they are the
reference pattern, not new work.

### Rule

1. New contract enums added to a contract crate are `#[non_exhaustive]` from birth.
2. Existing contract enums gain `#[non_exhaustive]` before the 1.0 freeze (finding I2).
3. A contract enum deliberately kept exhaustive (a closed set that is itself the
   contract — see Exceptions) MUST carry a one-line rustdoc note stating why, so the
   omission is a documented decision, not an oversight.

### Exceptions (enums that stay exhaustive by design)

`#[non_exhaustive]` is a default, not a mandate. An enum is exempt when its **closed set
is the contract** and an in-crate exhaustive `match` is a load-bearing safety property:

- **`PipelineOutcome`** (`camel-api/src/pipeline_outcome.rs`) — the three-variant set
  `Completed | Stopped | Failed` is the deliberate outcome algebra of ADR-0024. The
  translation site `into_tower_result()` relies on exhaustive matching; a silent
  `_ =>` arm there would be a correctness hole, not forward-compat. If a fourth outcome
  (e.g. `Suspended`) is ever needed it is a deliberate, reviewed breaking change, not an
  additive slip. **Stays exhaustive.**
- **`ExchangePattern`** (`camel-api/src/exchange.rs`) — the `InOnly | InOut` MEP set is a
  fixed, spec-level dichotomy. **Stays exhaustive.**
- **Enums whose exhaustive match is a compile-time safety guard** (the `variant_name()`
  pattern): these already work correctly *with* `#[non_exhaustive]` because the guard test
  is in-crate, where `#[non_exhaustive]` does not force a wildcard arm. They keep
  `#[non_exhaustive]` — no exception needed.

The `#[non_exhaustive]` cost is the forced `_ =>` arm in **out-of-crate** matches; it has
**no effect in-crate**, so exhaustive-guard tests and internal executors keep compiling
without wildcards.

## Context

### Problem

The camel-api audit (2026-08-05) found that 148 of 149 public types carried no
`#[non_exhaustive]`, including the entire CQRS control-plane surface (ADR-0002) and the
`Canonical*` versioned route contract (ADR-0011/0016). An external implementer of
`RuntimeCommandBus::execute(cmd: RuntimeCommand)` must match every variant; adding a new
lifecycle command post-1.0 (`PauseRoute`, `DrainRoute`) would break every such match — a
major version bump for what should be an additive change.

The same gap exists structurally in the other two contract crates
(`camel-component-api`, `camel-language-api`), which expose their own contract enums to
external component and language authors. The question is therefore **workspace-wide**, not
crate-local.

### Why a workspace ADR and not a per-crate triage note

`CONTEXT-MAP.md` already treats "public contract crates" as one category with shared
obligations (CONTEXT.md coverage policy). API-stability posture is exactly such a shared
obligation: deciding it once, uniformly, prevents three crates from each choosing a
different answer as their audits land. Per the L6 rule "cross-crate semver/API decisions
default to a workspace ADR," this belongs in one authoritative document.

### Why not amend an existing ADR

No existing ADR governs enum extensibility. ADR-0002 (CQRS) and ADR-0011/0016
(`CanonicalRouteSpec`) define *what* the contracts are, not *how they evolve*. ADR-0045 is
the camel-core architecture charter — a crate-scoped module-discipline document, not an
API-stability policy for the contract crates. This is a genuinely new decision, so it is a
new ADR.

### Relationship to the "no deprecation" policy

ADR-0024 records the project directive "no deprecamos xq no tenemos usuarios" — pre-release,
breaking changes are made cleanly without deprecation shims. `#[non_exhaustive]` is not in
tension with that: it is a *one-time, pre-1.0* investment so that *post-1.0* additive
growth stays additive. It is applied now, while breaking changes are still free, precisely
because adding it after the freeze is itself the kind of change we want to stop needing.

## Considered options

| Option | Description | Ruling |
|---|---|---|
| A | Resolve as triage note attached to finding I2, camel-api only | Rejected — leaves component-api/language-api to decide ad-hoc; reintroduces the cross-crate drift a workspace ADR exists to prevent |
| B | Blanket `#[non_exhaustive]` on *every* public enum in every crate | Rejected — over-broad; forces wildcard arms on internal/runtime enums with no external implementers, hurting maintainability for no semver benefit |
| **C** | **`#[non_exhaustive]` default on contract enums in the three contract crates, with documented exceptions for deliberate closed sets** | **CHOSEN** — matches the existing "contract crate" category, targets the real external-match surface, keeps closed-set safety enums exhaustive |
| D | Defer until more T1 contract crates are audited | Rejected — the three contract crates are already identified; the freeze is the deadline, and adding the attribute post-freeze is the exact cost we are avoiding |

## Consequences

- **Mechanical fix (I2)** applies `#[non_exhaustive]` to the enums in the scope table via
  the code stream (post-audit triage / conductor-light), not this ADR. This ADR is the
  policy; I2 is the execution.
- **In-crate exhaustive matches are unaffected** — executors, `variant_name()` guards, and
  test impls continue to match without wildcards, because `#[non_exhaustive]` only forces a
  `_ =>` arm outside the defining crate.
- **External implementers gain a forward-compat arm requirement** — an intentional,
  one-time ergonomic cost that converts future additive variant growth from breaking to
  non-breaking.
- **Future contract crates** (e.g. as new `*-api` crates appear) declare their contract
  enums under this policy by default; a deliberate exhaustive enum carries the required
  one-line rustdoc justification.
- **Deliberate closed-set enums** (`PipelineOutcome`, `ExchangePattern`) are documented
  exceptions; changing their variant set remains a reviewed breaking change, which is the
  intended semantics.
- **`CanonicalConcurrencySpec` codegen note** (audit L1): the missing
  `#[ts(rename_all = "snake_case")]` is a separate, non-breaking codegen consistency fix
  and is out of scope for this ADR — tracked with I2's mechanical batch.

### Self-grill record

**Questions generated:**
1. [glossary] Does "contract enum" / "non_exhaustive policy" collide with an existing
   CONTEXT-MAP Key Term or ADR-0045's "module-discipline ceiling"?
2. [sharpen] Is this one decision or two — "MUST contract enums be non_exhaustive" vs
   "WHERE is the policy recorded"?
3. [scenario] If `#[non_exhaustive]` is added to `PipelineOutcome`, does the
   `into_tower_result()` exhaustive match break or silently degrade?
4. [cross-ref] Do `camel-component-api` and `camel-language-api` actually exist with public
   contract enums, or is the cross-crate claim speculative?

**Answers (with citations):**
1. [glossary] No collision. `CONTEXT-MAP.md:97` "module-discipline ceiling" is ADR-0045's
   camel-core crate-split charter — an internal-layering term, not an API-stability term.
   The contract-crate category itself is already defined (`CONTEXT-MAP.md:161`), so this
   ADR names an existing category rather than inventing one. No existing Key Term covers
   enum extensibility.
2. [sharpen] Two questions, both resolved here: (a) the semver decision — contract enums
   default to `#[non_exhaustive]` — and (b) the recording site — a workspace ADR, because
   the surface spans three crates (L6 rule #4). The new-ADR criteria hold: hard-to-reverse
   (removing the attribute post-1.0 is breaking), surprising (a v1.0 crate mostly open is
   non-obvious), real trade-off (`_ =>` ergonomics vs forward-compat).
3. [scenario] It would degrade dangerously if applied blindly — an out-of-crate `_ =>` on
   `PipelineOutcome` at the Tower translation boundary could silently mishandle a future
   variant, which is a correctness hole, not forward-compat. `into_tower_result()` lives
   *in-crate* (`camel-api`), where `#[non_exhaustive]` does not force a wildcard, so the
   compiler still checks exhaustiveness there. Nonetheless, the outcome algebra is a
   deliberate closed set (ADR-0024), so `PipelineOutcome` is listed as an explicit
   exception — the correct semantics is "changing it is a reviewed breaking change."
   (`camel-api/src/pipeline_outcome.rs`, ADR-0024 §Decision)
4. [cross-ref] Confirmed real, not speculative. `crates/components/camel-component-api`
   exposes `ConsumerStartupMode` (`consumer.rs:38`) and `ConcurrencyModel`
   (`consumer.rs:374`); `crates/languages/camel-language-api` exposes `LanguageError`
   (`error.rs:4`). The cross-crate blast radius asserted in audit finding I2 / DP-6 is
   verified by mechanical enum enumeration.

**Outcome:** approve as new workspace ADR (0049) — scope narrowed to contract crates
(rejecting the blanket Option B), deliberate closed-set enums carved out as documented
exceptions, execution delegated to finding I2's code stream.
**Self-grill mode:** self-grill-proposals skill
