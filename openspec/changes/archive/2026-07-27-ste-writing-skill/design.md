# Design: ste-writing-skill

## Approach

Adopt the distilled ASD-STE100 skill as a **project-local, version-controlled**
skill and wire it into the OpenSpec expert-gated workflow as a pre-blessing
prose pass. No Rust code, no runtime, no data plane.

The skill ships as one file, `.opencode/skills/ste-writing/SKILL.md`, with two
modes and a self-lint block:

1. **STE-flavored (default).** Applies to hand-authored durable prose: ADRs,
   per-crate `CONTEXT.md`, `CONTEXT-MAP.md`, `README.md`, OpenSpec
   proposal/design/specs/tasks, PR descriptions, release notes. It does NOT
   apply to `docs/ARCHITECT.md` (a code-derived snapshot regenerated from
   source; rewriting it would create drift). The self-lint rules run as
   **editorial heuristics**, overridable when precision or argument requires.

2. **Strict.** Applies only to procedure-class documents: operator runbooks,
   migration/remediation steps, safety/security instructions where sequence
   and actor must be unambiguous, and actionable error-message guidance. The
   self-lint rules are mandatory here.

3. **Voice-preservation clause.** The skill explicitly preserves canonical
   terms, causal argument, and project-defining formulations (e.g. "Every
   processor and producer is a `Service<Exchange>`"). STE is a clarity pass,
   not a voice-removal pass.

4. **Self-lint rules (the machine-checkable subset).** The skill applies six
   mechanical rules, plus slop-marker detection:
   - **Sentence length:** split any sentence over 20 words.
   - **Punctuation:** replace semicolons with periods.
   - **Contractions:** expand them ("don't" → "do not").
   - **Passive voice:** make active when the actor is known.
   - **Verb form:** replace `-ing` main verbs, nominalizations ("perform an
     analysis" → "analyze"), and phrasal verbs ("spin up" → "start") with
     plain verbs.
   - **Naming:** pick one name per concept; do not name the same thing two
     ways.
   - **Slop markers:** flag banned verbs (`leverage`, `utilize`, `facilitate`,
     `ensure`, `prior to`), marketing adjectives (`seamless`, `robust`,
     `powerful`), modal hedges ("it is important to note"), and em-dashes (an
     LLM tell).

   In STE-flavored mode every rule is an overridable heuristic. In strict mode
   every rule is mandatory; the only exception is a quoted identifier, code
   span, or verbatim command that the rule would corrupt.

**Policy amendment.** `.opencode/instructions/openspec-skills.md` currently
allows only `self-grill-proposals` during `/opsx:*` commands. This change adds
an explicit exception: `ste-writing` may run as a prose pass **before** a
blessing, never replacing one. The blessing hashes and gates (spec blessing,
plan blessing, holistic review) are unchanged.

**Workflow hooks.** The skill is AVAILABLE at three points in the
conductor-light flow. Running it is PERMITTED, not mandatory — the hard gate
remains the blessing, never the prose pass:
- PHASE 1 SPEC — proposal/design/specs prose (permitted before the spec
  blessing)
- PHASE 2 PLAN — tasks.md prose (permitted before the plan blessing)
- PHASE 4 CLOSE — PR descriptions, release notes (permitted)

Commit messages stay exclusively under `caveman-commit`. Conversational chat
stays under `caveman`. The three skills divide by **document function**, not by
persistence.

## Affected crates

- None. This change is confined to `.opencode/` (skills, instructions) and
  documentation. No `Cargo.toml`, no `src/`, no `xtask/`.

## Architecture boundaries

This change does not enter the runtime. It respects the data/control plane
split (ADR-0001) trivially by operating entirely in the tooling and
documentation layer; it never touches the Exchange-data trust boundary
(ADR-0032). It does not touch Runtime, DSL, Components, Services, Languages,
or Functions.

The relevant authority structure is the **Documentation Authority & Refresh**
section of `CONTEXT-MAP.md`. The skill operates on authority tiers 3 (ADRs,
`CONTEXT-MAP.md`, crate `CONTEXT.md` — curated prose) and 4 (README files). It
must NOT override tier 1 (source code) or tier 2 (`docs/ARCHITECT.md`, a
code-derived snapshot). When prose conflicts with code, code wins, and the
skill must not "clarify" prose away from the code-derived truth.

## Alternatives considered

- **User-global placement** (`~/.config/opencode/skills/`). Rejected: a workflow
  dependency that lives on one operator's machine is unpinned and breaks
  reproducibility. The project pins everything else; the skill must be too.
- **Full strict STE everywhere.** Rejected: STE strips voice on purpose. ADRs
  are argumentative documents; flattening them into maintenance-manual prose
  destroys the reasoning force. Strict mode is reserved for procedure-class
  documents only.
- **Bundle the deterministic linter now.** Deferred to a separate change. The
  user explicitly wants the skill evaluated first; coupling it to a CI gate
  adds scope and a false-positive surface before the value is proven.
- **Rely on `caveman` + `caveman-commit` + `behavior.md` alone.** Rejected:
  those govern ephemeral output (token cost, chat, commit density). They do
  not address durable prose quality and do not detect slop markers like
  em-dashes, banned verbs, or modal hedges.
- **Do nothing.** Rejected: the slop pattern is observable in current
  artifacts and will compound as more changes land.
