# Design: conductor-stage-rename

## Context

"PHASE" is overloaded: conductor workflow stages (PHASE 0..4) vs OpenSpec
delivery phases (`## Phase N`). Renaming one side removes the ambiguity.
The conductor stages are the cheaper side to rename: they live in 4 living
config/doc files plus one spec capability, while delivery-phase vocabulary
is baked into 3+ living specs and ~15 archived changes (which must never be
rewritten).

## Decision

Rename conductor stages to **STAGE 0..4**.

"STAGE" denotes a step in a temporal sequence (rocket stages, CI pipeline
stages), which matches the always-ordered conductor flow.

Rejected alternatives:

- **TIER** — denotes hierarchy/ranking, not sequence; already used in this
  repo for worker tiers (`w_fast`/`w_balanced`/`w_heavy`), so it would
  create a second collision.
- **STEP** — collides with the `Steps` field of every task block.
- **Renaming delivery phases instead** — touches 3+ living specs
  (`conductor-workflow`, `security-kernel`, `endpoint-metadata-derivation`)
  and permanently diverges from ~15 archived changes that use `## Phase N`.

Naming convention after this change:

- `STAGE N` (uppercase) — conductor workflow stages only.
- `Phase` (title/lowercase) — delivery phases only (`## Phase N`,
  phase-group, inter-phase review, Phase-boundary coherence). The single
  deliberate uppercase exception is `DELIVERY-PHASE-AWARE`, which names
  delivery-phase awareness explicitly.

## Phase decomposition

Single-phase change (mechanical rename + one spec delta). No "Phases"
section in this design; `tasks.md` has no `## Phase N` headings.
