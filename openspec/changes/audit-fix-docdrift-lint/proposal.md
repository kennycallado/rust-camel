# Proposal: audit-fix-docdrift-lint

## Why

The T1 fidelity audit found `FC-DOC-DRIFT` findings manually: stale TODO
citations (`TODO(PROC-004)`), phantom rustdoc references, and version-stale
strings. Change D1 (rc-bwbg) closed that baseline by hand. Without an
enforcement gate, the same drift class regenerates on every commit and must be
re-swept manually each freeze.

bd rc-9h5a specifies a new xtask, `lint-context-citations`, that validates the
citation hygiene of every `CONTEXT.md` so the drift D1 fixed by hand is caught
automatically going forward. It is the pre-freeze enforcement gate for
documentation integrity: cited file paths resolve, cited symbols exist, line
numbers are not used as the primary citation mechanism (they drift), and no
glossary term is owned by two crates (the L6 recurrent collision hazard).

## What Changes

**In scope:**
- A new xtask subcommand `cargo xtask lint-context-citations` in
  `scripts/xtask/src/main.rs`, mirroring the structure of `lint-unwrap` and
  `lint-log-levels` (clap `Commands` enum variant, dispatch match arm,
  `pub fn lint_context_citations(workspace_root) -> Result<Vec<Violation>, String>`,
  and a unit-testable `_src` extractor).
- Four validation rules over the 58 context files in the workspace (57
  `CONTEXT.md` files plus the root `CONTEXT-MAP.md`):
  (a) cited file paths / markdown links resolve to existing files;
  (b) backtick-quoted Rust symbols (fn, struct, enum, trait) cited in prose
  resolve to a definition in the crate's `src/`;
  (c) line numbers (`file.rs:NN`) are not used as the primary citation
  mechanism;
  (d) glossary headings (`**Term:**`) are owned by at most one CONTEXT.md
  (cross-file collision detection).
- Wiring the new gate into `AGENTS.md ## QUALITY GATES` and
  `.github/workflows/ci.yml` alongside the existing lint-unwrap /
  lint-log-levels steps.
- Inline unit tests for the `_src` extractor (rule-by-rule, mirroring the
  test layout of `lint_non_exhaustive_src`).

**Explicitly excluded:**
- README semantic validation (rc-9h5a explicitly excludes it; READMEs are out
  of scope).
- Bare `impl <Ident>` citations as validation targets (impl blocks are not
  stable citation units; v1 validates types, fns, and `Type::member`
  associations only).

Any violation the lint's first run discovers IS fixed in-scope — the CI gate
cannot be enabled over a dirty tree, so the change lands only when
`cargo xtask lint-context-citations` exits clean.

## Acceptance criteria

- `cargo xtask lint-context-citations` exists and runs over all 58
  context files (57 `CONTEXT.md` + `CONTEXT-MAP.md`).
- It emits a `Violation` per: dangling path, dangling anchor, unresolved
  symbol, line-number-as-primary-citation, and cross-file glossary collision.
- It exits non-zero when any violation is found, zero when clean.
- It is listed in `AGENTS.md ## QUALITY GATES` and `.github/workflows/ci.yml`.
- The `_src` extractor has inline unit tests covering each rule (pass + fail
  per rule).
- The tree is CLEAN when the change lands — any violation the lint's first run
  discovers is fixed in-scope (the CI gate cannot be enabled over a dirty
  tree).

## Risks

- **False positives on symbol validation.** Backtick-quoted identifiers that
  are not Rust symbols (e.g. config keys, CLI flags) could be mis-flagged.
  Mitigation: the symbol rule only validates identifiers that match a Rust
  definition pattern (`fn name`, `struct Name`, `enum Name`, `trait Name`,
  `impl Name`, or `Name::method` path fragments), not every backtick token.
- **Glossary term ambiguity.** Common words used as glossary headings in
  unrelated crates could collide spuriously. Mitigation: only `**Term:**`
  bold-heading glossary entries are tracked (not inline bold); the collision
  report names both files so a human can confirm whether it is a real
  ownership conflict or a coincidental reuse.
