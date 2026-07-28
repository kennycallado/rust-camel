# conductor-workflow Specification

## Purpose
TBD - created by archiving change conductor-delivery-phases. Update Purpose after archive.
## Requirements
### Requirement: Delivery Phases

A feature MAY be decomposed into ordered delivery phases at design time. The
decomposition SHALL be recorded in a "Phases" section of `design.md`, with each
phase specifying goal, dependencies, externally-visible types/interfaces,
deliverable, and exit-criteria. The decomposition is a design decision made
before spec-bless, not a blessing construct.

#### Scenario: single-phase feature

- **GIVEN** a feature whose scope is one coherent change
- **WHEN** the designer evaluates phase decomposition
- **THEN** `design.md` has no "Phases" section and `tasks.md` has no
  `## Phase N` headings, and the flow is identical to the pre-change
  conductor-light.

#### Scenario: multi-phase feature

- **GIVEN** a feature spanning multiple coherent slices
- **WHEN** the designer decomposes it
- **THEN** the `design.md` "Phases" section lists N≥2 phases and `tasks.md`
  groups all task blocks under `## Phase N: <name>` headings.

### Requirement: Phase-aware Implementation Ordering

Delivery phases SHALL be an implementation-ordering construct in PHASE 3, not a
blessing construct. The full multi-phase `tasks.md` SHALL be written and
plan-blessed ONCE before any phase is implemented. PHASE 3 SHALL implement
phase-groups in order.

#### Scenario: plan blessed once

- **GIVEN** a multi-phase change with all phases' task blocks written under
  `## Phase N` headings
- **WHEN** PHASE 2 completes
- **THEN** exactly one plan-bless covers the complete `tasks.md` and no
  per-phase blessing occurs.

#### Scenario: inter-phase review gating

- **GIVEN** PHASE 3 has implemented a phase-group with two or more tasks
- **WHEN** the phase-group completes
- **THEN** an inter-phase `r_glm` review runs on that phase's diff before the
  next phase-group begins.

#### Scenario: single-task phase skips inter-phase review

- **GIVEN** a phase-group containing exactly one task
- **WHEN** that task completes
- **THEN** no separate inter-phase review runs (per-task `r_glm` + final
  holistic review suffice).

### Requirement: Session Re-entrancy and Compaction Safety

The conductor SHALL be re-entrant: re-running a change whose worktree exists and
has commits beyond base SHALL resume from durable state, not destroy it. Per-task
diffs and review verdicts SHALL be indexed to the persistent context-mode
knowledge base and held in conductor context only as a one-line pointer plus the
verdict. KB searches SHALL be scoped via `ctx_search(source: "<change-name>")`.
The conductor SHALL NOT maintain an agent-written checkpoint file; the automatic
compaction hook is the re-entry mechanism.

#### Scenario: resume existing worktree

- **GIVEN** a worktree for a change already exists with committed progress
- **WHEN** the conductor is invoked again for that change
- **THEN** PHASE 0 reconstructs state from `tasks.md` checkboxes and
  `bd show --json` instead of running the collision guard's force-remove.

#### Scenario: mid-PHASE-3 compaction recovery

- **GIVEN** an automatic compaction fires during PHASE 3 with tasks remaining
- **WHEN** the conductor resumes
- **THEN** it resumes at the next unchecked task in `tasks.md`, recovers prior
  task detail via scoped `ctx_search(source: "<change-name>")`, and maintains
  no agent-written checkpoint file.

#### Scenario: scoped knowledge-base search

- **GIVEN** two concurrent changes in worktrees of the same project
- **WHEN** the conductor searches the KB during one change
- **THEN** the search uses `ctx_search(source: "<change-name>")` so it does not
  return the other change's indexed content.

### Requirement: Task-Authoring Skill

A local skill `openspec-task-authoring` SHALL provide no-placeholders
discipline, self-review (spec coverage, placeholder scan, NEW-symbol consistency,
phase-boundary coherence), per-task quality rules, and scope-check. It SHALL be
loaded in PHASE 2 before tasks are authored. Phase validation failure SHALL
trigger a return to PHASE 1 for spec re-bless rather than patching `tasks.md`
around a bad phase boundary.

#### Scenario: skill loaded during planning

- **GIVEN** PHASE 2 begins authoring task blocks
- **WHEN** the conductor prepares to write tasks
- **THEN** the `openspec-task-authoring` skill is loaded and its self-review
  applied before plan-bless.

#### Scenario: phase validation failure escape hatch

- **GIVEN** the authoring skill's validation finds a phase too large or
  incoherent during PHASE 2
- **WHEN** the conductor handles the failure
- **THEN** it deletes the draft `tasks.md`, returns to PHASE 1 to revise
  `design.md`/specs, obtains a fresh spec-bless, and regenerates tasks in a
  restarted PHASE 2; it does not patch `tasks.md` to mask the bad boundary.

### Requirement: Quality Gates Applicability

PHASE 4 quality gates whose scope is untouched by a change's diff (no `*.rs` or
`Cargo.toml` in the diff) SHALL be reported as "N/A — no Rust changed" rather
than run or recorded as a pre-existing-failure exemption.

#### Scenario: docs-only change

- **GIVEN** a change whose diff contains no `.rs` or `Cargo.toml` files
- **WHEN** PHASE 4 enumerates the mandatory gates
- **THEN** each Rust/cargo gate is reported "N/A — no Rust changed", and is
  neither executed nor recorded as a pre-existing-failure exemption.

