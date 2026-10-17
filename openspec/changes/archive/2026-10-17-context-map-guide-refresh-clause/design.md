# Design: context-map-guide-refresh-clause

## Approach

Append one sentence to bullet 1 of the "Refresh is event-driven" list in `CONTEXT-MAP.md` (the "Documentation Authority & Refresh" section). The bullet currently ends:

> After an **architecture-shaping merge** (new EIP, lifecycle change, contract change): update the Contexts and Relationships sections of this map, plus the `CONTEXT.md` of every crate the merge touched, in the same change.

The appended sentence extends the SAME bullet (same event, same "in the same change" contract), narrowing to user-visible contract changes and naming the guide refresh target:

> If the merge changes a *user-visible* contract (a new EIP builder method, a new component scheme, a DSL key rename, a lifecycle-state rename, a public contract enum gaining a variant), also refresh the affected mdBook guide section and its anchored `examples/` include in the same change.

This is an **append**, not a new bullet. It fits the existing event-driven shape and adds no scheduled maintenance. It introduces no authority citation beyond the existing tiers.

### Wording rationale

- **"user-visible contract"** scopes the trigger so internal refactors (ADR-0045 slice reshuffles, crate-internal renames) do NOT force guide churn — matching the `architecture/index.md` stub's promise that "the narrative stays stable as internal crate boundaries evolve."
- **"public contract enum gaining a variant"** is named explicitly because ADR-0049 makes contract enums `#[non_exhaustive]`; new variants are the expected additive change a user-facing guide must track.
- **"its anchored `examples/` include"** ties the clause to the include-first drift machinery (`mdbook test` + `documentation_examples.rs`) that the guide change establishes.

## Affected crates

None. This is a governance-documentation edit. No Rust crate, no `Cargo.toml`, no code.

- `CONTEXT-MAP.md` (repo root): one sentence appended to refresh bullet 1.

## Architecture boundaries

Respects all boundaries by construction — the change touches no Runtime, DSL, Components, Services, Languages, or Functions code. It edits a tier-3 authority prose file that governs documentation refresh behavior.

Relevant ADRs (cited, not modified):
- **ADR-0049** (`#[non_exhaustive]` policy) — motivates naming enum-variant addition as a tracked user-visible change.
- **ADR-0046** (Apache Camel: inspiration, not conformance) — the guide's divergence framing is downstream of this clause.
- **ADR-0001** (Tower data plane / control plane split) — the guide section affected by such merges is the one explaining this split; the clause does not change the ADR.

## Alternatives considered

1. **Fold the clause into the guide content change** (`guide-foundation-concepts-and-drift-contract`). Rejected: CONTEXT-MAP.md was just reorganized in `168a8673`; editing the same freshly-landed section inside a content change layers an authority-model edit under a tutorial review and muddies the blessing.
2. **Add a new bullet** instead of appending to bullet 1. Rejected: the trigger is the same event ("architecture-shaping merge") and the same contract ("in the same change"); a new bullet would read as a competing rule. Appending keeps it one coherent rule.
3. **Defer the clause until the guide exists.** Rejected: the guide change needs the clause as its authority basis (the guide is a consumer of the refresh rule); building guide content first then back-filling the rule inverts the dependency.
4. **Make the clause tooling-enforced now.** Rejected: tooling enforcement is out of scope for this prerequisite. The advisory linters planned for the guide change (`lint-glossary`, `lint-slop`, `lint-adr-cite`) check glossary vocabulary, slop markers, and ADR citation validity — none of them establish user-visible-change-to-guide synchronization. This clause stays advisory, enforced by the existing `AGENTS.md` reading hook and reviewer judgment until a dedicated guide-drift check exists.

## Phases

Single-phase. One sentence, one file, one task. No `## Phase N` headings.
