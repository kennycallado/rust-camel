# Proposal: conductor-stage-rename

## Why

The keyword "PHASE" names two unrelated concepts in the OpenSpec workflow:

1. **Conductor workflow stages** — `PHASE 0: ISOLATE` .. `PHASE 4: CLOSE` in
   the conductor-light agent definition.
2. **Delivery phases** — `## Phase N` task groups in `tasks.md` and the
   "Phases" section of `design.md`.

The collision produces ambiguous text. The `conductor-workflow` spec mixes
both meanings in one sentence ("loaded in PHASE 2 before tasks are authored.
Phase validation failure…"), and delivery "Phase 3" work executes during
conductor "PHASE 3", which is unreadable out of context.

## What Changes

- The five conductor workflow stages are renamed **PHASE N → STAGE N**
  (STAGE 0: ISOLATE, STAGE 1: SPEC, STAGE 2: PLAN, STAGE 3: IMPLEMENTATION,
  STAGE 4: CLOSE) in the conductor-light agent definition, the skills
  policy, the merge prompt, and the task-authoring skill.
- "Phase" becomes the exclusive term for OpenSpec delivery phases
  (`design.md` "Phases" section, `tasks.md` `## Phase N` headings,
  phase-groups, inter-phase reviews). No delivery-phase vocabulary changes.
- The `conductor-workflow` spec gains a **Stage Terminology** requirement
  and its stage references switch to STAGE N.

## Impact

Docs/config only. Affected files:

- `.opencode/agents/conductor-light.md`
- `.opencode/instructions/openspec-skills.md`
- `.opencode/prompts/merge-to-main.md`
- `.opencode/skills/openspec-task-authoring/SKILL.md`
- `openspec/schemas/expert-gated/templates/design.md` (stage references in
  scaffolding comments)
- `openspec/schemas/expert-gated/templates/tasks.md` (same)
- `openspec/specs/conductor-workflow/spec.md` (via delta, synced at archive)
- `openspec/specs/skills-policy/spec.md` (via delta, synced at archive: one
  stage reference in the ste-writing requirement)

No code, no behavior change. Archived changes keep their historical
vocabulary and are not edited. The `openspec` CLI (1.7.0) does not parse
stage or phase headings, so nothing breaks mechanically.
