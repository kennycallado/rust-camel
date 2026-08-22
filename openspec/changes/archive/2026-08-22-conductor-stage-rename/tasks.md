# Tasks

## Task 1: Rename conductor stages in config, doc, and template files

**Files**:
- `.opencode/agents/conductor-light.md` (modified)
- `.opencode/skills/openspec-task-authoring/SKILL.md` (modified)
- `.opencode/instructions/openspec-skills.md` (modified)
- `.opencode/prompts/merge-to-main.md` (modified)
- `openspec/schemas/expert-gated/templates/design.md` (modified)
- `openspec/schemas/expert-gated/templates/tasks.md` (modified)

**Steps**:
1. Replace every numbered stage reference `PHASE N` (N=0..4) with `STAGE N`
   in the six files, including the `### PHASE N:` section headings of
   `conductor-light.md` and the stage references in the expert-gated
   scaffolding templates (they inject stage vocabulary into every future
   change's design.md/tasks.md).
2. In `conductor-light.md` only: `mid-PHASE-3` → `mid-STAGE-3`,
   `TASK LOOP — PHASE-AWARE` → `TASK LOOP — DELIVERY-PHASE-AWARE`,
   `5 phases, 2 blessing gates` → `5 stages, 2 blessing gates`,
   `right phase` → `right stage` (×2), `appropriate phase` →
   `appropriate stage`, `at this phase` → `at this stage`.
3. Do NOT touch delivery-phase vocabulary: `## Phase N` headings,
   "Phases" section, `phase-group`, `inter-phase review`, `pre-phase`,
   `Phase-boundary coherence`, `phase decomposition`, `single-phase`,
   `multi-phase`.

**Tests**:
- name: `no_numbered_PHASE_left`
  setup: the six files renamed
  action: `grep -rnE 'PHASE [0-4]|mid-PHASE' .opencode/ openspec/schemas/`
  assert: zero matches (the only uppercase `PHASE` substring allowed is
  inside `DELIVERY-PHASE-AWARE`, which this pattern excludes)
  command: `grep -rnE 'PHASE [0-4]|mid-PHASE' .opencode/ openspec/schemas/ ; test $? -eq 1`
  expected: exits 0 after the rename; exits 1 before it.
- name: `five_stage_headings`
  setup: `conductor-light.md` renamed
  action: `grep -c '^### STAGE' .opencode/agents/conductor-light.md`
  assert: prints 5 (STAGE 0..4 section headings), and
  `grep -c '^### PHASE'` prints 0
  command: `test "$(grep -c '^### STAGE' .opencode/agents/conductor-light.md)" = 5`
  expected: exits 0 after the rename.

**Acceptance**:
- `grep -rnE 'PHASE [0-4]|mid-PHASE' .opencode/ openspec/schemas/` returns
  nothing.
- Delivery-phase terms still present: at least 3 mentions of `## Phase N`
  (as inline backticked references, not headings) survive in
  `conductor-light.md`.

- [x] 1.1

## Task 2: Author the conductor-workflow and skills-policy delta specs

**Files**:
- `openspec/changes/conductor-stage-rename/specs/conductor-workflow/spec.md` (new)
- `openspec/changes/conductor-stage-rename/specs/skills-policy/spec.md` (new)

**Steps**:
1. In the conductor-workflow delta, add requirement **Stage Terminology**
   (STAGE 0..4 naming; "Phase" reserved for delivery phases; no "PHASE"
   for conductor stages).
2. Modify the five conductor-workflow requirements that reference conductor
   stages (Phase-aware Implementation Ordering, Session Re-entrancy and
   Compaction Safety, Task-Authoring Skill, Quality Gates Applicability,
   Merge-to-main Authorization) replacing stage references `PHASE N` →
   `STAGE N` and `mid-PHASE-3` → `mid-STAGE-3`, full-text per delta format.
3. In the skills-policy delta, modify requirement **ste-writing permitted
   during OpenSpec commands**: its "prose pass before spec blessing"
   scenario says "during PHASE 1" → "during STAGE 1" (full-text, only that
   stage reference changes).
4. Leave the Delivery Phases and Review-finding resolution gating
   requirements unchanged.

**Tests**:
- name: `delta_validates`
  setup: both delta specs written
  action: `openspec validate conductor-stage-rename --type change --json`
  assert: no delta-structure errors
  command: `openspec validate conductor-stage-rename --type change --json`
  expected: passes validation after authoring.

**Acceptance**:
- `openspec validate conductor-stage-rename --type change --json` reports
  no delta-structure errors.

- [x] 2.1

## Task 3: Canon sync check at archive

**Files**:
- `openspec/specs/conductor-workflow/spec.md` (modified at archive time by
  `openspec archive`)
- `openspec/specs/skills-policy/spec.md` (modified at archive time by
  `openspec archive`)

**Steps**:
1. After the human approves the squash-merge, run `openspec archive
   conductor-stage-rename` from the repo root.
2. Verify both canon specs picked up the rename.

**Tests**:
- name: `canon_uses_STAGE`
  setup: archive completed
  action: `grep -c 'STAGE 2' openspec/specs/conductor-workflow/spec.md` and
  `grep -cE 'PHASE [0-4]' openspec/specs/conductor-workflow/spec.md
  openspec/specs/skills-policy/spec.md`
  assert: first count ≥ 3; combined second count = 0
  command: `test "$(grep -c 'STAGE 2' openspec/specs/conductor-workflow/spec.md)" -ge 3 && test "$(grep -cE 'PHASE [0-4]' openspec/specs/conductor-workflow/spec.md openspec/specs/skills-policy/spec.md | cut -d: -f2 | paste -sd+ | bc)" -eq 0`
  expected: exits 0 after archive.

**Acceptance**:
- Canon `conductor-workflow/spec.md` contains the Stage Terminology
  requirement; canon `skills-policy/spec.md` references STAGE 1; neither
  canon spec contains numbered `PHASE` references.

- [x] 3.1
