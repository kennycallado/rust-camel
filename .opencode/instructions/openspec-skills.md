---
description: Skills policy for the expert-gated OpenSpec workflow. Controls which skills load during commands.
---

# Skills Policy — OpenSpec Exception

## When executing OpenSpec commands (/opsx:*, /bless)

SKIP all skill checks EXCEPT:
- `self-grill-proposals` — explicitly loaded by /bless to stress-test
  artifacts before producing a verdict. This is the expert's thinking
  tool, not a workflow replacement.

Do NOT invoke any other Skill tool during these commands.
OpenSpec commands are self-contained workflows with their own quality gates
(blessing + drift detection). Other skill augmentation causes conflicts.

The command templates and schema instructions contain everything needed.
Follow them directly.

## Skills removed from this project's workflow

These skills are REPLACED by OpenSpec. Do NOT load them:
- `brainstorming` — replaced by /opsx:propose + /bless
- `writing-plans` — replaced by OpenSpec tasks.md
- `executing-plans` — replaced by /opsx:apply

## Skills available contextually (load only when explicitly relevant)

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
