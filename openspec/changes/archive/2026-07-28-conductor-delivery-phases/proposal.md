# Proposal: conductor-delivery-phases

## Why

conductor-light implements features as a single linear spec→plan→implement→close flow. Two strains show on large multi-task features:

1. Autonomous implementation degrades: the conductor accumulates every task diff and review verdict verbatim across the whole feature; by late tasks its orchestration context is bloated and reasoning quality drops. There is no safe point to compact, because mid-PHASE-3 state lives only in context.
2. Large features give agent workers coarse, low-coherence slices with no milestone boundary to review against.

A reviewed design (e_gpt, then e_opus with empirical tool-driven validation) established that the OpenSpec whole-directory artifact hash makes per-phase incremental plan-blessing unsound. The sound shape is: phases as a PHASE 3 implementation-ordering construct over a once-blessed FULL tasks.md, plus a minimal session-compaction mechanism built on three already-existing layers (context-mode KB, automatic compaction hook, bd).

## What Changes

Included:
- **conductor-light.md**: delivery phases as optional PHASE 3 ordering groups (`## Phase N` headings over a complete, once-blessed tasks.md); inter-phase r_glm review only for multi-task phases; PHASE 0 re-entrancy branch (do not destroy an existing worktree with commits — reconstruct from tasks.md checkboxes + `bd show --json`); index-not-hold per task (ctx_index each diff/verdict, keep a one-line pointer, scope ctx_search via `ctx_search(source: "<change-name>")`); PHASE 4 N/A-gate rule (Rust gates untouched in a docs-only diff reported N/A, not run, not exempted); commit shape deferred to caveman-commit.
- **NEW skill openspec-task-authoring**: no-placeholders discipline, self-review (spec coverage / placeholder scan / NEW-symbol consistency / phase-boundary coherence), per-task quality (executable test specs, concrete acceptance), scope-check, and a mandatory escape hatch (phase-validation failure ⇒ re-spec-bless, never patch a bad boundary).
- **openspec-skills.md**: openspec-task-authoring permitted as the sanctioned in-tree task-authoring skill; external writing-plans stays removed.
- **templates/tasks.md + design.md**: optional "Phases" section (design: per-phase goal/deps/externally-visible types/deliverable/exit-criteria) and `## Phase N` heading convention; single-phase = today.

Excluded (validated out): per-phase incremental plan-bless (unsound vs whole-dir hash); dedicated `.phase-checkpoints/` files (redundant vs git log + tasks.md); bd-per-phase issues (redundant); xtask / schema.yaml apply-gate changes (none needed).

Affected crates: none. This is a `.opencode/` tooling/governance change.

## Acceptance criteria

- A single-phase change behaves identically to today.
- A multi-phase change writes one full tasks.md, blesses once, implements phase-by-phase with inter-phase review on multi-task phases.
- Re-running a change with an existing worktree + commits resumes instead of wiping state.
- Each task's diff+verdict is indexed to the persistent KB and retrievable via scoped ctx_search.
- openspec-skills.md permits openspec-task-authoring; writing-plans remains removed.

## Risk budget

Acceptable: behavioral change to an agent many features depend on; mitigated by single-phase backward-compatibility and by gating this change itself through a real spec-bless.
Out of bounds: changes to xtask hash semantics or schema.yaml apply gate; changes to the OpenSpec CLI; any new per-phase blessing artifact format.
