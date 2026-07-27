# Tasks: ste-writing-skill

Two tasks. No Rust code, no crate changes. All paths relative to the worktree
root `/home/kenny/dev/rust-camel/.worktrees/ste-writing-skill`.

## Skill

### Task 1: Create the ste-writing skill file

**Files:**
- `.opencode/skills/ste-writing/SKILL.md` (new)

**Steps:**
1. Create the directory `.opencode/skills/ste-writing/`.
2. Write `SKILL.md` with YAML frontmatter:
   - `name: ste-writing`
   - `description:` one paragraph: rewrite hand-authored durable prose (ADRs,
     `CONTEXT.md`, `CONTEXT-MAP.md`, `README.md`, OpenSpec artifacts, PR
     descriptions, release notes) toward ASD-STE100 Simplified Technical
     English to remove AI slop. State that it does NOT apply to code,
     identifiers, command syntax, `docs/ARCHITECT.md` (code-derived), commit
     messages (owned by `caveman-commit`), or chat (owned by `caveman`).
   - Frontmatter is `name` + `description` ONLY; no `allowed-tools` (this
     skill issues no tool calls).
3. Body section **Two modes**:
   - **STE-flavored (default).** Overridable editorial heuristics. Applied to
     ADRs, CONTEXT, CONTEXT-MAP, README, OpenSpec artifacts, PR descriptions,
     release notes.
   - **Strict.** Mandatory. Applied only to procedure-class prose: operator
     runbooks, migration/remediation steps, safety/security instructions,
     actionable error-message guidance.
4. Body section **Self-lint rules** listing the six mechanical rules by name:
   (a) split sentences over 20 words; (b) replace semicolons with periods;
   (c) expand contractions; (d) make passive voice active when the actor is
   known; (e) replace `-ing` main verbs, nominalizations ("perform an
   analysis" → "analyze"), and phrasal verbs ("spin up" → "start") with plain
   verbs; (f) one name per concept. State that in flavored mode these are
   overridable; in strict mode mandatory for prose.
5. Body section **Slop markers** to flag: banned verbs (`leverage`, `utilize`,
   `facilitate`, `ensure`, `prior to`), marketing adjectives (`seamless`,
   `robust`, `powerful`), modal hedges ("it is important to note"), em-dashes
   (an LLM tell).
6. Body clause **Code protection**: code spans, fenced blocks, identifiers,
   and verbatim commands are never rewritten in either mode, and the lint
   rules do not fire on them.
7. Body clause **Voice preservation**: in flavored mode the skill preserves
   canonical terms, causal argument, deliberate emphasis, and project-defining
   formulations (e.g. "Every processor and producer is a `Service<Exchange>`").
   STE is a clarity pass, not a voice-removal pass.
8. Body clause **Surface division**: `caveman` owns chat; `caveman-commit`
   owns commit metadata; `ste-writing` owns durable explanatory, procedural,
   and operator-facing prose. No two run over the same text at once.

**Tests:** (content assertions — run after writing; each is a literal grep)
- `skill-has-frontmatter`: `grep -q '^name: ste-writing$'` and
  `grep -q '^description:'` both succeed on the file.
- `skill-lists-six-rules`: assert each of these literal substrings appears:
  "20 words", "semicolon", "contraction", "passive", "-ing", "nominalization",
  "phrasal", "one name".
- `skill-lists-slop-markers`: assert each of these literal substrings appears:
  "leverage", "utilize", "facilitate", "ensure", "prior to", "seamless",
  "robust", "powerful", "modal hedge", "em-dash".
- `skill-states-two-modes`: assert "STE-flavored" appears; assert "Strict"
  appears on a line whose section also contains "runbook" or "procedure".
- `skill-has-voice-clause`: assert the literal substrings "voice" and
  "Service<Exchange>" both appear.
- `skill-has-code-protection`: assert the literal substrings "never rewritten"
  and "do not fire" both appear.

**Acceptance:**
- File exists at `.opencode/skills/ste-writing/SKILL.md`.
- All six content assertions pass.
- No claim that the skill rewrites code, `docs/ARCHITECT.md`, commit messages,
  or chat.

- [ ] 1

## Policy

### Task 2: Amend the OpenSpec skills policy to permit ste-writing

**Files:**
- `.opencode/instructions/openspec-skills.md` (modified)

**Steps:**
1. In the section `## When executing OpenSpec commands (/opsx:*, /bless)`,
   after the `self-grill-proposals` bullet, add a bullet for `ste-writing`:
   it MAY run as a prose pass on the OpenSpec artifacts
   (proposal/design/specs/tasks) DURING artifact-producing `/opsx:*` work and
   BEFORE `/bless` begins. State three things verbatim: (a) it never replaces
   or weakens a blessing gate; (b) the blessing hash is computed AFTER the
   prose pass; (c) during the `/bless` act itself ONLY `self-grill-proposals`
   loads — `ste-writing` does not run during `/bless`.
2. Add a short subsection `## Writing-skill surface division` stating the
   function-based split: `caveman` (chat), `caveman-commit` (commit
   metadata), `ste-writing` (durable explanatory/procedural/operator-facing
   prose). State that `ste-writing` excludes `docs/ARCHITECT.md` (code-derived)
   and any `*.rs`/`Cargo.toml`.
3. Do NOT alter the `self-grill-proposals`-only rule for other skills, the
   "Skills removed" list, the "Skills available contextually" list, or the
   "Skills always active" list, except to add `ste-writing` where it belongs.

**Tests:** (content assertions; literal greps)
- `policy-permits-ste-writing`: assert `ste-writing` occurs between the
  `## When executing OpenSpec commands` heading and the next `## ` heading in
  `openspec-skills.md`.
- `policy-scopes-prose-pass`: assert the literal substrings "BEFORE" and
  "/bless" both appear in the `ste-writing` bullet, and that the bullet
  contains "never replaces or weakens".
- `policy-bless-isolation`: assert the literal substring "during the `/bless`
  act itself" appears and that "self-grill-proposals" appears in that same
  sentence.
- `policy-preserves-self-grill-rule`: assert `self-grill-proposals` still
  appears as the thinking tool for `/bless` outside the new bullet.
- `policy-has-surface-division`: assert a heading containing "surface
  division" exists and that `caveman`, `caveman-commit`, and `ste-writing`
  all appear under it.
- `policy-excludes-architect`: assert the literal string `docs/ARCHITECT.md`
  appears in the surface-division subsection marked as excluded.
- `scope-allowlist`: after both tasks, run
  `git -C <worktree> status --porcelain` → assert the ONLY listed paths are
  `.opencode/skills/ste-writing/SKILL.md` and
  `.opencode/instructions/openspec-skills.md` (plus the openspec change
  artifacts). Assert NO path matches `*.rs`, `Cargo.toml`, `src/`, `xtask/`,
  or `docs/ARCHITECT.md`.

**Acceptance:**
- `openspec-skills.md` lists `ste-writing` as the only additional permitted
  skill during artifact-producing `/opsx:*` work (before `/bless`).
- The existing `self-grill-proposals` rule and the removed/contextual/active
  skill lists are intact; `/bless` still loads only `self-grill-proposals`.
- No claim that `ste-writing` is mandatory or that it replaces a blessing.

- [ ] 2
