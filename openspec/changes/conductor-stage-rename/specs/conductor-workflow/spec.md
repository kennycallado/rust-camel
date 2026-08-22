# conductor-workflow Delta

## ADDED Requirements

### Requirement: Stage Terminology

The conductor workflow stages SHALL be named STAGE 0 through STAGE 4
(STAGE 0: ISOLATE, STAGE 1: SPEC, STAGE 2: PLAN, STAGE 3: IMPLEMENTATION,
STAGE 4: CLOSE). The word "Phase" SHALL be reserved exclusively for
OpenSpec delivery phases (`design.md` "Phases" section, `tasks.md`
`## Phase N` headings, phase-groups, inter-phase reviews). Conductor
documentation, agent definitions, and skills SHALL NOT use "PHASE" to
refer to conductor workflow stages.

#### Scenario: unambiguous stage reference

- **GIVEN** conductor documentation, an agent definition, or a skill file
  mentions a conductor workflow stage
- **WHEN** it names the stage
- **THEN** it uses "STAGE N" (e.g. "STAGE 3", "mid-STAGE-3"), never
  "PHASE N".

#### Scenario: delivery phases keep the word Phase

- **GIVEN** a multi-phase change
- **WHEN** tasks or conductor docs refer to its delivery slices
- **THEN** they use "Phase" (`## Phase N`, phase-group, inter-phase
  review), never "STAGE".

## MODIFIED Requirements

### Requirement: Phase-aware Implementation Ordering

Delivery phases SHALL be an implementation-ordering construct in STAGE 3,
not a blessing construct. The full multi-phase `tasks.md` SHALL be written
and plan-blessed ONCE before any phase is implemented. STAGE 3 SHALL
implement phase-groups in order.

#### Scenario: plan blessed once

- **GIVEN** a multi-phase change with all phases' task blocks written under
  `## Phase N` headings
- **WHEN** STAGE 2 completes
- **THEN** exactly one plan-bless covers the complete `tasks.md` and no
  per-phase blessing occurs.

#### Scenario: inter-phase review gating

- **GIVEN** STAGE 3 has implemented a phase-group with two or more tasks
- **WHEN** the phase-group completes
- **THEN** an inter-phase `r_glm` review runs on that phase's diff before
  the next phase-group begins.

#### Scenario: single-task phase skips inter-phase review

- **GIVEN** a phase-group containing exactly one task
- **WHEN** that task completes
- **THEN** no separate inter-phase review runs (per-task `r_glm` + final
  holistic review suffice).

### Requirement: Session Re-entrancy and Compaction Safety

The conductor SHALL be re-entrant: re-running a change whose worktree
exists and has commits beyond base SHALL resume from durable state, not
destroy it. Per-task diffs and review verdicts SHALL be indexed to the
persistent context-mode knowledge base and held in conductor context only
as a one-line pointer plus the verdict. KB searches SHALL be scoped via
`ctx_search(source: "<change-name>")`. The conductor SHALL NOT maintain an
agent-written checkpoint file; the automatic compaction hook is the
re-entry mechanism.

#### Scenario: resume existing worktree

- **GIVEN** a worktree for a change already exists with committed progress
- **WHEN** the conductor is invoked again for that change
- **THEN** STAGE 0 reconstructs state from `tasks.md` checkboxes and
  `bd show --json` instead of running the collision guard's force-remove.

#### Scenario: mid-STAGE-3 compaction recovery

- **GIVEN** an automatic compaction fires during STAGE 3 with tasks
  remaining
- **WHEN** the conductor resumes
- **THEN** it resumes at the next unchecked task in `tasks.md`, recovers
  prior task detail via scoped `ctx_search(source: "<change-name>")`, and
  maintains no agent-written checkpoint file.

#### Scenario: scoped knowledge-base search

- **GIVEN** two concurrent changes in worktrees of the same project
- **WHEN** the conductor searches the KB during one change
- **THEN** the search uses `ctx_search(source: "<change-name>")` so it does
  not return the other change's indexed content.

### Requirement: Task-Authoring Skill

A local skill `openspec-task-authoring` SHALL provide no-placeholders
discipline, self-review (spec coverage, placeholder scan, NEW-symbol
consistency, phase-boundary coherence), per-task quality rules, and
scope-check. It SHALL be loaded in STAGE 2 before tasks are authored.
Phase validation failure SHALL trigger a return to STAGE 1 for spec
re-bless rather than patching `tasks.md` around a bad phase boundary.

#### Scenario: skill loaded during planning

- **GIVEN** STAGE 2 begins authoring task blocks
- **WHEN** the conductor prepares to write tasks
- **THEN** the `openspec-task-authoring` skill is loaded and its self-review
  applied before plan-bless.

#### Scenario: phase validation failure escape hatch

- **GIVEN** the authoring skill's validation finds a phase too large or
  incoherent during STAGE 2
- **WHEN** the conductor handles the failure
- **THEN** it deletes the draft `tasks.md`, returns to STAGE 1 to revise
  `design.md`/specs, obtains a fresh spec-bless, and regenerates tasks in
  a restarted STAGE 2; it does not patch `tasks.md` to mask the bad
  boundary.

### Requirement: Quality Gates Applicability

STAGE 4 quality gates whose scope is untouched by a change's diff (no
`*.rs` or `Cargo.toml` in the diff) SHALL be reported as "N/A — no Rust
changed" rather than run or recorded as a pre-existing-failure exemption.

#### Scenario: docs-only change

- **GIVEN** a change whose diff contains no `.rs` or `Cargo.toml` files
- **WHEN** STAGE 4 enumerates the mandatory gates
- **THEN** each Rust/cargo gate is reported "N/A — no Rust changed", and is
  neither executed nor recorded as a pre-existing-failure exemption.

### Requirement: Merge-to-main Authorization

After STAGE 4 (quality gates + holistic review + archive) the conductor
SHALL NOT merge to main without explicit human approval. The MERGE GATE
SHALL be mandatory and never autonomous, even in autopilot. On approval
the conductor SHALL squash-merge per feature (`git merge --squash` +
commit) into the root worktree on `main`, after verifying the root is on
`main` and clean. On merge conflict the conductor SHALL NOT force or
auto-resolve; it SHALL report and hand back to the human. The conductor
SHALL NEVER run `git push` — push is the human's exclusive action.

#### Scenario: merge requires human approval

- **GIVEN** STAGE 4 has completed on the feature branch
- **WHEN** the conductor reaches the MERGE GATE
- **THEN** it pauses for explicit human approval and does not merge
  autonomously, even in autopilot mode.

#### Scenario: squash-merge per feature on approval

- **GIVEN** the human approves the merge
- **WHEN** the root worktree is on `main` and clean
- **THEN** the conductor runs `git -C "$ROOT" merge --squash feature/<name>`
  followed by a single caveman-commit on `main`, collapsing all branch
  commits into one.

#### Scenario: conflict is not force-resolved

- **GIVEN** the squash-merge produces a conflict
- **WHEN** the conductor handles it
- **THEN** it does NOT force or auto-resolve; it reports the conflict and
  hands back to the human.

#### Scenario: push is the human's exclusive action

- **GIVEN** the squash-merge has committed to `main` locally
- **WHEN** the conductor reports completion
- **THEN** it states that push is the human's action and the conductor
  SHALL NOT run `git push`.
