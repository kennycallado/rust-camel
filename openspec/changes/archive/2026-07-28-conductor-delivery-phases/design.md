# Design: conductor-delivery-phases

## Approach

The change restructures conductor-light around two validated ideas, both of which preserve the existing blessing mechanism unchanged.

**Delivery phases.** A feature MAY be decomposed into ordered delivery phases at design time. The decomposition is a design decision recorded in a new "Phases" section of design.md (per-phase: goal, dependencies, externally-visible types/interfaces, deliverable, exit-criteria). It is NOT a blessing construct: the whole tasks.md — including all phases' task blocks under `## Phase N` headings — is written and blessed ONCE in PHASE 2 (today's single plan-bless). PHASE 3 then implements phase-group by phase-group, with an inter-phase r_glm review confined to multi-task/cross-task phases (single-task phases already receive per-task r_glm + the final holistic review).

This was forced by the OpenSpec artifact-hash mechanism: `xtask hash-artifacts` (`artifact_hash::compute`) hashes the whole change directory as one checkbox-normalized blob, and the apply gate (`schema.yaml`) recognizes exactly one `.bless.json`. Per-phase incremental blessing is unsound against that mechanism (appending phase-2 blocks mutates the one hash and invalidates a phase-1 bless the gate already accepted) and unnecessary — a single full-plan bless preserves the "reviewer sees the whole plan before work" property while needing zero tooling changes. The blessed task blocks remain immutable; editing a blessed block requires re-bless of the affected scope (standard drift detection).

**Session compaction (minimal, evidence-backed).** Autonomous degradation is addressed by three existing layers and one new branch, validated empirically by e_opus:

1. **Index-not-hold.** The conductor `ctx_index`es each task diff + review verdict to the persistent, project-scoped context-mode KB (proven persistent across compaction and session restart; content rows carry `session_id=""` so they are not session-bound) and retains only a one-line pointer + verdict in its own context. ctx_search recovers detail; searches are scoped via `ctx_search(source: "<change-name>")` to avoid cross-worktree bleed (the KB is shared across all sessions of the same project).
2. **Automatic compaction hook.** opencode fires `experimental.session.compacting` mid-session and context-mode auto-builds and re-injects a resume snapshot (buildResumeSnapshot / upsertResume, plugin.js). The conductor does NOT maintain this — it is hook-written, not agent-written.
3. **PHASE 0 re-entrancy.** Today PHASE 0 force-destroys an existing worktree (`worktree remove --force`, `branch -D`) — the opposite of resume. The change adds a branch: if the worktree exists and has commits beyond the base, reconstruct state from git-tracked tasks.md checkboxes (the durable, hash-normalized progress ledger) + `bd show --json`, instead of wiping.

Dropped as YAGNI: a dedicated `.phase-checkpoints/phase-N.md` file (duplicates git log + tasks.md) and per-phase bd issues (bd already tracks the change).

The naming "PHASE" is already overloaded by the conductor's PHASE 0–4 stages; feature units are therefore called **delivery phases** in prose, while the `## Phase N` heading keeps the short token inside tasks.md.

## Affected crates

None. This change touches `.opencode/` (agent, skill, instructions) and `openspec/schemas/expert-gated/templates/` only.

## Architecture boundaries

Not applicable in the Runtime/DSL/Components sense. The relevant boundary is the OpenSpec blessing artifact: `xtask hash-artifacts` (whole-dir, checkbox-normalized) + `schema.yaml` apply gate (single `.bless.json`). This change deliberately does NOT cross that boundary — no xtask or schema.yaml edit — which is what makes the once-blessed-full-plan design load-bearing. It references the conductor design history in `docs/superpowers/archived/2026-06-30-conductor-primary-agent-design.md` and the existing skills policy in `.opencode/instructions/openspec-skills.md`.

## Alternatives considered

- **Option A (phases inside one change, single bless, no incremental gate)** — rejected: eases nothing and adds no milestone review.
- **Option B (phase = separate OpenSpec change, chained by bd deps)** — rejected for the common case: heavy orchestration (multi-worktree/re-entry) for what the phase-group construct handles in-place. Retained as the correct answer only when phases are genuinely independent subsystems.
- **Option C as incremental bless (spec-bless once, plan-bless per phase)** — rejected by e_opus after inspecting the hash mechanism: unsound against the whole-dir hash and single-`.bless.json` apply gate.
- **Dedicated checkpoint file + bd-per-phase** — rejected as redundant (git log + tasks.md checkboxes already encode progress).

The chosen design is the minimum that satisfies agent-coherence and compaction-safety while preserving every existing blessing property.
