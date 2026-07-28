# Design: <change-name>

## Approach

<Technical approach — how will this be implemented?>

## Affected crates

- <crate name>: <what changes>

## Architecture boundaries

<How does this respect Runtime / DSL / Components / Services / Languages / Functions?>

## Phases (optional)

<!--
  Omit this section entirely for single-phase changes (a single
  coherent slice that does not benefit from milestone grouping). The
  absence of "Phases" AND the absence of `## Phase N` headings in
  tasks.md together signal a single-phase change.

  Each phase is a design-time decision recorded BEFORE spec-bless.
  Phases are a PHASE 3 implementation-ordering construct, not a
  blessing construct: tasks.md is written and plan-blessed ONCE over
  the complete phase set, then implemented phase-group by phase-group.
-->

### Phase 1: <one-line phase goal>
- **Goal:** <what this phase delivers>
- **Dependencies:** <prior phases, external crates, design decisions>
- **Externally-visible types/interfaces:** <new public surface introduced>
- **Deliverable:** <commit, doc, schema, example>
- **Exit-criteria:** <machine-checkable or testable pass conditions>

### Phase 2: <one-line phase goal>
- **Goal:** <...>
- **Dependencies:** <...>
- **Externally-visible types/interfaces:** <...>
- **Deliverable:** <...>
- **Exit-criteria:** <...>

## Alternatives considered

<What other approaches were considered and why rejected?>
