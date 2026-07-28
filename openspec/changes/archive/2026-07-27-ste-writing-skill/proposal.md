# Proposal: ste-writing-skill

## Why

The OpenSpec expert-gated workflow produces persistent prose artifacts — ADRs
(`docs/adr/`), per-crate `CONTEXT.md`, `CONTEXT-MAP.md`, `README.md`, and the
proposal/design/specs/tasks files of each change. These artifacts drift toward "AI slop": long run-on sentences, nominalizations
("perform an analysis"), banned verbs (`leverage`, `utilize`, `facilitate`),
marketing adjectives (`seamless`, `robust`), modal hedges
("it is important to note"), and em-dashes as an LLM tell.

The project already governs two output surfaces: `caveman` (interactive chat,
token cost) and `caveman-commit` (git metadata, density). Neither covers
durable explanatory or operator-facing prose. There is no controlled-vocabulary
or anti-slop discipline for the documents that outlive the chat session.

The ASD-STE100 Simplified Technical English standard (aviation maintenance
manuals since 1986) defines a small machine-checkable ruleset that removes the
form of slop without rewriting meaning. A distilled skill of this standard is
the candidate.

## What Changes

**In scope:**

- Add a new project-local skill `ste-writing` under `.opencode/skills/`.
- Amend `.opencode/instructions/openspec-skills.md` to permit `ste-writing` as
  a prose pass before blessing. The blessing hashes and gates do not change.
- Define the surface split by **document function**: `caveman` owns
  conversational output, `caveman-commit` owns git metadata, `ste-writing` owns
  durable explanatory, procedural, and operator-facing prose.
- Two modes: STE-flavored (default — ADRs, CONTEXT, README, proposals,
  designs, specs, PR descriptions, release notes) and strict (operator
  procedures, runbooks, migration steps, actionable error guidance).

**Explicitly out of scope (deferred):**

- The deterministic Python linter (`ste-lint.py`) and any `cargo xtask
  lint-prose` / CI integration. Evaluated separately if the skill proves
  useful.

**Affected crates:** None. This change touches skills, an instruction file,
and documentation only.

## Acceptance criteria

- `ste-writing` skill file exists at `.opencode/skills/ste-writing/SKILL.md`
  and is available to run on hand-authored persistent prose (ADRs, CONTEXT,
  CONTEXT-MAP, README, OpenSpec artifacts, PR descriptions, release notes);
  it explicitly excludes `docs/ARCHITECT.md` (code-derived).
- `openspec-skills.md` explicitly permits `ste-writing` as a pre-blessing prose
  pass during `/opsx:*` commands, without weakening the existing
  `self-grill-proposals`-only rule for other skills.
- The skill enforces STE-flavored by default and strict only for procedure-class
  documents. In STE-flavored mode the self-lint rules are editorial heuristics
  (overridable); in strict mode they are mandatory.
- The skill preserves architectural voice and canonical terms in ADRs and
  CONTEXT files: STE is a clarity pass, not a voice-removal pass.
- Commit messages remain exclusively under `caveman-commit`.

## Risk budget

**Acceptable:** minor friction when prose legitimately needs a banned verb or a
sentence over 20 words (ADRs that justify against alternatives). The skill must
let the writer override a heuristic when precision or argument requires it.

**Out of bounds:** semantic distortion of architectural argument; flattening
canonical formulations ("Every processor and producer is a `Service<Exchange>`");
any change to blessing gates, hashes, or the data/control plane boundary.
