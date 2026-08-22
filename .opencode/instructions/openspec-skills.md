---
description: Skills policy for the expert-gated OpenSpec workflow. Controls which skills load during commands.
---

# Skills Policy — OpenSpec Exception

## When executing OpenSpec commands (/opsx:*, /bless)

SKIP all skill checks EXCEPT:
- `self-grill-proposals` — explicitly loaded by /bless to stress-test
  artifacts before producing a verdict. This is the expert's thinking
  tool, not a workflow replacement.
- `ste-writing` — MAY run as a prose pass on the OpenSpec artifacts
  (proposal/design/specs/tasks) DURING artifact-producing `/opsx:*` work
  and BEFORE `/bless` begins. It never replaces or weakens a blessing gate.
  Compute the blessing hash AFTER the prose pass. Only
  `self-grill-proposals` loads during the `/bless` act itself, and
  `ste-writing` does not run during `/bless`.

Do NOT invoke any other Skill tool during these commands (the two exceptions
above are the only ones).
OpenSpec commands are self-contained workflows with their own quality gates
(blessing + drift detection). Other skill augmentation causes conflicts.

The command templates and schema instructions contain everything needed.
Follow them directly.

## Writing-skill surface division

This subsection classifies by document FUNCTION. It is orthogonal to the
activation-status lists below (always-active vs on-demand).

Writing skills divide by document function, not by persistence:
- `caveman` — conversational chat output.
- `caveman-commit` — git commit metadata.
- `ste-writing` — durable explanatory, procedural, and operator-facing prose (ADRs, CONTEXT, CONTEXT-MAP, README, OpenSpec artifacts, PR descriptions, release notes).

`ste-writing` excludes `docs/ARCHITECT.md` (a code-derived snapshot) and any `*.rs` or `Cargo.toml`. No two of these skills run over the same text at once.

## Skills removed from this project's workflow

These skills are REPLACED by OpenSpec. Do NOT load them:
- `brainstorming` — replaced by /opsx:propose + /bless
- `writing-plans` — replaced by OpenSpec tasks.md. Its task-authoring
  quality discipline (no placeholders, executable test specs, concrete
  acceptance, scope-check) has been ported into the in-tree
  `openspec-task-authoring` skill (see below); `writing-plans` itself
  stays removed.
- `executing-plans` — replaced by /opsx:apply

## Skills available contextually (load only when explicitly relevant)

- `openspec-task-authoring` — loaded by `conductor-light` during STAGE 2
  (task authoring) before the plan-bless. This is the sanctioned
  in-tree replacement for `writing-plans`' task-authoring discipline
  (no placeholders, executable test specs, concrete acceptance, scope-
  check, escape hatch on phase-validation failure). It is NOT a
  workflow skill and does not replace the reviewer loop or the bless.
- `subagent-driven-development` — the implement→review pattern is embedded
  in apply.instruction; load this skill only if you need detailed dispatch guidance
- `test-driven-development` — for tasks that change behavior
- `systematic-debugging` — after test failures
- `verification-before-completion` — before archive
- `using-git-worktrees` — worktree is created by default for feature changes
- `finishing-a-development-branch` — at merge/archive time

## Skills always active (orthogonal)

- `caveman` — communication style
- `caveman-commit` — commit format
- `beads` — issue tracking

## Reviewer agents

Reviewers (r_glm, r_gpt) invoke `thermo-nuclear-code-quality-review` and
`ponytail` directly from their agent prompts. This is unaffected by this policy.
