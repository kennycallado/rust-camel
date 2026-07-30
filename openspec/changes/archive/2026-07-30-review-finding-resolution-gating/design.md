# Design: review-finding-resolution-gating

## Approach

Add a single requirement to the `conductor-workflow` spec canon as a delta spec under `specs/conductor-workflow/spec.md`. On archive, OpenSpec syncs the delta into `openspec/specs/conductor-workflow/spec.md`. The requirement captures the invariant only — what blocks advancement. The per-task and holistic verdict branching remains in the prompt as the implementation of that floor. The prompt was corrected in the same session to conform; this change does not edit the prompt further.

The design decision is the spec/prompt seam: elevate the coarse, stable invariant into the bless-gated canon; leave the granular, churning branching in free-form prompt text. This matches the project's intentional-minimalism stance — specs hold stable contracts, not every operational branch.

## Affected crates

- None. Docs/spec only. The change touches `openspec/specs/conductor-workflow/spec.md` (via delta, synced on archive) and the change's own artifacts.

## Architecture boundaries

This change stays inside the conductor-workflow domain (the agent-workflow layer). It does not cross the Runtime, DSL, Components, Services, Languages, or Functions boundaries. It complements the existing "Phase-aware Implementation Ordering" requirement by defining what a review verdict obligates — completing a gate the spec already names structurally.

No ADR applies. The project ADRs record runtime and architecture decisions; this is a workflow-governance change. Provenance: the `conductor-workflow` spec canon itself, established by the archived change `conductor-delivery-phases`.

## Alternatives considered

- Keep the rule in the prompt only. Rejected: the observed drift is an existence proof that prompt-only was insufficient — the rule already eroded once across three locations.
- Elevate the full verdict-branching prose into the spec. Rejected: bless-gating operational detail makes the spec brittle and raises the cost of legitimate refinement. The invariant is coarse and stable; the branching is not.
