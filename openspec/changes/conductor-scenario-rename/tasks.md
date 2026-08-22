# Tasks

## Task 1: Rename the legacy scenario header via manual sync

**Files**:
- `openspec/specs/conductor-workflow/spec.md` (modified)
- `openspec/changes/conductor-scenario-rename/specs/conductor-workflow/spec.md` (new)

**Steps**:
1. Author the delta: MODIFIED Session Re-entrancy and Compaction Safety
   (full-text, scenario header renamed to "mid-STAGE-3 compaction
   recovery") and MODIFIED Stage Terminology (full-text, exception
   paragraph removed).
2. Apply the same two edits to the canon spec manually (the
   `openspec-sync-specs` sanctioned path: delta is intent, canon edit is
   the sync).
3. Validate the change; confirm canon and delta agree so the post-merge
   archive is idempotent.

**Tests**:
- name: `canon_header_renamed`
  setup: canon edited
  action: `grep -c 'mid-PHASE-3' openspec/specs/conductor-workflow/spec.md`
  assert: prints 0, and `grep -c 'mid-STAGE-3 compaction recovery'` prints 1
  command: `test "$(grep -c 'mid-PHASE-3' openspec/specs/conductor-workflow/spec.md)" = 0 && test "$(grep -c 'mid-STAGE-3 compaction recovery' openspec/specs/conductor-workflow/spec.md)" = 1`
  expected: exits 0 after the edit.
- name: `exception_removed`
  setup: canon edited
  action: `grep -c 'SHALL remain until the openspec CLI' openspec/specs/conductor-workflow/spec.md`
  assert: prints 0
  command: `test "$(grep -c 'SHALL remain until the openspec CLI' openspec/specs/conductor-workflow/spec.md)" = 0`
  expected: exits 0 after the edit.
- name: `delta_validates`
  setup: delta written
  action: `openspec validate conductor-scenario-rename --type change`
  assert: valid
  command: `openspec validate conductor-scenario-rename --type change`
  expected: passes.

**Acceptance**:
- Canon has zero `mid-PHASE-3` and zero numbered `PHASE [0-4]`.
- `openspec validate conductor-scenario-rename --type change` passes.

- [x] 1.1
