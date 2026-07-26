---
description: Holistic implementation review gate — fresh-eyes reviewer on complete diff against blessed spec
agent: build
---

# Holistic review for OpenSpec change: $ARGUMENTS

You are performing a **HOLISTIC IMPLEMENTATION REVIEW** (not per-task review).
This means fresh eyes on the COMPLETE implementation — catching cross-task
interactions, emergent inconsistency, and spec drift that per-task reviews
cannot see. This runs AFTER all tasks are done but BEFORE archive.

## Input

The argument after `/review` is the change name (e.g. `/review add-feature-x`).
If no argument provided, infer from conversation context or prompt the user.

## Steps

### 1. Verify all tasks complete

```bash
openspec status --change "$1" --json
```

Confirm all tasks are done. If not, STOP: "Not all tasks complete. Finish implementation first."

### 2. Gather the complete implementation diff

```bash
git diff $(git merge-base HEAD main)...HEAD
```

This captures ALL changes on the feature branch — code + specs + artifacts.

### 3. Stage the spec merge (if not already done)

Merge delta specs into `openspec/specs/` so the reviewer sees the complete
picture including canonical spec changes:

```bash
openspec show --change "$1"  # check if specs need syncing
openspec sync --change "$1"  # stage the merge (don't commit yet)
```

The reviewer MUST see the merged specs — this closes the archive-merge blind spot.

### 4. Read the blessed plan

Read `.bless.json` to get the plan hash the implementation was built against:
```bash
cat openspec/changes/$1/.bless.json
```

### 5. Dispatch reviewer with fresh eyes

Dispatch `@reviewers/r_glm` **WITHOUT task_id** (fresh eyes on the whole) with:

```
## HOLISTIC IMPLEMENTATION REVIEW

Change: $1
Feature branch diff: git diff $(git merge-base HEAD main)...HEAD
Blessed plan hash: <from .bless.json>
Spec artifacts: openspec/changes/$1/specs/
Canonical specs: openspec/specs/

Review the COMPLETE implementation against the blessed spec and plan.
Look specifically for:
1. Cross-task interactions — changes in one area that break another
2. Emergent inconsistency — patterns that diverge between tasks
3. Spec drift — implementation that doesn't match the delta specs
4. Missing coverage — edge cases no task addressed
5. Code quality — anything per-task reviews could not see

Return verdict: APPROVE | APPROVE-WITH-FINDINGS | REJECT
Include findings ordered by severity.
```

Wait for the reviewer's verdict.

### 6. Write review

Write `openspec/changes/$1/.review.json`:

```json
{
  "verdict": "APPROVE",
  "reviewer": "r_glm",
  "impl_hash": "git:<HEAD commit hash>",
  "findings": [<reviewer findings if any>]
}
```

### 7. Report

**If APPROVE:**
```
✓ Holistic review passed by @reviewers/r_glm
  Impl: git:<hash>
  Against plan: sha256:<hash>
  Ready to /opsx:archive $1
```

**If APPROVE-WITH-FINDINGS:**
```
⚠ Holistic review: findings by @reviewers/r_glm
  Findings:
  - <finding 1>
  - <finding 2>
  Address findings, then re-run /review $1
```

**If REJECT:**
```
✗ Holistic review REJECTED by @reviewers/r_glm
  Reason: <reason>
  Loop back to implementation, then re-run /review $1
```

## Guardrails

- NEVER review without staging the spec merge first (blind spot fix)
- NEVER reuse a task_id for holistic review (fresh eyes is the point)
- The reviewer gets the COMPLETE diff, not individual task diffs
- This gate is MANDATORY before archive — conductor-light enforces it
