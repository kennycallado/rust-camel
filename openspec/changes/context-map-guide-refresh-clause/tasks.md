# Tasks: context-map-guide-refresh-clause

<!--
  Single-phase change. No `## Phase N` heading: one coherent slice
  (one-sentence append to one governance file). The absence of phase
  headings matches design.md's single-phase declaration and its
  requirement for no `## Phase N` task headings.
-->

## CONTEXT-MAP governance

### Task 1.1: Append user-visible-contract refresh clause to CONTEXT-MAP.md

**Files:**
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Open `CONTEXT-MAP.md` and locate the "Refresh is event-driven, not scheduled:" list in the "Documentation Authority & Refresh" section. Its first bullet currently reads (across three wrapped lines):

   ```
   - After an **architecture-shaping merge** (new EIP, lifecycle change, contract change): update the
     Contexts and Relationships sections of this map, plus the `CONTEXT.md` of every crate the merge
     touched, in the same change.
   ```

2. Append exactly one sentence to the END of that bullet, immediately after `touched, in the same change.` and before the line break that starts the next bullet (`- When **adding/renaming a domain term**:`). The appended sentence is a continuation of the same bullet, so it begins with a space and the markdown wraps at ~75 columns. Append this exact text (the leading space merges it into the bullet; the 2-space indent on continuation lines matches the existing bullet's style):

   ```
    If the merge changes a *user-visible* contract
     (a new EIP builder method, a new component scheme, a DSL key rename,
     a lifecycle-state rename, a public contract enum gaining a variant), also refresh
     the affected mdBook guide section and its anchored `examples/` include in the same
     change.
   ```

   The full bullet after the edit is one bullet item: the original three lines (rewrapped so the appended text flows from `change.`) plus the appended sentence continuation. Indent continuation lines 2 spaces to match the existing bullet. The line break is placed after `contract` so no trigger phrase (e.g. `component scheme`) is split across lines — this keeps the `trigger-set-complete` loop sound.

3. Do not edit any other line of `CONTEXT-MAP.md`. Do not reorder or reword the existing authority-order list, the ARCHITECT.md explanation paragraph, the term-landing rule, or any other bullet of the refresh list.

**Tests:** (verification — this is a governance-prose edit with no Rust code; checks are shell-based and machine-checkable)

- `refresh-clause-present`: CONTEXT-MAP.md with the edited bullet → run `rg -c 'also refresh' CONTEXT-MAP.md` → exits with exactly 1 match (the new sentence appears once and only once).
- `clause-anchored-to-bullet-1`: the new sentence sits inside the "architecture-shaping merge" bullet, not as a standalone bullet → run `rg -n -A5 'architecture-shaping merge' CONTEXT-MAP.md` → the output contains both `in the same change.` and `also refresh` within the same bullet block, with no `- ` list marker between them.
- `trigger-set-complete`: all five named triggers are present in the appended sentence → run a loop checking each trigger independently: `for t in 'EIP builder method' 'component scheme' 'DSL key rename' 'lifecycle-state rename' 'public contract enum'; do rg -q "$t" CONTEXT-MAP.md || exit 1; done` → exits 0 (each trigger is found; the loop is used instead of a single `.*` regex because the triggers span multiple wrapped lines and `.*` does not cross newlines).
- `minimal-diff`: the edit touches only bullet 1 → run `git diff HEAD -- CONTEXT-MAP.md` → the only hunk is within bullet 1 (the lines between `architecture-shaping merge` and the next bullet `adding/renaming a domain term`); no `+`/`-` line appears outside that region.
- `validate-clean`: the OpenSpec change still parses → run `openspec validate context-map-guide-refresh-clause --type change --json` → `"valid": true`, `"failed": 0`.

**Acceptance:**
- `rg -c 'also refresh' CONTEXT-MAP.md` returns 1.
- `openspec validate context-map-guide-refresh-clause --type change --json` reports `"valid": true`.
- `git status --porcelain=v1 --untracked-files=all` lists exactly one changed path: `CONTEXT-MAP.md` (no other modified, staged, or untracked files in the working tree).
- The appended sentence contains no `docs/ARCHITECT.md` reference (it cites the mdBook guide and `examples/` only).

- [x] 1.1
