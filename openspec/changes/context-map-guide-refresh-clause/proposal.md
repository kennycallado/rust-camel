# Proposal: context-map-guide-refresh-clause

## Why

CONTEXT-MAP.md reorganized its authority model in commit `168a8673` (07 Aug 2026): the former code-derived prose snapshot was dropped from the authority order, `docs/adr/*` became tier 2, and the refresh rule was rewritten from "regenerate the snapshot" to "update Contexts/Relationships and touched CONTEXT.md files."

The upcoming mdBook user guide (change `guide-foundation-concepts-and-drift-contract`) depends on one rule that the rewritten refresh rule does not yet state: when an architecture-shaping merge changes a *user-visible* contract, the affected guide section and its `examples/` include must refresh in the same change. Without this clause, the guide has no event-driven refresh contract of its own and drifts silently.

The architect pass recommended landing this clause as a **separate prerequisite change**, not folded into the guide content change. CONTEXT-MAP.md is a tier-3 authority file that was just edited; touching the same section again inside a content change would layer an authority-model edit under a tutorial review and muddle the blessing. This change settles the clause first, on its own, so the guide change builds on a stable authority model.

## What Changes

- **In:** one sentence appended to bullet 1 of the "Refresh is event-driven" list in `CONTEXT-MAP.md`. The sentence names the user-visible-contract triggers precisely (new EIP builder method, new component scheme, DSL key rename, lifecycle-state rename, public contract enum gaining a variant) and the refresh target (the affected mdBook guide section plus its anchored `examples/` include).
- **Out:** any guide content, any linter, any change to the authority order itself, any edit beyond that one sentence. The guide content and the advisory linters belong to the follow-up change.

## Acceptance criteria

- The sentence is appended to bullet 1 of the refresh list, after "...touched, in the same change.", and reads as a natural continuation of that bullet (not a competing rule).
- `openspec validate context-map-guide-refresh-clause --type change --json` reports no delta-structure errors.
- The wording introduces no authority citation beyond tiers 1-3 (source / ADRs / CONTEXT-MAP).
- The change touches exactly one file: `CONTEXT-MAP.md`.

## Risk budget

Acceptable: a one-sentence edit to a tier-3 authority governance file, reviewed through a full spec-bless. The clause is advisory governance (it tells contributors what to refresh), enforced by the existing `AGENTS.md` reading hook; no tooling blocks on it yet.

Out of bounds: any change to the authority order, any new tooling, any rewording of the existing bullets, any edit to a second file.
