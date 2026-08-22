# conductor-workflow Delta

## MODIFIED Requirements

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
